//! Ports for the durable task lifecycle (ADR-028 §8, #8577).
//!
//! [`TaskCommandPort`] and [`TaskQueryPort`] are narrow, object-safe async traits.
//! The storage adapter implements them and performs the transactional
//! compare-and-swap + receipt writes; it does not decide which transitions are
//! legal. The `src-tauri` application use cases validate transitions with the
//! pure functions in [`crate::models::task`], enforce live consent, mint the ids
//! carried in these requests, compute the canonical `request_hash`, and emit
//! events only after commit.

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::error::CoreError;
use crate::models::task::{
    CandidateState, TaskBlocker, TaskCandidate, TaskOutcome, TodoItem, TodoState,
};

/// Request to ingest a freshly-generated candidate under at-least-once dedupe.
#[derive(Debug, Clone)]
pub struct IngestCandidateRequest {
    /// The fully-formed candidate to insert. Its `dedupe_key` guards ingestion.
    pub candidate: TaskCandidate,
}

/// Result of an ingestion: the stored candidate and whether it was newly created.
#[derive(Debug, Clone)]
pub struct IngestResult {
    /// The candidate now in the store (the existing row on a dedupe hit).
    pub candidate: TaskCandidate,
    /// `true` if a new row was inserted; `false` if an existing dedupe row won.
    pub created: bool,
}

/// Confirm a proposed candidate into a new durable to-do (ADR-028 §3).
#[derive(Debug, Clone)]
pub struct ConfirmCandidateRequest {
    /// The candidate to confirm.
    pub candidate_id: String,
    /// Expected candidate revision for compare-and-swap.
    pub expected_revision: i64,
    /// Opaque idempotency key.
    pub idempotency_key: String,
    /// Canonical request hash (use-case computed).
    pub request_hash: String,
    /// Pre-minted `todo`-prefixed id for the new to-do.
    pub new_todo_id: String,
    /// Pre-minted `tmut`-prefixed receipt id.
    pub receipt_id: String,
    /// Confirmed due time; overrides the candidate's proposal when `Some`.
    pub confirmed_due: Option<DateTime<Utc>>,
    /// Confirmed owner reference; overrides the candidate's proposal when `Some`.
    pub confirmed_owner_ref: Option<String>,
    /// Human-edited to-do title; overrides the candidate's proposal when `Some`.
    /// The confirming human may refine the proposed next step before it becomes a
    /// durable to-do. Still a sanitized, bounded value — never raw source text.
    pub confirmed_title: Option<String>,
    /// Human-edited to-do body; overrides the candidate's proposal when `Some`.
    pub confirmed_body: Option<String>,
    /// Transaction timestamp.
    pub now: DateTime<Utc>,
}

/// Dismiss a proposed candidate (ADR-028 §3).
#[derive(Debug, Clone)]
pub struct DismissCandidateRequest {
    /// The candidate to dismiss.
    pub candidate_id: String,
    /// Expected candidate revision.
    pub expected_revision: i64,
    /// Opaque idempotency key.
    pub idempotency_key: String,
    /// Canonical request hash.
    pub request_hash: String,
    /// Pre-minted receipt id.
    pub receipt_id: String,
    /// Optional bounded, typed dismiss reason (never free source text).
    pub reason: Option<String>,
    /// Transaction timestamp.
    pub now: DateTime<Utc>,
}

/// Transition a confirmed to-do to a new state (ADR-028 §3).
#[derive(Debug, Clone)]
pub struct TransitionTodoRequest {
    /// The to-do to transition.
    pub todo_id: String,
    /// Target state (already validated as legal by the caller).
    pub target: TodoState,
    /// Expected to-do revision.
    pub expected_revision: i64,
    /// Opaque idempotency key.
    pub idempotency_key: String,
    /// Canonical request hash.
    pub request_hash: String,
    /// Pre-minted receipt id.
    pub receipt_id: String,
    /// Transaction timestamp.
    pub now: DateTime<Utc>,
}

/// Explicitly delete a to-do and its incident blocker edges (ADR-028 §6/§8).
#[derive(Debug, Clone)]
pub struct DeleteTodoRequest {
    /// The to-do to delete.
    pub todo_id: String,
    /// Expected to-do revision.
    pub expected_revision: i64,
    /// Opaque idempotency key.
    pub idempotency_key: String,
    /// Canonical request hash.
    pub request_hash: String,
    /// Pre-minted receipt id.
    pub receipt_id: String,
    /// Transaction timestamp.
    pub now: DateTime<Utc>,
}

