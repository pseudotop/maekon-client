use crate::error::StorageError;
use chrono::Utc;

use super::super::SqliteStorage;

/// egress_ledger retention cap — keep only the most recent N rows (V36, #4803).
/// A cap to prevent unbounded growth. Excess rows are deleted in `enforce_all_retention`.
const EGRESS_LEDGER_MAX_ROWS: i64 = 5000;

/// Compliance retention window (days) for the security audit trails
/// (`audit_log` + `session_audit_log`). Two years — the upper bound of common
/// SOC2 / HIPAA / SOX audit-log retention requirements. Both tables are RETAINED
/// across GDPR erasure (Art.17(3) legal-obligation basis), so without an age cap
/// they grow unbounded (#8056 P3). 730 days keeps well beyond a typical audit
/// horizon while bounding growth.
const AUDIT_TRAIL_RETENTION_DAYS: i64 = 730;

impl SqliteStorage {
    /// Delete activity segments older than `max_days`. Returns the number of deleted rows.
    pub fn enforce_segment_retention(&self, max_days: u32) -> Result<usize, StorageError> {
        let cutoff = (Utc::now() - chrono::Duration::days(max_days as i64)).to_rfc3339();
        // Write (retention DELETE) — write_lock (skipped when deletion_flag is set).
        self.conn.write_lock().run(0usize, |conn| {
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='activity_segments'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(false);
        if !table_exists {
            return Ok(0);
        }
        // #8043: `activity_segments` is a cross-device-synced table, so an age DELETE that
        // leaves no tombstone lets a peer that still holds a locally-authored segment
        // re-push it on the next sync (peer resurrection → Art.5(1)(e) violation). Capture a
        // LOCAL-origin suppression tombstone for each aged row BEFORE deleting — same
        // predicate as the DELETE so the tombstone set matches the removed rows exactly.
        if let Some(local) = crate::sync_retention_tombstone::local_device_id(conn) {
            crate::sync_retention_tombstone::capture_local_origin_retention_tombstones(
                conn,
                "activity_segments",
                &format!("start_time < '{cutoff}' AND start_time IS NOT NULL"),
                &local,
            )?;
        }
        let deleted = conn
            .execute(
                "DELETE FROM activity_segments WHERE start_time < ?1 AND start_time IS NOT NULL",
                rusqlite::params![cutoff],
            )
            .map_err(|e| StorageError::Internal(format!("segment retention failure: {e}")))?;
        tracing::debug!(
            "Enforced segment retention: deleted {deleted} rows older than {max_days} days"
        );
        Ok(deleted)
        })
    }

    /// GC the retained GDPR Art.17 erasure tombstone outbox (#5174 S5/R4).
    ///
    /// Hard-deletes `sync_tombstones` rows whose `deleted_at` is older than
    /// `max(data_retention_days, 90)` days. This bounds the retained outbox (GDPR
    /// Art.5(1)(e) storage limitation) so it cannot grow without limit. The caller
    /// passes `segment_retention_days`, deliberately tying the suppression-set lifetime
    /// to the data-retention horizon: a peer offline ≥ this horizon would itself have
    /// GC'd the source row under the same retention, so it cannot relay a row older than
    /// the suppression tombstone's lifetime (this is the safety floor for the cliff).
    ///
    /// **Accepted convergence-cliff limitation** (spec v3 §S5): a peer offline LONGER
    /// than the horizon — reconnecting after its suppression tombstone has been GC'd —
    /// could be re-hydrated by a still-circulating stale insert. This is a conscious
    /// trade-off: by that horizon the data is long-erased at the source and on every
    /// online peer, and unbounded tombstone retention is itself a storage-limitation
    /// problem. A hard guarantee would need per-peer delivery acks (out of scope).
    ///
    /// `deleted_at` is normalized via SQLite `datetime(...)`: production stores the
    /// `datetime('now')` space-format (the producer writes it and a peer stores it
    /// verbatim), and the `datetime()` wrap also defensively accepts ISO-8601 variants.
    /// An unparseable value yields NULL and is left untouched (never over-deleted).
    /// Returns the number of GC'd rows.
    ///
    /// NOTE (wall-clock dependence): the GC horizon is measured in wall-clock days
    /// (`datetime('now')`), NOT HLC — so the "90d" cliff assumes the local clock is
    /// roughly honest. A badly forward-skewed clock could GC a suppression tombstone
    /// earlier than 90 real days, narrowing the cliff for that device. This only narrows
    /// an already-accepted bound (the row is already hard-deleted) and never under-erases;
    /// anti-resurrection correctness is HLC-based and skew-immune.
    pub fn gc_sync_tombstones(&self, data_retention_days: u32) -> Result<usize, StorageError> {
        let horizon = data_retention_days.max(90);
        // Write (GC DELETE) — write_lock (skipped when deletion_flag is set).
        self.conn.write_lock().run(0usize, |conn| {
            let deleted = conn
                .execute(
                    "DELETE FROM sync_tombstones \
                     WHERE datetime(deleted_at) < datetime('now', ?1)",
                    rusqlite::params![format!("-{horizon} days")],
                )
                .map_err(|e| StorageError::Internal(format!("tombstone GC failure: {e}")))?;
            tracing::debug!("GC'd {deleted} sync_tombstones older than {horizon} days");
            Ok(deleted)
        })
    }

