//! ChangeMerger implementation for SQLite.
//!
//! Applies incoming changesets from remote peers with conflict resolution:
//! - Append-only tables: INSERT OR IGNORE (union merge)
//! - LWW tables: compare HLC, higher wins
//! - Suggestions: monotonic status merge (acted > dismissed > shown > null)
//! - DeletionEvent: hard-delete all rows from originating device (GDPR Art. 17)

use async_trait::async_trait;
use rusqlite::{Connection, OptionalExtension};
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::error::StorageError;
use crate::sqlite::GuardedConnection;
use maekon_core::error::CoreError;
use maekon_core::models::sync::{ChangeSet, ChangeSetKind, SyncResult, Tombstone};

/// Unit-separator joining `embedding_vectors`' cross-device-stable composite tombstone key
/// `segment_id || US || model_id` (its `id` is a per-device autoincrement, not stable).
const EMB_KEY_SEP: char = '\u{1f}';
use maekon_core::ports::change_merger::ChangeMerger;
use maekon_core::sync::Hlc;

/// SQLite-backed ChangeMerger adapter.
///
/// Accesses the connection only through the shared [`GuardedConnection`] — a barrier-free
/// handle cannot be obtained (#4928).
pub struct SqliteSyncMerger {
    conn: Arc<GuardedConnection>,
    local_device_id: String,
}

impl SqliteSyncMerger {
    pub fn new(conn: Arc<GuardedConnection>, local_device_id: String) -> Self {
        Self {
            conn,
            local_device_id,
        }
    }

    /// Compute suggestion status ordinal from timestamp fields.
    /// acted (3) > dismissed (2) > shown (1) > null (0)
    fn suggestion_status_ordinal(row: &serde_json::Value) -> u8 {
        if row.get("acted_at").and_then(|v| v.as_str()).is_some() {
            3
        } else if row.get("dismissed_at").and_then(|v| v.as_str()).is_some() {
            2
        } else if row.get("shown_at").and_then(|v| v.as_str()).is_some() {
            1
        } else {
            0
        }
    }

    /// Apply a device-wide GDPR Art.17 `DeletionEvent` from `origin_device_id`.
    ///
    /// `bound` is the erasure HLC carried in the DeletionEvent watermark (#5181). When
    /// `Some((wall, counter))` the delete is BOUNDED to rows authored at-or-before the
    /// erasure moment, so a row re-created/re-synced AFTER the erasure (HLC > anchor —
    /// e.g. a post-re-grant capture) SURVIVES. When `None` (a pre-#5181 sender that
    /// stamped only `Hlc::now`, or an unstamped/ZERO watermark) the delete is unbounded
    /// — the conservative pre-#5181 behavior (R3 compat: over-erase, never under-erase).
    fn handle_deletion_event(
        conn: &Connection,
        origin_device_id: &str,
        bound: Option<(u64, u32)>,
    ) -> Result<usize, StorageError> {
        let tables = [
            "activity_segments",
            "regimes",
            "regime_overrides",
            "embedding_vectors",
            "suggestions",
            "trigger_params_snapshots",
        ];
        let mut total_deleted = 0usize;
        for table in &tables {
            let deleted = match bound {
                Some((bw, bc)) => {
                    // hlc <= (bw, bc), lexicographically — spares HLC strictly above the anchor.
                    let sql = format!(
                        "DELETE FROM {table} WHERE origin_device_id = ?1 \
                         AND (hlc_wall_ms < ?2 OR (hlc_wall_ms = ?2 AND hlc_counter <= ?3))"
                    );
                    conn.execute(&sql, rusqlite::params![origin_device_id, bw, bc])
                }
                None => {
                    let sql = format!("DELETE FROM {table} WHERE origin_device_id = ?1");
                    conn.execute(&sql, rusqlite::params![origin_device_id])
                }
            }
            .map_err(|e| StorageError::Internal(format!("GDPR deletion on {table}: {e}")))?;
            total_deleted += deleted;
        }
        info!(
            origin_device_id = origin_device_id,
            total_deleted = total_deleted,
            bounded = bound.is_some(),
            "GDPR Article 17 deletion event processed"
        );
        Ok(total_deleted)
    }
}

#[async_trait]
impl ChangeMerger for SqliteSyncMerger {
    async fn apply_changes(&self, changes: ChangeSet) -> Result<SyncResult, CoreError> {
        let conn = self.conn.clone();
        let local_device_id = self.local_device_id.clone();

        tokio::task::spawn_blocking(move || {
            // After consent is revoked (deletion_flag set), skip all sync merge writes. Even
            // when skipping, advance the watermark so sync progress is not blocked.
            let skipped = SyncResult {
                new_watermark: changes.watermark.clone(),
                ..Default::default()
            };
            conn.write_lock().run_mut(skipped, move |guard| {
                // SECURITY (#6560): reject any changeset claiming THIS device as origin BEFORE
                // dispatching on kind. The merger only ever applies REMOTE changes; a changeset
                // (data OR a DeletionEvent) whose origin == the local device can only come from a
                // hostile/relaying peer spoofing our identity. A self-origin DeletionEvent would
                // otherwise run `DELETE ... WHERE origin_device_id = <local>`, hard-deleting ALL of
                // this device's own data. The push receiver binds origin to the authenticated peer
                // (lan_server/handlers.rs #5211); this is the symmetric guard that ALSO covers the
                // pull path (which performs no such bind) — one chokepoint for both transports.
                if changes.origin_device_id == local_device_id {
                    debug!("skipping self-originated changeset (origin == local device)");
                    return Ok(SyncResult {
                        new_watermark: changes.watermark,
                        ..Default::default()
                    });
                }

                // Handle GDPR deletion event
                if changes.kind == ChangeSetKind::DeletionEvent {
                    // #5181: the watermark carries the sender's erasure HLC anchor; bound
                    // the delete by it. An unstamped/ZERO watermark = pre-#5181 sender →
                    // unbounded delete (R3 compat). Post-F0 all real rows have HLC > 0, so
                    // a ZERO bound would match nothing (under-erase) — hence the None guard.
                    let wm = &changes.watermark;
                    let bound = if (wm.wall_ms, wm.counter) == (0, 0) {
                        None
                    } else {
                        Some((wm.wall_ms, wm.counter))
                    };
                    let deleted =
                        Self::handle_deletion_event(guard, &changes.origin_device_id, bound)?;
                    return Ok(SyncResult {
                        tombstoned: deleted,
                        new_watermark: changes.watermark,
                        ..Default::default()
                    });
                }

                let mut result = SyncResult::default();

                // All merge operations run inside a single transaction
                let tx = guard.transaction().map_err(|e| CoreError::Storage {
                    code: maekon_core::error_codes::StorageCode::Failed,
                    message: format!("begin transaction: {e}"),
                })?;

                // --- Row-level erasure tombstones (#5174 S3) ---
                // Applied FIRST so the local suppression set is populated before any merge
                // below attempts an insert (anti-resurrection within the same changeset).
                for t in &changes.tombstones {
                    // SECURITY (#6560): `apply_tombstone` hard-deletes
                    // `WHERE <pk> = row_id AND origin_device_id = t.origin_device_id`. A peer-origin
                    // changeset (which passes the changeset-level self guard above) can still carry a
                    // tombstone forged with OUR origin + one of our row ids, letting a hostile peer
                    // delete this device's own rows by id. We never need a remote peer to erase our
                    // own-origin data — local erasure is driven locally. (Legit tombstones carry the
                    // ERASED row's original origin so relays can forward content-free erasures, so the
                    // reject is scoped to the local-origin case; peer-origin tombstones still apply.)
                    if t.origin_device_id == local_device_id {
                        warn!(
                            table = %t.table_name,
                            row_id = %t.row_id,
                            "rejecting sync tombstone: origin claims the local device (self-erasure spoofing guard)"
                        );
                        continue;
                    }
                    // Far-future poison guard: a tombstone with an implausibly
                    // far-future HLC would suppress all legitimate future writes.
                    if !Hlc::wall_ms_within_drift_bound(t.hlc_wall_ms) {
                        warn!(
                            table = %t.table_name,
                            hlc_wall_ms = t.hlc_wall_ms,
                            "rejecting sync tombstone: HLC wall-clock exceeds max clock drift (far-future poison guard)"
                        );
                        continue;
                    }
                    apply_tombstone(&tx, t, &mut result)?;
                }

                // --- Append-only tables ---
                for row in &changes.segments {
                    if hlc_drift_rejected(row, "segments") {
                        continue;
                    }
                    merge_segment(&tx, row, &mut result)?;
                }
                for row in &changes.overrides {
                    if hlc_drift_rejected(row, "overrides") {
                        continue;
                    }
                    merge_override(&tx, row, &mut result)?;
                }
                for row in &changes.param_snapshots {
                    if hlc_drift_rejected(row, "param_snapshots") {
                        continue;
                    }
                    merge_param_snapshot(&tx, row, &mut result)?;
                }

                // --- LWW tables ---
                for row in &changes.regimes {
                    if hlc_drift_rejected(row, "regimes") {
                        continue;
                    }
                    merge_regime(&tx, row, &mut result)?;
                }
                for row in &changes.embeddings {
                    if hlc_drift_rejected(row, "embeddings") {
                        continue;
                    }
                    merge_embedding(&tx, row, &mut result)?;
                }

                // --- Monotonic status merge (suggestions) ---
                for row in &changes.suggestions {
                    if hlc_drift_rejected(row, "suggestions") {
                        continue;
                    }
                    merge_suggestion(&tx, row, &mut result)?;
                }

                // Update sync_peers watermark
                tx.execute(
                    "INSERT INTO sync_peers (device_id, device_name, last_sync_at, \
                 watermark_wall_ms, watermark_counter) \
                 VALUES (?1, ?2, datetime('now'), ?3, ?4) \
                 ON CONFLICT(device_id) DO UPDATE SET \
                   device_name = excluded.device_name, \
                   last_sync_at = excluded.last_sync_at, \
                   watermark_wall_ms = excluded.watermark_wall_ms, \
                   watermark_counter = excluded.watermark_counter",
                    rusqlite::params![
                        changes.origin_device_id,
                        changes.origin_device_name,
                        changes.watermark.wall_ms,
                        changes.watermark.counter,
                    ],
                )
                .map_err(|e| CoreError::Storage {
                    code: maekon_core::error_codes::StorageCode::Failed,
                    message: format!("update sync_peers: {e}"),
                })?;

                tx.commit().map_err(|e| CoreError::Storage {
                    code: maekon_core::error_codes::StorageCode::Failed,
                    message: format!("commit transaction: {e}"),
                })?;

                result.new_watermark = changes.watermark;

                debug!(
                    applied = result.applied,
                    skipped_lww = result.skipped_lww,
                    skipped_dup = result.skipped_dup,
                    tombstoned = result.tombstoned,
                    "changeset merge completed"
                );

                Ok(result)
            })
        })
        .await
        .map_err(|e| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("spawn_blocking join error: {e}"),
        })?
    }
}

