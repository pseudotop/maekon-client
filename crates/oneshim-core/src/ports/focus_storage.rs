//! Synchronous storage port for focus-analysis data (work sessions, interruptions, focus metrics).

use crate::error::CoreError;
use crate::models::suggestion::Suggestion;
use crate::models::work_session::{AppCategory, FocusMetrics, Interruption, WorkSession};

/// Port trait for focus-analysis storage operations.
///
/// Binary crates (`oneshim-app`, `src-tauri`) consume this trait via
/// `Arc<dyn FocusStorage>`.  The canonical implementation lives in
/// `oneshim-storage` (backed by SQLite).
///
/// # Errors
/// `CoreError::Storage` (wire: `storage.failed`) for all SQLite operations
/// (iter-47 mass fix pattern: execute/query/transaction/lastInsertRowId).
/// Conventions (verified against `SqliteStorage` adapter):
/// - `get_pending_interruption` returns `Ok(None)` when no active row
///   exists; it does not surface NotFound.
/// - `record_interruption_resume` / `increment_work_session_interruption`
///   / `mark_suggestion_shown_by_id` with an unknown id are rowcount=0
///   no-ops (Ok(())) — the UPDATE skips silently.
/// - `end_work_session` DIFFERS: it uses a `RETURNING` query to fetch
///   the computed `duration_secs`, so an unknown `session_id` surfaces
///   as `CoreError::Storage` via `QueryReturnedNoRows`. Callers that
///   need silent no-op semantics must guard with a SELECT first.
/// - `save_rule_suggestion` returns the persisted `suggestion_id`
///   (string UUID) on success; uniqueness violations bubble up as Storage.
pub trait FocusStorage: Send + Sync {
    fn increment_focus_metrics(
        &self,
        date: &str,
        active_secs: u64,
        deep_work_secs: u64,
        communication_secs: u64,
        context_switches: u32,
        interruption_count: u32,
    ) -> Result<(), CoreError>;

    fn add_deep_work_secs(&self, session_id: i64, secs: u64) -> Result<(), CoreError>;
    fn record_interruption(&self, interruption: &Interruption) -> Result<i64, CoreError>;
    fn increment_work_session_interruption(&self, session_id: i64) -> Result<(), CoreError>;
    fn record_interruption_resume(
        &self,
        interruption_id: i64,
        resumed_to_app: &str,
    ) -> Result<(), CoreError>;
    fn end_work_session(&self, session_id: i64) -> Result<(), CoreError>;
    fn start_work_session(
        &self,
        primary_app: &str,
        category: AppCategory,
    ) -> Result<WorkSession, CoreError>;
    fn get_or_create_focus_metrics(&self, date: &str) -> Result<FocusMetrics, CoreError>;
    fn update_focus_metrics(&self, date: &str, metrics: &FocusMetrics) -> Result<(), CoreError>;
    /// Save a unified Suggestion (rule-based) to the `suggestions` table.
    fn save_rule_suggestion(&self, suggestion: &Suggestion) -> Result<String, CoreError>;
    fn mark_suggestion_shown_by_id(&self, suggestion_id: &str) -> Result<(), CoreError>;
    fn get_pending_interruption(&self) -> Result<Option<Interruption>, CoreError>;
}