    /// Delete weekly digests older than `max_weeks`. Returns the number of deleted rows.
    pub fn enforce_digest_retention(&self, max_weeks: u32) -> Result<usize, StorageError> {
        let cutoff = (Utc::now() - chrono::Duration::days(max_weeks as i64 * 7)).to_rfc3339();
        // Write (retention DELETE) — write_lock (skipped when deletion_flag is set).
        self.conn.write_lock().run(0usize, |conn| {
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='weekly_digests'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(false);
        if !table_exists {
            return Ok(0);
        }
        let deleted = conn
            .execute(
                "DELETE FROM weekly_digests WHERE week_start < ?1",
                rusqlite::params![cutoff],
            )
            .map_err(|e| StorageError::Internal(format!("digest retention failure: {e}")))?;
        tracing::debug!(
            "Enforced digest retention: deleted {deleted} rows older than {max_weeks} weeks"
        );
        Ok(deleted)
        })
    }

    /// Enforce retention for all auxiliary tables that would otherwise grow
    /// unbounded. Each table has its own retention window. Tables that may
    /// not exist in older schema versions are handled gracefully (errors
    /// from `conn.execute` are silently ignored via `let _ = …`).
    ///
    /// Returns the total number of rows deleted across all tables.
    pub fn enforce_all_retention(&self) -> Result<u64, StorageError> {
        // Write (retention DELETE) — write_lock (skipped when deletion_flag is set).
        self.conn.write_lock().run(0u64, |conn| {
            let mut total: u64 = 0;

            // #8043: `suggestions` (90d) and `regime_overrides` (180d) below are
            // cross-device-synced tables. Their age DELETEs must leave a suppression
            // tombstone or a peer that still holds a locally-authored row re-pushes it on
            // the next sync (peer resurrection → Art.5(1)(e) violation). Read the local
            // device id once; capture LOCAL-origin tombstones immediately BEFORE those two
            // DELETEs, using the identical predicate so the tombstone set matches the removed
            // rows. Every OTHER table in this fn (work_sessions/interruptions/gui_interactions/
            // local_suggestions/focus_metrics/daily_digests/digest_processing_markers/
            // coaching_events/egress_ledger) is absent from the synced set
            // (`sync_table_descriptor::ALL_TABLE_NAMES`), so it carries no cross-device
            // resurrection risk and needs no tombstone.
            let local_device = crate::sync_retention_tombstone::local_device_id(conn);

            // work_sessions: 90 days (only closed sessions with ended_at set)
            let n = conn
                .execute(
                    "DELETE FROM work_sessions WHERE ended_at < datetime('now', '-90 days')",
                    [],
                )
                .unwrap_or(0) as u64;
            total += n;

            // interruptions: 90 days
            let n = conn
                .execute(
                    "DELETE FROM interruptions WHERE interrupted_at < datetime('now', '-90 days')",
                    [],
                )
                .unwrap_or(0) as u64;
            total += n;

            // gui_interactions: 30 days
            let n = conn
                .execute(
                    "DELETE FROM gui_interactions WHERE timestamp < datetime('now', '-30 days')",
                    [],
                )
                .unwrap_or(0) as u64;
            total += n;

            // suggestions: 90 days (#8043: capture LOCAL-origin tombstones first — synced table)
            if let Some(local) = local_device.as_deref() {
                crate::sync_retention_tombstone::capture_local_origin_retention_tombstones(
                    conn,
                    "suggestions",
                    "created_at < datetime('now', '-90 days')",
                    local,
                )?;
            }
            let n = conn
                .execute(
                    "DELETE FROM suggestions WHERE created_at < datetime('now', '-90 days')",
                    [],
                )
                .unwrap_or(0) as u64;
            total += n;

            // local_suggestions: 90 days
            let n = conn
                .execute(
                    "DELETE FROM local_suggestions WHERE created_at < datetime('now', '-90 days')",
                    [],
                )
                .unwrap_or(0) as u64;
            total += n;

            // focus_metrics: 365 days
            let n = conn
                .execute(
                    "DELETE FROM focus_metrics WHERE date < date('now', '-365 days')",
                    [],
                )
                .unwrap_or(0) as u64;
            total += n;

            // daily_digests: 365 days
            let n = conn
                .execute(
                    "DELETE FROM daily_digests WHERE date < date('now', '-365 days')",
                    [],
                )
                .unwrap_or(0) as u64;
            total += n;

            // digest_processing_markers: same retention window as daily digests.
            let n = conn
                .execute(
                    "DELETE FROM digest_processing_markers
                     WHERE period_key < date('now', '-365 days')",
                    [],
                )
                .unwrap_or(0) as u64;
            total += n;

            // regime_overrides: 180 days (#8043: capture LOCAL-origin tombstones first — synced table)
            if let Some(local) = local_device.as_deref() {
                crate::sync_retention_tombstone::capture_local_origin_retention_tombstones(
                    conn,
                    "regime_overrides",
                    "created_at < datetime('now', '-180 days')",
                    local,
                )?;
            }
            let n = conn
                .execute(
                    "DELETE FROM regime_overrides WHERE created_at < datetime('now', '-180 days')",
                    [],
                )
                .unwrap_or(0) as u64;
            total += n;

            // coaching_events (V17): 90 days. INSERT-only coaching-message log that
            // would otherwise grow unbounded; pruned on the `shown_at` column (TEXT
            // NOT NULL), mirroring the 90-day window applied to the sibling
            // suggestion/behavior tables above. The `shown_at IS NOT NULL` guard is
            // defensive (the column is NOT NULL in schema) and matches the segment
            // retention style — a NULL/unparseable timestamp is left untouched.
            let n = conn
                .execute(
                    "DELETE FROM coaching_events \
                     WHERE shown_at < datetime('now', '-90 days') AND shown_at IS NOT NULL",
                    [],
                )
                .unwrap_or(0) as u64;
            total += n;

            // transcripts (V47, #8059): 90 days. Voice transcripts are user
            // activity content that would otherwise grow unbounded; pruned on
            // `timestamp` like the sibling 90-day behavior tables above. NOT a
            // synced table → no suppression tombstone. Delete the paired
            // `search_fts` rows FIRST (the subquery still sees the rows), scoped
            // to `content_type = 'transcript'` so no real segment index row is
            // touched — otherwise the keyword index would accumulate orphaned
            // transcript entries pointing at deleted rows.
            let _ = conn.execute(
                "DELETE FROM search_fts \
                 WHERE content_type = 'transcript' \
                   AND segment_id IN ( \
                       SELECT id FROM transcripts \
                       WHERE timestamp < datetime('now', '-90 days') \
                   )",
                [],
            );
            let n = conn
                .execute(
                    "DELETE FROM transcripts WHERE timestamp < datetime('now', '-90 days')",
                    [],
                )
                .unwrap_or(0) as u64;
            total += n;

            // pomodoro_state (V48, #8218): singleton focus-session state. A
            // session older than the behavior-data window is no longer useful
            // for resume and must not bypass local retention indefinitely.
            let n = conn
                .execute(
                    "DELETE FROM pomodoro_state
                     WHERE started_at < datetime('now', '-90 days')",
                    [],
                )
                .unwrap_or(0) as u64;
            total += n;

            // egress_ledger (V36, #4803): prevent unbounded growth — keep only the most
            // recent EGRESS_LEDGER_MAX_ROWS rows and delete older ones ordered by id
            // (safe even with an occurred_at tie-break).
            let n = conn
                .execute(
                    "DELETE FROM egress_ledger WHERE id NOT IN (
                     SELECT id FROM egress_ledger ORDER BY id DESC LIMIT ?1
                 )",
                    rusqlite::params![EGRESS_LEDGER_MAX_ROWS],
                )
                .unwrap_or(0) as u64;
            total += n;

            if total > 0 {
                tracing::info!(
                    "Enforced table retention: deleted {total} rows across auxiliary tables"
                );
            }

            Ok(total)
        })
    }

    /// Enforce the compliance-window age cap on the security audit trails
    /// (#8056 P3). `audit_log` and `session_audit_log` are excluded from
    /// `enforce_all_retention` and RETAINED across GDPR erasure, so they would
    /// otherwise grow without bound.
    ///
    /// - `session_audit_log` (not hash-chained): plain age DELETE past the
    ///   window.
    /// - `audit_log` (V37 SHA-256 hash chain, ADR-072 mirror): a CHAIN-SAFE
    ///   prefix prune. Only the oldest contiguous prefix that is entirely older
    ///   than the window is removed, and the pruned prefix's final `entry_hash`
    ///   is recorded as the retained chain's root anchor
    ///   ([`crate::audit_chain::AUDIT_CHAIN_PRUNED_ROOT_META_KEY`]) in the SAME
    ///   transaction, so `verify_audit_chain` still passes (it accepts the
    ///   anchor as the chain root). Rows within the window are never pruned, so
    ///   the chain is never emptied and tamper-evidence is preserved.
    ///
    /// Returns the total number of rows deleted across both trails.
    pub fn enforce_audit_retention(&self) -> Result<u64, StorageError> {
        let cutoff = (Utc::now() - chrono::Duration::days(AUDIT_TRAIL_RETENTION_DAYS)).to_rfc3339();
        // `run_mut` — the audit_log prune needs a transaction (anchor write +
        // prefix DELETE must be atomic). write_lock is skipped while the
        // deletion_flag is set (never prune during an erase).
        self.conn.write_lock().run_mut(0u64, |conn| {
            let tx = conn
                .transaction()
                .map_err(|e| StorageError::Internal(format!("audit retention tx begin: {e}")))?;
            let mut total: u64 = 0;

            // session_audit_log: not chained → simple age DELETE.
            total += tx
                .execute(
                    "DELETE FROM session_audit_log WHERE timestamp < ?1",
                    rusqlite::params![cutoff],
                )
                .unwrap_or(0) as u64;

            // audit_log: chain-safe prefix prune. `first_recent_seq` is the
            // smallest seq WITHIN the window; every chained row below it is the
            // prunable prefix. `None` → all chained rows are within the window
            // (or there are none), so there is nothing to prune and the chain is
            // left untouched (never emptied).
            let first_recent_seq: Option<i64> = tx
                .query_row(
                    "SELECT MIN(seq) FROM audit_log WHERE seq IS NOT NULL AND timestamp >= ?1",
                    rusqlite::params![cutoff],
                    |row| row.get::<_, Option<i64>>(0),
                )
                .unwrap_or(None);

            if let Some(recent_seq) = first_recent_seq {
                // The row at `recent_seq - 1` is the last row of the prunable
                // prefix; its `entry_hash` already equals the retained first
                // row's `prev_hash`, so it becomes the recorded chain root. If it
                // does not exist, there is no older prefix to prune.
                let anchor: Option<String> = tx
                    .query_row(
                        "SELECT entry_hash FROM audit_log WHERE seq = ?1",
                        rusqlite::params![recent_seq - 1],
                        |row| row.get::<_, String>(0),
                    )
                    .ok();

                if let Some(anchor_hash) = anchor {
                    // Record the new chain root BEFORE deleting the prefix, in
                    // this same transaction so the two are atomic.
                    tx.execute(
                        "INSERT OR REPLACE INTO app_meta (key, value) VALUES (?1, ?2)",
                        rusqlite::params![
                            crate::audit_chain::AUDIT_CHAIN_PRUNED_ROOT_META_KEY,
                            anchor_hash
                        ],
                    )
                    .map_err(|e| StorageError::Internal(format!("audit anchor write: {e}")))?;

                    total += tx
                        .execute(
                            "DELETE FROM audit_log WHERE seq IS NOT NULL AND seq < ?1",
                            rusqlite::params![recent_seq],
                        )
                        .map_err(|e| StorageError::Internal(format!("audit prefix prune: {e}")))?
                        as u64;
                }
            }

            tx.commit()
                .map_err(|e| StorageError::Internal(format!("audit retention commit: {e}")))?;

            if total > 0 {
                tracing::info!(
                    "Enforced audit-trail retention: pruned {total} rows older than {AUDIT_TRAIL_RETENTION_DAYS} days"
                );
            }
            Ok(total)
        })
    }
}

