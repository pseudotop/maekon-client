//! Async storage port for focus-analysis data (work sessions, interruptions, focus metrics).

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::error::CoreError;
use crate::models::suggestion::Suggestion;
use crate::models::work_session::{AppCategory, FocusMetrics, Interruption, WorkSession};

/// Port trait for focus-analysis storage operations.
///
/// Binary crates (`maekon-app`, `src-tauri`) consume this trait via
/// `Arc<dyn FocusStorage>`.  The canonical implementation lives in
/// `maekon-storage` (backed by SQLite).
///
/// # Async (ADR-026 PR-2)
/// This port is `#[async_trait]`: every SQLite operation is offloaded to
/// `spawn_blocking` via the `SqliteStorage::with_conn*` funnel so the
/// `parking_lot` connection guard is acquired on a blocking-pool thread and
/// never held across `.await` (the #4928 erase-barrier design). The previous
/// synchronous shape blocked the tokio runtime thread for the duration of each
/// SQLite write when called from `focus_analyzer`'s `async fn`s.
///
/// # Errors
/// `CoreError::Storage` (wire: `storage.failed`) for all SQLite operations
/// (iter-47 mass fix pattern: execute/query/transaction/lastInsertRowId).
/// Conventions (verified against `SqliteStorage` adapter):
/// - `get_pending_interruption` returns `Ok(None)` when no active row
///   exists; it does not surface NotFound.
/// - `resume_interruption` updates only the exact still-pending interruption
///   and returns its pre-update snapshot with the committed resume fields.
///   Unknown or already-resumed IDs return `Ok(None)`.
/// - `increment_work_session_interruption` / `mark_suggestion_shown_by_id`
///   with an unknown id are rowcount=0 no-ops (Ok(())) — the UPDATE skips
///   silently.
/// - `end_work_session` DIFFERS: it uses a `RETURNING` query to fetch
///   the computed `duration_secs`, so an unknown `session_id` surfaces
///   as `CoreError::Storage` via `QueryReturnedNoRows`. Callers that
///   need silent no-op semantics must guard with a SELECT first.
/// - `save_rule_suggestion` returns the persisted `suggestion_id`
///   (string UUID) on success; uniqueness violations bubble up as Storage.
#[async_trait]
pub trait FocusStorage: Send + Sync {
    async fn increment_focus_metrics(
        &self,
        date: &str,
        active_secs: u64,
        deep_work_secs: u64,
        communication_secs: u64,
        context_switches: u32,
        interruption_count: u32,
    ) -> Result<(), CoreError>;

    async fn add_deep_work_secs(&self, session_id: i64, secs: u64) -> Result<(), CoreError>;
    async fn record_interruption(&self, interruption: &Interruption) -> Result<i64, CoreError>;
    async fn increment_work_session_interruption(&self, session_id: i64) -> Result<(), CoreError>;
    async fn resume_interruption(
        &self,
        interruption_id: i64,
        resumed_to_app: &str,
        resumed_at: DateTime<Utc>,
    ) -> Result<Option<Interruption>, CoreError>;
    async fn end_work_session(&self, session_id: i64) -> Result<(), CoreError>;
    async fn start_work_session(
        &self,
        primary_app: &str,
        category: AppCategory,
    ) -> Result<WorkSession, CoreError>;
    async fn get_or_create_focus_metrics(&self, date: &str) -> Result<FocusMetrics, CoreError>;
    async fn update_focus_metrics(
        &self,
        date: &str,
        metrics: &FocusMetrics,
    ) -> Result<(), CoreError>;
    /// Save a unified Suggestion (rule-based) to the `suggestions` table.
    async fn save_rule_suggestion(&self, suggestion: &Suggestion) -> Result<String, CoreError>;
    async fn mark_suggestion_shown_by_id(&self, suggestion_id: &str) -> Result<(), CoreError>;
    async fn get_pending_interruption(&self) -> Result<Option<Interruption>, CoreError>;
}
