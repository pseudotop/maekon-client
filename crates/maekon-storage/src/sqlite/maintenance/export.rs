use crate::error::StorageError;

use super::super::{
    EventExportRecord, FrameExportRecord, MetricExportRecord, SearchEventRow, SearchFrameRow,
    SqliteStorage,
};

impl SqliteStorage {
    pub fn list_event_exports(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<EventExportRecord>, StorageError> {
        // 읽기 — read_lock(deletion_flag 무관).
        let read = self.conn.read_lock();
        Self::list_event_exports_inner(read.conn(), from, to)
    }

    /// Async `list_event_exports` over the read funnel (ADR-026 PR-7).
    pub(crate) async fn list_event_exports_async(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<EventExportRecord>, StorageError> {
        // owned move into the Send + 'static closure (no borrowed &str).
        let from = from.to_owned();
        let to = to.to_owned();
        self.with_conn_read(move |conn| Self::list_event_exports_inner(conn, &from, &to))
            .await
    }

    fn list_event_exports_inner(
        conn: &rusqlite::Connection,
        from: &str,
        to: &str,
    ) -> Result<Vec<EventExportRecord>, StorageError> {
        let mut stmt = conn
            .prepare(
                "SELECT event_id, event_type, timestamp,
                        json_extract(data, '$.app_name'),
                        json_extract(data, '$.window_title')
                 FROM events
                 WHERE timestamp >= ?1 AND timestamp <= ?2
                 ORDER BY timestamp ASC",
            )
            .map_err(|e| StorageError::Internal(format!("Failed to prepare query: {e}")))?;

        let rows = stmt
            .query_map(rusqlite::params![from, to], |row| {
                Ok(EventExportRecord {
                    event_id: row.get(0)?,
                    event_type: row.get(1)?,
                    timestamp: row.get(2)?,
                    app_name: row.get(3)?,
                    window_title: row.get(4)?,
                })
            })
            .map_err(|e| StorageError::Internal(format!("Failed to execute query: {e}")))?;

        let mut records = Vec::new();
        for row in rows {
            records
                .push(row.map_err(|e| StorageError::Internal(format!("Failed to read row: {e}")))?);
        }
        Ok(records)
    }

    pub fn list_metric_exports(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<MetricExportRecord>, StorageError> {
        // 읽기 — read_lock(deletion_flag 무관).
        let read = self.conn.read_lock();
        Self::list_metric_exports_inner(read.conn(), from, to)
    }

    /// Async `list_metric_exports` over the read funnel (ADR-026 PR-7).
    pub(crate) async fn list_metric_exports_async(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<MetricExportRecord>, StorageError> {
        // owned move into the Send + 'static closure (no borrowed &str).
        let from = from.to_owned();
        let to = to.to_owned();
        self.with_conn_read(move |conn| Self::list_metric_exports_inner(conn, &from, &to))
            .await
    }

    fn list_metric_exports_inner(
        conn: &rusqlite::Connection,
        from: &str,
        to: &str,
    ) -> Result<Vec<MetricExportRecord>, StorageError> {
        let mut stmt = conn
            .prepare(
                "SELECT timestamp, cpu_usage, memory_used, memory_total, disk_used, disk_total,
                        network_upload, network_download
                 FROM system_metrics
                 WHERE timestamp >= ?1 AND timestamp <= ?2
                 ORDER BY timestamp ASC",
            )
            .map_err(|e| StorageError::Internal(format!("Failed to prepare query: {e}")))?;

        let rows = stmt
            .query_map(rusqlite::params![from, to], |row| {
                Ok(MetricExportRecord {
                    timestamp: row.get(0)?,
                    cpu_usage: row.get(1)?,
                    memory_used: row.get(2)?,
                    memory_total: row.get(3)?,
                    disk_used: row.get(4)?,
                    disk_total: row.get(5)?,
                    network_upload: row.get(6)?,
                    network_download: row.get(7)?,
                })
            })
            .map_err(|e| StorageError::Internal(format!("Failed to execute query: {e}")))?;

        let mut records = Vec::new();
        for row in rows {
            records
                .push(row.map_err(|e| StorageError::Internal(format!("Failed to read row: {e}")))?);
        }
        Ok(records)
    }

    pub fn list_frame_exports(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<FrameExportRecord>, StorageError> {
        // 읽기 — read_lock(deletion_flag 무관).
        let read = self.conn.read_lock();
        Self::list_frame_exports_inner(read.conn(), from, to)
    }

    /// Async `list_frame_exports` over the read funnel (ADR-026 PR-7).
    pub(crate) async fn list_frame_exports_async(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<FrameExportRecord>, StorageError> {
        // owned move into the Send + 'static closure (no borrowed &str).
        let from = from.to_owned();
        let to = to.to_owned();
        self.with_conn_read(move |conn| Self::list_frame_exports_inner(conn, &from, &to))
            .await
    }

    fn list_frame_exports_inner(
        conn: &rusqlite::Connection,
        from: &str,
        to: &str,
    ) -> Result<Vec<FrameExportRecord>, StorageError> {
        let mut stmt = conn
            .prepare(
                "SELECT id, timestamp, trigger_type, app_name, window_title, importance,
                        resolution_w, resolution_h, ocr_text
                 FROM frames
                 WHERE timestamp >= ?1 AND timestamp <= ?2
                 ORDER BY timestamp ASC",
            )
            .map_err(|e| StorageError::Internal(format!("Failed to prepare query: {e}")))?;

        let rows = stmt
            .query_map(rusqlite::params![from, to], |row| {
                Ok(FrameExportRecord {
                    id: row.get(0)?,
                    timestamp: row.get(1)?,
                    trigger_type: row.get(2)?,
                    app_name: row.get(3)?,
                    window_title: row.get(4)?,
                    importance: row.get(5)?,
                    resolution_w: row.get(6)?,
                    resolution_h: row.get(7)?,
                    ocr_text: row.get(8)?,
                })
            })
            .map_err(|e| StorageError::Internal(format!("Failed to execute query: {e}")))?;

        let mut records = Vec::new();
        for row in rows {
            records
                .push(row.map_err(|e| StorageError::Internal(format!("Failed to read row: {e}")))?);
        }
        Ok(records)
    }

    pub fn count_search_frames(
        &self,
        count_sql: &str,
        pattern: Option<&str>,
    ) -> Result<u64, StorageError> {
        // 읽기 — read_lock(deletion_flag 무관).
        let read = self.conn.read_lock();
        Self::count_search_frames_inner(read.conn(), count_sql, pattern)
    }

    /// Async `count_search_frames` over the read funnel (ADR-026 PR-4).
    pub(crate) async fn count_search_frames_async(
        &self,
        count_sql: &str,
        pattern: Option<&str>,
    ) -> Result<u64, StorageError> {
        // owned move into the Send + 'static closure (no borrowed &str).
        let count_sql = count_sql.to_owned();
        let pattern = pattern.map(str::to_owned);
        self.with_conn_read(move |conn| {
            Self::count_search_frames_inner(conn, &count_sql, pattern.as_deref())
        })
        .await
    }

    fn count_search_frames_inner(
        conn: &rusqlite::Connection,
        count_sql: &str,
        pattern: Option<&str>,
    ) -> Result<u64, StorageError> {
        let count: i64 = match pattern {
            Some(p) => conn
                .query_row(count_sql, rusqlite::params![p], |row| row.get(0))
                .map_err(|e| {
                    StorageError::Internal(format!("Failed to count frame search results: {e}"))
                })?,
            None => conn
                .query_row(count_sql, [], |row| row.get(0))
                .map_err(|e| {
                    StorageError::Internal(format!("Failed to count frame search results: {e}"))
                })?,
        };

        Ok(count as u64)
    }

    pub fn search_frames_with_sql(
        &self,
        select_sql: &str,
        pattern: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<SearchFrameRow>, StorageError> {
        // 읽기 — read_lock(deletion_flag 무관).
        let read = self.conn.read_lock();
        Self::search_frames_with_sql_inner(read.conn(), select_sql, pattern, limit, offset)
    }

    /// Async `search_frames_with_sql` over the read funnel (ADR-026 PR-4).
    pub(crate) async fn search_frames_with_sql_async(
        &self,
        select_sql: &str,
        pattern: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<SearchFrameRow>, StorageError> {
        // owned move into the Send + 'static closure (no borrowed &str).
        let select_sql = select_sql.to_owned();
        let pattern = pattern.map(str::to_owned);
        self.with_conn_read(move |conn| {
            Self::search_frames_with_sql_inner(conn, &select_sql, pattern.as_deref(), limit, offset)
        })
        .await
    }

    fn search_frames_with_sql_inner(
        conn: &rusqlite::Connection,
        select_sql: &str,
        pattern: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<SearchFrameRow>, StorageError> {
        let mut stmt = conn
            .prepare(select_sql)
            .map_err(|e| StorageError::Internal(format!("Failed to prepare query: {e}")))?;

        if let Some(p) = pattern {
            let rows = stmt
                .query_map(
                    rusqlite::params![p, limit.to_string(), offset.to_string()],
                    |row| {
                        Ok(SearchFrameRow {
                            id: row.get(0)?,
                            timestamp: row.get(1)?,
                            app_name: row.get(2)?,
                            window_title: row.get(3)?,
                            matched_text: row.get(4)?,
                            importance: row.get(5)?,
                            file_path: row.get(6)?,
                        })
                    },
                )
                .map_err(|e| StorageError::Internal(format!("Failed to query frames: {e}")))?;

            let mut records = Vec::new();
            for row in rows {
                records.push(
                    row.map_err(|e| StorageError::Internal(format!("Failed to read row: {e}")))?,
                );
            }
            Ok(records)
        } else {
            let rows = stmt
                .query_map(
                    rusqlite::params![limit.to_string(), offset.to_string()],
                    |row| {
                        Ok(SearchFrameRow {
                            id: row.get(0)?,
                            timestamp: row.get(1)?,
                            app_name: row.get(2)?,
                            window_title: row.get(3)?,
                            matched_text: row.get(4)?,
                            importance: row.get(5)?,
                            file_path: row.get(6)?,
                        })
                    },
                )
                .map_err(|e| StorageError::Internal(format!("Failed to query frames: {e}")))?;

            let mut records = Vec::new();
            for row in rows {
                records.push(
                    row.map_err(|e| StorageError::Internal(format!("Failed to read row: {e}")))?,
                );
            }
            Ok(records)
        }
    }

    pub fn count_search_events(&self, pattern: &str) -> Result<u64, StorageError> {
        // 읽기 — read_lock(deletion_flag 무관).
        let read = self.conn.read_lock();
        Self::count_search_events_inner(read.conn(), pattern)
    }

    /// Async `count_search_events` over the read funnel (ADR-026 PR-4).
    pub(crate) async fn count_search_events_async(
        &self,
        pattern: &str,
    ) -> Result<u64, StorageError> {
        // owned move into the Send + 'static closure (no borrowed &str).
        let pattern = pattern.to_owned();
        self.with_conn_read(move |conn| Self::count_search_events_inner(conn, &pattern))
            .await
    }

    fn count_search_events_inner(
        conn: &rusqlite::Connection,
        pattern: &str,
    ) -> Result<u64, StorageError> {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events
                 WHERE data LIKE ?1",
                rusqlite::params![pattern],
                |row| row.get(0),
            )
            .map_err(|e| {
                StorageError::Internal(format!("Failed to count event search results: {e}"))
            })?;

        Ok(count as u64)
    }

    pub fn search_events(
        &self,
        pattern: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<SearchEventRow>, StorageError> {
        // 읽기 — read_lock(deletion_flag 무관).
        let read = self.conn.read_lock();
        Self::search_events_inner(read.conn(), pattern, limit, offset)
    }

    /// Async `search_events` over the read funnel (ADR-026 PR-4).
    pub(crate) async fn search_events_async(
        &self,
        pattern: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<SearchEventRow>, StorageError> {
        // owned move into the Send + 'static closure (no borrowed &str).
        let pattern = pattern.to_owned();
        self.with_conn_read(move |conn| Self::search_events_inner(conn, &pattern, limit, offset))
            .await
    }

    fn search_events_inner(
        conn: &rusqlite::Connection,
        pattern: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<SearchEventRow>, StorageError> {
        let mut stmt = conn
            .prepare(
                "SELECT event_id, timestamp,
                        json_extract(data, '$.app_name'),
                        json_extract(data, '$.window_title'),
                        data
                 FROM events
                 WHERE data LIKE ?1
                 ORDER BY timestamp DESC
                 LIMIT ?2 OFFSET ?3",
            )
            .map_err(|e| StorageError::Internal(format!("Failed to prepare query: {e}")))?;

        let rows = stmt
            .query_map(
                rusqlite::params![pattern, limit.to_string(), offset.to_string()],
                |row| {
                    Ok(SearchEventRow {
                        event_id: row.get(0)?,
                        timestamp: row.get(1)?,
                        app_name: row.get(2)?,
                        window_title: row.get(3)?,
                        data: row.get(4)?,
                    })
                },
            )
            .map_err(|e| StorageError::Internal(format!("Failed to query events: {e}")))?;

        let mut records = Vec::new();
        for row in rows {
            records
                .push(row.map_err(|e| StorageError::Internal(format!("Failed to read row: {e}")))?);
        }
        Ok(records)
    }
}