#[cfg(test)]
mod audit_retention_tests {
    use super::*;
    use maekon_core::models::audit::{AuditEntry, AuditStatus};

    fn audit_entry(i: usize, ts: chrono::DateTime<Utc>) -> AuditEntry {
        AuditEntry {
            entry_id: format!("id-{i}"),
            timestamp: ts,
            session_id: "sess".to_string(),
            command_id: format!("cmd-{i}"),
            action_type: "test".to_string(),
            status: AuditStatus::Completed,
            details: None,
            execution_time_ms: Some(1),
        }
    }

    fn count(storage: &SqliteStorage, table: &str) -> i64 {
        let conn = storage.connection_arc();
        let n = conn
            .read_lock()
            .run::<_, i64, rusqlite::Error>(|c| {
                c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            })
            .unwrap_or(-1);
        n
    }

    #[test]
    fn audit_log_prune_is_chain_safe_and_keeps_recent() {
        let storage = SqliteStorage::open_in_memory(30).expect("in-memory sqlite");
        let now = Utc::now();
        let old = now - chrono::Duration::days(AUDIT_TRAIL_RETENTION_DAYS + 70);

        // 3 old + 3 recent chained rows (seq 0..5 in insertion order).
        for i in 0..3 {
            storage.save_audit_entry(&audit_entry(i, old));
        }
        for i in 3..6 {
            storage.save_audit_entry(&audit_entry(i, now));
        }
        assert_eq!(count(&storage, "audit_log"), 6);
        assert!(
            storage.verify_audit_chain().ok,
            "chain must verify before prune"
        );

        let pruned = storage.enforce_audit_retention().expect("audit retention");
        assert_eq!(pruned, 3, "the 3 rows older than the window must be pruned");
        assert_eq!(
            count(&storage, "audit_log"),
            3,
            "the 3 recent rows must survive"
        );

        // Chain-safe: the retained chain (first row now links to the recorded
        // pruned-root anchor, not GENESIS) must still verify (ADR-072).
        let report = storage.verify_audit_chain();
        assert!(
            report.ok,
            "chain must still verify after a chain-safe prefix prune: {:?}",
            report.first_break
        );

        // Idempotent: a second pass prunes nothing (all remaining rows are recent).
        assert_eq!(
            storage.enforce_audit_retention().expect("second pass"),
            0,
            "recent rows within the window must never be pruned"
        );
        assert!(storage.verify_audit_chain().ok);
    }

