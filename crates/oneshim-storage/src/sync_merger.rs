//! ChangeMerger implementation for SQLite.
//!
//! Applies incoming changesets from remote peers with conflict resolution:
//! - Append-only tables: INSERT OR IGNORE (union merge)
//! - LWW tables: compare HLC, higher wins
//! - Suggestions: monotonic status merge (acted > dismissed > shown > null)
//! - DeletionEvent: hard-delete all rows from originating device (GDPR Art. 17)

use async_trait::async_trait;
use rusqlite::Connection;
use std::sync::{Arc, Mutex};
use tracing::{debug, info, warn};

use crate::error::StorageError;
use oneshim_core::error::CoreError;
use oneshim_core::models::sync::{ChangeSet, ChangeSetKind, SyncResult};
use oneshim_core::ports::change_merger::ChangeMerger;
use oneshim_core::sync::Hlc;

/// SQLite-backed ChangeMerger adapter.
pub struct SqliteSyncMerger {
    conn: Arc<Mutex<Connection>>,
    local_device_id: String,
}

impl SqliteSyncMerger {
    pub fn new(conn: Arc<Mutex<Connection>>, local_device_id: String) -> Self {
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

    /// Handle GDPR Article 17 deletion event: hard-delete all synced data
    /// from the originating device.
    fn handle_deletion_event(
        conn: &Connection,
        origin_device_id: &str,
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
            let sql = format!("DELETE FROM {table} WHERE origin_device_id = ?1");
            let deleted = conn
                .execute(&sql, rusqlite::params![origin_device_id])
                .map_err(|e| StorageError::Internal(format!("GDPR deletion on {table}: {e}")))?;
            total_deleted += deleted;
        }
        info!(
            origin_device_id = origin_device_id,
            total_deleted = total_deleted,
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
            let mut guard = conn.lock().map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("SQLite lock poisoned: {e}"),
            })?;

            // Handle GDPR deletion event
            if changes.kind == ChangeSetKind::DeletionEvent {
                let deleted = Self::handle_deletion_event(&guard, &changes.origin_device_id)?;
                return Ok(SyncResult {
                    tombstoned: deleted,
                    new_watermark: changes.watermark,
                    ..Default::default()
                });
            }

            // Skip self-originated changesets
            if changes.origin_device_id == local_device_id {
                debug!("skipping self-originated changeset");
                return Ok(SyncResult {
                    new_watermark: changes.watermark,
                    ..Default::default()
                });
            }

            let mut result = SyncResult::default();

            // All merge operations run inside a single transaction
            let tx = guard.transaction().map_err(|e| CoreError::Storage {
                code: oneshim_core::error_codes::StorageCode::Failed,
                message: format!("begin transaction: {e}"),
            })?;

            // --- Append-only tables ---
            for row in &changes.segments {
                merge_segment(&tx, row, &mut result)?;
            }
            for row in &changes.overrides {
                merge_override(&tx, row, &mut result)?;
            }
            for row in &changes.param_snapshots {
                merge_param_snapshot(&tx, row, &mut result)?;
            }

            // --- LWW tables ---
            for row in &changes.regimes {
                merge_regime(&tx, row, &mut result)?;
            }
            for row in &changes.embeddings {
                merge_embedding(&tx, row, &mut result)?;
            }

            // --- Monotonic status merge (suggestions) ---
            for row in &changes.suggestions {
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
                code: oneshim_core::error_codes::StorageCode::Failed,
                message: format!("update sync_peers: {e}"),
            })?;

            tx.commit().map_err(|e| CoreError::Storage {
                code: oneshim_core::error_codes::StorageCode::Failed,
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
        .await
        .map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("spawn_blocking join error: {e}"),
        })?
    }
}

// ── Per-table merge functions (called inside transaction) ──

