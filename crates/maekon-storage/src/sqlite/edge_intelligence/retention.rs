use crate::error::StorageError;
use chrono::Utc;

use super::super::SqliteStorage;

impl SqliteStorage {
    /// Delete activity segments older than `max_days`. Returns the number of deleted rows.
    pub fn enforce_segment_retention(&self, max_days: u32) -> Result<usize, StorageError> {
        let cutoff = (Utc::now() - chrono::Duration::days(max_days as i64)).to_rfc3339();
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("SQLite lock poisoned: {e}")))?;
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
    }

    /// Delete weekly digests older than `max_weeks`. Returns the number of deleted rows.
    pub fn enforce_digest_retention(&self, max_weeks: u32) -> Result<usize, StorageError> {
        let cutoff = (Utc::now() - chrono::Duration::days(max_weeks as i64 * 7)).to_rfc3339();
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("SQLite lock poisoned: {e}")))?;
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
    }

    /// Enforce retention for all auxiliary tables that would otherwise grow
    /// unbounded. Each table has its own retention window. Tables that may
    /// not exist in older schema versions are handled gracefully (errors
    /// from `conn.execute` are silently ignored via `let _ = …`).
    ///
    /// Returns the total number of rows deleted across all tables.
    pub fn enforce_all_retention(&self) -> Result<u64, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("SQLite lock poisoned: {e}")))?;

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

        // regime_overrides: 180 days
        let n = conn
            .execute(
                "DELETE FROM regime_overrides WHERE created_at < datetime('now', '-180 days')",
                [],
            )
            .unwrap_or(0) as u64;
        total += n;

        if total > 0 {
            tracing::info!(
                "Enforced table retention: deleted {total} rows across auxiliary tables"
            );
        }

        Ok(total)
    }
}
