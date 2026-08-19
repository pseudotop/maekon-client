//! Durable task lifecycle IPC commands (ADR-028 §8, #8577).
//!
//! These commands are the application use cases: they mint the ids and compute
//! the canonical `request_hash` that the WebView is forbidden to supply, validate
//! to-do transitions with the pure matrix in `maekon_core::models::task`, and call
//! the storage ports. The client can never pass a source reference,
//! `origin_candidate_id`, receipt result, or raw context in a mutation.

use chrono::{DateTime, Utc};
use maekon_core::id_generation::generate_id;
use maekon_core::models::task::{
    CandidateState, SourceKind, SourceLifecycle, TaskCandidate, TaskOutcome, TodoItem, TodoState,
};
use maekon_core::ports::task_store::{
    CandidateFilter, ConfirmCandidateRequest, DeleteTodoRequest, DismissCandidateRequest,
    TaskCommandPort, TaskQueryPort, TodoFilter, TransitionTodoRequest,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::command;

use crate::ipc_error::IpcError;
use crate::runtime_state::AppState;

/// Sanitized candidate view for the WebView. Provenance is minimized to its kind
/// and lifecycle; the dedupe namespace, content hash, and upstream identifiers
/// never leave the application layer.
#[derive(Debug, Clone, Serialize)]
pub struct TaskCandidateView {
    pub id: String,
    pub state: CandidateState,
    pub title: Option<String>,
    pub body: Option<String>,
    pub proposed_due: Option<DateTime<Utc>>,
    pub expires_at: DateTime<Utc>,
    pub source_kind: SourceKind,
    pub source_lifecycle: SourceLifecycle,
    pub revision: i64,
}

impl From<TaskCandidate> for TaskCandidateView {
    fn from(c: TaskCandidate) -> Self {
        Self {
            id: c.id,
            state: c.state,
            title: c.title,
            body: c.body,
            proposed_due: c.proposed_due,
            expires_at: c.expires_at,
            source_kind: c.source_ref.source_kind,
            source_lifecycle: c.source_ref.lifecycle,
            revision: c.revision,
        }
    }
}

/// Sanitized to-do view for the WebView.
#[derive(Debug, Clone, Serialize)]
pub struct TodoView {
    pub id: String,
    pub state: TodoState,
    pub title: String,
    pub body: Option<String>,
    pub due: Option<DateTime<Utc>>,
    pub revision: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<TodoItem> for TodoView {
    fn from(t: TodoItem) -> Self {
        Self {
            id: t.id,
            state: t.state,
            title: t.title,
            body: t.body,
            due: t.due,
            revision: t.revision,
            created_at: t.created_at,
            updated_at: t.updated_at,
        }
    }
}

/// Canonical request hash for idempotency-mismatch detection. Hashes the
/// operation content (never the idempotency key itself), so replaying the same
/// key with different content is detected.
fn request_hash(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"task-request/v1\0");
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    let digest = hasher.finalize();
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("sha256:{hex}")
}

/// List reviewable task candidates.
#[command]
pub async fn list_task_candidates(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<TaskCandidateView>, IpcError> {
    let candidates = state
        .storage
        .list_candidates(CandidateFilter::default())
        .await
        .map_err(IpcError::from)?;
    Ok(candidates
        .into_iter()
        .map(TaskCandidateView::from)
        .collect())
}

/// List durable to-dos.
#[command]
pub async fn list_todos(state: tauri::State<'_, AppState>) -> Result<Vec<TodoView>, IpcError> {
    let todos = state
        .storage
        .list_todos(TodoFilter::default())
        .await
        .map_err(IpcError::from)?;
    Ok(todos.into_iter().map(TodoView::from).collect())
}

/// Confirm a proposed candidate into a durable to-do. The to-do id and receipt id
/// are minted here — the client cannot supply them.
#[allow(clippy::too_many_arguments)] // Tauri command surface: confirm args are the wire contract
#[command]
pub async fn confirm_task_candidate(
    state: tauri::State<'_, AppState>,
    candidate_id: String,
    expected_revision: i64,
    idempotency_key: String,
    confirmed_due: Option<DateTime<Utc>>,
    confirmed_owner_ref: Option<String>,
    confirmed_title: Option<String>,
    confirmed_body: Option<String>,
) -> Result<TaskOutcome, IpcError> {
    let hash = request_hash(&[
        "confirm",
        &candidate_id,
        &expected_revision.to_string(),
        confirmed_due
            .map(|d| d.to_rfc3339())
            .as_deref()
            .unwrap_or(""),
        confirmed_owner_ref.as_deref().unwrap_or(""),
        confirmed_title.as_deref().unwrap_or(""),
        confirmed_body.as_deref().unwrap_or(""),
    ]);
    let request = ConfirmCandidateRequest {
        candidate_id,
        expected_revision,
        idempotency_key,
        request_hash: hash,
        new_todo_id: generate_id("todo"),
        receipt_id: generate_id("tmut"),
        confirmed_due,
        confirmed_owner_ref,
        confirmed_title,
        confirmed_body,
        now: Utc::now(),
    };
    state
        .storage
        .confirm_candidate(request)
        .await
        .map_err(IpcError::from)
}

/// Dismiss a proposed candidate. Content is cleared in the terminal transition.
#[command]
pub async fn dismiss_task_candidate(
    state: tauri::State<'_, AppState>,
    candidate_id: String,
    expected_revision: i64,
    idempotency_key: String,
    reason: Option<String>,
) -> Result<TaskOutcome, IpcError> {
    // Bound the free-text reason so it never becomes an unbounded source-text sink.
    let reason = reason.map(|r| r.chars().take(120).collect::<String>());
    let hash = request_hash(&[
        "dismiss",
        &candidate_id,
        &expected_revision.to_string(),
        reason.as_deref().unwrap_or(""),
    ]);
    let request = DismissCandidateRequest {
        candidate_id,
        expected_revision,
        idempotency_key,
        request_hash: hash,
        receipt_id: generate_id("tmut"),
        reason,
        now: Utc::now(),
    };
    state
        .storage
        .dismiss_candidate(request)
        .await
        .map_err(IpcError::from)
}

/// Transition a confirmed to-do to a new state. The transition is validated
/// against the authoritative matrix before the storage CAS.
#[command]
pub async fn transition_todo(
    state: tauri::State<'_, AppState>,
    todo_id: String,
    target_state: String,
    expected_revision: i64,
    idempotency_key: String,
) -> Result<TaskOutcome, IpcError> {
    let target = TodoState::from_sql_str(&target_state).ok_or_else(|| {
        IpcError::new(
            "validation.invalid_arguments",
            format!("unknown to-do target state {target_state}"),
        )
    })?;
    // Validate legality against the authoritative matrix (fail closed on unknown
    // stored state). A replay of a completed transition still returns cleanly from
    // the storage receipt path, so only reject genuinely illegal fresh moves.
    if let Some(current) = state
        .storage
        .get_todo(&todo_id)
        .await
        .map_err(IpcError::from)?
    {
        if current.state != target && !current.state.can_transition_to(target) {
            return Err(IpcError::new(
                "validation.invalid_arguments",
                format!(
                    "illegal to-do transition {} -> {}",
                    current.state.as_sql_str(),
                    target.as_sql_str()
                ),
            ));
        }
    }
    let hash = request_hash(&[
        "transition",
        &todo_id,
        target.as_sql_str(),
        &expected_revision.to_string(),
    ]);
    let request = TransitionTodoRequest {
        todo_id,
        target,
        expected_revision,
        idempotency_key,
        request_hash: hash,
        receipt_id: generate_id("tmut"),
        now: Utc::now(),
    };
    state
        .storage
        .transition_todo(request)
        .await
        .map_err(IpcError::from)
}

/// Explicitly delete a to-do and its incident blocker edges.
#[command]
pub async fn delete_todo(
    state: tauri::State<'_, AppState>,
    todo_id: String,
    expected_revision: i64,
    idempotency_key: String,
) -> Result<TaskOutcome, IpcError> {
    let hash = request_hash(&["delete", &todo_id, &expected_revision.to_string()]);
    let request = DeleteTodoRequest {
        todo_id,
        expected_revision,
        idempotency_key,
        request_hash: hash,
        receipt_id: generate_id("tmut"),
        now: Utc::now(),
    };
    state
        .storage
        .delete_todo(request)
        .await
        .map_err(IpcError::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_hash_is_stable_and_content_sensitive() {
        let a = request_hash(&["confirm", "tcand_1", "1", "", ""]);
        let b = request_hash(&["confirm", "tcand_1", "1", "", ""]);
        assert_eq!(a, b);
        let c = request_hash(&["confirm", "tcand_1", "2", "", ""]);
        assert_ne!(a, c);
        assert!(a.starts_with("sha256:"));
    }
}
