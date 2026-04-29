use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use oneshim_core::error::CoreError;
use oneshim_core::models::event::Event;
use oneshim_core::models::suggestion::Suggestion;
use oneshim_core::ports::storage::StorageService;
use oneshim_core::types::TimeWindow;
use tracing::{debug, info, warn};

use super::edge_intelligence::enum_to_sql_str;
use super::SqliteStorage;
use crate::error::StorageError;

impl SqliteStorage {
    pub fn count_events_in_range(&self, window: &TimeWindow) -> Result<u64, StorageError> {
        let (from, to) = window.to_sql_pair();
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE timestamp >= ?1 AND timestamp <= ?2",
                rusqlite::params![from, to],
                |row| row.get(0),
            )
            .map_err(|e| StorageError::Internal(format!("Failed to count events: {e}")))?;

        Ok(count as u64)
    }

    pub(super) fn extract_event_id(event: &Event) -> String {
        match event {
            Event::User(user_event) => user_event.event_id.to_string(),
            Event::System(system_event) => system_event.event_id.to_string(),
            Event::Context(context_event) => {
                format!(
                    "ctx_{}_{}_{}",
                    context_event.timestamp.timestamp_millis(),
                    context_event.app_name,
                    context_event
                        .window_title
                        .chars()
                        .take(20)
                        .collect::<String>()
                )
            }
            Event::Input(input_event) => {
                format!(
                    "input_{}_{}",
                    input_event.timestamp.timestamp_millis(),
                    input_event.app_name
                )
            }
            Event::Process(process_event) => {
                format!("proc_{}", process_event.timestamp.timestamp_millis())
            }
            Event::Window(window_event) => {
                format!(
                    "win_{}_{:?}",
                    window_event.timestamp.timestamp_millis(),
                    window_event.event_type
                )
            }
            Event::Clipboard(cb) => {
                format!("clip_{}", cb.timestamp.timestamp_millis())
            }
            Event::FileAccess(fa) => {
                format!(
                    "fa_{}_{}",
                    fa.timestamp.timestamp_millis(),
                    fa.relative_path.display()
                )
            }
        }
    }

    pub(super) fn extract_event_type(event: &Event) -> String {
        match event {
            Event::User(user_event) => format!("{:?}", user_event.event_type),
            Event::System(system_event) => format!("{:?}", system_event.event_type),
            Event::Context(_) => "context_change".to_string(),
            Event::Input(_) => "input_activity".to_string(),
            Event::Process(_) => "process_snapshot".to_string(),
            Event::Window(w) => format!("window_{:?}", w.event_type),
            Event::Clipboard(_) => "clipboard_change".to_string(),
            Event::FileAccess(fa) => format!("file_{:?}", fa.event_type),
        }
    }

    pub(super) fn extract_timestamp(event: &Event) -> DateTime<Utc> {
        match event {
            Event::User(user_event) => user_event.timestamp,
            Event::System(system_event) => system_event.timestamp,
            Event::Context(context_event) => context_event.timestamp,
            Event::Input(input_event) => input_event.timestamp,
            Event::Process(process_event) => process_event.timestamp,
            Event::Window(window_event) => window_event.timestamp,
            Event::Clipboard(cb) => cb.timestamp,
            Event::FileAccess(fa) => fa.timestamp,
        }
    }

    /// 이벤트 슬라이스를 SQLite에 일괄 저장한다. 트랜잭션 단위로 처리하여
    /// 성능을 최적화한다.
    ///
    /// # Arguments
    ///
    /// * `events` - 저장할 이벤트 슬라이스. 비어 있으면 즉시 `Ok(0)`을 반환한다.
    ///
    /// # Returns
    ///
    /// Returns `Ok(events.len())` — the count of events in the input slice.
    /// Duplicate `event_id` values are silently ignored by `INSERT OR IGNORE`,
    /// so the returned count may exceed the number of rows actually written.
    /// 실제 삽입된 행 수가 아닌 입력 슬라이스의 길이를 반환한다는 점에 주의한다.
    pub fn save_events_batch(&self, events: &[Event]) -> Result<usize, StorageError> {
        if events.is_empty() {
            return Ok(0);
        }

        let mut conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        let tx = conn
            .transaction()
            .map_err(|e| StorageError::Internal(format!("Failed to start transaction: {e}")))?;

        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT OR IGNORE INTO events (event_id, event_type, timestamp, data) VALUES (?1, ?2, ?3, ?4)",
                )
                .map_err(|e| StorageError::Internal(format!("Failed to prepare query: {e}")))?;

            for event in events {
                let event_id = Self::extract_event_id(event);
                let event_type = Self::extract_event_type(event);
                let timestamp = Self::extract_timestamp(event).to_rfc3339();
                let data = serde_json::to_string(event).map_err(|e| {
                    StorageError::Internal(format!("event serialization failed: {e}"))
                })?;

                stmt.execute(rusqlite::params![event_id, event_type, timestamp, data])
                    .map_err(|e| StorageError::Internal(format!("batch save failure: {e}")))?;
            }
        }

        tx.commit()
            .map_err(|e| StorageError::Internal(format!("Failed to commit transaction: {e}")))?;

        // Refresh query planner statistics after large batch inserts (>100 events).
        // Uses the already-held MutexGuard — do NOT re-lock self.conn.
        if events.len() > 100 {
            if let Err(e) = Self::run_analyze_with_conn(&conn) {
                warn!("ANALYZE after batch insert failed: {e}");
            }
        }

        debug!("event batch save: {}items", events.len());
        Ok(events.len())
    }
}