// ── Row-level erasure tombstone application + suppression (#5174 S3) ──

/// Map a synced `table_name` to its primary-key column (for the hard-DELETE). Returns
/// `None` for an unknown table — never format an unvalidated wire-supplied name into SQL.
/// (`embedding_vectors` is handled separately by its composite key, not via this.)
fn tombstone_pk_col(table_name: &str) -> Option<&'static str> {
    match table_name {
        "activity_segments" | "regimes" | "trigger_params_snapshots" => Some("id"),
        "regime_overrides" => Some("override_id"),
        "suggestions" => Some("suggestion_id"),
        _ => None,
    }
}

/// Apply one incoming tombstone: hard-DELETE the row (content gone on this peer, GDPR-
/// complete) and record it into the local `sync_tombstones` suppression set (keep-higher-HLC).
/// Origin-scoped so it only erases the erasing device's rows.
fn apply_tombstone(
    conn: &Connection,
    t: &Tombstone,
    result: &mut SyncResult,
) -> Result<(), StorageError> {
    let deleted = if t.table_name == "embedding_vectors" {
        // embeddings: row_id is the cross-device-stable composite `segment_id US model_id`
        // (its `id` is a per-device autoincrement, so it can't be matched across peers).
        let (segment_id, model_id) = match t.row_id.split_once(EMB_KEY_SEP) {
            Some(parts) => parts,
            None => {
                // A malformed key would DELETE with an empty model_id and silently match
                // nothing — a GDPR erasure that quietly fails. Make it observable.
                warn!(
                    row_id = %t.row_id,
                    "embedding tombstone missing composite separator; erasure may not match"
                );
                (t.row_id.as_str(), "")
            }
        };
        conn.execute(
            "DELETE FROM embedding_vectors \
             WHERE segment_id = ?1 AND model_id = ?2 AND origin_device_id = ?3",
            rusqlite::params![segment_id, model_id, t.origin_device_id],
        )
        .map_err(|e| StorageError::Internal(format!("tombstone delete embedding: {e}")))?
    } else {
        let Some(pk) = tombstone_pk_col(&t.table_name) else {
            warn!(table = %t.table_name, "ignoring tombstone for unknown table");
            return Ok(());
        };
        conn.execute(
            &format!(
                "DELETE FROM {} WHERE {} = ?1 AND origin_device_id = ?2",
                t.table_name, pk
            ),
            rusqlite::params![t.row_id, t.origin_device_id],
        )
        .map_err(|e| StorageError::Internal(format!("tombstone delete {}: {e}", t.table_name)))?
    };
    result.tombstoned += deleted;

    // Record into the local suppression set, keeping the higher HLC (out-of-order safe, P3).
    conn.execute(
        "INSERT INTO sync_tombstones \
         (table_name, row_id, origin_device_id, hlc_wall_ms, hlc_counter, deleted_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
         ON CONFLICT(table_name, row_id) DO UPDATE SET \
           origin_device_id = excluded.origin_device_id, \
           hlc_wall_ms = excluded.hlc_wall_ms, \
           hlc_counter = excluded.hlc_counter, \
           deleted_at  = excluded.deleted_at \
         WHERE excluded.hlc_wall_ms > sync_tombstones.hlc_wall_ms \
            OR (excluded.hlc_wall_ms = sync_tombstones.hlc_wall_ms \
                AND excluded.hlc_counter > sync_tombstones.hlc_counter)",
        rusqlite::params![
            t.table_name,
            t.row_id,
            t.origin_device_id,
            t.hlc_wall_ms,
            t.hlc_counter,
            t.deleted_at
        ],
    )
    .map_err(|e| StorageError::Internal(format!("record suppression tombstone: {e}")))?;
    Ok(())
}

/// Suppression gate run at the top of every merge fn. Returns `true` if an incoming row
/// `(table_name, row_id)` with HLC `(iw, ic)` must be SUPPRESSED because a tombstone with
/// HLC >= it exists (anti-resurrection; idempotent on exact `==` replay). When the incoming
/// HLC is strictly higher (post-re-grant), the superseded tombstone is cleared and `false`
/// is returned so the row applies normally (P1). Compares the `(wall, counter)` pair only —
/// NOT via `Hlc::is_after`, which would tiebreak on `device_id`.
fn tombstone_suppresses(
    conn: &Connection,
    table_name: &str,
    row_id: &str,
    iw: u64,
    ic: u32,
) -> Result<bool, StorageError> {
    let existing: Option<(u64, u32)> = conn
        .query_row(
            "SELECT hlc_wall_ms, hlc_counter FROM sync_tombstones \
             WHERE table_name = ?1 AND row_id = ?2",
            rusqlite::params![table_name, row_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| StorageError::Internal(format!("suppression lookup: {e}")))?;
    match existing {
        None => Ok(false),
        Some((tw, tc)) => {
            if (iw, ic) <= (tw, tc) {
                Ok(true) // incoming <= tombstone → suppress (idempotent on ==)
            } else {
                // Post-re-grant: strictly-higher HLC wins → clear the stale tombstone, apply.
                conn.execute(
                    "DELETE FROM sync_tombstones WHERE table_name = ?1 AND row_id = ?2",
                    rusqlite::params![table_name, row_id],
                )
                .map_err(|e| StorageError::Internal(format!("clear superseded tombstone: {e}")))?;
                Ok(false)
            }
        }
    }
}

// ── Per-table merge functions (called inside transaction) ──

fn merge_segment(
    conn: &Connection,
    row: &serde_json::Value,
    result: &mut SyncResult,
) -> Result<(), StorageError> {
    let id = json_str(row, "id")?;
    // #6174: reject just this row if the wire HLC counter overflows u32 (no silent truncation).
    let Some(hlc_counter) = extract_hlc_counter(row, id)? else {
        result.skipped_dup += 1;
        return Ok(());
    };
    // #5174 S3: suppress if a tombstone with HLC >= this row exists (anti-resurrection).
    if tombstone_suppresses(
        conn,
        "activity_segments",
        id,
        json_u64(row, "hlc_wall_ms")?,
        hlc_counter,
    )? {
        result.skipped_dup += 1;
        return Ok(());
    }
    let exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM activity_segments WHERE id = ?1",
            rusqlite::params![id],
            |r| r.get(0),
        )
        .map_err(|e| StorageError::Internal(format!("check segment: {e}")))?;

    if exists {
        result.skipped_dup += 1;
        return Ok(());
    }

    conn.execute(
        "INSERT INTO activity_segments \
         (id, start_time, end_time, duration_secs, trigger_reason, regime_id, \
          dominant_category, app_breakdown, llm_summary, content_activities_json, \
          hlc_wall_ms, hlc_counter, origin_device_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        rusqlite::params![
            id,
            json_str(row, "start_time")?,
            json_str(row, "end_time")?,
            json_i64(row, "duration_secs")?,
            // #5202: trigger_reason is `TEXT NOT NULL` (no default). The extractor now
            // emits it; default for a pre-#5202 peer's changeset (which omits it) so the
            // insert never hits the NOT NULL constraint and silently rolls back the merge.
            json_str_or_default(row, "trigger_reason", "sync"),
            json_str_opt(row, "regime_id"),
            json_str(row, "dominant_category")?,
            json_str_or_default(row, "app_breakdown", "{}"),
            json_str_opt(row, "llm_summary"),
            json_str_or_default(row, "content_activities_json", "[]"),
            json_u64(row, "hlc_wall_ms")?,
            hlc_counter,
            json_str(row, "origin_device_id")?,
        ],
    )
    .map_err(|e| StorageError::Internal(format!("insert segment: {e}")))?;

    result.applied += 1;
    Ok(())
}

