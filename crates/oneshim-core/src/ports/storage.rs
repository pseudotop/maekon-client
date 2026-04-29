//! Local persistence ports — defines contracts for storing events, metrics,
//! sessions, suggestions, and activity segments in the local SQLite database.
//! Implemented by `SqliteStorage` in `oneshim-storage`.

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::error::CoreError;
use crate::models::activity::{IdlePeriod, ProcessSnapshot, SessionStats};
use crate::models::event::Event;
use crate::models::suggestion::Suggestion;
use crate::models::system::SystemMetrics;

/// Local event and suggestion persistence.
///
/// # Errors
/// All methods return `CoreError::Storage` (wire: `storage.failed`) on
/// SQLite failures (lock contention, constraint violation, disk I/O).
/// Iter-47: previously documented as `Internal`; the mass conversion of
/// `StorageError::Internal` → `CoreError::Storage` made the earlier doc
/// stale for ~476 call sites.
#[async_trait]
pub trait StorageService: Send + Sync {
    async fn save_event(&self, event: &Event) -> Result<(), CoreError>;

    async fn get_events(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<Event>, CoreError>;

    async fn get_pending_events(&self, limit: usize) -> Result<Vec<Event>, CoreError>;

    async fn mark_as_sent(&self, event_ids: &[String]) -> Result<(), CoreError>;

    async fn mark_unsent_as_sent_before(&self, before: DateTime<Utc>) -> Result<usize, CoreError>;

    async fn enforce_retention(&self) -> Result<usize, CoreError>;

    /// Persist an LLM/rule-based suggestion to the unified `suggestions` table.
    async fn save_suggestion(&self, suggestion: &Suggestion) -> Result<(), CoreError>;

    /// Update the llm_summary column of an existing activity_segments row.
    async fn update_segment_llm_summary(
        &self,
        segment_id: &str,
        llm_summary: &str,
    ) -> Result<(), CoreError>;
}

/// System metrics, process snapshots, idle periods, and session counters.
///
/// # Errors
/// All methods return `CoreError::Internal` on SQLite failures.
#[async_trait]
pub trait MetricsStorage: Send + Sync {
    async fn save_metrics(&self, metrics: &SystemMetrics) -> Result<(), CoreError>;

    async fn get_metrics(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<SystemMetrics>, CoreError>;

    async fn aggregate_hourly_metrics(&self, hour: DateTime<Utc>) -> Result<(), CoreError>;

    async fn cleanup_old_metrics(&self, before: DateTime<Utc>) -> Result<usize, CoreError>;

    async fn save_process_snapshot(&self, snapshot: &ProcessSnapshot) -> Result<(), CoreError>;

    async fn get_process_snapshots(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<ProcessSnapshot>, CoreError>;

    async fn cleanup_old_process_snapshots(
        &self,
        before: DateTime<Utc>,
    ) -> Result<usize, CoreError>;

    async fn start_idle_period(&self, start_time: DateTime<Utc>) -> Result<i64, CoreError>;

    async fn end_idle_period(&self, id: i64, end_time: DateTime<Utc>) -> Result<(), CoreError>;

    async fn get_ongoing_idle_period(&self) -> Result<Option<(i64, IdlePeriod)>, CoreError>;

    async fn get_idle_periods(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<IdlePeriod>, CoreError>;

    async fn cleanup_old_idle_periods(&self, before: DateTime<Utc>) -> Result<usize, CoreError>;

    async fn upsert_session(&self, stats: &SessionStats) -> Result<(), CoreError>;

    async fn get_session(&self, session_id: &str) -> Result<Option<SessionStats>, CoreError>;

    async fn end_session(&self, session_id: &str, ended_at: DateTime<Utc>)
        -> Result<(), CoreError>;

    async fn increment_session_counters(
        &self,
        session_id: &str,
        events: u64,
        frames: u64,
        idle_secs: u64,
    ) -> Result<(), CoreError>;
}
