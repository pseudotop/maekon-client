use crate::error::StorageError;
use chrono::Utc;

use super::super::SqliteStorage;

/// egress_ledger retention cap — keep only the most recent N rows (V36, #4803).
/// A cap to prevent unbounded growth. Excess rows are deleted in `enforce_all_retention`.
const EGRESS_LEDGER_MAX_ROWS: i64 = 5000;

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

            // suggestions: 90 days
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

            // regime_overrides: 180 days
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
}