fn merge_regime(
    conn: &Connection,
    row: &serde_json::Value,
    result: &mut SyncResult,
) -> Result<(), StorageError> {
    let id = json_str(row, "id")?;
    // #6174: reject just this row if the wire HLC counter overflows u32 (no silent truncation).
    let Some(remote_hlc) = extract_hlc(row, id)? else {
        result.skipped_dup += 1;
        return Ok(());
    };
    // #5174 S3: anti-resurrection suppression (gates BOTH the insert and the LWW update).
    if tombstone_suppresses(conn, "regimes", id, remote_hlc.wall_ms, remote_hlc.counter)? {
        result.skipped_dup += 1;
        return Ok(());
    }

    let local: Option<(u64, u32, String)> = conn
        .query_row(
            "SELECT hlc_wall_ms, hlc_counter, origin_device_id FROM regimes WHERE id = ?1",
            rusqlite::params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(|e| StorageError::Internal(format!("lookup regime {id}: {e}")))?;

    match local {
        None => {
            conn.execute(
                "INSERT INTO regimes \
                 (id, label, detected_at, last_seen_at, occurrence_count, \
                  avg_density, avg_importance, dominant_category, params_snapshot_id, \
                  is_active, is_deleted, deleted_at, \
                  hlc_wall_ms, hlc_counter, origin_device_id) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
                rusqlite::params![
                    id,
                    json_str(row, "label")?,
                    json_str(row, "detected_at")?,
                    json_str(row, "last_seen_at")?,
                    json_i64(row, "occurrence_count")?,
                    json_f64(row, "avg_density")?,
                    json_f64(row, "avg_importance")?,
                    json_str(row, "dominant_category")?,
                    json_str_opt(row, "params_snapshot_id"),
                    json_i64(row, "is_active")?,
                    json_i64_or_default(row, "is_deleted", 0),
                    json_str_opt(row, "deleted_at"),
                    remote_hlc.wall_ms,
                    remote_hlc.counter,
                    json_str(row, "origin_device_id")?,
                ],
            )
            .map_err(|e| StorageError::Internal(format!("insert regime: {e}")))?;
            result.applied += 1;
        }
        Some((lw, lc, ld)) => {
            let local_hlc = Hlc {
                wall_ms: lw,
                counter: lc,
                device_id: ld,
            };
            if remote_hlc.is_after(&local_hlc) {
                warn!(
                    regime_id = %id,
                    local_device = %local_hlc.device_id,
                    remote_device = %remote_hlc.device_id,
                    local_hlc_ms = local_hlc.wall_ms,
                    remote_hlc_ms = remote_hlc.wall_ms,
                    "sync conflict: regime overwritten by remote (LWW)"
                );
                conn.execute(
                    "UPDATE regimes SET label=?2, detected_at=?3, last_seen_at=?4, \
                     occurrence_count=?5, avg_density=?6, avg_importance=?7, \
                     dominant_category=?8, params_snapshot_id=?9, is_active=?10, \
                     is_deleted=?11, deleted_at=?12, \
                     hlc_wall_ms=?13, hlc_counter=?14, origin_device_id=?15 \
                     WHERE id = ?1",
                    rusqlite::params![
                        id,
                        json_str(row, "label")?,
                        json_str(row, "detected_at")?,
                        json_str(row, "last_seen_at")?,
                        json_i64(row, "occurrence_count")?,
                        json_f64(row, "avg_density")?,
                        json_f64(row, "avg_importance")?,
                        json_str(row, "dominant_category")?,
                        json_str_opt(row, "params_snapshot_id"),
                        json_i64(row, "is_active")?,
                        json_i64_or_default(row, "is_deleted", 0),
                        json_str_opt(row, "deleted_at"),
                        remote_hlc.wall_ms,
                        remote_hlc.counter,
                        json_str(row, "origin_device_id")?,
                    ],
                )
                .map_err(|e| StorageError::Internal(format!("update regime: {e}")))?;

                let is_tombstone = json_i64_or_default(row, "is_deleted", 0) == 1;
                if is_tombstone {
                    result.tombstoned += 1;
                } else {
                    result.applied += 1;
                }
            } else {
                debug!(
                    regime_id = %id,
                    local_device = %local_hlc.device_id,
                    remote_device = %remote_hlc.device_id,
                    "sync conflict: remote regime discarded (local wins LWW)"
                );
                result.skipped_lww += 1;
            }
        }
    }
    Ok(())
}

fn merge_override(
    conn: &Connection,
    row: &serde_json::Value,
    result: &mut SyncResult,
) -> Result<(), StorageError> {
    let id = json_str(row, "override_id")?;
    // #6174: reject just this row if the wire HLC counter overflows u32 (no silent truncation).
    let Some(hlc_counter) = extract_hlc_counter(row, id)? else {
        result.skipped_dup += 1;
        return Ok(());
    };
    // #5174 S3: anti-resurrection suppression.
    if tombstone_suppresses(
        conn,
        "regime_overrides",
        id,
        json_u64(row, "hlc_wall_ms")?,
        hlc_counter,
    )? {
        result.skipped_dup += 1;
        return Ok(());
    }
    let exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM regime_overrides WHERE override_id = ?1",
            rusqlite::params![id],
            |r| r.get(0),
        )
        .map_err(|e| StorageError::Internal(format!("check override: {e}")))?;

    if exists {
        result.skipped_dup += 1;
        return Ok(());
    }

    conn.execute(
        "INSERT INTO regime_overrides \
         (override_id, segment_id, original_regime_id, action_type, action_data, \
          created_at, hlc_wall_ms, hlc_counter, origin_device_id) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        rusqlite::params![
            id,
            json_str(row, "segment_id")?,
            json_str_opt(row, "original_regime_id"),
            json_str(row, "action_type")?,
            json_str_opt(row, "action_data"),
            json_str(row, "created_at")?,
            json_u64(row, "hlc_wall_ms")?,
            hlc_counter,
            json_str(row, "origin_device_id")?,
        ],
    )
    .map_err(|e| StorageError::Internal(format!("insert override: {e}")))?;
    result.applied += 1;
    Ok(())
}

