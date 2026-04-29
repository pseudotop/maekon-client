use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use oneshim_core::error::CoreError;
use oneshim_core::models::tiered_memory::{CalibrationEntry, TriggerAction};
use oneshim_core::models::work_session::AppCategory;
use oneshim_core::ports::calibration_store::{CalibrationReader, CalibrationWriter};
use oneshim_core::types::TimeWindow;
use rusqlite::params;
use tracing::{debug, warn};

use super::edge_intelligence::enum_to_sql_str;
use super::SqliteStorage;
use crate::error::StorageError;

// ---------------------------------------------------------------------------
// CalibrationWriter (synchronous)
// ---------------------------------------------------------------------------

impl CalibrationWriter for SqliteStorage {
    fn log_batch(&self, entries: &[CalibrationEntry]) -> Result<(), CoreError> {
        if entries.is_empty() {
            return Ok(());
        }

        let conn = self.conn.lock().map_err(|e| CoreError::Storage {
            code: oneshim_core::error_codes::StorageCode::Failed,
            message: format!("SQLite lock poisoned: {e}"),
        })?;

        let tx = conn
            .unchecked_transaction()
            .map_err(|e| CoreError::Storage {
                code: oneshim_core::error_codes::StorageCode::Failed,
                message: format!("Failed to begin transaction: {e}"),
            })?;

        // Ensure params snapshot exists (idempotent upsert).
        // Each entry carries `params_json` — we deduplicate by `params_version_id`
        // and only insert when a snapshot for that version doesn't already exist.
        {
            let mut insert_snapshot = tx
                .prepare_cached(
                    "INSERT OR IGNORE INTO trigger_params_snapshots (id, preset, params_json)
                     VALUES (?1, ?2, ?3)",
                )
                .map_err(|e| CoreError::Storage {
                    code: oneshim_core::error_codes::StorageCode::Failed,
                    message: format!("prepare snapshot stmt: {e}"),
                })?;

            let mut seen_versions = std::collections::HashSet::new();
            for entry in entries {
                if seen_versions.insert(&entry.params_version_id) {
                    let json = if entry.params_json.is_empty() {
                        "{}"
                    } else {
                        &entry.params_json
                    };
                    insert_snapshot
                        .execute(params![entry.params_version_id, "default", json])
                        .map_err(|e| CoreError::Storage {
                            code: oneshim_core::error_codes::StorageCode::Failed,
                            message: format!("insert params snapshot: {e}"),
                        })?;
                }
            }
        }

        // Insert calibration entries
        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT INTO calibration_log
                     (timestamp, event_type, app_name, app_category,
                      event_importance, density_signal, importance_signal,
                      context_signal, buffer_signal, trigger_score,
                      trigger_action, active_regime_id, params_version_id, is_noise)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                )
                .map_err(|e| CoreError::Storage {
                    code: oneshim_core::error_codes::StorageCode::Failed,
                    message: format!("prepare calibration stmt: {e}"),
                })?;

            for entry in entries {
                let action_str = entry.trigger_action.map(|a| enum_to_sql_str(&a));
                let category_str = enum_to_sql_str(&entry.app_category);

                stmt.execute(params![
                    entry.timestamp.to_rfc3339(),
                    entry.event_type,
                    entry.app_name,
                    category_str,
                    entry.event_importance,
                    entry.density_signal,
                    entry.importance_signal,
                    entry.context_signal,
                    entry.buffer_signal,
                    entry.trigger_score,
                    action_str,
                    entry.active_regime_id,
                    entry.params_version_id,
                    entry.is_noise as i32,
                ])
                .map_err(|e| CoreError::Storage {
                    code: oneshim_core::error_codes::StorageCode::Failed,
                    message: format!("insert calibration entry: {e}"),
                })?;
            }
        }

        tx.commit().map_err(|e| CoreError::Storage {
            code: oneshim_core::error_codes::StorageCode::Failed,
            message: format!("commit calibration batch: {e}"),
        })?;

        debug!("logged {} calibration entries", entries.len());
        Ok(())
    }

    fn flag_noise_range(&self, window: &TimeWindow) -> Result<u64, CoreError> {
        let from = window.start;
        let to = window.end;
        let conn = self.conn.lock().map_err(|e| CoreError::Storage {
            code: oneshim_core::error_codes::StorageCode::Failed,
            message: format!("SQLite lock poisoned: {e}"),
        })?;

        let updated = conn
            .execute(
                "UPDATE calibration_log SET is_noise = 1
                 WHERE timestamp >= ?1 AND timestamp <= ?2",
                params![from.to_rfc3339(), to.to_rfc3339()],
            )
            .map_err(|e| CoreError::Storage {
                code: oneshim_core::error_codes::StorageCode::Failed,
                message: format!("flag noise range: {e}"),
            })?;

        debug!("flagged {} calibration entries as noise", updated);
        Ok(updated as u64)
    }
}