fn merge_segment(
    conn: &Connection,
    row: &serde_json::Value,
    result: &mut SyncResult,
) -> Result<(), StorageError> {
    let id = json_str(row, "id")?;
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
         (id, start_time, end_time, duration_secs, regime_id, dominant_category, \
          app_breakdown, llm_summary, content_activities_json, \
          hlc_wall_ms, hlc_counter, origin_device_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        rusqlite::params![
            id,
            json_str(row, "start_time")?,
            json_str(row, "end_time")?,
            json_i64(row, "duration_secs")?,
            json_str_opt(row, "regime_id"),
            json_str(row, "dominant_category")?,
            json_str_or_default(row, "app_breakdown", "{}"),
            json_str_opt(row, "llm_summary"),
            json_str_or_default(row, "content_activities_json", "[]"),
            json_u64(row, "hlc_wall_ms")?,
            json_u32(row, "hlc_counter")?,
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
    let remote_hlc = extract_hlc(row)?;

    let local: Option<(u64, u32, String)> = conn
        .query_row(
            "SELECT hlc_wall_ms, hlc_counter, origin_device_id FROM regimes WHERE id = ?1",
            rusqlite::params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok();

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
            json_u32(row, "hlc_counter")?,
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
    let remote_hlc = extract_hlc(row)?;

    let local: Option<(i64, u64, u32, String)> = conn
        .query_row(
            "SELECT id, hlc_wall_ms, hlc_counter, origin_device_id \
             FROM embedding_vectors WHERE segment_id = ?1 AND model_id = ?2",
            rusqlite::params![segment_id, model_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .ok();

    match local {
        None => {
            // Decode hex-encoded vector back to BLOB
            let vector_hex = json_str(row, "vector")?;
            let vector_bytes = hex::decode(vector_hex).unwrap_or_default();

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
                warn!(
                    segment_id = %segment_id,
                    model_id = %model_id,
                    local_device = %local_hlc.device_id,
                    remote_device = %remote_hlc.device_id,
                    "sync conflict: embedding overwritten by remote (LWW)"
                );
                let vector_hex = json_str(row, "vector")?;
                let vector_bytes = hex::decode(vector_hex).unwrap_or_default();

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
    let remote_hlc = extract_hlc(row)?;
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
        .ok();

    match local {
        None => {
            conn.execute(
                "INSERT INTO suggestions \
                 (suggestion_id, suggestion_type, source, content, priority, \
                  confidence_score, relevance_score, is_actionable, reasoning, \
                  shown_at, dismissed_at, acted_at, created_at, expires_at, \
                  is_deleted, deleted_at, \
                  hlc_wall_ms, hlc_counter, origin_device_id) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",
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
                     reasoning=?9, shown_at=?10, dismissed_at=?11, acted_at=?12, \
                     expires_at=?13, is_deleted=?14, deleted_at=?15, \
                     hlc_wall_ms=?16, hlc_counter=?17, origin_device_id=?18 \
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
            json_u32(row, "hlc_counter")?,
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

fn json_u32(v: &serde_json::Value, key: &str) -> Result<u32, StorageError> {
    json_u64(v, key).map(|n| n as u32)
}

fn json_f64(v: &serde_json::Value, key: &str) -> Result<f64, StorageError> {
    v.get(key)
        .and_then(|v| v.as_f64())
        .ok_or_else(|| StorageError::Internal(format!("missing f64 field: {key}")))
}

fn extract_hlc(row: &serde_json::Value) -> Result<Hlc, StorageError> {
    Ok(Hlc {
        wall_ms: json_u64(row, "hlc_wall_ms")?,
        counter: json_u32(row, "hlc_counter")?,
        device_id: json_str(row, "origin_device_id")?.to_string(),
    })
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
            let guard = conn.lock().unwrap();
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
        let guard = conn.lock().unwrap();
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
    async fn suggestion_monotonic_merge_acted_wins() {
        let (storage, local_id) = setup();

        // Insert a local suggestion at status "dismissed"
        {
            let conn = storage.connection_arc();
            let guard = conn.lock().unwrap();
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
}