fn merge_embedding(
    conn: &Connection,
    row: &serde_json::Value,
    result: &mut SyncResult,
) -> Result<(), StorageError> {
    let segment_id = json_str(row, "segment_id")?;
    let model_id = json_str(row, "model_id")?;
    // #6174: reject just this row if the wire HLC counter overflows u32 (no silent truncation).
    let emb_label = format!("{segment_id}/{model_id}");
    let Some(remote_hlc) = extract_hlc(row, &emb_label)? else {
        result.skipped_dup += 1;
        return Ok(());
    };
    // #5174 S3: anti-resurrection suppression by the cross-device-stable composite key
    // (`id` is a per-device autoincrement; the tombstone keys on segment_id+model_id).
    let emb_key = format!("{segment_id}{EMB_KEY_SEP}{model_id}");
    if tombstone_suppresses(
        conn,
        "embedding_vectors",
        &emb_key,
        remote_hlc.wall_ms,
        remote_hlc.counter,
    )? {
        result.skipped_dup += 1;
        return Ok(());
    }

    let local: Option<(i64, u64, u32, String)> = conn
        .query_row(
            "SELECT id, hlc_wall_ms, hlc_counter, origin_device_id \
             FROM embedding_vectors WHERE segment_id = ?1 AND model_id = ?2",
            rusqlite::params![segment_id, model_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .map_err(|e| {
            StorageError::Internal(format!("lookup embedding {segment_id}/{model_id}: {e}"))
        })?;

    match local {
        None => {
            // Decode hex-encoded vector back to BLOB. A decode failure (truncated /
            // odd-length / non-hex from a corrupt or hostile peer) must NOT collapse to
            // an empty blob — that would silently store a zero-length vector. Reject just
            // this row (counted as skipped, warning already logged naming the row) and
            // keep merging the rest of the changeset (#35).
            let vector_hex = json_str(row, "vector")?;
            let Some(vector_bytes) = decode_vector(vector_hex, segment_id, model_id) else {
                result.skipped_dup += 1;
                return Ok(());
            };

            conn.execute(
                "INSERT INTO embedding_vectors \
                 (segment_id, content_type, content_label, original_text, \
                  vector, model_id, timestamp, is_stale, \
                  is_deleted, deleted_at, \
                  hlc_wall_ms, hlc_counter, origin_device_id) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
                rusqlite::params![
                    segment_id,
                    json_str(row, "content_type")?,
                    json_str_opt(row, "content_label"),
                    json_str_opt(row, "original_text"),
                    vector_bytes,
                    model_id,
                    json_str(row, "timestamp")?,
                    json_i64_or_default(row, "is_stale", 0),
                    json_i64_or_default(row, "is_deleted", 0),
                    json_str_opt(row, "deleted_at"),
                    remote_hlc.wall_ms,
                    remote_hlc.counter,
                    json_str(row, "origin_device_id")?,
                ],
            )
            .map_err(|e| StorageError::Internal(format!("insert embedding: {e}")))?;
            result.applied += 1;
        }
        Some((local_id, lw, lc, ld)) => {
            let local_hlc = Hlc {
                wall_ms: lw,
                counter: lc,
                device_id: ld,
            };
            if remote_hlc.is_after(&local_hlc) {
                // Decode BEFORE clobbering: a corrupt remote vector must never overwrite
                // a valid local vector with an empty blob. Reject just this row (counted
                // as skipped, warning already logged naming the row) and leave the local
                // vector untouched (#35).
                let vector_hex = json_str(row, "vector")?;
                let Some(vector_bytes) = decode_vector(vector_hex, segment_id, model_id) else {
                    result.skipped_dup += 1;
                    return Ok(());
                };
                warn!(
                    segment_id = %segment_id,
                    model_id = %model_id,
                    local_device = %local_hlc.device_id,
                    remote_device = %remote_hlc.device_id,
                    "sync conflict: embedding overwritten by remote (LWW)"
                );

                conn.execute(
                    "UPDATE embedding_vectors SET \
                     content_type=?2, content_label=?3, original_text=?4, \
                     vector=?5, model_id=?6, timestamp=?7, is_stale=?8, \
                     is_deleted=?9, deleted_at=?10, \
                     hlc_wall_ms=?11, hlc_counter=?12, origin_device_id=?13 \
                     WHERE id = ?1",
                    rusqlite::params![
                        local_id,
                        json_str(row, "content_type")?,
                        json_str_opt(row, "content_label"),
                        json_str_opt(row, "original_text"),
                        vector_bytes,
                        json_str(row, "model_id")?,
                        json_str(row, "timestamp")?,
                        json_i64_or_default(row, "is_stale", 0),
                        json_i64_or_default(row, "is_deleted", 0),
                        json_str_opt(row, "deleted_at"),
                        remote_hlc.wall_ms,
                        remote_hlc.counter,
                        json_str(row, "origin_device_id")?,
                    ],
                )
                .map_err(|e| StorageError::Internal(format!("update embedding: {e}")))?;

                let is_tombstone = json_i64_or_default(row, "is_deleted", 0) == 1;
                if is_tombstone {
                    result.tombstoned += 1;
                } else {
                    result.applied += 1;
                }
            } else {
                result.skipped_lww += 1;
            }
        }
    }
    Ok(())
}

#[allow(clippy::type_complexity)]
fn merge_suggestion(
    conn: &Connection,
    row: &serde_json::Value,
    result: &mut SyncResult,
) -> Result<(), StorageError> {
    let suggestion_id = json_str(row, "suggestion_id")?;
    // #6174: reject just this row if the wire HLC counter overflows u32 (no silent truncation).
    let Some(remote_hlc) = extract_hlc(row, suggestion_id)? else {
        result.skipped_dup += 1;
        return Ok(());
    };
    // #5174 S3 (the IMPORTANT-2 fix): run the suppression gate BEFORE the status-monotonic
    // merge below. A tombstone is a hard delete already applied; once one exists with HLC >=
    // this row, we return here so a re-synced lower-HLC `acted` row can NEVER resurrect an
    // erased suggestion by winning on status ordinal. Only a strictly-higher-HLC (post-
    // re-grant) suggestion passes, and for that the status-monotonic logic stays correct.
    if tombstone_suppresses(
        conn,
        "suggestions",
        suggestion_id,
        remote_hlc.wall_ms,
        remote_hlc.counter,
    )? {
        result.skipped_dup += 1;
        return Ok(());
    }
    let remote_status = SqliteSyncMerger::suggestion_status_ordinal(row);

    let local: Option<(
        u64,
        u32,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
    )> = conn
        .query_row(
            "SELECT hlc_wall_ms, hlc_counter, origin_device_id, \
             shown_at, dismissed_at, acted_at \
             FROM suggestions WHERE suggestion_id = ?1",
            rusqlite::params![suggestion_id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .optional()
        .map_err(|e| StorageError::Internal(format!("lookup suggestion {suggestion_id}: {e}")))?;

    match local {
        None => {
            conn.execute(
                "INSERT INTO suggestions \
                 (suggestion_id, suggestion_type, source, content, priority, \
                  confidence_score, relevance_score, is_actionable, reasoning, \
                  context_app, context_window, context_target_id, \
                  shown_at, dismissed_at, acted_at, created_at, expires_at, \
                  is_deleted, deleted_at, \
                  hlc_wall_ms, hlc_counter, origin_device_id) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22)",
                rusqlite::params![
                    suggestion_id,
                    json_str(row, "suggestion_type")?,
                    json_str(row, "source")?,
                    json_str(row, "content")?,
                    json_str(row, "priority")?,
                    json_f64(row, "confidence_score")?,
                    json_f64(row, "relevance_score")?,
                    json_i64(row, "is_actionable")?,
                    json_str_opt(row, "reasoning"),
                    json_str_opt(row, "context_app"),
                    json_str_opt(row, "context_window"),
                    json_str_opt(row, "context_target_id"),
                    json_str_opt(row, "shown_at"),
                    json_str_opt(row, "dismissed_at"),
                    json_str_opt(row, "acted_at"),
                    json_str(row, "created_at")?,
                    json_str_opt(row, "expires_at"),
                    json_i64_or_default(row, "is_deleted", 0),
                    json_str_opt(row, "deleted_at"),
                    remote_hlc.wall_ms,
                    remote_hlc.counter,
                    json_str(row, "origin_device_id")?,
                ],
            )
            .map_err(|e| StorageError::Internal(format!("insert suggestion: {e}")))?;
            result.applied += 1;
        }
        Some((lw, lc, ld, shown, dismissed, acted)) => {
            // Compute local status ordinal
            let local_status = if acted.is_some() {
                3
            } else if dismissed.is_some() {
                2
            } else if shown.is_some() {
                1
            } else {
                0
            };

            // Monotonic merge: higher status always wins
            let remote_wins = if remote_status != local_status {
                remote_status > local_status
            } else {
                // Same status -- fall back to HLC LWW
                let local_hlc = Hlc {
                    wall_ms: lw,
                    counter: lc,
                    device_id: ld,
                };
                remote_hlc.is_after(&local_hlc)
            };

            if remote_wins {
                conn.execute(
                    "UPDATE suggestions SET \
                     suggestion_type=?2, source=?3, content=?4, priority=?5, \
                     confidence_score=?6, relevance_score=?7, is_actionable=?8, \
                     reasoning=?9, context_app=?10, context_window=?11, \
                     context_target_id=?12, shown_at=?13, dismissed_at=?14, acted_at=?15, \
                     expires_at=?16, is_deleted=?17, deleted_at=?18, \
                     hlc_wall_ms=?19, hlc_counter=?20, origin_device_id=?21 \
                     WHERE suggestion_id = ?1",
                    rusqlite::params![
                        suggestion_id,
                        json_str(row, "suggestion_type")?,
                        json_str(row, "source")?,
                        json_str(row, "content")?,
                        json_str(row, "priority")?,
                        json_f64(row, "confidence_score")?,
                        json_f64(row, "relevance_score")?,
                        json_i64(row, "is_actionable")?,
                        json_str_opt(row, "reasoning"),
                        json_str_opt(row, "context_app"),
                        json_str_opt(row, "context_window"),
                        json_str_opt(row, "context_target_id"),
                        json_str_opt(row, "shown_at"),
                        json_str_opt(row, "dismissed_at"),
                        json_str_opt(row, "acted_at"),
                        json_str_opt(row, "expires_at"),
                        json_i64_or_default(row, "is_deleted", 0),
                        json_str_opt(row, "deleted_at"),
                        remote_hlc.wall_ms,
                        remote_hlc.counter,
                        json_str(row, "origin_device_id")?,
                    ],
                )
                .map_err(|e| StorageError::Internal(format!("update suggestion: {e}")))?;
                result.applied += 1;
            } else {
                result.skipped_lww += 1;
            }
        }
    }
    Ok(())
}

fn merge_param_snapshot(
    conn: &Connection,
    row: &serde_json::Value,
    result: &mut SyncResult,
) -> Result<(), StorageError> {
    let id = json_str(row, "id")?;
    // #6174: reject just this row if the wire HLC counter overflows u32 (no silent truncation).
    let Some(hlc_counter) = extract_hlc_counter(row, id)? else {
        result.skipped_dup += 1;
        return Ok(());
    };
    // #5174 S3: anti-resurrection suppression.
    if tombstone_suppresses(
        conn,
        "trigger_params_snapshots",
        id,
        json_u64(row, "hlc_wall_ms")?,
        hlc_counter,
    )? {
        result.skipped_dup += 1;
        return Ok(());
    }
    let exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM trigger_params_snapshots WHERE id = ?1",
            rusqlite::params![id],
            |r| r.get(0),
        )
        .map_err(|e| StorageError::Internal(format!("check param_snapshot: {e}")))?;

    if exists {
        result.skipped_dup += 1;
        return Ok(());
    }

    conn.execute(
        "INSERT INTO trigger_params_snapshots \
         (id, created_at, preset, params_json, hlc_wall_ms, hlc_counter, origin_device_id) \
         VALUES (?1,?2,?3,?4,?5,?6,?7)",
        rusqlite::params![
            id,
            json_str(row, "created_at")?,
            json_str(row, "preset")?,
            json_str(row, "params_json")?,
            json_u64(row, "hlc_wall_ms")?,
            hlc_counter,
            json_str(row, "origin_device_id")?,
        ],
    )
    .map_err(|e| StorageError::Internal(format!("insert param_snapshot: {e}")))?;
    result.applied += 1;
    Ok(())
}

// ── JSON extraction helpers ──

fn json_str<'a>(v: &'a serde_json::Value, key: &str) -> Result<&'a str, StorageError> {
    v.get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| StorageError::Internal(format!("missing string field: {key}")))
}

fn json_str_opt(v: &serde_json::Value, key: &str) -> Option<String> {
    v.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}

fn json_str_or_default<'a>(v: &'a serde_json::Value, key: &str, default: &'a str) -> &'a str {
    v.get(key).and_then(|v| v.as_str()).unwrap_or(default)
}