// ---------------------------------------------------------------------------
// CalibrationReader (asynchronous)
// ---------------------------------------------------------------------------

#[async_trait]
impl CalibrationReader for SqliteStorage {
    async fn get_entries(
        &self,
        window: &TimeWindow,
        exclude_noise: bool,
    ) -> Result<Vec<CalibrationEntry>, CoreError> {
        let from_str = window.start.to_rfc3339();
        let to_str = window.end.to_rfc3339();

        self.with_conn(move |conn| {
            let sql = if exclude_noise {
                "SELECT timestamp, event_type, app_name, app_category,
                        event_importance, density_signal, importance_signal,
                        context_signal, buffer_signal, trigger_score,
                        trigger_action, active_regime_id, params_version_id, is_noise
                 FROM calibration_log
                 WHERE timestamp >= ?1 AND timestamp <= ?2 AND is_noise = 0
                 ORDER BY timestamp ASC"
            } else {
                "SELECT timestamp, event_type, app_name, app_category,
                        event_importance, density_signal, importance_signal,
                        context_signal, buffer_signal, trigger_score,
                        trigger_action, active_regime_id, params_version_id, is_noise
                 FROM calibration_log
                 WHERE timestamp >= ?1 AND timestamp <= ?2
                 ORDER BY timestamp ASC"
            };

            let mut stmt = conn
                .prepare(sql)
                .map_err(|e| StorageError::Internal(format!("prepare get_entries: {e}")))?;

            let rows = stmt
                .query_map(params![from_str, to_str], map_calibration_row)
                .map_err(|e| StorageError::Internal(format!("query calibration entries: {e}")))?;

            let mut entries = Vec::new();
            for row_result in rows {
                let entry = row_result
                    .map_err(|e| StorageError::Internal(format!("read calibration row: {e}")))?;
                entries.push(entry);
            }
            Ok(entries)
        })
        .await
        .map_err(Into::into)
    }

