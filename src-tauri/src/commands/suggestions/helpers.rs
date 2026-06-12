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
/// SQLite persist happens synchronously (primary durability guarantee).
/// In-memory enqueue uses `tokio::spawn` to avoid blocking the IPC caller;
/// it is best-effort for the current session — on restart, SQLite is restored.
// pub(crate) so sibling module `feedback` can import this.
pub(crate) fn enqueue_feedback_retry(
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
    // Fire-and-forget tokio task to avoid holding the caller's async context.
    let rq = mgr.retry_queue().clone();
    let sid = suggestion_id.to_string();
    tokio::spawn(async move {
        rq.lock()
            .await
            .enqueue(maekon_suggestion::feedback_retry::PendingFeedback {
                suggestion_id: sid,
                feedback_type,
                comment,
                attempts: 0,
                next_retry_at: chrono::Utc::now(),
            });
    });
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
