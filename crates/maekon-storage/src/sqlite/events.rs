use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use maekon_core::error::CoreError;
use maekon_core::models::event::Event;
use maekon_core::models::suggestion::Suggestion;
use maekon_core::models::tiered_memory::SegmentSummary;
use maekon_core::ports::storage::StorageService;
use maekon_core::types::TimeWindow;
use tracing::{debug, info, warn};

use super::edge_intelligence::enum_to_sql_str;
use super::edge_intelligence::suggestion_stamp;
use super::SqliteStorage;
use crate::error::StorageError;

const MAX_UNSENT_EVENTS_RETAINED: i64 = 10_000;

impl SqliteStorage {
    pub fn count_events_in_range(&self, window: &TimeWindow) -> Result<u64, StorageError> {
        let (from, to) = window.to_sql_pair();
        // Read — read_lock (deletion_flag irrelevant).
        let read = self.conn.read_lock();
        Self::count_events_in_range_inner(read.conn(), &from, &to)
    }

    /// Async `count_events_in_range` over the read funnel (ADR-026 PR-4).
    pub(crate) async fn count_events_in_range_async(
        &self,
        window: &TimeWindow,
    ) -> Result<u64, StorageError> {
        // owned move into the Send + 'static closure.
        let (from, to) = window.to_sql_pair();
        self.with_conn_read(move |conn| Self::count_events_in_range_inner(conn, &from, &to))
            .await
    }

    fn count_events_in_range_inner(
        conn: &rusqlite::Connection,
        from: &str,
        to: &str,
    ) -> Result<u64, StorageError> {
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
        storage_event_id(event)
    }
}

