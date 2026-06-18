use crate::error::StorageError;
use std::sync::atomic::Ordering;
use tracing::{debug, info};

use super::super::{SqliteStorage, FTS_AVAILABLE};

impl SqliteStorage {
    /// Execute a PASSIVE WAL checkpoint. Non-blocking — does not wait for
    /// concurrent readers or writers to finish.
    pub fn wal_checkpoint_passive(&self) -> Result<(), StorageError> {
        // Structural maintenance (not a user-data mutation) — retained_write_lock (always runs).
        self.conn.retained_write_lock().run(|conn| {
            conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE)")
                .map_err(|e| {
                    StorageError::Internal(format!("WAL checkpoint PASSIVE failed: {e}"))
                })?;

            debug!("WAL checkpoint PASSIVE completed");
            Ok(())
        })
    }

    /// Execute a TRUNCATE WAL checkpoint. Blocks until all readers/writers
    /// finish, then checkpoints and truncates the WAL file to zero bytes.
    /// Intended for graceful shutdown after all background loops have stopped.
    pub fn wal_checkpoint_truncate(&self) -> Result<(), StorageError> {
        // Structural maintenance — retained_write_lock (always runs).
        self.conn.retained_write_lock().run(|conn| {
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
                .map_err(|e| {
                    StorageError::Internal(format!("WAL checkpoint TRUNCATE failed: {e}"))
                })?;

            debug!("WAL checkpoint TRUNCATE completed");
            Ok(())
        })
    }

    /// Run VACUUM if freelist_count / page_count exceeds `threshold_percent`.
    /// Returns `true` when VACUUM was actually executed.
    pub fn maybe_vacuum(&self, threshold_percent: u64) -> Result<bool, StorageError> {
        // Structural maintenance — retained_write_lock (always runs).
        self.conn.retained_write_lock().run(|conn| {
            let freelist_count: u64 = conn
                .query_row("PRAGMA freelist_count", [], |row| row.get(0))
                .unwrap_or(0);
            let page_count: u64 = conn
                .query_row("PRAGMA page_count", [], |row| row.get(0))
                .unwrap_or(1); // avoid div-by-zero

            if page_count == 0 {
                return Ok(false);
            }

            let free_pct = (freelist_count * 100) / page_count;
            if free_pct > threshold_percent {
                info!(
                "Running VACUUM: freelist={freelist_count} pages={page_count} ({free_pct}% free)"
            );
                conn.execute_batch("VACUUM")
                    .map_err(|e| StorageError::Internal(format!("VACUUM failed: {e}")))?;
                Ok(true)
            } else {
                debug!(
                "VACUUM skipped: freelist={freelist_count} pages={page_count} ({free_pct}% free)"
            );
                Ok(false)
            }
        })
    }

    /// Incrementally merge up to `pages` FTS5 b-tree pages.
    /// No-op if the search_fts table does not exist.
    pub fn fts_merge(&self, pages: u32) -> Result<(), StorageError> {
        if !FTS_AVAILABLE.load(Ordering::Relaxed) {
            return Ok(());
        }

        // FTS index maintenance — retained_write_lock (always runs, not a user-row mutation).
        self.conn.retained_write_lock().run(|conn| {
            conn.execute(
                "INSERT INTO search_fts(search_fts, rank) VALUES('merge', ?1)",
                rusqlite::params![pages as i64],
            )
            .map_err(|e| StorageError::Internal(format!("FTS5 merge failed: {e}")))?;

            debug!("FTS5 incremental merge completed ({pages} pages)");
            Ok(())
        })
    }

    /// Full FTS5 optimize — merges all b-tree segments into one. Expensive
    /// but dramatically speeds up subsequent FTS queries.
    /// No-op if the search_fts table does not exist.
    pub fn fts_optimize(&self) -> Result<(), StorageError> {
        if !FTS_AVAILABLE.load(Ordering::Relaxed) {
            return Ok(());
        }

        // FTS index maintenance — retained_write_lock (always runs).
        self.conn.retained_write_lock().run(|conn| {
            conn.execute("INSERT INTO search_fts(search_fts) VALUES('optimize')", [])
                .map_err(|e| StorageError::Internal(format!("FTS5 optimize failed: {e}")))?;

            info!("FTS5 full optimize completed");
            Ok(())
        })
    }

    /// Run ANALYZE to refresh query planner statistics for all tables.
    pub fn run_analyze(&self) -> Result<(), StorageError> {
        // Structural maintenance — retained_write_lock (always runs).
        self.conn.retained_write_lock().run(|conn| {
            conn.execute_batch("ANALYZE")
                .map_err(|e| StorageError::Internal(format!("ANALYZE failed: {e}")))?;

            debug!("ANALYZE completed");
            Ok(())
        })
    }

    /// Run ANALYZE using an already-held connection guard. Use this inside
    /// methods that have already locked `self.conn` to avoid deadlocking.
    pub(in super::super) fn run_analyze_with_conn(
        conn: &rusqlite::Connection,
    ) -> Result<(), StorageError> {
        conn.execute_batch("ANALYZE")
            .map_err(|e| StorageError::Internal(format!("ANALYZE failed: {e}")))?;
        debug!("ANALYZE (inline) completed");
        Ok(())
    }
}