fn json_i64(v: &serde_json::Value, key: &str) -> Result<i64, StorageError> {
    v.get(key)
        .and_then(|v| v.as_i64())
        .ok_or_else(|| StorageError::Internal(format!("missing i64 field: {key}")))
}

fn json_i64_or_default(v: &serde_json::Value, key: &str, default: i64) -> i64 {
    v.get(key).and_then(|v| v.as_i64()).unwrap_or(default)
}

fn json_u64(v: &serde_json::Value, key: &str) -> Result<u64, StorageError> {
    v.get(key)
        .and_then(|v| v.as_u64())
        .ok_or_else(|| StorageError::Internal(format!("missing u64 field: {key}")))
}

/// Far-future poison guard for cross-device sync ingestion.
///
/// `Hlc::merge` rejects peer HLCs more than `MAX_CLOCK_DRIFT_MS` (1h) ahead of the
/// local clock, but the merge path here writes peer `hlc_wall_ms` straight into the
/// LWW/tombstone tables without calling `merge`. A buggy or compromised paired
/// device could otherwise stamp a far-future wall-clock to permanently win every
/// LWW conflict and have its tombstones suppress legitimate future writes. Returns
/// `true` (skip the row) only when the HLC is present and implausibly far ahead;
/// a missing/invalid HLC returns `false` so the per-table merge surfaces that error.
fn hlc_drift_rejected(row: &serde_json::Value, table: &str) -> bool {
    match json_u64(row, "hlc_wall_ms") {
        Ok(wall) if !Hlc::wall_ms_within_drift_bound(wall) => {
            warn!(
                table = %table,
                hlc_wall_ms = wall,
                "rejecting sync row: HLC wall-clock exceeds max clock drift (far-future poison guard)"
            );
            true
        }
        _ => false,
    }
}

fn json_f64(v: &serde_json::Value, key: &str) -> Result<f64, StorageError> {
    v.get(key)
        .and_then(|v| v.as_f64())
        .ok_or_else(|| StorageError::Internal(format!("missing f64 field: {key}")))
}

/// Extract a wire-supplied HLC counter, rejecting just the offending row on overflow.
///
/// #6174: the `hlc_counter` field is part of every synced row's HLC. A missing field is a
/// structurally malformed changeset and stays a hard error (same as every other required
/// field below). But a *present* counter that exceeds `u32::MAX` (corrupt/buggy/hostile
/// peer) must NEVER be silently truncated — that would corrupt causal ordering on merge.
/// Mirroring [`decode_vector`], an out-of-range counter logs a warning naming the row and
/// returns `None` so the caller REJECTS just that row and keeps merging the rest of the
/// changeset, instead of aborting the whole transaction (and stalling sync, since the
/// watermark would never advance) over one bad peer row.
fn extract_hlc_counter(
    row: &serde_json::Value,
    row_label: &str,
) -> Result<Option<u32>, StorageError> {
    let raw = json_u64(row, "hlc_counter")?;
    match u32::try_from(raw) {
        Ok(counter) => Ok(Some(counter)),
        Err(_) => {
            warn!(
                row = %row_label,
                "rejected remote row with out-of-range HLC counter (would corrupt causal ordering): {raw}"
            );
            Ok(None)
        }
    }
}

/// Extract a full HLC from a wire row. Returns `None` (caller skips just this row) when the
/// counter is out of `u32` range — see [`extract_hlc_counter`] (#6174).
fn extract_hlc(row: &serde_json::Value, row_label: &str) -> Result<Option<Hlc>, StorageError> {
    let Some(counter) = extract_hlc_counter(row, row_label)? else {
        return Ok(None);
    };
    Ok(Some(Hlc {
        wall_ms: json_u64(row, "hlc_wall_ms")?,
        counter,
        device_id: json_str(row, "origin_device_id")?.to_string(),
    }))
}