/// Storage primary key for an [`Event`] — the SAME derivation `save_event` /
/// `save_events_batch` use for the `events.event_id` column.
///
/// Public so upload producers can pair the persisted id with the (possibly
/// egress-filtered) upload payload (#7946): the id must be derived from the
/// ORIGINAL event, because egress filtering can change id-relevant fields
/// (e.g. a Context event's window title feeds the id).
pub fn storage_event_id(event: &Event) -> String {
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

impl SqliteStorage {
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

    /// Batch-save a slice of events to SQLite. Processed as a single transaction
    /// to optimize performance.
    ///
    /// # Arguments
    ///
    /// * `events` - The slice of events to save. Returns `Ok(0)` immediately if empty.
    ///
    /// # Returns
    ///
    /// Returns `Ok(events.len())` — the count of events in the input slice.
    /// Duplicate `event_id` values are silently ignored by `INSERT OR IGNORE`,
    /// so the returned count may exceed the number of rows actually written.
    /// Note that this returns the length of the input slice, not the actual
    /// number of rows inserted.
    pub fn save_events_batch(&self, events: &[Event]) -> Result<usize, StorageError> {
        if events.is_empty() {
            return Ok(0);
        }

        // Write (batch transaction) — write_lock (skip when deletion_flag is set → 0 rows; events ∈ ALL_TABLES).
        self.conn.write_lock().run_mut(0usize, |conn| {
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
        // Uses the already-held guard — do NOT re-lock self.conn.
        if events.len() > 100 {
            if let Err(e) = Self::run_analyze_with_conn(conn) {
                warn!("ANALYZE after batch insert failed: {e}");
            }
        }

        debug!("event batch save: {}items", events.len());
        Ok(events.len())
        })
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

    async fn enforce_retention(&self) -> Result<usize, CoreError> {
        let cutoff = (Utc::now() - Duration::days(self.retention_days as i64)).to_rfc3339();
        let retention_days = self.retention_days;

        self.with_conn(move |conn| {
            let sent_deleted = conn
                .execute(
                    "DELETE FROM events WHERE timestamp < ?1 AND is_sent = 1",
                    rusqlite::params![cutoff],
                )
                .map_err(|e| {
                    StorageError::Internal(format!("Failed to apply retention policy: {e}"))
                })?;

            let unsent_overflow_deleted = conn
                .execute(
                    "DELETE FROM events \
                     WHERE rowid IN ( \
                        SELECT rowid FROM events \
                        WHERE is_sent = 0 \
                        ORDER BY timestamp DESC, rowid DESC \
                        LIMIT -1 OFFSET ?1 \
                     )",
                    rusqlite::params![MAX_UNSENT_EVENTS_RETAINED],
                )
                .map_err(|e| {
                    StorageError::Internal(format!(
                        "Failed to cap unsent event retention: {e}"
                    ))
                })?;

            let deleted = sent_deleted + unsent_overflow_deleted;

            if sent_deleted > 0 {
                info!(
                    "retention policy: deleted {sent_deleted} sent events (>{} days)",
                    retention_days
                );
            }
            if unsent_overflow_deleted > 0 {
                info!(
                    "retention policy: deleted {unsent_overflow_deleted} unsent overflow events (cap={MAX_UNSENT_EVENTS_RETAINED})"
                );
            }
            Ok(deleted)
        })
        .await
        .map_err(Into::into)
    }

    async fn save_activity_segment(&self, summary: &SegmentSummary) -> Result<(), CoreError> {
        let summary = summary.clone();
        let clock = self.clock.clone();
        self.with_conn(move |conn| {
            // Local INSERT producer for activity_segments (#5662). Until now no
            // production path wrote this table on a single device — rows only
            // arrived from peers via the append-only sync merger — so every
            // segment-derived surface (Daily/Weekly Digest, Timeline, Dashboard
            // timetable, regime distribution, Recalibration) rendered empty on a
            // standalone install. Stamp the LOCAL hlc + origin_device_id (unlike
            // update_segment_llm_summary, which preserves a peer's origin) so the
            // row propagates correctly once cross-device sync is enabled.
            //
            // FK note (#9735): `activity_segments.regime_id` REFERENCES
            // `regimes(id)`, and that FK **is enforced** — `libsqlite3-sys`
            // builds the bundled amalgamation with
            // `-DSQLITE_DEFAULT_FOREIGN_KEYS=1` and nothing here turns it off.
            // (A comment that used to sit here claimed the opposite. Measured:
            // `PRAGMA foreign_keys = 1`.)
            //
            // Its INTENT was right: a not-yet-persisted regime must not block
            // the segment. But the mechanism it relied on does not exist, so
            // the insert was rejected outright with
            // `FOREIGN KEY constraint failed` — losing the whole segment.
            //
            // The window is ordinary, not exotic: regimes reach `regimes` via
            // `save_all`, which runs on a 30-minute checkpoint and at shutdown,
            // while segments close far more often. Any segment closed under a
            // freshly-detected regime hits it.
            //
            // So honour the intent explicitly: keep the reference when the
            // regime is already known, and degrade to NULL when it is not. The
            // segment — the row every digest/timeline/dashboard surface is
            // built from — survives either way, and the column is nullable by
            // design (v09_v18.rs: `regime_id TEXT`, no NOT NULL).
            let regime_id = summary.regime_id.as_deref().filter(|id| {
                conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM regimes WHERE id = ?1)",
                    rusqlite::params![id],
                    |row| row.get::<_, bool>(0),
                )
                .unwrap_or(false)
            });
            let h = clock
                .next(conn)
                .map_err(|e| StorageError::Internal(format!("hlc stamp: {e}")))?;
            let app_breakdown =
                serde_json::to_string(&summary.app_breakdown).unwrap_or_else(|_| "{}".to_string());
            let category_breakdown = serde_json::to_string(&summary.category_breakdown)
                .unwrap_or_else(|_| "{}".to_string());
            let patterns_json = serde_json::to_string(&summary.patterns_detected)
                .unwrap_or_else(|_| "[]".to_string());
            let content_json = serde_json::to_string(&summary.content_activities)
                .unwrap_or_else(|_| "[]".to_string());
            let container_json = summary
                .container
                .as_ref()
                .and_then(|c| serde_json::to_string(c).ok());
            // INSERT OR IGNORE: idempotent on the `id` PK (segment close may
            // retry) and safe alongside the sync merger's append-only inserts.
            conn.execute(
                "INSERT OR IGNORE INTO activity_segments \
                 (id, start_time, end_time, duration_secs, regime_id, trigger_reason, \
                  event_count, app_breakdown, category_breakdown, context_switch_count, \
                  dominant_category, avg_importance, patterns_json, content_activities_json, \
                  container_json, llm_summary, hlc_wall_ms, hlc_counter, origin_device_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
                rusqlite::params![
                    summary.segment_id,
                    summary.start_time.to_rfc3339(),
                    summary.end_time.to_rfc3339(),
                    summary.duration_secs as i64,
                    regime_id,
                    enum_to_sql_str(&summary.trigger_reason),
                    summary.event_count as i64,
                    app_breakdown,
                    category_breakdown,
                    summary.context_switch_count as i64,
                    summary.dominant_category,
                    summary.avg_importance as f64,
                    patterns_json,
                    content_json,
                    container_json,
                    summary.llm_summary,
                    h.wall_ms,
                    h.counter,
                    h.device_id,
                ],
            )
            .map_err(|e| {
                StorageError::Internal(format!("Failed to save activity segment: {e}"))
            })?;
            Ok(())
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
        let clock = self.clock.clone();
        self.with_conn(move |conn| {
            // F0/#5186: `llm_summary` is a synced column → bump the HLC so the enrichment
            // re-propagates. activity_segments has a local INSERT producer since #5662
            // (save_activity_segment INSERT-OR-IGNORE); rows also arrive from peers via
            // the append-only merger. In both cases:
            //   - we **preserve `origin_device_id`** (do NOT set it to local): the segment's
            //     activity data belongs to the device that created it, and the device-wide
            //     erasure (`DeletionEvent` deletes `WHERE origin_device_id`) must keep
            //     covering it for that device. Only the HLC wall/counter advance.
            //   - the bumped HLC makes the row re-extract; an existing peer keeps its copy
            //     (append-only union merge), a device first-syncing it LATER gets the
            //     enriched version. (Updating an existing peer's copy would require making
            //     activity_segments LWW in the merger — separate follow-up.)
            let h = clock
                .next(conn)
                .map_err(|e| StorageError::Internal(format!("hlc stamp: {e}")))?;
            conn.execute(
                "UPDATE activity_segments SET llm_summary = ?1, \
                 hlc_wall_ms = ?3, hlc_counter = ?4 WHERE id = ?2",
                rusqlite::params![summary, id, h.wall_ms, h.counter],
            )
            .map_err(|e| {
                StorageError::Internal(format!("Failed to update segment summary: {e}"))
            })?;
            Ok(())
        })
        .await
        .map_err(Into::into)
    }

    async fn update_segment_ai_summary(
        &self,
        segment_id: &str,
        artifact: &maekon_core::models::ai_summary::AiSummaryArtifact,
    ) -> Result<(), CoreError> {
        let id = segment_id.to_string();
        let artifact = artifact.clone();
        let status_json = serde_json::to_string(&artifact).map_err(|e| CoreError::Storage {
            code: maekon_core::error_codes::StorageCode::Failed,
            message: format!("Failed to serialize AI summary status: {e}"),
        })?;
        let clock = self.clock.clone();
        self.with_conn(move |conn| {
            let h = clock
                .next(conn)
                .map_err(|e| StorageError::Internal(format!("hlc stamp: {e}")))?;
            conn.execute(
                "UPDATE activity_segments SET llm_summary = ?1,
                 llm_summary_status_json = ?2, hlc_wall_ms = ?4,
                 hlc_counter = ?5 WHERE id = ?3",
                rusqlite::params![artifact.text, status_json, id, h.wall_ms, h.counter],
            )
            .map_err(|e| {
                StorageError::Internal(format!("Failed to update segment AI summary: {e}"))
            })?;
            Ok(())
        })
        .await
        .map_err(Into::into)
    }

    async fn save_suggestion(&self, suggestion: &Suggestion) -> Result<(), CoreError> {
        let suggestion = suggestion.clone();
        let clock = self.clock.clone();

        self.with_conn(move |conn| {
            // F0/#5186 + D2: this is the site that persists server-issued suggestions.
            // The SHARED `suggestion_stamp` gate (same one the unified.rs INSERTs use)
            // stamps RuleBased/LlmLocal so they sync but leaves `LlmServer` at (0,0,'') so
            // server-synthesized prose never cross-broadcasts between a user's devices.
            let (hw, hc, hd) = suggestion_stamp(&clock, conn, &suggestion.source)
                .map_err(|e| StorageError::Internal(format!("hlc stamp: {e}")))?;
            conn.execute(
                "INSERT OR REPLACE INTO suggestions \
                 (suggestion_id, suggestion_type, source, content, priority, \
                  confidence_score, relevance_score, is_actionable, reasoning, \
                  created_at, expires_at, hlc_wall_ms, hlc_counter, origin_device_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
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
                    hw,
                    hc,
                    hd,
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
#[path = "events_ai_summary_tests.rs"]
mod ai_summary_tests;

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

    #[tokio::test]
    async fn enforce_retention_caps_unsent_events() {
        let storage = SqliteStorage::open_in_memory(30).expect("open_in_memory failed");
        let base = Utc::now() - Duration::hours(4);

        {
            let conn = storage.conn.test_lock();
            for index in 0..(MAX_UNSENT_EVENTS_RETAINED + 2) {
                conn.execute(
                    "INSERT INTO events (event_id, event_type, timestamp, data, is_sent) VALUES (?1, 'test', ?2, '{}', 0)",
                    rusqlite::params![
                        format!("unsent-{index:05}"),
                        (base + Duration::seconds(index)).to_rfc3339(),
                    ],
                )
                .expect("insert unsent event");
            }
        }

        let deleted = storage
            .enforce_retention()
            .await
            .expect("enforce retention");

        assert_eq!(deleted, 2);

        let conn = storage.conn.test_lock();
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM events WHERE is_sent = 0", [], |row| {
                row.get(0)
            })
            .expect("count unsent events");
        assert_eq!(remaining, MAX_UNSENT_EVENTS_RETAINED);

        let oldest_remaining: String = conn
            .query_row(
                "SELECT event_id FROM events WHERE is_sent = 0 ORDER BY timestamp ASC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("oldest remaining event");
        assert_eq!(oldest_remaining, "unsent-00002");
    }

    /// #9735: a segment closed under a regime that has not been checkpointed
    /// yet must still be saved.
    ///
    /// `regimes` rows are written by `save_all`, which runs on a 30-minute
    /// checkpoint and at shutdown; segments close far more often. Before this,
    /// the enforced FK rejected the whole insert with
    /// `FOREIGN KEY constraint failed` and the segment was lost — taking every
    /// surface built on it (digests, timeline, dashboard timetable, regime
    /// distribution, recalibration) with it for that window.
    #[tokio::test]
    async fn save_activity_segment_degrades_an_unpersisted_regime_instead_of_failing() {
        use maekon_core::models::tiered_memory::{SegmentSummary, TriggerReason};
        use std::collections::HashMap;

        let storage = SqliteStorage::open_in_memory(30).expect("open_in_memory failed");
        let now = Utc::now();
        let make = |id: &str, regime: Option<&str>| SegmentSummary {
            segment_id: id.to_string(),
            start_time: now - Duration::minutes(10),
            end_time: now,
            duration_secs: 600,
            regime_id: regime.map(String::from),
            trigger_reason: TriggerReason::ScoreLow,
            event_count: 1,
            app_breakdown: HashMap::new(),
            category_breakdown: HashMap::new(),
            context_switch_count: 0,
            dominant_category: "coding".to_string(),
            avg_importance: 0.5,
            patterns_detected: vec![],
            content_activities: vec![],
            container: None,
            llm_summary: None,
        };

        // The regime has been DETECTED but not yet checkpointed to `regimes`.
        storage
            .save_activity_segment(&make("seg-unpersisted", Some("regime-not-yet-saved")))
            .await
            .expect("an unpersisted regime must not cost us the segment");

        let stored: Option<String> = {
            let conn = storage.conn.test_lock();
            conn.query_row(
                "SELECT regime_id FROM activity_segments WHERE id = 'seg-unpersisted'",
                [],
                |row| row.get(0),
            )
            .expect("the segment row must exist")
        };
        assert_eq!(
            stored, None,
            "the dangling reference is dropped, the segment is kept"
        );

        // Once the regime IS persisted, the reference is preserved.
        {
            let conn = storage.conn.test_lock();
            conn.execute(
                "INSERT INTO regimes (id, label, detected_at, last_seen_at, occurrence_count, \
                 avg_density, avg_importance, dominant_category, is_active) \
                 VALUES ('regime-saved', 'Focus', ?1, ?1, 1, 0.5, 0.5, 'coding', 1)",
                rusqlite::params![now.to_rfc3339()],
            )
            .expect("seed regime");
        }
        storage
            .save_activity_segment(&make("seg-persisted", Some("regime-saved")))
            .await
            .expect("a known regime must save normally");

        let kept: Option<String> = {
            let conn = storage.conn.test_lock();
            conn.query_row(
                "SELECT regime_id FROM activity_segments WHERE id = 'seg-persisted'",
                [],
                |row| row.get(0),
            )
            .expect("the segment row must exist")
        };
        assert_eq!(
            kept,
            Some("regime-saved".to_string()),
            "a valid reference must be preserved, not blanked"
        );
    }
}
