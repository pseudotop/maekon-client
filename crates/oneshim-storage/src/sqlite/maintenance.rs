use crate::error::StorageError;
use oneshim_core::types::TimeWindow;
use std::sync::atomic::Ordering;
use tracing::{debug, info};

use super::{
    DeletedRangeCounts, EventExportRecord, FrameExportRecord, FrameTagLinkRecord,
    MetricExportRecord, SearchEventRow, SearchFrameRow, SqliteStorage, StorageStatsSummaryRecord,
    FTS_AVAILABLE,
};

impl SqliteStorage {
    pub fn list_backup_tags(&self) -> Result<Vec<super::TagRecord>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        let mut stmt = conn
            .prepare("SELECT id, name, color, created_at FROM tags ORDER BY id")
            .map_err(|e| StorageError::Internal(format!("Failed to prepare query: {e}")))?;

        let rows = stmt
            .query_map([], |row| {
                Ok(super::TagRecord {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    color: row.get(2)?,
                    created_at: row.get(3)?,
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

    pub fn list_backup_frame_tags(&self) -> Result<Vec<FrameTagLinkRecord>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        let mut stmt = conn
            .prepare(
                "SELECT frame_id, tag_id, created_at FROM frame_tags ORDER BY frame_id, tag_id",
            )
            .map_err(|e| StorageError::Internal(format!("Failed to prepare query: {e}")))?;

        let rows = stmt
            .query_map([], |row| {
                Ok(FrameTagLinkRecord {
                    frame_id: row.get(0)?,
                    tag_id: row.get(1)?,
                    created_at: row.get(2)?,
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

    pub fn upsert_backup_tag(
        &self,
        id: i64,
        name: &str,
        color: &str,
        created_at: &str,
    ) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        conn.execute(
            "INSERT OR IGNORE INTO tags (id, name, color, created_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![id, name, color, created_at],
        )
        .map_err(|e| StorageError::Internal(format!("Failed to save tag: {e}")))?;

        Ok(())
    }

    pub fn upsert_backup_frame_tag(
        &self,
        frame_id: i64,
        tag_id: i64,
        created_at: &str,
    ) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        conn.execute(
            "INSERT OR IGNORE INTO frame_tags (frame_id, tag_id, created_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![frame_id, tag_id, created_at],
        )
        .map_err(|e| StorageError::Internal(format!("frame-Failed to save tag: {e}")))?;

        Ok(())
    }

    pub fn upsert_backup_event(
        &self,
        event_id: &str,
        event_type: &str,
        timestamp: &str,
        app_name: Option<&str>,
        window_title: Option<&str>,
    ) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        let data = serde_json::json!({
            "app_name": app_name,
            "window_title": window_title,
        })
        .to_string();

        conn.execute(
            "INSERT OR IGNORE INTO events (event_id, event_type, timestamp, data)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![event_id, event_type, timestamp, data],
        )
        .map_err(|e| StorageError::Internal(format!("event save failure: {e}")))?;

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn upsert_backup_frame(
        &self,
        id: i64,
        timestamp: &str,
        trigger_type: &str,
        app_name: &str,
        window_title: &str,
        importance: f32,
        width: i32,
        height: i32,
        ocr_text: Option<&str>,
    ) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM frames WHERE id = ?1)",
                rusqlite::params![id],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if !exists {
            conn.execute(
                "INSERT INTO frames (
                    id, timestamp, trigger_type, app_name, window_title,
                    importance, resolution_w, resolution_h, has_image, ocr_text, file_path
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9, NULL)",
                rusqlite::params![
                    id,
                    timestamp,
                    trigger_type,
                    app_name,
                    window_title,
                    importance,
                    width,
                    height,
                    ocr_text,
                ],
            )
            .map_err(|e| StorageError::Internal(format!("frame save failure: {e}")))?;
        }

        Ok(())
    }

    pub fn get_storage_stats_summary(&self) -> Result<StorageStatsSummaryRecord, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

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
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

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

    #[allow(clippy::too_many_arguments)]
    pub fn delete_data_in_range(
        &self,
        window: &TimeWindow,
        delete_events: bool,
        delete_frames: bool,
        delete_metrics: bool,
        delete_processes: bool,
        delete_idle: bool,
    ) -> Result<DeletedRangeCounts, StorageError> {
        let (from, to) = window.to_sql_pair();
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        let mut counts = DeletedRangeCounts::default();

        if delete_events {
            counts.events_deleted = conn
                .execute(
                    "DELETE FROM events WHERE timestamp >= ?1 AND timestamp <= ?2",
                    rusqlite::params![from, to],
                )
                .map_err(|e| StorageError::Internal(format!("event delete failure: {e}")))?
                as u64;
        }

        if delete_frames {
            counts.frames_deleted = conn
                .execute(
                    "DELETE FROM frames WHERE timestamp >= ?1 AND timestamp <= ?2",
                    rusqlite::params![from, to],
                )
                .map_err(|e| StorageError::Internal(format!("frame delete failure: {e}")))?
                as u64;
        }

        if delete_metrics {
            counts.metrics_deleted = conn
                .execute(
                    "DELETE FROM system_metrics WHERE timestamp >= ?1 AND timestamp <= ?2",
                    rusqlite::params![from, to],
                )
                .map_err(|e| StorageError::Internal(format!("Failed to delete metrics: {e}")))?
                as u64;

            let _ = conn.execute(
                "DELETE FROM system_metrics_hourly WHERE hour >= ?1 AND hour <= ?2",
                rusqlite::params![from, to],
            );
        }

        if delete_processes {
            counts.process_snapshots_deleted = conn
                .execute(
                    "DELETE FROM process_snapshots WHERE timestamp >= ?1 AND timestamp <= ?2",
                    rusqlite::params![from, to],
                )
                .map_err(|e| {
                    StorageError::Internal(format!("Failed to delete process snapshots: {e}"))
                })? as u64;
        }

        if delete_idle {
            counts.idle_periods_deleted = conn
                .execute(
                    "DELETE FROM idle_periods WHERE start_time >= ?1 AND start_time <= ?2",
                    rusqlite::params![from, to],
                )
                .map_err(|e| StorageError::Internal(format!("idle record delete failure: {e}")))?
                as u64;
        }

        Ok(counts)
    }

    /// Atomically delete all user data from every known table inside a single
    /// SQLite transaction. On any failure the transaction auto-rolls-back so
    /// the database is never left in a partially-deleted state (GDPR compliance).
    pub fn delete_all_data(&self) -> Result<(), StorageError> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        // All tables created by V1-V17 migrations (excluding schema_version).
        // Order: child/referencing tables before parent tables to avoid FK issues
        // if foreign keys are ever enabled.
        const ALL_TABLES: &[&str] = &[
            // V1-V7
            "events",
            "frames",
            "system_metrics",
            "system_metrics_hourly",
            "process_snapshots",
            "idle_periods",
            "session_stats",
            "work_sessions",
            "interruptions",
            "focus_metrics",
            "suggestions",
            "local_suggestions",
            "frame_tags",
            "tags",
            // V8-V10
            "activity_segments",
            "calibration_log",
            "daily_digests",
            "weekly_digests",
            "embedding_vectors",
            "regime_overrides",
            "regimes",
            "trigger_params_snapshots",
            // V11: FTS5 virtual table
            "search_fts",
            // V18: Korean trigram FTS5 table
            "search_trigram",
            // V12-V14
            "vector_binary_codes",
            "vector_index_meta",
            "ivf_centroids",
            "ivf_assignments",
            "gui_interactions",
            "device_identity",
            "sync_peers",
            // V15-V16
            "lan_peer_pins",
            // V17: coaching
            "coaching_events",
            "regime_goals",
            "coaching_effectiveness",
        ];

        let tx = conn
            .transaction()
            .map_err(|e| StorageError::Internal(format!("Failed to begin transaction: {e}")))?;

        for table in ALL_TABLES {
            tx.execute(&format!("DELETE FROM {table}"), [])
                .map_err(|e| {
                    StorageError::Internal(format!("GDPR delete failed on table '{table}': {e}"))
                })?;
        }

        tx.commit()
            .map_err(|e| StorageError::Internal(format!("Failed to commit GDPR deletion: {e}")))?;

        Ok(())
    }

    pub fn list_event_exports(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<EventExportRecord>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

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
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

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
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

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
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

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
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

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
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

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
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

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

    // --- SQLite maintenance methods ---

    /// Execute a PASSIVE WAL checkpoint. Non-blocking — does not wait for
    /// concurrent readers or writers to finish.
    pub fn wal_checkpoint_passive(&self) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE)")
            .map_err(|e| StorageError::Internal(format!("WAL checkpoint PASSIVE failed: {e}")))?;

        debug!("WAL checkpoint PASSIVE completed");
        Ok(())
    }

    /// Execute a TRUNCATE WAL checkpoint. Blocks until all readers/writers
    /// finish, then checkpoints and truncates the WAL file to zero bytes.
    /// Intended for graceful shutdown after all background loops have stopped.
    pub fn wal_checkpoint_truncate(&self) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .map_err(|e| StorageError::Internal(format!("WAL checkpoint TRUNCATE failed: {e}")))?;

        debug!("WAL checkpoint TRUNCATE completed");
        Ok(())
    }