    async fn enforce_retention(&self, max_days: u32, max_rows: u64) -> Result<u64, CoreError> {
        self.with_conn(move |conn| {
            let mut total_deleted: u64 = 0;

            // 1. Delete entries older than max_days
            let cutoff = Utc::now() - Duration::days(max_days as i64);
            let cutoff_str = cutoff.to_rfc3339();

            let deleted_by_age = conn
                .execute(
                    "DELETE FROM calibration_log WHERE timestamp < ?1",
                    params![cutoff_str],
                )
                .map_err(|e| StorageError::Internal(format!("retention age delete: {e}")))?;
            total_deleted += deleted_by_age as u64;

            // 2. If remaining rows exceed max_rows, delete oldest
            let remaining: i64 = conn
                .query_row("SELECT COUNT(*) FROM calibration_log", [], |row| row.get(0))
                .map_err(|e| StorageError::Internal(format!("count calibration rows: {e}")))?;

            if remaining as u64 > max_rows {
                let excess = remaining as u64 - max_rows;
                let deleted_by_count = conn
                    .execute(
                        "DELETE FROM calibration_log WHERE id IN (
                            SELECT id FROM calibration_log ORDER BY timestamp ASC LIMIT ?1
                         )",
                        params![excess as i64],
                    )
                    .map_err(|e| StorageError::Internal(format!("retention count delete: {e}")))?;
                total_deleted += deleted_by_count as u64;
            }

            debug!("calibration retention: deleted {} entries", total_deleted);
            Ok(total_deleted)
        })
        .await
        .map_err(Into::into)
    }

    async fn list_segment_time_ranges(
        &self,
        window: &TimeWindow,
    ) -> Result<Vec<(String, TimeWindow)>, CoreError> {
        let from_str = window.start.to_rfc3339();
        let to_str = window.end.to_rfc3339();

        self.with_conn(move |conn| {
            // Check table existence (may not have run V9 migration yet)
            let table_exists: bool = conn
                .query_row(
                    "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='activity_segments'",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(false);

            if !table_exists {
                return Ok(vec![]);
            }

            let mut stmt = conn
                .prepare(
                    "SELECT id, start_time, end_time FROM activity_segments \
                     WHERE start_time >= ?1 AND end_time <= ?2 \
                     ORDER BY start_time ASC",
                )
                .map_err(|e| StorageError::Internal(format!("prepare segment ranges: {e}")))?;

            let rows = stmt
                .query_map(params![from_str, to_str], |row| {
                    let id: String = row.get(0)?;
                    let start_str: String = row.get(1)?;
                    let end_str: String = row.get(2)?;
                    Ok((id, start_str, end_str))
                })
                .map_err(|e| StorageError::Internal(format!("query segment ranges: {e}")))?;

            let mut result = Vec::new();
            for row_result in rows {
                let (id, start_str, end_str) =
                    row_result.map_err(|e| StorageError::Internal(format!("read segment row: {e}")))?;
                let start = DateTime::parse_from_rfc3339(&start_str)
                    .map(|dt| dt.with_timezone(&Utc))
                    .map_err(|e| StorageError::Internal(format!("invalid segment start: {e}")))?;
                let end = DateTime::parse_from_rfc3339(&end_str)
                    .map(|dt| dt.with_timezone(&Utc))
                    .map_err(|e| StorageError::Internal(format!("invalid segment end: {e}")))?;
                // Phase 2 iter-2 N-C4: consolidate two DateTime<Utc> into TimeWindow.
                // DB-stored segment ranges are trusted (start <= end invariant).
                let segment_window = TimeWindow::new(start, end)
                    .expect("DB-stored segment ranges satisfy start <= end invariant");
                result.push((id, segment_window));
            }
            Ok(result)
        })
        .await
        .map_err(Into::into)
    }
}

// ---------------------------------------------------------------------------
// Row mapper
// ---------------------------------------------------------------------------