/// Decode a wire-supplied hex embedding vector back to its BLOB bytes.
///
/// #35 / #6081: a malformed hex string (truncated, odd-length, or non-hex char from a
/// corrupt/buggy/hostile peer) must NEVER silently collapse to an empty `Vec<u8>` that
/// would overwrite a valid local vector with a zero-length blob under last-write-wins.
/// Returns `None` on a decode failure (logging a warning that names the offending row)
/// so the caller REJECTS just that row and keeps merging the rest of the changeset,
/// instead of aborting the whole transaction over one bad peer row.
fn decode_vector(vector_hex: &str, segment_id: &str, model_id: &str) -> Option<Vec<u8>> {
    match hex::decode(vector_hex) {
        Ok(bytes) => Some(bytes),
        Err(e) => {
            warn!(
                segment_id = %segment_id,
                model_id = %model_id,
                "rejected corrupt remote embedding vector (malformed hex): {e}"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::SqliteStorage;

    fn setup() -> (SqliteStorage, String) {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let (device_id, _) = storage.ensure_device_identity("Local").unwrap();
        (storage, device_id)
    }

    #[tokio::test]
    async fn empty_changeset_returns_zero_counts() {
        let (storage, device_id) = setup();
        let merger = SqliteSyncMerger::new(storage.connection_arc(), device_id);
        let cs = ChangeSet {
            origin_device_id: "remote-dev".to_string(),
            origin_device_name: "Remote".to_string(),
            ..Default::default()
        };
        let result = merger.apply_changes(cs).await.unwrap();
        assert_eq!(result.applied, 0);
        assert_eq!(result.skipped_lww, 0);
        assert_eq!(result.skipped_dup, 0);
    }

    #[tokio::test]
    async fn self_originated_changeset_is_skipped() {
        let (storage, device_id) = setup();
        let merger = SqliteSyncMerger::new(storage.connection_arc(), device_id.clone());
        let cs = ChangeSet {
            origin_device_id: device_id,
            origin_device_name: "Local".to_string(),
            segments: vec![serde_json::json!({"id": "seg-1"})],
            ..Default::default()
        };
        let result = merger.apply_changes(cs).await.unwrap();
        assert_eq!(result.applied, 0);
    }

    #[tokio::test]
    async fn deletion_event_hard_deletes() {
        let (storage, local_id) = setup();
        let remote_id = "remote-dev";

        // Insert a segment from the remote device
        {
            let conn = storage.connection_arc();
            let guard = conn.test_lock();
            guard
                .execute(
                    "INSERT INTO activity_segments \
                 (id, start_time, end_time, duration_secs, trigger_reason, \
                  dominant_category, hlc_wall_ms, hlc_counter, origin_device_id) \
                 VALUES ('seg-r1', '2026-01-01', '2026-01-01', 3600, 'timer', \
                         'Dev', 100, 1, ?1)",
                    rusqlite::params![remote_id],
                )
                .unwrap();
        }

        let merger = SqliteSyncMerger::new(storage.connection_arc(), local_id);
        let cs = ChangeSet {
            kind: ChangeSetKind::DeletionEvent,
            origin_device_id: remote_id.to_string(),
            origin_device_name: "Remote".to_string(),
            ..Default::default()
        };
        let result = merger.apply_changes(cs).await.unwrap();
        assert!(result.tombstoned > 0);

        // Verify row is gone
        let conn = storage.connection_arc();
        let guard = conn.test_lock();
        let count: i64 = guard
            .query_row(
                "SELECT COUNT(*) FROM activity_segments WHERE id = 'seg-r1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn deletion_event_bounded_spares_post_anchor_rows() {
        // #5181: a DeletionEvent stamped with the erasure HLC anchor must delete the
        // erasing device's pre-erasure rows but SPARE a post-re-grant row (HLC > anchor).
        let (storage, local_id) = setup();
        let remote_id = "remote-dev";
        {
            let conn = storage.connection_arc();
            let guard = conn.test_lock();
            // pre-erasure row (HLC 100,1 < anchor) and post-re-grant row (HLC 200,0 > anchor).
            for (id, w, c) in [("seg-old", 100, 1), ("seg-new", 200, 0)] {
                guard
                    .execute(
                        "INSERT INTO activity_segments (id, start_time, end_time, duration_secs, \
                         trigger_reason, dominant_category, hlc_wall_ms, hlc_counter, origin_device_id) \
                         VALUES (?1, '2026-01-01', '2026-01-01', 3600, 'timer', 'Dev', ?2, ?3, ?4)",
                        rusqlite::params![id, w, c, remote_id],
                    )
                    .unwrap();
            }
        }
        let merger = SqliteSyncMerger::new(storage.connection_arc(), local_id);
        let cs = ChangeSet {
            kind: ChangeSetKind::DeletionEvent,
            origin_device_id: remote_id.to_string(),
            // anchor = (150, 0): seg-old (100,1) <= it, seg-new (200,0) is above it.
            watermark: Hlc {
                wall_ms: 150,
                counter: 0,
                device_id: remote_id.to_string(),
            },
            ..Default::default()
        };
        let r = merger.apply_changes(cs).await.unwrap();
        assert_eq!(r.tombstoned, 1, "only the pre-anchor row is deleted");
        assert_eq!(
            count(
                &storage,
                "SELECT COUNT(*) FROM activity_segments WHERE id='seg-old'"
            ),
            0,
            "pre-erasure row erased"
        );
        assert_eq!(
            count(
                &storage,
                "SELECT COUNT(*) FROM activity_segments WHERE id='seg-new'"
            ),
            1,
            "post-re-grant row (HLC > anchor) survives"
        );
    }

    #[tokio::test]
    async fn self_origin_deletion_event_does_not_delete_local_rows() {
        // SECURITY (#6560): a DeletionEvent spoofing THIS device's own origin_device_id must NOT
        // run a device-wide DELETE of our own data. Before the fix the DeletionEvent branch ran
        // ahead of the self-origin skip, so a hostile/relaying peer answering /sync/pull (or a
        // compromised push peer) could return a self-origin DeletionEvent and hard-delete the
        // victim's own-origin rows across every synced table.
        let (storage, local_id) = setup();
        // A LOCAL-origin segment authored by this device.
        {
            let conn = storage.connection_arc();
            let guard = conn.test_lock();
            guard
                .execute(
                    "INSERT INTO activity_segments \
                     (id, start_time, end_time, duration_secs, trigger_reason, \
                      dominant_category, hlc_wall_ms, hlc_counter, origin_device_id) \
                     VALUES ('seg-local', '2026-01-01', '2026-01-01', 3600, 'timer', \
                             'Dev', 100, 1, ?1)",
                    rusqlite::params![local_id],
                )
                .unwrap();
        }
        let merger = SqliteSyncMerger::new(storage.connection_arc(), local_id.clone());
        // Hostile DeletionEvent spoofing our own origin.
        let cs = ChangeSet {
            kind: ChangeSetKind::DeletionEvent,
            origin_device_id: local_id.clone(),
            origin_device_name: "Spoofed".to_string(),
            ..Default::default()
        };
        let result = merger.apply_changes(cs).await.unwrap();
        assert_eq!(
            result.tombstoned, 0,
            "a self-origin DeletionEvent must delete nothing"
        );
        assert_eq!(
            count(
                &storage,
                "SELECT COUNT(*) FROM activity_segments WHERE id='seg-local'"
            ),
            1,
            "local-origin data survives a spoofed self-DeletionEvent"
        );
    }

    #[tokio::test]
    async fn self_origin_tombstone_in_peer_changeset_is_rejected() {
        // SECURITY (#6560): a peer-origin changeset (which passes the changeset-level self guard)
        // can still carry a tombstone forged with OUR origin + one of our row ids. apply_tombstone
        // deletes `WHERE <pk> = ? AND origin_device_id = t.origin`, so without the per-tombstone
        // guard a hostile peer could delete this device's own rows by id.
        let (storage, local_id) = setup();
        {
            let conn = storage.connection_arc();
            let guard = conn.test_lock();
            guard
                .execute(
                    "INSERT INTO activity_segments \
                     (id, start_time, end_time, duration_secs, trigger_reason, \
                      dominant_category, hlc_wall_ms, hlc_counter, origin_device_id) \
                     VALUES ('seg-local', '2026-01-01', '2026-01-01', 3600, 'timer', \
                             'Dev', 100, 1, ?1)",
                    rusqlite::params![local_id],
                )
                .unwrap();
        }
        let merger = SqliteSyncMerger::new(storage.connection_arc(), local_id.clone());
        // Peer-origin changeset carrying a tombstone that spoofs OUR origin to target our row.
        let cs = ChangeSet {
            origin_device_id: "remote-dev".to_string(),
            origin_device_name: "Remote".to_string(),
            tombstones: vec![tombstone(
                "activity_segments",
                "seg-local",
                999,
                0,
                &local_id,
            )],
            ..Default::default()
        };
        let r = merger.apply_changes(cs).await.unwrap();
        assert_eq!(
            r.tombstoned, 0,
            "a self-origin tombstone must be rejected, not applied"
        );
        assert_eq!(
            count(
                &storage,
                "SELECT COUNT(*) FROM activity_segments WHERE id='seg-local'"
            ),
            1,
            "local row survives a spoofed self-origin tombstone"
        );
    }

    #[tokio::test]
    async fn merge_segment_inserts_with_trigger_reason() {
        // #5202 regression: a peer segment must INSERT cleanly — trigger_reason is
        // `TEXT NOT NULL`, and before the fix neither the extractor emitted it nor the
        // merge INSERT set it, so every real peer segment failed the NOT NULL constraint.
        let (storage, local_id) = setup();
        let merger = SqliteSyncMerger::new(storage.connection_arc(), local_id);
        let cs = ChangeSet {
            origin_device_id: "remote-dev".to_string(),
            segments: vec![seg_json("seg-tr", 100, 0, "remote-dev")],
            ..Default::default()
        };
        let r = merger.apply_changes(cs).await.unwrap();
        assert_eq!(r.applied, 1, "peer segment inserts (no NOT NULL failure)");
        let tr: String = count_str(
            &storage,
            "SELECT trigger_reason FROM activity_segments WHERE id='seg-tr'",
        );
        assert_eq!(tr, "timer", "trigger_reason synced through");
    }

    #[tokio::test]
    async fn merge_segment_defaults_trigger_reason_for_pre_fix_peer() {
        // #5202 compat: a pre-fix peer's changeset omits trigger_reason → default 'sync',
        // not a constraint crash that rolls back the whole merge.
        let (storage, local_id) = setup();
        let merger = SqliteSyncMerger::new(storage.connection_arc(), local_id);
        let row = serde_json::json!({
            "id": "seg-old", "start_time": "2026-01-01", "end_time": "2026-01-01",
            "duration_secs": 1, "dominant_category": "Dev",
            "hlc_wall_ms": 100, "hlc_counter": 0, "origin_device_id": "remote-dev"
        });
        let cs = ChangeSet {
            origin_device_id: "remote-dev".to_string(),
            segments: vec![row],
            ..Default::default()
        };
        let r = merger.apply_changes(cs).await.unwrap();
        assert_eq!(r.applied, 1, "inserts with default trigger_reason");
        let tr: String = count_str(
            &storage,
            "SELECT trigger_reason FROM activity_segments WHERE id='seg-old'",
        );
        assert_eq!(tr, "sync", "pre-fix peer row defaults to 'sync'");
    }

    #[tokio::test]
    async fn embedding_tombstone_hard_deletes_by_composite_key() {
        // #5174: embeddings key on the cross-device-stable composite (segment_id, model_id),
        // NOT the per-device autoincrement `id`. A tombstone for "<seg>\x1f<model>" must
        // hard-delete the row and suppress a lower-HLC re-insert (the most failure-prone
        // tombstone path — a malformed key silently matches nothing).
        let (storage, local_id) = setup();
        let remote_id = "remote-dev";
        {
            let conn = storage.connection_arc();
            let g = conn.test_lock();
            g.execute(
                "INSERT INTO embedding_vectors (segment_id, content_type, original_text, vector, \
                 model_id, timestamp, hlc_wall_ms, hlc_counter, origin_device_id) \
                 VALUES ('seg-e', 'screen', 'secret text', x'0102', 'm1', '2026-01-01', 100, 1, ?1)",
                rusqlite::params![remote_id],
            )
            .unwrap();
        }
        let merger = SqliteSyncMerger::new(storage.connection_arc(), local_id);
        let key = format!("seg-e{EMB_KEY_SEP}m1");
        let cs = ChangeSet {
            origin_device_id: remote_id.to_string(),
            tombstones: vec![Tombstone {
                table_name: "embedding_vectors".to_string(),
                row_id: key,
                origin_device_id: remote_id.to_string(),
                hlc_wall_ms: 200,
                hlc_counter: 0,
                deleted_at: "2026-01-02T00:00:00Z".to_string(),
            }],
            ..Default::default()
        };
        let r = merger.apply_changes(cs).await.unwrap();
        assert!(r.tombstoned > 0, "embedding hard-deleted by composite key");
        assert_eq!(
            count(
                &storage,
                "SELECT COUNT(*) FROM embedding_vectors WHERE segment_id='seg-e'"
            ),
            0,
            "embedding erased"
        );

        // A re-synced embedding at HLC <= the tombstone is suppressed (no resurrection).
        let cs2 = ChangeSet {
            origin_device_id: remote_id.to_string(),
            embeddings: vec![serde_json::json!({
                "segment_id": "seg-e", "model_id": "m1", "content_type": "screen",
                "original_text": "secret text", "vector": [1, 2], "timestamp": "2026-01-01",
                "hlc_wall_ms": 150, "hlc_counter": 0, "origin_device_id": remote_id
            })],
            ..Default::default()
        };
        let r2 = merger.apply_changes(cs2).await.unwrap();
        assert_eq!(r2.applied, 0, "lower-HLC embedding re-sync suppressed");
        assert_eq!(
            count(
                &storage,
                "SELECT COUNT(*) FROM embedding_vectors WHERE segment_id='seg-e'"
            ),
            0,
            "embedding stays erased"
        );
    }

    #[tokio::test]
    async fn cross_device_erasure_converges_on_offline_peer() {
        // S5 convergence E2E: A erases → extract a changeset carrying the retained
        // tombstone → an OFFLINE peer B (that already holds A's row) reconnects, applies
        // it, and converges to erasure; a later still-circulating stale insert is
        // suppressed (no re-hydration). Wires the real producer→extractor→merger chain
        // across two independent storage instances.
        use crate::sync_extractor::SqliteSyncExtractor;
        use maekon_core::config::SyncConfig;
        use maekon_core::ports::change_extractor::ChangeExtractor;

        let insert_a_row = |storage: &SqliteStorage, origin: &str| {
            let conn = storage.connection_arc();
            let g = conn.test_lock();
            g.execute(
                "INSERT INTO activity_segments (id, start_time, end_time, duration_secs, \
                 trigger_reason, dominant_category, hlc_wall_ms, hlc_counter, origin_device_id) \
                 VALUES ('seg-x', '2026-01-01', '2026-01-01', 3600, 'timer', 'Dev', 100, 1, ?1)",
                rusqlite::params![origin],
            )
            .unwrap();
        };

        // Device A authors a row, then erases (GDPR Art.17).
        let (storage_a, dev_a) = setup();
        insert_a_row(&storage_a, &dev_a);
        storage_a.delete_all_data().unwrap();

        // A extracts the post-erase changeset — it carries the tombstone skeleton.
        let extractor_a = SqliteSyncExtractor::new(
            storage_a.connection_arc(),
            dev_a.clone(),
            "A".to_string(),
            SyncConfig::default(),
        );
        let cs = extractor_a
            .get_changes_since(&Hlc::default())
            .await
            .unwrap();
        assert!(!cs.tombstones.is_empty(), "A emits the erasure tombstone");

        // Device B (offline at erasure) already holds A's row (origin = A).
        let (storage_b, dev_b) = setup();
        insert_a_row(&storage_b, &dev_a);

        // B reconnects and applies A's changeset → converges (row hard-deleted).
        let merger_b = SqliteSyncMerger::new(storage_b.connection_arc(), dev_b);
        let r = merger_b.apply_changes(cs).await.unwrap();
        assert!(r.tombstoned > 0, "B hard-deletes A's erased row");
        assert_eq!(
            count(
                &storage_b,
                "SELECT COUNT(*) FROM activity_segments WHERE id='seg-x'"
            ),
            0,
            "B converged to erasure"
        );

        // A still-circulating stale insert of the same row (relayed by a third peer) is
        // suppressed by B's recorded tombstone — no re-hydration.
        let stale = ChangeSet {
            origin_device_id: dev_a.clone(),
            segments: vec![seg_json("seg-x", 100, 1, &dev_a)],
            ..Default::default()
        };
        let r2 = merger_b.apply_changes(stale).await.unwrap();
        assert_eq!(r2.applied, 0, "stale re-insert suppressed");
        assert_eq!(
            count(
                &storage_b,
                "SELECT COUNT(*) FROM activity_segments WHERE id='seg-x'"
            ),
            0,
            "B stays erased (no re-hydration)"
        );
    }

    fn seg_json(id: &str, wall: u64, counter: u32, origin: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id, "start_time": "2026-01-01", "end_time": "2026-01-01",
            "duration_secs": 3600, "trigger_reason": "timer", "dominant_category": "Dev",
            "hlc_wall_ms": wall, "hlc_counter": counter, "origin_device_id": origin
        })
    }

    fn tombstone(table: &str, row_id: &str, wall: u64, counter: u32, origin: &str) -> Tombstone {
        Tombstone {
            table_name: table.to_string(),
            row_id: row_id.to_string(),
            origin_device_id: origin.to_string(),
            hlc_wall_ms: wall,
            hlc_counter: counter,
            deleted_at: "2026-01-02T00:00:00Z".to_string(),
        }
    }

    fn count(storage: &SqliteStorage, sql: &str) -> i64 {
        let conn = storage.connection_arc();
        let g = conn.test_lock();
        g.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    fn count_str(storage: &SqliteStorage, sql: &str) -> String {
        let conn = storage.connection_arc();
        let g = conn.test_lock();
        g.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[tokio::test]
    async fn tombstone_hard_deletes_and_suppresses_resurrection() {
        let (storage, local_id) = setup();
        let remote_id = "remote-dev";
        {
            let conn = storage.connection_arc();
            let guard = conn.test_lock();
            guard
                .execute(
                    "INSERT INTO activity_segments (id, start_time, end_time, duration_secs, \
                     trigger_reason, dominant_category, hlc_wall_ms, hlc_counter, origin_device_id) \
                     VALUES ('seg-t', '2026-01-01', '2026-01-01', 3600, 'timer', 'Dev', 100, 1, ?1)",
                    rusqlite::params![remote_id],
                )
                .unwrap();
        }
        let merger = SqliteSyncMerger::new(storage.connection_arc(), local_id);

        // Apply a tombstone (HLC 200) → row hard-deleted + suppression recorded.
        let cs = ChangeSet {
            origin_device_id: remote_id.to_string(),
            tombstones: vec![tombstone("activity_segments", "seg-t", 200, 0, remote_id)],
            ..Default::default()
        };
        let r = merger.apply_changes(cs).await.unwrap();
        assert!(r.tombstoned > 0, "row hard-deleted");
        assert_eq!(
            count(
                &storage,
                "SELECT COUNT(*) FROM activity_segments WHERE id='seg-t'"
            ),
            0
        );

        // A re-synced copy at HLC 150 (<= tombstone) is SUPPRESSED — not resurrected.
        let cs2 = ChangeSet {
            origin_device_id: remote_id.to_string(),
            segments: vec![seg_json("seg-t", 150, 0, remote_id)],
            ..Default::default()
        };
        let r2 = merger.apply_changes(cs2).await.unwrap();
        assert_eq!(r2.applied, 0, "lower-HLC re-sync suppressed");
        assert_eq!(
            count(
                &storage,
                "SELECT COUNT(*) FROM activity_segments WHERE id='seg-t'"
            ),
            0,
            "row stays erased"
        );
    }

    #[tokio::test]
    async fn regrant_higher_hlc_beats_tombstone() {
        let (storage, local_id) = setup();
        let remote_id = "remote-dev";
        let merger = SqliteSyncMerger::new(storage.connection_arc(), local_id);

        let cs1 = ChangeSet {
            origin_device_id: remote_id.to_string(),
            tombstones: vec![tombstone("regime_overrides", "ov-rg", 100, 0, remote_id)],
            ..Default::default()
        };
        merger.apply_changes(cs1).await.unwrap();

        // Post-re-grant row at HLC 200 (> tombstone) → applies, tombstone cleared (P1).
        let cs2 = ChangeSet {
            origin_device_id: remote_id.to_string(),
            overrides: vec![serde_json::json!({
                "override_id": "ov-rg", "segment_id": "seg-1", "action_type": "reassign",
                "created_at": "2026-01-01", "hlc_wall_ms": 200, "hlc_counter": 0,
                "origin_device_id": remote_id
            })],
            ..Default::default()
        };
        let r = merger.apply_changes(cs2).await.unwrap();
        assert_eq!(r.applied, 1, "higher-HLC post-re-grant row applies");
        assert_eq!(
            count(
                &storage,
                "SELECT COUNT(*) FROM regime_overrides WHERE override_id='ov-rg'"
            ),
            1
        );
        assert_eq!(
            count(
                &storage,
                "SELECT COUNT(*) FROM sync_tombstones WHERE row_id='ov-rg'"
            ),
            0,
            "superseded tombstone cleared"
        );
    }

    #[tokio::test]
    async fn suggestion_tombstone_beats_acted_status() {
        // The merge_suggestion fix: a re-synced `acted` suggestion at a LOWER HLC must not
        // resurrect an erased one by winning on status ordinal — the suppression gate
        // short-circuits before the status-monotonic merge.
        let (storage, local_id) = setup();
        let remote_id = "remote-dev";
        let merger = SqliteSyncMerger::new(storage.connection_arc(), local_id);

        let cs1 = ChangeSet {
            origin_device_id: remote_id.to_string(),
            tombstones: vec![tombstone("suggestions", "sug-x", 100, 0, remote_id)],
            ..Default::default()
        };
        merger.apply_changes(cs1).await.unwrap();

        let cs2 = ChangeSet {
            origin_device_id: remote_id.to_string(),
            suggestions: vec![serde_json::json!({
                "suggestion_id": "sug-x", "suggestion_type": "WORK_GUIDANCE",
                "source": "RULE_BASED", "content": "c", "priority": "MEDIUM",
                "confidence_score": 0.5, "relevance_score": 0.5, "is_actionable": 1,
                "acted_at": "2026-01-01T00:00:00Z", "created_at": "2026-01-01",
                "hlc_wall_ms": 50, "hlc_counter": 0, "origin_device_id": remote_id
            })],
            ..Default::default()
        };
        let r = merger.apply_changes(cs2).await.unwrap();
        assert_eq!(
            r.applied, 0,
            "acted suggestion must not resurrect an erased one"
        );
        assert_eq!(
            count(
                &storage,
                "SELECT COUNT(*) FROM suggestions WHERE suggestion_id='sug-x'"
            ),
            0,
            "erased suggestion stays erased"
        );
    }

    #[tokio::test]
    async fn suggestion_monotonic_merge_acted_wins() {
        let (storage, local_id) = setup();

        // Insert a local suggestion at status "dismissed"
        {
            let conn = storage.connection_arc();
            let guard = conn.test_lock();
            guard
                .execute(
                    "INSERT INTO suggestions \
                 (suggestion_id, suggestion_type, content, priority, \
                  confidence_score, relevance_score, is_actionable, \
                  shown_at, dismissed_at, created_at, source, \
                  hlc_wall_ms, hlc_counter, origin_device_id) \
                 VALUES ('sug-1', 'focus', 'Take a break', 'MEDIUM', \
                         0.8, 0.7, 1, '2026-01-01T10:00:00', '2026-01-01T10:05:00', \
                         '2026-01-01T10:00:00', 'RULE_BASED', 200, 5, ?1)",
                    rusqlite::params![local_id],
                )
                .unwrap();
        }

        let merger = SqliteSyncMerger::new(storage.connection_arc(), local_id);

        // Remote has same suggestion at status "acted" with LOWER HLC
        // Monotonic merge should still pick "acted" because acted(3) > dismissed(2)
        let remote_suggestion = serde_json::json!({
            "suggestion_id": "sug-1",
            "suggestion_type": "focus",
            "source": "RULE_BASED",
            "content": "Take a break",
            "priority": "MEDIUM",
            "confidence_score": 0.8,
            "relevance_score": 0.7,
            "is_actionable": 1,
            "reasoning": null,
            "shown_at": "2026-01-01T10:00:00",
            "dismissed_at": "2026-01-01T10:05:00",
            "acted_at": "2026-01-01T10:06:00",
            "created_at": "2026-01-01T10:00:00",
            "expires_at": null,
            "is_deleted": 0,
            "deleted_at": null,
            "hlc_wall_ms": 100,
            "hlc_counter": 1,
            "origin_device_id": "remote-dev"
        });

        let cs = ChangeSet {
            origin_device_id: "remote-dev".to_string(),
            origin_device_name: "Remote".to_string(),
            suggestions: vec![remote_suggestion],
            ..Default::default()
        };
        let result = merger.apply_changes(cs).await.unwrap();
        assert_eq!(result.applied, 1, "acted status should win over dismissed");
    }

    #[test]
    fn decode_vector_accepts_valid_hex() {
        // Round-trips the exact bytes — the happy path the embedding merge depends on.
        let bytes = decode_vector("0102ff", "seg-d", "m1").unwrap();
        assert_eq!(bytes, vec![0x01, 0x02, 0xff]);
    }

    #[test]
    fn decode_vector_rejects_malformed_hex() {
        // #35 / #6081: odd length, non-hex char, and truncation must all return None
        // (so the caller skips the row) rather than collapse to an empty blob.
        for bad in ["0", "zz", "010"] {
            assert!(
                decode_vector(bad, "seg-d", "m1").is_none(),
                "malformed hex {bad:?} must be rejected"
            );
        }
    }

    #[tokio::test]
    async fn merge_embedding_corrupt_vector_skips_no_empty_blob() {
        // #35 regression: a peer changeset with a non-hex vector must SKIP just that row,
        // NOT silently INSERT a zero-length blob and NOT abort the whole merge.
        let (storage, local_id) = setup();
        let merger = SqliteSyncMerger::new(storage.connection_arc(), local_id);
        let cs = ChangeSet {
            origin_device_id: "remote-dev".to_string(),
            embeddings: vec![serde_json::json!({
                "segment_id": "seg-bad", "model_id": "m1", "content_type": "screen",
                "original_text": "t", "vector": "nothex", "timestamp": "2026-01-01",
                "hlc_wall_ms": 100, "hlc_counter": 0, "origin_device_id": "remote-dev"
            })],
            ..Default::default()
        };
        let result = merger
            .apply_changes(cs)
            .await
            .expect("corrupt vector is skipped, not fatal");
        assert_eq!(result.applied, 0, "corrupt row not applied");
        assert_eq!(result.skipped_dup, 1, "corrupt row counted as skipped");
        assert_eq!(
            count(
                &storage,
                "SELECT COUNT(*) FROM embedding_vectors WHERE segment_id='seg-bad'"
            ),
            0,
            "no empty blob persisted (row skipped)"
        );
    }

    #[tokio::test]
    async fn merge_embedding_corrupt_remote_does_not_zero_local_vector() {
        // #35 regression for the dangerous LWW path: a valid local vector must NOT be
        // overwritten by an empty blob when a strictly-newer remote row carries a corrupt
        // (non-hex) vector. The local bytes survive intact; the bad row is skipped.
        let (storage, local_id) = setup();
        {
            let conn = storage.connection_arc();
            let g = conn.test_lock();
            g.execute(
                "INSERT INTO embedding_vectors (segment_id, content_type, original_text, vector, \
                 model_id, timestamp, hlc_wall_ms, hlc_counter, origin_device_id) \
                 VALUES ('seg-keep', 'screen', 'local text', x'0102ff', 'm1', '2026-01-01', 100, 0, ?1)",
                rusqlite::params![local_id],
            )
            .unwrap();
        }
        let merger = SqliteSyncMerger::new(storage.connection_arc(), local_id);
        // Strictly-newer HLC (would win LWW) but the vector is non-hex -> must be rejected.
        let cs = ChangeSet {
            origin_device_id: "remote-dev".to_string(),
            embeddings: vec![serde_json::json!({
                "segment_id": "seg-keep", "model_id": "m1", "content_type": "screen",
                "original_text": "remote text", "vector": "nothex", "timestamp": "2026-02-01",
                "hlc_wall_ms": 200, "hlc_counter": 0, "origin_device_id": "remote-dev"
            })],
            ..Default::default()
        };
        let result = merger.apply_changes(cs).await.expect("corrupt row skipped");
        assert_eq!(result.applied, 0, "corrupt LWW row not applied");
        assert_eq!(result.skipped_dup, 1, "corrupt LWW row counted as skipped");

        let stored: Vec<u8> = {
            let conn = storage.connection_arc();
            let g = conn.test_lock();
            g.query_row(
                "SELECT vector FROM embedding_vectors WHERE segment_id='seg-keep' AND model_id='m1'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(
            stored,
            vec![0x01, 0x02, 0xff],
            "valid local vector preserved - never zeroed by corrupt remote"
        );
    }

    #[test]
    fn extract_hlc_counter_accepts_max_u32_and_rejects_overflow() {
        // #6174: the boundary value u32::MAX is a valid counter and must round-trip.
        let ok = serde_json::json!({ "hlc_counter": u32::MAX as u64 });
        assert_eq!(
            extract_hlc_counter(&ok, "row-ok").unwrap(),
            Some(u32::MAX),
            "u32::MAX counter is in range"
        );

        // A counter one past u32::MAX must be REJECTED (return None so the caller skips the
        // row), NOT silently truncated. The old `as u32` cast wrapped this to 0 — assert we
        // never produce that corrupting value.
        let overflow = serde_json::json!({ "hlc_counter": u32::MAX as u64 + 1 });
        let got = extract_hlc_counter(&overflow, "row-overflow").unwrap();
        assert_eq!(got, None, "out-of-range counter rejected, not truncated");
        assert_ne!(
            got,
            Some(0),
            "regression guard: overflow must NOT silently wrap to counter 0"
        );

        // extract_hlc threads the same rejection through (returns None on overflow).
        let row = serde_json::json!({
            "hlc_wall_ms": 100, "hlc_counter": u32::MAX as u64 + 1, "origin_device_id": "d"
        });
        assert!(
            extract_hlc(&row, "row-overflow").unwrap().is_none(),
            "extract_hlc rejects an out-of-range counter row"
        );
    }

    #[tokio::test]
    async fn merge_segment_out_of_range_hlc_counter_is_skipped_not_truncated() {
        // #6174 regression: a wire segment whose HLC counter exceeds u32 must SKIP just that
        // row (no silently-truncated counter inserted), NOT abort the whole merge.
        let (storage, local_id) = setup();
        let merger = SqliteSyncMerger::new(storage.connection_arc(), local_id);
        let mut bad = seg_json("seg-of", 100, 0, "remote-dev");
        // u32::MAX + 1 would `as u32`-truncate to 0 under the old cast.
        bad["hlc_counter"] = serde_json::json!(u32::MAX as u64 + 1);
        let cs = ChangeSet {
            origin_device_id: "remote-dev".to_string(),
            segments: vec![bad],
            ..Default::default()
        };
        let r = merger
            .apply_changes(cs)
            .await
            .expect("out-of-range counter is skipped, not fatal");
        assert_eq!(r.applied, 0, "row with overflow counter not applied");
        assert_eq!(r.skipped_dup, 1, "overflow row counted as skipped");
        assert_eq!(
            count(
                &storage,
                "SELECT COUNT(*) FROM activity_segments WHERE id='seg-of'"
            ),
            0,
            "no row persisted with a truncated HLC counter",
        );
    }

    #[tokio::test]
    async fn merge_one_overflow_counter_row_does_not_block_a_valid_sibling() {
        // #6174 poison-pill guard: one corrupt peer row (overflow HLC counter) must be
        // skipped while every other valid row in the SAME changeset still merges. A hard
        // error here would roll back the transaction AND stall sync (watermark never
        // advances), so the per-row skip is the correct behavior.
        let (storage, local_id) = setup();
        let merger = SqliteSyncMerger::new(storage.connection_arc(), local_id);
        let mut bad = seg_json("seg-bad", 100, 0, "remote-dev");
        bad["hlc_counter"] = serde_json::json!(u64::MAX);
        let good = seg_json("seg-good", 100, 0, "remote-dev");
        let cs = ChangeSet {
            origin_device_id: "remote-dev".to_string(),
            segments: vec![bad, good],
            ..Default::default()
        };
        let r = merger.apply_changes(cs).await.expect("merge keeps going");
        assert_eq!(r.applied, 1, "the valid sibling row still merges");
        assert_eq!(r.skipped_dup, 1, "only the overflow row is skipped");
        assert_eq!(
            count(
                &storage,
                "SELECT COUNT(*) FROM activity_segments WHERE id='seg-good'"
            ),
            1,
            "valid row committed (transaction not aborted by the bad row)",
        );
        assert_eq!(
            count(
                &storage,
                "SELECT COUNT(*) FROM activity_segments WHERE id='seg-bad'"
            ),
            0,
            "overflow row skipped",
        );
    }
}