    /// Run VACUUM if freelist_count / page_count exceeds `threshold_percent`.
    /// Returns `true` when VACUUM was actually executed.
    pub fn maybe_vacuum(&self, threshold_percent: u64) -> Result<bool, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

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
    }

    /// Incrementally merge up to `pages` FTS5 b-tree pages.
    /// No-op if the search_fts table does not exist.
    pub fn fts_merge(&self, pages: u32) -> Result<(), StorageError> {
        if !FTS_AVAILABLE.load(Ordering::Relaxed) {
            return Ok(());
        }

        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        conn.execute(
            "INSERT INTO search_fts(search_fts, rank) VALUES('merge', ?1)",
            rusqlite::params![pages as i64],
        )
        .map_err(|e| StorageError::Internal(format!("FTS5 merge failed: {e}")))?;

        debug!("FTS5 incremental merge completed ({pages} pages)");
        Ok(())
    }

    /// Full FTS5 optimize — merges all b-tree segments into one. Expensive
    /// but dramatically speeds up subsequent FTS queries.
    /// No-op if the search_fts table does not exist.
    pub fn fts_optimize(&self) -> Result<(), StorageError> {
        if !FTS_AVAILABLE.load(Ordering::Relaxed) {
            return Ok(());
        }

        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        conn.execute("INSERT INTO search_fts(search_fts) VALUES('optimize')", [])
            .map_err(|e| StorageError::Internal(format!("FTS5 optimize failed: {e}")))?;

        info!("FTS5 full optimize completed");
        Ok(())
    }

    /// Run ANALYZE to refresh query planner statistics for all tables.
    pub fn run_analyze(&self) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        conn.execute_batch("ANALYZE")
            .map_err(|e| StorageError::Internal(format!("ANALYZE failed: {e}")))?;

        debug!("ANALYZE completed");
        Ok(())
    }

    /// Run ANALYZE using an already-held connection guard. Use this inside
    /// methods that have already locked `self.conn` to avoid deadlocking.
    pub(super) fn run_analyze_with_conn(conn: &rusqlite::Connection) -> Result<(), StorageError> {
        conn.execute_batch("ANALYZE")
            .map_err(|e| StorageError::Internal(format!("ANALYZE failed: {e}")))?;
        debug!("ANALYZE (inline) completed");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Helper: insert test data via sync upsert methods ────────────

    fn insert_events(storage: &SqliteStorage, timestamps: &[&str]) {
        for (i, ts) in timestamps.iter().enumerate() {
            storage
                .upsert_backup_event(
                    &format!("evt-{i}"),
                    "WindowChange",
                    ts,
                    Some("Code"),
                    Some("test.rs"),
                )
                .unwrap();
        }
    }

    fn insert_frame(storage: &SqliteStorage, id: i64, timestamp: &str) {
        storage
            .upsert_backup_frame(
                id, timestamp, "manual", "Code", "main.rs", 0.5, 1920, 1080, None,
            )
            .unwrap();
    }

    fn insert_metric(storage: &SqliteStorage, timestamp: &str) {
        let conn = storage.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO system_metrics (timestamp, cpu_usage, memory_used, memory_total, disk_used, disk_total, network_upload, network_download)
             VALUES (?1, 45.5, 8589934592, 17179869184, 107374182400, 536870912000, 1000, 5000)",
            rusqlite::params![timestamp],
        )
        .unwrap();
    }

    // ── maybe_vacuum ────────────────────────────────────────────────

    #[test]
    fn maybe_vacuum_fresh_db_returns_false() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let vacuumed = storage.maybe_vacuum(10).unwrap();
        assert!(
            !vacuumed,
            "fresh DB has no freelist pages, should skip VACUUM"
        );
    }

    #[test]
    fn maybe_vacuum_after_bulk_delete() {
        // In-memory databases may not accumulate freelist pages the same way
        // as disk databases, so we just verify the method runs without error.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("vacuum_test.db");
        let storage = SqliteStorage::open(&db_path, 30, None).unwrap();

        // Insert and delete bulk data to create freelist pages.
        for i in 0..500 {
            storage
                .upsert_backup_event(
                    &format!("bulk-{i}"),
                    "WindowChange",
                    "2025-06-01T00:00:00Z",
                    Some("App"),
                    Some("Title"),
                )
                .unwrap();
        }
        storage
            .delete_data_in_range(
                &TimeWindow::from_rfc3339_pair("2025-01-01T00:00:00Z", "2025-12-31T23:59:59Z")
                    .expect("trusted test bounds"),
                true,
                false,
                false,
                false,
                false,
            )
            .unwrap();

        // With threshold 0, any freelist pages will trigger VACUUM.
        let result = storage.maybe_vacuum(0);
        assert!(result.is_ok());
    }

    // ── wal_checkpoint_passive ──────────────────────────────────────

    #[test]
    fn wal_checkpoint_passive_on_fresh_db() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let result = storage.wal_checkpoint_passive();
        assert!(result.is_ok());
    }

    // ── wal_checkpoint_truncate ─────────────────────────────────────

    #[test]
    fn wal_checkpoint_truncate_on_fresh_db() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let result = storage.wal_checkpoint_truncate();
        assert!(result.is_ok());
    }

    // ── run_analyze ─────────────────────────────────────────────────

    #[test]
    fn run_analyze_on_fresh_db() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let result = storage.run_analyze();
        assert!(result.is_ok());
    }

    #[test]
    fn run_analyze_with_conn_on_fresh_db() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let conn = storage.conn.lock().unwrap();
        let result = SqliteStorage::run_analyze_with_conn(&conn);
        assert!(result.is_ok());
    }

    // ── get_storage_stats_summary ───────────────────────────────────

    #[test]
    fn stats_summary_empty_db() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let stats = storage.get_storage_stats_summary().unwrap();

        assert_eq!(stats.event_count, 0);
        assert_eq!(stats.frame_count, 0);
        assert_eq!(stats.metric_count, 0);
        assert!(stats.oldest_data_date.is_none());
        assert!(stats.newest_data_date.is_none());
        assert!(stats.page_size > 0, "page_size should be positive");
    }

    #[test]
    fn stats_summary_after_inserts() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        insert_events(&storage, &["2025-06-01T10:00:00Z", "2025-06-02T12:00:00Z"]);
        insert_frame(&storage, 1, "2025-06-01T11:00:00Z");
        insert_metric(&storage, "2025-06-03T08:00:00Z");

        let stats = storage.get_storage_stats_summary().unwrap();
        assert_eq!(stats.event_count, 2);
        assert_eq!(stats.frame_count, 1);
        assert_eq!(stats.metric_count, 1);
        assert!(stats.oldest_data_date.is_some());
        assert!(stats.newest_data_date.is_some());
    }

    // ── delete_data_in_range ────────────────────────────────────────

    #[test]
    fn delete_range_empty_db() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let counts = storage
            .delete_data_in_range(
                &TimeWindow::from_rfc3339_pair("2025-01-01T00:00:00Z", "2025-12-31T23:59:59Z")
                    .expect("trusted test bounds"),
                true,
                true,
                true,
                true,
                true,
            )
            .unwrap();

        assert_eq!(counts.events_deleted, 0);
        assert_eq!(counts.frames_deleted, 0);
        assert_eq!(counts.metrics_deleted, 0);
        assert_eq!(counts.process_snapshots_deleted, 0);
        assert_eq!(counts.idle_periods_deleted, 0);
    }

    #[test]
    fn delete_range_removes_matching_events() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        insert_events(
            &storage,
            &[
                "2025-06-01T10:00:00Z",
                "2025-06-15T10:00:00Z",
                "2025-07-01T10:00:00Z",
            ],
        );

        // Delete only June events
        let counts = storage
            .delete_data_in_range(
                &TimeWindow::from_rfc3339_pair("2025-06-01T00:00:00Z", "2025-06-30T23:59:59Z")
                    .expect("trusted test bounds"),
                true,
                false,
                false,
                false,
                false,
            )
            .unwrap();

        assert_eq!(counts.events_deleted, 2);

        // July event should remain — verify via count_events_in_range
        let remaining = storage
            .count_events_in_range(
                &TimeWindow::from_rfc3339_pair("2025-01-01T00:00:00Z", "2025-12-31T23:59:59Z")
                    .expect("trusted test bounds"),
            )
            .unwrap();
        assert_eq!(remaining, 1);
    }

    #[test]
    fn delete_range_selective_flags() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let ts = "2025-06-15T10:00:00Z";

        insert_events(&storage, &[ts]);
        insert_frame(&storage, 1, ts);
        insert_metric(&storage, ts);

        // Delete only events, not frames or metrics
        let counts = storage
            .delete_data_in_range(
                &TimeWindow::from_rfc3339_pair("2025-06-01T00:00:00Z", "2025-06-30T23:59:59Z")
                    .expect("trusted test bounds"),
                true,
                false,
                false,
                false,
                false,
            )
            .unwrap();

        assert_eq!(counts.events_deleted, 1);
        assert_eq!(counts.frames_deleted, 0);

        // Frames and metrics should still exist
        let frames = storage
            .list_frame_exports("2025-01-01T00:00:00Z", "2025-12-31T23:59:59Z")
            .unwrap();
        assert_eq!(frames.len(), 1);

        let metrics = storage
            .list_metric_exports("2025-01-01T00:00:00Z", "2025-12-31T23:59:59Z")
            .unwrap();
        assert_eq!(metrics.len(), 1);
    }

    // ── delete_all_data ─────────────────────────────────────────────

    #[test]
    fn delete_all_data_clears_everything() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        insert_events(&storage, &["2025-06-01T10:00:00Z", "2025-06-02T10:00:00Z"]);
        insert_frame(&storage, 1, "2025-06-01T11:00:00Z");
        insert_metric(&storage, "2025-06-01T12:00:00Z");

        storage.delete_all_data().unwrap();

        let stats = storage.get_storage_stats_summary().unwrap();
        assert_eq!(stats.event_count, 0);
        assert_eq!(stats.frame_count, 0);
        assert_eq!(stats.metric_count, 0);
    }

    #[test]
    fn delete_all_data_on_empty_db() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let result = storage.delete_all_data();
        assert!(result.is_ok(), "delete_all_data should succeed on empty DB");
    }

    // ── list_event_exports ──────────────────────────────────────────

    #[test]
    fn list_event_exports_empty() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let result = storage
            .list_event_exports("2025-01-01T00:00:00Z", "2025-12-31T23:59:59Z")
            .unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn list_event_exports_extracts_json_fields() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        insert_events(&storage, &["2025-06-15T10:00:00Z"]);

        let records = storage
            .list_event_exports("2025-01-01T00:00:00Z", "2025-12-31T23:59:59Z")
            .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].app_name.as_deref(), Some("Code"));
        assert_eq!(records[0].window_title.as_deref(), Some("test.rs"));
    }

    // ── count_events_in_range (exercised as event-query alternative) ─

    #[test]
    fn count_events_in_range_empty() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let count = storage
            .count_events_in_range(
                &TimeWindow::from_rfc3339_pair("2025-01-01T00:00:00Z", "2025-12-31T23:59:59Z")
                    .expect("trusted test bounds"),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn count_events_in_range_filters_by_range() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        insert_events(
            &storage,
            &[
                "2025-03-01T10:00:00Z",
                "2025-06-15T10:00:00Z",
                "2025-09-01T10:00:00Z",
            ],
        );

        let count = storage
            .count_events_in_range(
                &TimeWindow::from_rfc3339_pair("2025-06-01T00:00:00Z", "2025-06-30T23:59:59Z")
                    .expect("trusted test bounds"),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    // ── list_metric_exports ─────────────────────────────────────────

    #[test]
    fn list_metric_exports_empty() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let exports = storage
            .list_metric_exports("2025-01-01T00:00:00Z", "2025-12-31T23:59:59Z")
            .unwrap();
        assert!(exports.is_empty());
    }

    #[test]
    fn list_metric_exports_filters_by_range() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        insert_metric(&storage, "2025-03-01T10:00:00Z");
        insert_metric(&storage, "2025-06-15T10:00:00Z");

        let exports = storage
            .list_metric_exports("2025-06-01T00:00:00Z", "2025-06-30T23:59:59Z")
            .unwrap();
        assert_eq!(exports.len(), 1);
        assert!(exports[0].cpu_usage > 40.0);
    }

    // ── list_frame_exports ──────────────────────────────────────────

    #[test]
    fn list_frame_exports_empty() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let exports = storage
            .list_frame_exports("2025-01-01T00:00:00Z", "2025-12-31T23:59:59Z")
            .unwrap();
        assert!(exports.is_empty());
    }

    #[test]
    fn list_frame_exports_filters_by_range() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        insert_frame(&storage, 1, "2025-03-01T10:00:00Z");
        insert_frame(&storage, 2, "2025-06-15T10:00:00Z");
        insert_frame(&storage, 3, "2025-09-01T10:00:00Z");

        let exports = storage
            .list_frame_exports("2025-06-01T00:00:00Z", "2025-06-30T23:59:59Z")
            .unwrap();
        assert_eq!(exports.len(), 1);
        assert_eq!(exports[0].app_name, "Code");
        assert!((exports[0].importance - 0.5).abs() < f32::EPSILON);
    }

    // ── fts_merge ───────────────────────────────────────────────────

    #[test]
    fn fts_merge_runs_without_error() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        // FTS_AVAILABLE is set true after migrations in open_in_memory.
        let result = storage.fts_merge(64);
        assert!(result.is_ok());
    }

    #[test]
    fn fts_merge_skipped_when_unavailable() {
        // Temporarily set FTS_AVAILABLE to false
        let prev = FTS_AVAILABLE.load(Ordering::Relaxed);
        FTS_AVAILABLE.store(false, Ordering::Relaxed);

        let storage = SqliteStorage::open_in_memory(30).unwrap();
        // Restore the flag before calling — open_in_memory resets it to true,
        // so we set it again after opening.
        FTS_AVAILABLE.store(false, Ordering::Relaxed);

        let result = storage.fts_merge(64);
        assert!(result.is_ok(), "should no-op when FTS unavailable");

        FTS_AVAILABLE.store(prev, Ordering::Relaxed);
    }

    // ── fts_optimize ────────────────────────────────────────────────

    #[test]
    fn fts_optimize_runs_without_error() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let result = storage.fts_optimize();
        assert!(result.is_ok());
    }

    #[test]
    fn fts_optimize_skipped_when_unavailable() {
        let prev = FTS_AVAILABLE.load(Ordering::Relaxed);
        FTS_AVAILABLE.store(false, Ordering::Relaxed);

        let storage = SqliteStorage::open_in_memory(30).unwrap();
        FTS_AVAILABLE.store(false, Ordering::Relaxed);

        let result = storage.fts_optimize();
        assert!(result.is_ok(), "should no-op when FTS unavailable");

        FTS_AVAILABLE.store(prev, Ordering::Relaxed);
    }

    // ── Backup upsert helpers ───────────────────────────────────────

    #[test]
    fn upsert_backup_event_roundtrip() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        storage
            .upsert_backup_event(
                "evt-100",
                "Idle",
                "2025-08-01T09:00:00Z",
                Some("Finder"),
                Some("Desktop"),
            )
            .unwrap();

        // Verify the event was persisted via count_events_in_range
        let count = storage
            .count_events_in_range(
                &TimeWindow::from_rfc3339_pair("2025-08-01T00:00:00Z", "2025-08-01T23:59:59Z")
                    .expect("trusted test bounds"),
            )
            .unwrap();
        assert_eq!(count, 1);

        // Verify event_id and event_type via direct SQL
        let conn = storage.conn.lock().unwrap();
        let (eid, etype): (String, String) = conn
            .query_row(
                "SELECT event_id, event_type FROM events WHERE event_id = 'evt-100'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(eid, "evt-100");
        assert_eq!(etype, "Idle");
    }

    #[test]
    fn upsert_backup_frame_roundtrip() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        storage
            .upsert_backup_frame(
                42,
                "2025-08-01T09:00:00Z",
                "smart",
                "Safari",
                "Google",
                0.9,
                2560,
                1440,
                Some("Hello World"),
            )
            .unwrap();

        let exports = storage
            .list_frame_exports("2025-08-01T00:00:00Z", "2025-08-01T23:59:59Z")
            .unwrap();
        assert_eq!(exports.len(), 1);
        assert_eq!(exports[0].id, 42);
        assert_eq!(exports[0].trigger_type, "smart");
        assert_eq!(exports[0].ocr_text.as_deref(), Some("Hello World"));
    }

    // ── list_frame_file_paths_in_range ──────────────────────────────

    #[test]
    fn list_frame_file_paths_empty_db() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let paths = storage
            .list_frame_file_paths_in_range(
                &TimeWindow::from_rfc3339_pair("2025-01-01T00:00:00Z", "2025-12-31T23:59:59Z")
                    .expect("trusted test bounds"),
            )
            .unwrap();
        assert!(paths.is_empty());
    }

    // ── search_events ───────────────────────────────────────────────

    #[test]
    fn count_search_events_searches_data_column() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        insert_events(&storage, &["2025-06-01T10:00:00Z"]);

        let count = storage.count_search_events("%Code%").unwrap();
        assert_eq!(count, 1);

        let count = storage.count_search_events("%nonexistent%").unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn search_events_returns_matching_rows() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        insert_events(&storage, &["2025-06-01T10:00:00Z"]);

        let rows = storage.search_events("%Code%", 10, 0).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].app_name.as_deref(), Some("Code"));
        assert_eq!(rows[0].window_title.as_deref(), Some("test.rs"));
    }

    // ── Backup tag helpers ──────────────────────────────────────────

    #[test]
    fn backup_tag_roundtrip() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        storage
            .upsert_backup_tag(1, "work", "#3b82f6", "2025-06-01T00:00:00Z")
            .unwrap();
        storage
            .upsert_backup_tag(2, "personal", "#ef4444", "2025-06-01T00:00:00Z")
            .unwrap();

        let tags = storage.list_backup_tags().unwrap();
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0].name, "work");
        assert_eq!(tags[1].name, "personal");
    }

    #[test]
    fn backup_frame_tag_roundtrip() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        // Create prerequisite frame and tag
        insert_frame(&storage, 1, "2025-06-01T10:00:00Z");
        storage
            .upsert_backup_tag(10, "important", "#f59e0b", "2025-06-01T00:00:00Z")
            .unwrap();

        storage
            .upsert_backup_frame_tag(1, 10, "2025-06-01T10:00:00Z")
            .unwrap();

        let links = storage.list_backup_frame_tags().unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].frame_id, 1);
        assert_eq!(links[0].tag_id, 10);
    }

    // ── Closed-closed boundary regression tests (Phase 3 Task 5) ─────
    //
    // Per spec §5.1 and NG6: TimeWindow is closed-closed [start, end] —
    // both bounds are INCLUDED. These tests guard against accidental drift
    // to half-open semantics during future refactors.
    //
    // Note: Test fixture timestamps use the canonical `+00:00` form
    // because `TimeWindow::to_sql_pair()` uses `chrono::DateTime::to_rfc3339()`
    // which emits `+00:00` (not `Z`) for UTC. Lexicographic SQL comparison
    // requires matching formats at the exact boundary. Production code goes
    // through the same `to_rfc3339()` path on insert, so this fixture choice
    // mirrors real-world data.

    #[test]
    fn count_frames_in_range_includes_both_boundaries() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let t1 = "2026-04-01T00:00:00+00:00";
        let t2 = "2026-04-25T00:00:00+00:00";
        insert_frame(&storage, 1, t1); // exactly at start
        insert_frame(&storage, 2, "2026-04-15T00:00:00+00:00"); // middle
        insert_frame(&storage, 3, t2); // exactly at end
        let window = TimeWindow::from_rfc3339_pair(t1, t2).expect("trusted test bounds");
        assert_eq!(storage.count_frames_in_range(&window).unwrap(), 3);
    }

    #[test]
    fn count_events_in_range_includes_both_boundaries() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let t1 = "2026-04-01T00:00:00+00:00";
        let t2 = "2026-04-25T00:00:00+00:00";
        insert_events(&storage, &[t1, "2026-04-15T00:00:00+00:00", t2]);
        let window = TimeWindow::from_rfc3339_pair(t1, t2).expect("trusted test bounds");
        assert_eq!(storage.count_events_in_range(&window).unwrap(), 3);
    }

    #[test]
    fn delete_data_in_range_respects_delete_flags() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let t1 = "2026-04-01T00:00:00+00:00";
        let t2 = "2026-04-25T00:00:00+00:00";
        let middle = "2026-04-15T00:00:00+00:00";
        // Seed one of each: event + frame + metric (process and idle have
        // no convenient sync helper here; their delete flags are still
        // exercised via unrelated tests above).
        insert_events(&storage, &[middle]);
        insert_frame(&storage, 1, middle);
        insert_metric(&storage, middle);

        let window = TimeWindow::from_rfc3339_pair(t1, t2).expect("trusted test bounds");
        // delete_events=true, all others false
        let counts = storage
            .delete_data_in_range(&window, true, false, false, false, false)
            .unwrap();

        assert_eq!(counts.events_deleted, 1);
        assert_eq!(counts.frames_deleted, 0);
        assert_eq!(counts.metrics_deleted, 0);
        assert_eq!(counts.process_snapshots_deleted, 0);
        assert_eq!(counts.idle_periods_deleted, 0);

        // Frames + metrics should remain
        let remaining_frames = storage.count_frames_in_range(&window).unwrap();
        assert_eq!(remaining_frames, 1);
    }
}
