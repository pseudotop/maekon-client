use crate::error::CoreError;
use crate::models::suggestion::SuggestionHistoryEntry;

/// Storage port for few-shot prompt example retrieval and feedback recording.
/// Synchronous — matches StorageService / FocusStorage pattern (SQLite sync ops).
///
/// # Errors
/// `CoreError::Storage` (wire: `storage.failed`) for SQLite prepare/query/
/// execute operations (iter-47 mass fix pattern). Empty feedback history
/// is `Ok(Vec::new())`. The current `SqliteStorage` adapter's
/// `record_suggestion_feedback` executes an UPDATE without checking
/// rowcount, so a non-existent `suggestion_id` is a silent no-op
/// (`Ok(())`) — unknown IDs do NOT surface NotFound.
pub trait FewShotStorage: Send + Sync {
    fn get_suggestions_with_feedback(
        &self,
        limit: usize,
    ) -> Result<Vec<SuggestionHistoryEntry>, CoreError>;
    fn record_suggestion_feedback(
        &self,
        suggestion_id: &str,
        feedback_type: &str,
        context_app: &str,
        context_window: &str,
        regime_label: Option<&str>,
    ) -> Result<(), CoreError>;
}
