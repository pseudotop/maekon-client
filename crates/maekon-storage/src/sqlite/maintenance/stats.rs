use crate::error::StorageError;
use maekon_core::types::TimeWindow;

use super::super::{SqliteStorage, StorageStatsSummaryRecord};

impl SqliteStorage {
    pub fn get_storage_stats_summary(&self) -> Result<StorageStatsSummaryRecord, StorageError> {
        // 읽기 — read_lock(deletion_flag 무관).
        let read = self.conn.read_lock();
        Self::get_storage_stats_summary_inner(read.conn())
    }

    /// Async `get_storage_stats_summary` over the read funnel (ADR-026 PR-4).
    pub(crate) async fn get_storage_stats_summary_async(
        &self,
    ) -> Result<StorageStatsSummaryRecord, StorageError> {
        self.with_conn_read(Self::get_storage_stats_summary_inner)
            .await
    }

    fn get_storage_stats_summary_inner(
        conn: &rusqlite::Connection,
    ) -> Result<StorageStatsSummaryRecord, StorageError> {
        let frame_count: u64 = conn
            .query_row("SELECT COUNT(*) FROM frames", [], |row| row.get(0))
            .unwrap_or(0);
        let event_count: u64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
            .unwrap_or(0);
        let metric_count: u64 = conn
            .query_row("SELECT COUNT(*) FROM system_metrics", [], |row| row.get(0))
            .unwrap_or(0);

        let oldest_data_date: Option<String> = conn
            .query_row(
                "SELECT MIN(timestamp) FROM (
                    SELECT timestamp FROM events
                    UNION ALL
                    SELECT timestamp FROM frames
                    UNION ALL
                    SELECT timestamp FROM system_metrics
                )",
                [],
                |row| row.get(0),
            )
            .ok();

        let newest_data_date: Option<String> = conn
            .query_row(
                "SELECT MAX(timestamp) FROM (
                    SELECT timestamp FROM events
                    UNION ALL
                    SELECT timestamp FROM frames
                    UNION ALL
                    SELECT timestamp FROM system_metrics
                )",
                [],
                |row| row.get(0),
            )
            .ok();

        let page_count: u64 = conn
            .query_row("PRAGMA page_count", [], |row| row.get(0))
            .unwrap_or(0);
        let page_size: u64 = conn
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .unwrap_or(4096);

        Ok(StorageStatsSummaryRecord {
            frame_count,
            event_count,
            metric_count,
            oldest_data_date,
            newest_data_date,
            page_count,
            page_size,
        })
    }

    pub fn list_frame_file_paths_in_range(
        &self,
        window: &TimeWindow,
    ) -> Result<Vec<String>, StorageError> {
        let (from, to) = window.to_sql_pair();
        // 읽기 — read_lock(deletion_flag 무관).
        let read = self.conn.read_lock();
        Self::list_frame_file_paths_in_range_inner(read.conn(), &from, &to)
    }

    /// Async `list_frame_file_paths_in_range` over the read funnel (ADR-026 PR-4).
    pub(crate) async fn list_frame_file_paths_in_range_async(
        &self,
        window: &TimeWindow,
    ) -> Result<Vec<String>, StorageError> {
        // owned move into the Send + 'static closure.
        let (from, to) = window.to_sql_pair();
        self.with_conn_read(move |conn| {
            Self::list_frame_file_paths_in_range_inner(conn, &from, &to)
        })
        .await
    }

    fn list_frame_file_paths_in_range_inner(
        conn: &rusqlite::Connection,
        from: &str,
        to: &str,
    ) -> Result<Vec<String>, StorageError> {
        let mut stmt = conn
            .prepare("SELECT file_path FROM frames WHERE timestamp >= ?1 AND timestamp <= ?2")
            .map_err(|e| StorageError::Internal(format!("Failed to prepare query: {e}")))?;

        let rows = stmt
            .query_map(rusqlite::params![from, to], |row| {
                row.get::<_, Option<String>>(0)
            })
            .map_err(|e| StorageError::Internal(format!("Failed to execute query: {e}")))?;

        let mut paths = Vec::new();
        for row in rows {
            if let Some(path) = row
                .map_err(|e| StorageError::Internal(format!("Failed to read row: {e}")))?
                .filter(|p| !p.is_empty())
            {
                paths.push(path);
            }
        }
        Ok(paths)
    }

    pub fn list_all_frame_file_paths(&self) -> Result<Vec<String>, StorageError> {
        // 읽기 — read_lock(deletion_flag 무관).
        let read = self.conn.read_lock();
        Self::list_all_frame_file_paths_inner(read.conn())
    }

    /// Async `list_all_frame_file_paths` over the read funnel (ADR-026 PR-4).
    pub(crate) async fn list_all_frame_file_paths_async(
        &self,
    ) -> Result<Vec<String>, StorageError> {
        self.with_conn_read(Self::list_all_frame_file_paths_inner)
            .await
    }

    fn list_all_frame_file_paths_inner(
        conn: &rusqlite::Connection,
    ) -> Result<Vec<String>, StorageError> {
        let mut stmt = conn
            .prepare("SELECT file_path FROM frames")
            .map_err(|e| StorageError::Internal(format!("Failed to prepare query: {e}")))?;

        let rows = stmt
            .query_map([], |row| row.get::<_, Option<String>>(0))
            .map_err(|e| StorageError::Internal(format!("Failed to execute query: {e}")))?;

        let mut paths = Vec::new();
        for row in rows {
            if let Some(path) = row
                .map_err(|e| StorageError::Internal(format!("Failed to read row: {e}")))?
                .filter(|p| !p.is_empty())
            {
                paths.push(path);
            }
        }
        Ok(paths)
    }
}