#[async_trait]
impl StorageService for SqliteStorage {
    async fn save_event(&self, event: &Event) -> Result<(), CoreError> {
        let event_id = Self::extract_event_id(event);
        let event_type = Self::extract_event_type(event);
        let timestamp = Self::extract_timestamp(event).to_rfc3339();
        let data = serde_json::to_string(event)?;

        self.with_conn(move |conn| {
            conn.execute(
                "INSERT OR IGNORE INTO events (event_id, event_type, timestamp, data) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![event_id, event_type, timestamp, data],
            )
            .map_err(|e| StorageError::Internal(format!("event save failure: {e}")))?;
            debug!("event save: {event_id}");
            Ok(())
        })
        .await
        .map_err(Into::into)
    }

    async fn get_events(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<Event>, CoreError> {
        let from_str = from.to_rfc3339();
        let to_str = to.to_rfc3339();

        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare_cached(
                    "SELECT data FROM events WHERE timestamp >= ?1 AND timestamp <= ?2 ORDER BY timestamp DESC LIMIT ?3",
                )
                .map_err(|e| StorageError::Internal(format!("Failed to prepare query: {e}")))?;

            let events = stmt
                .query_map(rusqlite::params![from_str, to_str, limit as i64], |row| {
                    let data: String = row.get(0)?;
                    Ok(data)
                })
                .map_err(|e| StorageError::Internal(format!("Failed to execute query: {e}")))?
                .filter_map(|r| r.ok())
                .filter_map(|data| {
                    serde_json::from_str::<Event>(&data)
                        .map_err(|e| {
                            warn!("event deserialization failed, skipping row: {e}");
                        })
                        .ok()
                })
                .collect();

            Ok(events)
        })
        .await
        .map_err(Into::into)
    }

    async fn get_pending_events(&self, limit: usize) -> Result<Vec<Event>, CoreError> {
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare_cached(
                    "SELECT data FROM events WHERE is_sent = 0 ORDER BY timestamp ASC LIMIT ?1",
                )
                .map_err(|e| StorageError::Internal(format!("Failed to prepare query: {e}")))?;

            let events = stmt
                .query_map(rusqlite::params![limit as i64], |row| {
                    let data: String = row.get(0)?;
                    Ok(data)
                })
                .map_err(|e| StorageError::Internal(format!("Failed to execute query: {e}")))?
                .filter_map(|r| r.ok())
                .filter_map(|data| {
                    serde_json::from_str::<Event>(&data)
                        .map_err(|e| {
                            warn!("event deserialization failed, skipping row: {e}");
                        })
                        .ok()
                })
                .collect();

            Ok(events)
        })
        .await
        .map_err(Into::into)
    }

    async fn mark_as_sent(&self, event_ids: &[String]) -> Result<(), CoreError> {
        if event_ids.is_empty() {
            return Ok(());
        }

        // Clone before moving into the 'static closure
        let ids: Vec<String> = event_ids.to_vec();

        self.with_conn(move |conn| {
            let placeholders: Vec<String> = ids
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", i + 1))
                .collect();
            let sql = format!(
                "UPDATE events SET is_sent = 1 WHERE event_id IN ({})",
                placeholders.join(", ")
            );

            let params: Vec<Box<dyn rusqlite::types::ToSql>> = ids
                .iter()
                .map(|id| Box::new(id.clone()) as Box<dyn rusqlite::types::ToSql>)
                .collect();
            let param_refs: Vec<&dyn rusqlite::types::ToSql> =
                params.iter().map(|p| p.as_ref()).collect();

            conn.execute(&sql, param_refs.as_slice())
                .map_err(|e| StorageError::Internal(format!("Failed to mark as sent: {e}")))?;

            debug!("{}items event sent completed", ids.len());
            Ok(())
        })
        .await
        .map_err(Into::into)
    }

    async fn mark_unsent_as_sent_before(&self, before: DateTime<Utc>) -> Result<usize, CoreError> {
        let cutoff = before.to_rfc3339();

        self.with_conn(move |conn| {
            let updated: usize = conn
                .execute(
                    "UPDATE events SET is_sent = 1 WHERE is_sent = 0 AND timestamp < ?1",
                    rusqlite::params![cutoff],
                )
                .map_err(|e| {
                    StorageError::Internal(format!("Failed to mark unsent as sent: {e}"))
                })?;

            if updated > 0 {
                debug!("{updated} unsent events marked as sent");
            }
            Ok(updated)
        })
        .await
        .map_err(Into::into)
    }

    async fn enforce_retention(&self) -> Result<usize, CoreError> {
        let cutoff = (Utc::now() - Duration::days(self.retention_days as i64)).to_rfc3339();
        let retention_days = self.retention_days;

        self.with_conn(move |conn| {
            let deleted = conn
                .execute(
                    "DELETE FROM events WHERE timestamp < ?1 AND is_sent = 1",
                    rusqlite::params![cutoff],
                )
                .map_err(|e| {
                    StorageError::Internal(format!("Failed to apply retention policy: {e}"))
                })?;

            if deleted > 0 {
                info!(
                    "retention policy: deleted {deleted} events (>{} days)",
                    retention_days
                );
            }
            Ok(deleted)
        })
        .await
        .map_err(Into::into)
    }

    async fn update_segment_llm_summary(
        &self,
        segment_id: &str,
        llm_summary: &str,
    ) -> Result<(), CoreError> {
        let id = segment_id.to_string();
        let summary = llm_summary.to_string();
        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE activity_segments SET llm_summary = ?1 WHERE id = ?2",
                rusqlite::params![summary, id],
            )
            .map_err(|e| {
                StorageError::Internal(format!("Failed to update segment summary: {e}"))
            })?;
            Ok(())
        })
        .await
        .map_err(Into::into)
    }

    async fn save_suggestion(&self, suggestion: &Suggestion) -> Result<(), CoreError> {
        let suggestion = suggestion.clone();

        self.with_conn(move |conn| {
            conn.execute(
                "INSERT OR REPLACE INTO suggestions \
                 (suggestion_id, suggestion_type, source, content, priority, \
                  confidence_score, relevance_score, is_actionable, reasoning, \
                  created_at, expires_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                rusqlite::params![
                    suggestion.suggestion_id,
                    enum_to_sql_str(&suggestion.suggestion_type),
                    enum_to_sql_str(&suggestion.source),
                    suggestion.content,
                    enum_to_sql_str(&suggestion.priority),
                    suggestion.confidence_score,
                    suggestion.relevance_score,
                    suggestion.is_actionable as i32,
                    suggestion.reasoning,
                    suggestion.created_at.to_rfc3339(),
                    suggestion.expires_at.map(|t| t.to_rfc3339()),
                ],
            )
            .map_err(|e| StorageError::Internal(format!("Failed to save suggestion: {e}")))?;
            debug!(id = %suggestion.suggestion_id, "suggestion persisted to SQLite");
            Ok(())
        })
        .await
        .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};

    use super::super::test_utils::make_user_event;

    #[test]
    fn count_events_in_range_empty() {
        let storage = SqliteStorage::open_in_memory(30).expect("open_in_memory failed");
        let from = (Utc::now() - Duration::hours(1)).to_rfc3339();
        let to = (Utc::now() + Duration::hours(1)).to_rfc3339();
        let window = TimeWindow::from_rfc3339_pair(&from, &to).expect("trusted test bounds");
        let count = storage
            .count_events_in_range(&window)
            .expect("count_events_in_range failed");
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn count_events_in_range_after_save() {
        let storage = SqliteStorage::open_in_memory(30).expect("open_in_memory failed");
        storage
            .save_event(&make_user_event())
            .await
            .expect("save_event failed");
        storage
            .save_event(&make_user_event())
            .await
            .expect("save_event failed");

        let from = (Utc::now() - Duration::hours(1)).to_rfc3339();
        let to = (Utc::now() + Duration::hours(1)).to_rfc3339();
        let window = TimeWindow::from_rfc3339_pair(&from, &to).expect("trusted test bounds");
        let count = storage
            .count_events_in_range(&window)
            .expect("count_events_in_range failed");
        assert_eq!(count, 2);
    }

    #[test]
    fn save_events_batch_empty_is_noop() {
        let storage = SqliteStorage::open_in_memory(30).expect("open_in_memory failed");
        let saved = storage
            .save_events_batch(&[])
            .expect("save_events_batch failed");
        assert_eq!(saved, 0);
    }

    #[test]
    fn save_events_batch_inserts_all() {
        let storage = SqliteStorage::open_in_memory(30).expect("open_in_memory failed");
        let events = vec![make_user_event(), make_user_event(), make_user_event()];
        let saved = storage
            .save_events_batch(&events)
            .expect("save_events_batch failed");
        assert_eq!(saved, 3);

        let from = (Utc::now() - Duration::hours(1)).to_rfc3339();
        let to = (Utc::now() + Duration::hours(1)).to_rfc3339();
        let window = TimeWindow::from_rfc3339_pair(&from, &to).expect("trusted test bounds");
        let count = storage
            .count_events_in_range(&window)
            .expect("count_events_in_range failed");
        assert_eq!(count, 3);
    }

    #[test]
    fn save_events_batch_duplicate_ids_ignored() {
        let storage = SqliteStorage::open_in_memory(30).expect("open_in_memory failed");
        // Same event saved twice — INSERT OR IGNORE should deduplicate
        let event = make_user_event();
        let events = vec![event.clone(), event];
        let saved = storage
            .save_events_batch(&events)
            .expect("save_events_batch failed");
        assert_eq!(saved, 2); // returns count of input, not actually inserted

        let from = (Utc::now() - Duration::hours(1)).to_rfc3339();
        let to = (Utc::now() + Duration::hours(1)).to_rfc3339();
        let window = TimeWindow::from_rfc3339_pair(&from, &to).expect("trusted test bounds");
        let count = storage
            .count_events_in_range(&window)
            .expect("count_events_in_range failed");
        assert_eq!(count, 1); // only 1 unique event
    }
}
