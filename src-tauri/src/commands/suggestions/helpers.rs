//! Internal helper functions for suggestion Tauri commands.
//!
//! ADR-013 split from `suggestions/mod.rs`.

use crate::ipc_error::IpcError;

/// Canonical "Suggestions not available" error — suggestion manager missing.
pub(crate) fn suggestions_not_available() -> IpcError {
    IpcError::new("service.unavailable", "Suggestions not available")
}

/// Canonical "AI sessions not available" error — AI session manager missing.
pub(crate) fn ai_sessions_not_available() -> IpcError {
    IpcError::new("service.unavailable", "AI sessions not available")
}

/// Enqueue a failed feedback for background retry and persist to SQLite.
///
/// SQLite persist is the primary durability guarantee — it happens first.
/// The in-memory retry queue is then updated by awaiting the lock directly;
/// since the enqueue is a single fast push (no I/O, no long await), holding
/// the lock synchronously in the caller's async context is safe and eliminates
/// the pre-shutdown cancellation race that existed when a spawned task's
/// `JoinHandle` was discarded.
// pub(crate) so sibling module `feedback` can import this.
pub(crate) async fn enqueue_feedback_retry(
    mgr: &crate::suggestion_manager::SuggestionManager,
    suggestion_id: &str,
    feedback_type: maekon_core::models::suggestion::FeedbackType,
    comment: Option<String>,
) {
    let record = maekon_core::models::storage_records::PendingFeedbackRecord::new_for_insert(
        suggestion_id.to_string(),
        &feedback_type,
        comment.clone(),
        0,
        chrono::Utc::now(),
    );
    if let Err(e) = mgr.storage().save_pending_feedback(&record) {
        tracing::warn!(id = %suggestion_id, "failed to persist pending feedback: {e}");
    }
    // Await the lock directly — the critical section is a single VecDeque push,
    // so the lock is released immediately with no risk of contention.
    let evicted = mgr.retry_queue().lock().await.enqueue(
        maekon_suggestion::feedback_retry::PendingFeedback {
            suggestion_id: suggestion_id.to_string(),
            feedback_type,
            comment,
            attempts: 0,
            next_retry_at: chrono::Utc::now(),
        },
    );
    // If the bounded queue evicted an older pending retry, delete its durable row
    // so SQLite does not keep a pending-retry row the in-session maintenance loop
    // never drains (review4). The lock guard is already released here.
    if let Some(evicted) = evicted {
        if let Err(e) = mgr
            .storage()
            .delete_pending_feedback(&evicted.suggestion_id)
        {
            tracing::warn!(id = %evicted.suggestion_id, "failed to delete evicted pending feedback row: {e}");
        }
    }
}

// pub(crate) so sibling module `feedback` can import this.
pub(crate) fn feedback_type_for_action(
    action: &str,
) -> Result<maekon_core::models::suggestion::FeedbackType, IpcError> {
    use maekon_core::models::suggestion::FeedbackType;
    match action {
        "accept" => Ok(FeedbackType::Accepted),
        "reject" => Ok(FeedbackType::Rejected),
        "defer" => Ok(FeedbackType::Deferred),
        _ => Err(IpcError::new(
            "validation.invalid_arguments",
            format!("Unknown action: {action}. Use accept/reject/defer"),
        )),
    }
}

// pub(crate) so sibling module `feedback` can import this.
pub(crate) fn suggestion_not_found(suggestion_id: &str) -> IpcError {
    IpcError::new(
        "not_found.resource_missing",
        format!("Suggestion not found: {suggestion_id}"),
    )
}