    #[test]
    fn session_audit_log_prune_drops_old_keeps_recent() {
        let storage = SqliteStorage::open_in_memory(30).expect("in-memory sqlite");
        let old_ts =
            (Utc::now() - chrono::Duration::days(AUDIT_TRAIL_RETENTION_DAYS + 30)).to_rfc3339();
        {
            let conn = storage.connection_arc();
            let guard = conn.retained_write_lock();
            guard
                .execute(
                    "INSERT INTO session_audit_log (timestamp, session_id, category, event_type) \
                     VALUES (?1, 's', 'session', 'start')",
                    rusqlite::params![old_ts],
                )
                .expect("seed old session_audit_log row");
            guard
                .execute(
                    "INSERT INTO session_audit_log (session_id, category, event_type) \
                     VALUES ('s', 'session', 'start')",
                    [],
                )
                .expect("seed recent session_audit_log row");
        }
        assert_eq!(count(&storage, "session_audit_log"), 2);

        storage.enforce_audit_retention().expect("audit retention");

        assert_eq!(
            count(&storage, "session_audit_log"),
            1,
            "only the row older than the compliance window must be pruned"
        );
    }

    /// V47 (#8059): `enforce_all_retention` must prune voice transcripts older
    /// than the 90-day window AND their paired `search_fts` rows, while keeping
    /// recent transcripts and never touching unrelated FTS rows.
    #[test]
    fn transcript_retention_prunes_old_and_cleans_fts() {
        let storage = SqliteStorage::open_in_memory(30).expect("in-memory sqlite");
        let old_ts = (Utc::now() - chrono::Duration::days(120)).to_rfc3339();
        let recent_ts = (Utc::now() - chrono::Duration::days(3)).to_rfc3339();
        {
            let conn = storage.connection_arc();
            let guard = conn.retained_write_lock();
            for (id, ts) in [("tr-old", &old_ts), ("tr-recent", &recent_ts)] {
                guard
                    .execute(
                        "INSERT INTO transcripts (id, timestamp, duration_secs, source, text) \
                         VALUES (?1, ?2, 1.0, 'whisper', 'content')",
                        rusqlite::params![id, ts],
                    )
                    .expect("seed transcript");
                guard
                    .execute(
                        "INSERT INTO search_fts (segment_id, content_type, searchable_text, shadow) \
                         VALUES (?1, 'transcript', 'content', '')",
                        rusqlite::params![id],
                    )
                    .expect("seed transcript fts row");
            }
            // Unrelated segment FTS row that transcript cleanup must never touch.
            guard
                .execute(
                    "INSERT INTO search_fts (segment_id, content_type, searchable_text, shadow) \
                     VALUES ('seg-keep', 'segment', 'keep', '')",
                    [],
                )
                .expect("seed segment fts row");
        }
        assert_eq!(count(&storage, "transcripts"), 2);

        storage
            .enforce_all_retention()
            .expect("enforce_all_retention");

        assert_eq!(
            count(&storage, "transcripts"),
            1,
            "only the transcript older than 90 days must be pruned"
        );
        let survivor: String = {
            let conn = storage.connection_arc();
            let id = conn
                .read_lock()
                .run::<_, String, rusqlite::Error>(|c| {
                    c.query_row("SELECT id FROM transcripts", [], |r| r.get(0))
                })
                .expect("read survivor");
            id
        };
        assert_eq!(survivor, "tr-recent", "the recent transcript must survive");

        // The pruned transcript's FTS row is gone; the recent one's and the
        // unrelated segment's FTS rows survive.
        let fts_ids = |seg: &str| -> i64 {
            let conn = storage.connection_arc();
            let n = conn
                .read_lock()
                .run::<_, i64, rusqlite::Error>(|c| {
                    c.query_row(
                        "SELECT COUNT(*) FROM search_fts WHERE segment_id = ?1",
                        rusqlite::params![seg],
                        |r| r.get(0),
                    )
                })
                .unwrap_or(-1);
            n
        };
        assert_eq!(
            fts_ids("tr-old"),
            0,
            "pruned transcript FTS row must be gone"
        );
        assert_eq!(
            fts_ids("tr-recent"),
            1,
            "recent transcript FTS row survives"
        );
        assert_eq!(
            fts_ids("seg-keep"),
            1,
            "unrelated segment FTS row must never be touched"
        );
    }
}