fn map_calibration_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CalibrationEntry> {
    let ts_str: String = row.get(0)?;
    let timestamp = DateTime::parse_from_rfc3339(&ts_str)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| {
            warn!("invalid RFC3339 timestamp in calibration_log: {ts_str}: {e}");
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?;

    let action_str: Option<String> = row.get(10)?;
    let trigger_action = action_str.and_then(|s| match s.as_str() {
        "Start" => Some(TriggerAction::Start),
        "Close" => Some(TriggerAction::Close),
        "ForceClose" => Some(TriggerAction::ForceClose),
        _ => None,
    });

    let category_str: String = row.get(3)?;
    let app_category = parse_app_category_from_str(&category_str);

    let is_noise_int: i32 = row.get(13)?;

    Ok(CalibrationEntry {
        timestamp,
        event_type: row.get(1)?,
        app_name: row.get(2)?,
        app_category,
        event_importance: row.get(4)?,
        density_signal: row.get(5)?,
        importance_signal: row.get(6)?,
        context_signal: row.get(7)?,
        buffer_signal: row.get(8)?,
        trigger_score: row.get(9)?,
        trigger_action,
        active_regime_id: row.get::<_, Option<String>>(11)?,
        params_version_id: row.get(12)?,
        params_json: String::new(), // not stored in calibration_log; lives in trigger_params_snapshots
        is_noise: is_noise_int != 0,
    })
}

fn parse_app_category_from_str(s: &str) -> AppCategory {
    // Try serde deserialization first (handles snake_case from enum_to_sql_str)
    if let Ok(cat) = serde_json::from_str::<AppCategory>(&format!("\"{s}\"")) {
        return cat;
    }
    // Fallback for Debug format strings
    match s {
        "Communication" | "communication" => AppCategory::Communication,
        "Development" | "development" => AppCategory::Development,
        "Documentation" | "documentation" => AppCategory::Documentation,
        "Browser" | "browser" => AppCategory::Browser,
        "Design" | "design" => AppCategory::Design,
        "Media" | "media" => AppCategory::Media,
        "System" | "system" => AppCategory::System,
        _ => AppCategory::Other,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use oneshim_core::models::tiered_memory::TriggerAction;

    fn make_entry(idx: u32) -> CalibrationEntry {
        CalibrationEntry {
            timestamp: Utc::now() + Duration::seconds(idx as i64),
            event_type: "APP_SWITCH_NEW".to_string(),
            app_name: format!("App{idx}"),
            app_category: AppCategory::Development,
            event_importance: 0.5 + (idx as f32) * 0.05,
            density_signal: 0.3,
            importance_signal: 0.4,
            context_signal: 0.2,
            buffer_signal: 0.1,
            trigger_score: 0.6,
            trigger_action: if idx % 2 == 0 {
                Some(TriggerAction::Start)
            } else {
                None
            },
            active_regime_id: None,
            params_version_id: "v1-test".to_string(),
            params_json: r#"{"w_density":0.3}"#.to_string(),
            is_noise: false,
        }
    }

    #[tokio::test]
    async fn batch_insert_and_read() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let entries: Vec<_> = (0..5).map(make_entry).collect();

        storage.log_batch(&entries).unwrap();

        let from = Utc::now() - Duration::hours(1);
        let to = Utc::now() + Duration::hours(1);
        let window = TimeWindow::new(from, to).expect("trusted test bounds");
        let loaded = storage.get_entries(&window, false).await.unwrap();
        assert_eq!(loaded.len(), 5);
        assert_eq!(loaded[0].app_name, "App0");
    }

    #[tokio::test]
    async fn flag_noise_range_and_exclude() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let entries: Vec<_> = (0..5).map(make_entry).collect();
        storage.log_batch(&entries).unwrap();

        // Flag entries 0-2 as noise
        let from = entries[0].timestamp - Duration::seconds(1);
        let to = entries[2].timestamp + Duration::seconds(1);
        let noise_window = TimeWindow::new(from, to).expect("trusted test bounds");
        let flagged = storage.flag_noise_range(&noise_window).unwrap();
        assert!(flagged >= 3);

        // Exclude noise should return fewer entries
        let wide_from = Utc::now() - Duration::hours(1);
        let wide_to = Utc::now() + Duration::hours(1);
        let wide_window = TimeWindow::new(wide_from, wide_to).expect("trusted test bounds");
        let clean = storage.get_entries(&wide_window, true).await.unwrap();
        assert!(clean.len() < 5);

        // Include noise returns all
        let all = storage.get_entries(&wide_window, false).await.unwrap();
        assert_eq!(all.len(), 5);
    }

    #[tokio::test]
    async fn enforce_retention_by_max_rows() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let entries: Vec<_> = (0..10).map(make_entry).collect();
        storage.log_batch(&entries).unwrap();

        // Keep max 3 rows (30 day retention won't kick in)
        let deleted = storage.enforce_retention(30, 3).await.unwrap();
        assert_eq!(deleted, 7);

        let from = Utc::now() - Duration::hours(1);
        let to = Utc::now() + Duration::hours(1);
        let window = TimeWindow::new(from, to).expect("trusted test bounds");
        let remaining = storage.get_entries(&window, false).await.unwrap();
        assert_eq!(remaining.len(), 3);
    }
}