/// Add or remove a directed blocker edge between two existing to-dos.
///
/// The edge is keyed by `(blocked_todo_id, blocker_todo_id)` and the CAS runs
/// against the blocked to-do's revision (ADR-028 Amendment I3).
#[derive(Debug, Clone)]
pub struct BlockerEdgeRequest {
    /// The to-do that is blocked (the edge's owning entity).
    pub blocked_todo_id: String,
    /// The to-do that blocks it.
    pub blocker_todo_id: String,
    /// Expected revision of the blocked to-do.
    pub expected_revision: i64,
    /// Opaque idempotency key.
    pub idempotency_key: String,
    /// Canonical request hash.
    pub request_hash: String,
    /// Pre-minted receipt id.
    pub receipt_id: String,
    /// Transaction timestamp.
    pub now: DateTime<Utc>,
}

/// Outcome of a startup reconciliation pass (ADR-028 §7).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    /// Number of proposed candidates transitioned to `expired`.
    pub expired_candidates: usize,
    /// Number of confirmed candidates missing their originating to-do
    /// (integrity errors quarantined read-only, never synthesized).
    pub integrity_errors: usize,
    /// The advanced persisted reconciliation floor.
    pub reconciled_at: DateTime<Utc>,
}

/// Filter for listing candidates.
#[derive(Debug, Clone, Default)]
pub struct CandidateFilter {
    /// Restrict to these states; `None` returns all.
    pub states: Option<Vec<CandidateState>>,
    /// Maximum rows to return.
    pub limit: Option<u32>,
}

/// Filter for listing to-dos.
#[derive(Debug, Clone, Default)]
pub struct TodoFilter {
    /// Restrict to these states; `None` returns all.
    pub states: Option<Vec<TodoState>>,
    /// Maximum rows to return.
    pub limit: Option<u32>,
}

/// Command surface for task mutations. Every method is transactional and
/// idempotent; a replayed `(entity, idempotency_key)` returns the original
/// result and never creates a second effect.
#[async_trait]
pub trait TaskCommandPort: Send + Sync {
    /// Ingest a candidate under at-least-once dedupe. A matching `dedupe_key`
    /// returns the existing row (even if terminal) without resurrecting it.
    async fn ingest_candidate(
        &self,
        request: IngestCandidateRequest,
    ) -> Result<IngestResult, CoreError>;

    /// Confirm a proposed candidate, creating exactly one originating to-do.
    async fn confirm_candidate(
        &self,
        request: ConfirmCandidateRequest,
    ) -> Result<TaskOutcome, CoreError>;

    /// Dismiss a proposed candidate and clear its content.
    async fn dismiss_candidate(
        &self,
        request: DismissCandidateRequest,
    ) -> Result<TaskOutcome, CoreError>;

    /// Transition a confirmed to-do to a new state.
    async fn transition_todo(
        &self,
        request: TransitionTodoRequest,
    ) -> Result<TaskOutcome, CoreError>;

    /// Explicitly delete a to-do and its incident blocker edges.
    async fn delete_todo(&self, request: DeleteTodoRequest) -> Result<TaskOutcome, CoreError>;

    /// Add a directed blocker edge (rejects self-links and existing cycles).
    async fn add_blocker(&self, request: BlockerEdgeRequest) -> Result<TaskOutcome, CoreError>;

    /// Remove a directed blocker edge.
    async fn remove_blocker(&self, request: BlockerEdgeRequest) -> Result<TaskOutcome, CoreError>;

    /// Run one idempotent startup reconciliation transaction using
    /// `effective_now = max(current_utc, persisted_last_reconciled_at)`.
    async fn reconcile_tasks(
        &self,
        effective_now: DateTime<Utc>,
    ) -> Result<ReconcileReport, CoreError>;
}

/// Read surface for task views. Returns sanitized rows; callers never receive
/// raw source content.
#[async_trait]
pub trait TaskQueryPort: Send + Sync {
    /// List candidates matching a filter.
    async fn list_candidates(
        &self,
        filter: CandidateFilter,
    ) -> Result<Vec<TaskCandidate>, CoreError>;

    /// Fetch a single candidate by id.
    async fn get_candidate(&self, id: &str) -> Result<Option<TaskCandidate>, CoreError>;

    /// List to-dos matching a filter.
    async fn list_todos(&self, filter: TodoFilter) -> Result<Vec<TodoItem>, CoreError>;

    /// Fetch a single to-do by id.
    async fn get_todo(&self, id: &str) -> Result<Option<TodoItem>, CoreError>;

    /// List the blocker edges for which `todo_id` is the blocked side.
    async fn list_blockers(&self, todo_id: &str) -> Result<Vec<TaskBlocker>, CoreError>;
}
