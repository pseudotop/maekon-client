//! SQLite adapter for the durable task lifecycle ports (ADR-028, #8577).
//!
//! This adapter performs the transactional compare-and-swap and idempotent
//! receipt writes; it does not decide which transitions are legal (the caller
//! validates them with the pure functions in `maekon_core::models::task`).
//! Deletion and content-clearing are explicit child-first `DELETE`/`UPDATE`
//! statements inside each transaction — the connection runs with `PRAGMA
//! foreign_keys` OFF, so FK cascade is inert (ADR-028 Amendment B3).
//!
// OOS-TBD: ADR-013 file split — command port, query port, and row helpers can
// move into a `task_store_impl/` directory module if this file keeps growing.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use maekon_core::error::CoreError;
use maekon_core::models::task::{
    CandidateState, SourceKind, SourceLifecycle, SourceOutcome, TaskBlocker, TaskCandidate,
    TaskOutcome, TaskSourceRef, TodoItem, TodoState,
};
use maekon_core::ports::task_store::{
    BlockerEdgeRequest, CandidateFilter, ConfirmCandidateRequest, DeleteTodoRequest,
    DismissCandidateRequest, IngestCandidateRequest, IngestResult, ReconcileReport,
    TaskCommandPort, TaskQueryPort, TodoFilter, TransitionTodoRequest,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction};

use super::SqliteStorage;
use crate::error::StorageError;

const CANDIDATE_KIND: &str = "CANDIDATE";
const TODO_KIND: &str = "TODO";

/// Reconciliation floor key in `app_meta` (created lazily; local-only).
const RECONCILE_FLOOR_KEY: &str = "task_last_reconciled_at";

/// Bound a human-edited confirm-time override to `max_chars`, trimming
/// surrounding whitespace. An empty (or whitespace-only) edit becomes `None` so
/// the caller falls back to the candidate's proposal instead of clearing it.
fn bounded_edit(value: Option<String>, max_chars: usize) -> Option<String> {
    let trimmed = value?.trim().to_string();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(max_chars).collect())
}

fn parse_ts(value: &str) -> Result<DateTime<Utc>, StorageError> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| StorageError::Internal(format!("task timestamp decode failed: {e}")))
}

fn parse_ts_opt(value: Option<String>) -> Result<Option<DateTime<Utc>>, StorageError> {
    match value {
        Some(v) => Ok(Some(parse_ts(&v)?)),
        None => Ok(None),
    }
}

/// Outcome of a receipt replay lookup (ADR-028 §3 + Amendment B1).
enum ReceiptCheck {
    /// No prior receipt; proceed with the transition.
    Fresh,
    /// A matching receipt exists; replay its recorded result.
    Replay {
        resulting_revision: i64,
        resulting_entity_id: Option<String>,
    },
    /// The key was reused with different request content.
    Mismatch,
}

fn check_receipt(
    tx: &Transaction<'_>,
    entity_kind: &str,
    entity_id: &str,
    idempotency_key: &str,
    request_hash: &str,
) -> Result<ReceiptCheck, StorageError> {
    let row: Option<(String, String, i64, Option<String>)> = tx
        .query_row(
            "SELECT request_hash, to_state, resulting_revision, resulting_entity_id
             FROM task_transition_receipts
             WHERE entity_kind = ?1 AND entity_id = ?2 AND idempotency_key = ?3",
            params![entity_kind, entity_id, idempotency_key],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    Ok(match row {
        None => ReceiptCheck::Fresh,
        Some((stored_hash, _, _, _)) if stored_hash != request_hash => ReceiptCheck::Mismatch,
        Some((_, _to_state, resulting_revision, resulting_entity_id)) => ReceiptCheck::Replay {
            resulting_revision,
            resulting_entity_id,
        },
    })
}

#[allow(clippy::too_many_arguments)]
fn insert_receipt(
    tx: &Transaction<'_>,
    receipt_id: &str,
    entity_kind: &str,
    entity_id: &str,
    idempotency_key: &str,
    request_hash: &str,
    from_state: &str,
    to_state: &str,
    resulting_revision: i64,
    resulting_entity_id: Option<&str>,
    now: DateTime<Utc>,
) -> Result<(), StorageError> {
    tx.execute(
        "INSERT INTO task_transition_receipts
         (id, entity_kind, entity_id, idempotency_key, request_hash,
          from_state, to_state, resulting_revision, resulting_entity_id, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![
            receipt_id,
            entity_kind,
            entity_id,
            idempotency_key,
            request_hash,
            from_state,
            to_state,
            resulting_revision,
            resulting_entity_id,
            now.to_rfc3339(),
        ],
    )?;
    Ok(())
}

/// Read a candidate's `(state, revision)` for compare-and-swap.
fn candidate_state_rev(
    tx: &Transaction<'_>,
    candidate_id: &str,
) -> Result<Option<(String, i64)>, StorageError> {
    Ok(tx
        .query_row(
            "SELECT state, revision FROM task_candidates WHERE id = ?1",
            [candidate_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
        )
        .optional()?)
}

fn todo_state_rev(
    tx: &Transaction<'_>,
    todo_id: &str,
) -> Result<Option<(String, i64)>, StorageError> {
    Ok(tx
        .query_row(
            "SELECT state, revision FROM todo_items WHERE id = ?1",
            [todo_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
        )
        .optional()?)
}

#[async_trait]
impl TaskCommandPort for SqliteStorage {
    async fn ingest_candidate(
        &self,
        request: IngestCandidateRequest,
    ) -> Result<IngestResult, CoreError> {
        let candidate = request.candidate;
        let result: Option<IngestResult> = self
            .with_conn_mut(move |conn| {
                let tx = conn.transaction()?;
                let inserted = tx.execute(
                    "INSERT INTO task_candidates
                     (id, state, title, body, proposed_due, proposed_owner_ref,
                      expires_at, dedupe_key, revision, created_at, updated_at)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
                     ON CONFLICT(dedupe_key) DO NOTHING",
                    params![
                        candidate.id,
                        candidate.state.as_sql_str(),
                        candidate.title,
                        candidate.body,
                        candidate.proposed_due.map(|d| d.to_rfc3339()),
                        candidate.proposed_owner_ref,
                        candidate.expires_at.to_rfc3339(),
                        candidate.dedupe_key,
                        candidate.revision,
                        candidate.created_at.to_rfc3339(),
                        candidate.updated_at.to_rfc3339(),
                    ],
                )?;
                if inserted == 1 {
                    let s = &candidate.source_ref;
                    tx.execute(
                        "INSERT INTO task_source_refs
                         (candidate_id, source_kind, extension_id, install_id,
                          account_subject_ref, upstream_object_id, upstream_revision,
                          upstream_etag, occurred_at, observed_at, dedupe_namespace,
                          content_hash, lifecycle, source_outcome)
                         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
                        params![
                            candidate.id,
                            s.source_kind.as_sql_str(),
                            s.extension_id,
                            s.install_id,
                            s.account_subject_ref,
                            s.upstream_object_id,
                            s.upstream_revision,
                            s.upstream_etag,
                            s.occurred_at.map(|d| d.to_rfc3339()),
                            s.observed_at.to_rfc3339(),
                            s.dedupe_namespace,
                            s.content_hash,
                            s.lifecycle.as_sql_str(),
                            s.source_outcome.map(|o| o.as_sql_str()),
                        ],
                    )?;
                    tx.commit()?;
                    Ok(Some(IngestResult {
                        candidate,
                        created: true,
                    }))
                } else {
                    // Dedupe hit: return the existing row (even if terminal).
                    let existing = load_candidate(&tx, &candidate.dedupe_key_lookup())?;
                    tx.commit()?;
                    match existing {
                        Some(existing) => Ok(Some(IngestResult {
                            candidate: existing,
                            created: false,
                        })),
                        None => Err(StorageError::Internal(
                            "dedupe conflict without an existing row".to_string(),
                        )),
                    }
                }
            })
            .await
            .map_err(Into::<CoreError>::into)?;
        result.ok_or_else(skipped_err)
    }

    async fn confirm_candidate(
        &self,
        request: ConfirmCandidateRequest,
    ) -> Result<TaskOutcome, CoreError> {
        let result: Option<TaskOutcome> = self
            .with_conn_mut(move |conn| {
                let tx = conn.transaction()?;
                let cid = &request.candidate_id;
                // 1. Replay a matching receipt.
                match check_receipt(
                    &tx,
                    CANDIDATE_KIND,
                    cid,
                    &request.idempotency_key,
                    &request.request_hash,
                )? {
                    ReceiptCheck::Mismatch => {
                        tx.commit()?;
                        return Ok(Some(TaskOutcome::IdempotencyMismatch));
                    }
                    ReceiptCheck::Replay {
                        resulting_revision,
                        resulting_entity_id,
                        ..
                    } => {
                        tx.commit()?;
                        return Ok(Some(TaskOutcome::Confirmed {
                            candidate_id: cid.clone(),
                            todo_id: resulting_entity_id.unwrap_or_default(),
                            revision: resulting_revision,
                        }));
                    }
                    ReceiptCheck::Fresh => {}
                }
                // 2. Compare state/revision.
                let Some((state, revision)) = candidate_state_rev(&tx, cid)? else {
                    tx.commit()?;
                    return Ok(Some(TaskOutcome::RevisionConflict));
                };
                if state != CandidateState::Proposed.as_sql_str() {
                    tx.commit()?;
                    // Already terminal: idempotent no-op success carrying the state.
                    return Ok(Some(TaskOutcome::AlreadyTransitioned {
                        current_state: state,
                    }));
                }
                if revision != request.expected_revision {
                    tx.commit()?;
                    return Ok(Some(TaskOutcome::RevisionConflict));
                }
                let new_rev = revision + 1;
                // 3. Read the candidate content + proposed due/owner BEFORE clearing,
                //    so the confirmed to-do carries the sanitized values.
                let (title, body, pdue, powner): (
                    Option<String>,
                    Option<String>,
                    Option<String>,
                    Option<String>,
                ) = tx.query_row(
                    "SELECT title, body, proposed_due, proposed_owner_ref
                     FROM task_candidates WHERE id=?1",
                    [cid],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )?;
                // A human may refine the proposed next step at confirm time. The
                // edited values override the proposal but stay bounded so an edit
                // can never become an unbounded content sink (mirrors the dismiss
                // reason bound). Empty edits fall back to the proposal.
                let title = bounded_edit(request.confirmed_title.clone(), 200).or(title);
                let body = bounded_edit(request.confirmed_body.clone(), 2000).or(body);
                // 4. proposed -> confirmed, clear content, bump revision.
                tx.execute(
                    "UPDATE task_candidates
                     SET state='CONFIRMED', title=NULL, body=NULL, revision=?2, updated_at=?3
                     WHERE id=?1",
                    params![cid, new_rev, request.now.to_rfc3339()],
                )?;
                // 5. Insert exactly one originating to-do (origin_candidate_id UNIQUE).
                //    Confirmed due/owner override the proposal when provided.
                let due = request.confirmed_due.map(|d| d.to_rfc3339()).or(pdue);
                let owner = request.confirmed_owner_ref.clone().or(powner);
                tx.execute(
                    "INSERT INTO todo_items
                     (id, state, title, body, due, owner_ref, origin_candidate_id,
                      revision, created_at, updated_at)
                     VALUES (?1,'CONFIRMED',?2,?3,?4,?5,?6,1,?7,?7)",
                    params![
                        request.new_todo_id,
                        title.unwrap_or_default(),
                        body,
                        due,
                        owner,
                        cid,
                        request.now.to_rfc3339(),
                    ],
                )?;
                // 6. Insert the idempotent receipt.
                insert_receipt(
                    &tx,
                    &request.receipt_id,
                    CANDIDATE_KIND,
                    cid,
                    &request.idempotency_key,
                    &request.request_hash,
                    "PROPOSED",
                    "CONFIRMED",
                    new_rev,
                    Some(&request.new_todo_id),
                    request.now,
                )?;
                tx.commit()?;
                Ok(Some(TaskOutcome::Confirmed {
                    candidate_id: cid.clone(),
                    todo_id: request.new_todo_id.clone(),
                    revision: new_rev,
                }))
            })
            .await
            .map_err(Into::<CoreError>::into)?;
        result.ok_or_else(skipped_err)
    }

    async fn dismiss_candidate(
        &self,
        request: DismissCandidateRequest,
    ) -> Result<TaskOutcome, CoreError> {
        let result: Option<TaskOutcome> = self
            .with_conn_mut(move |conn| {
                let tx = conn.transaction()?;
                let cid = &request.candidate_id;
                match check_receipt(
                    &tx,
                    CANDIDATE_KIND,
                    cid,
                    &request.idempotency_key,
                    &request.request_hash,
                )? {
                    ReceiptCheck::Mismatch => {
                        tx.commit()?;
                        return Ok(Some(TaskOutcome::IdempotencyMismatch));
                    }
                    ReceiptCheck::Replay {
                        resulting_revision, ..
                    } => {
                        tx.commit()?;
                        return Ok(Some(TaskOutcome::Dismissed {
                            candidate_id: cid.clone(),
                            revision: resulting_revision,
                        }));
                    }
                    ReceiptCheck::Fresh => {}
                }
                let Some((state, revision)) = candidate_state_rev(&tx, cid)? else {
                    tx.commit()?;
                    return Ok(Some(TaskOutcome::RevisionConflict));
                };
                if state != CandidateState::Proposed.as_sql_str() {
                    tx.commit()?;
                    return Ok(Some(TaskOutcome::AlreadyTransitioned {
                        current_state: state,
                    }));
                }
                if revision != request.expected_revision {
                    tx.commit()?;
                    return Ok(Some(TaskOutcome::RevisionConflict));
                }
                let new_rev = revision + 1;
                // Clear content in the terminal transition; keep a minimal dedupe
                // tombstone (the row + dedupe_key, no content) per ADR Amendment B3.
                tx.execute(
                    "UPDATE task_candidates
                     SET state='DISMISSED', title=NULL, body=NULL, revision=?2, updated_at=?3
                     WHERE id=?1",
                    params![cid, new_rev, request.now.to_rfc3339()],
                )?;
                insert_receipt(
                    &tx,
                    &request.receipt_id,
                    CANDIDATE_KIND,
                    cid,
                    &request.idempotency_key,
                    &request.request_hash,
                    "PROPOSED",
                    "DISMISSED",
                    new_rev,
                    None,
                    request.now,
                )?;
                tx.commit()?;
                Ok(Some(TaskOutcome::Dismissed {
                    candidate_id: cid.clone(),
                    revision: new_rev,
                }))
            })
            .await
            .map_err(Into::<CoreError>::into)?;
        result.ok_or_else(skipped_err)
    }

    async fn transition_todo(
        &self,
        request: TransitionTodoRequest,
    ) -> Result<TaskOutcome, CoreError> {
        let result: Option<TaskOutcome> = self
            .with_conn_mut(move |conn| {
                let tx = conn.transaction()?;
                let tid = &request.todo_id;
                let target = request.target.as_sql_str().to_string();
                match check_receipt(
                    &tx,
                    TODO_KIND,
                    tid,
                    &request.idempotency_key,
                    &request.request_hash,
                )? {
                    ReceiptCheck::Mismatch => {
                        tx.commit()?;
                        return Ok(Some(TaskOutcome::IdempotencyMismatch));
                    }
                    ReceiptCheck::Replay {
                        resulting_revision, ..
                    } => {
                        tx.commit()?;
                        return Ok(Some(TaskOutcome::Transitioned {
                            todo_id: tid.clone(),
                            state: request.target,
                            revision: resulting_revision,
                        }));
                    }
                    ReceiptCheck::Fresh => {}
                }
                let Some((state, revision)) = todo_state_rev(&tx, tid)? else {
                    tx.commit()?;
                    return Ok(Some(TaskOutcome::RevisionConflict));
                };
                if state == target {
                    tx.commit()?;
                    return Ok(Some(TaskOutcome::AlreadyTransitioned {
                        current_state: state,
                    }));
                }
                if revision != request.expected_revision {
                    tx.commit()?;
                    return Ok(Some(TaskOutcome::RevisionConflict));
                }
                let new_rev = revision + 1;
                tx.execute(
                    "UPDATE todo_items SET state=?2, revision=?3, updated_at=?4 WHERE id=?1",
                    params![tid, target, new_rev, request.now.to_rfc3339()],
                )?;
                insert_receipt(
                    &tx,
                    &request.receipt_id,
                    TODO_KIND,
                    tid,
                    &request.idempotency_key,
                    &request.request_hash,
                    &state,
                    &target,
                    new_rev,
                    None,
                    request.now,
                )?;
                tx.commit()?;
                Ok(Some(TaskOutcome::Transitioned {
                    todo_id: tid.clone(),
                    state: request.target,
                    revision: new_rev,
                }))
            })
            .await
            .map_err(Into::<CoreError>::into)?;
        result.ok_or_else(skipped_err)
    }

    async fn delete_todo(&self, request: DeleteTodoRequest) -> Result<TaskOutcome, CoreError> {
        let result: Option<TaskOutcome> = self
            .with_conn_mut(move |conn| {
                let tx = conn.transaction()?;
                let tid = &request.todo_id;
                match check_receipt(
                    &tx,
                    TODO_KIND,
                    tid,
                    &request.idempotency_key,
                    &request.request_hash,
                )? {
                    ReceiptCheck::Mismatch => {
                        tx.commit()?;
                        return Ok(Some(TaskOutcome::IdempotencyMismatch));
                    }
                    ReceiptCheck::Replay {
                        resulting_revision, ..
                    } => {
                        tx.commit()?;
                        return Ok(Some(TaskOutcome::Transitioned {
                            todo_id: tid.clone(),
                            state: TodoState::Cancelled,
                            revision: resulting_revision,
                        }));
                    }
                    ReceiptCheck::Fresh => {}
                }
                let Some((state, revision)) = todo_state_rev(&tx, tid)? else {
                    tx.commit()?;
                    return Ok(Some(TaskOutcome::RevisionConflict));
                };
                if revision != request.expected_revision {
                    tx.commit()?;
                    return Ok(Some(TaskOutcome::RevisionConflict));
                }
                // Application-ordered child-first delete (FK cascade is inert).
                tx.execute(
                    "DELETE FROM todo_blockers WHERE blocked_todo_id=?1 OR blocker_todo_id=?1",
                    [tid],
                )?;
                tx.execute("DELETE FROM todo_items WHERE id=?1", [tid])?;
                insert_receipt(
                    &tx,
                    &request.receipt_id,
                    TODO_KIND,
                    tid,
                    &request.idempotency_key,
                    &request.request_hash,
                    &state,
                    "DELETED",
                    revision,
                    None,
                    request.now,
                )?;
                tx.commit()?;
                Ok(Some(TaskOutcome::Transitioned {
                    todo_id: tid.clone(),
                    state: TodoState::Cancelled,
                    revision,
                }))
            })
            .await
            .map_err(Into::<CoreError>::into)?;
        result.ok_or_else(skipped_err)
    }

    async fn add_blocker(&self, request: BlockerEdgeRequest) -> Result<TaskOutcome, CoreError> {
        blocker_edit(self, request, true).await
    }

    async fn remove_blocker(&self, request: BlockerEdgeRequest) -> Result<TaskOutcome, CoreError> {
        blocker_edit(self, request, false).await
    }

    async fn reconcile_tasks(
        &self,
        effective_now: DateTime<Utc>,
    ) -> Result<ReconcileReport, CoreError> {
        let result: Option<ReconcileReport> = self
            .with_conn_mut(move |conn| {
                let tx = conn.transaction()?;
                // Read the persisted floor; effective_now never moves backward.
                let stored: Option<String> = tx
                    .query_row(
                        "SELECT value FROM app_meta WHERE key=?1",
                        [RECONCILE_FLOOR_KEY],
                        |r| r.get(0),
                    )
                    .optional()?;
                let floor = match stored {
                    Some(v) => parse_ts(&v)?.max(effective_now),
                    None => effective_now,
                };
                // Expire proposed candidates past their TTL; clear content.
                let expired = tx.execute(
                    "UPDATE task_candidates
                     SET state='EXPIRED', title=NULL, body=NULL, revision=revision+1, updated_at=?2
                     WHERE state='PROPOSED' AND expires_at <= ?1",
                    params![floor.to_rfc3339(), floor.to_rfc3339()],
                )?;
                // Integrity: confirmed candidates missing their originating to-do.
                let integrity: i64 = tx.query_row(
                    "SELECT COUNT(*) FROM task_candidates c
                     WHERE c.state='CONFIRMED'
                       AND NOT EXISTS (SELECT 1 FROM todo_items t WHERE t.origin_candidate_id=c.id)",
                    [],
                    |r| r.get(0),
                )?;
                // Advance the persisted floor.
                tx.execute(
                    "INSERT INTO app_meta (key, value) VALUES (?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                    params![RECONCILE_FLOOR_KEY, floor.to_rfc3339()],
                )?;
                tx.commit()?;
                Ok(Some(ReconcileReport {
                    expired_candidates: expired,
                    integrity_errors: integrity as usize,
                    reconciled_at: floor,
                }))
            })
            .await
            .map_err(Into::<CoreError>::into)?;
        result.ok_or_else(skipped_err)
    }
}

async fn blocker_edit(
    storage: &SqliteStorage,
    request: BlockerEdgeRequest,
    add: bool,
) -> Result<TaskOutcome, CoreError> {
    let result: Option<TaskOutcome> = storage
        .with_conn_mut(move |conn| {
            let tx = conn.transaction()?;
            let blocked = &request.blocked_todo_id;
            if request.blocked_todo_id == request.blocker_todo_id {
                tx.commit()?;
                return Ok(Some(TaskOutcome::RevisionConflict));
            }
            match check_receipt(&tx, "TODO_BLOCKER", blocked, &request.idempotency_key, &request.request_hash)? {
                ReceiptCheck::Mismatch => {
                    tx.commit()?;
                    return Ok(Some(TaskOutcome::IdempotencyMismatch));
                }
                ReceiptCheck::Replay { resulting_revision, .. } => {
                    tx.commit()?;
                    return Ok(Some(TaskOutcome::Transitioned {
                        todo_id: blocked.clone(),
                        state: TodoState::Waiting,
                        revision: resulting_revision,
                    }));
                }
                ReceiptCheck::Fresh => {}
            }
            let Some((_, revision)) = todo_state_rev(&tx, blocked)? else {
                tx.commit()?;
                return Ok(Some(TaskOutcome::RevisionConflict));
            };
            if revision != request.expected_revision {
                tx.commit()?;
                return Ok(Some(TaskOutcome::RevisionConflict));
            }
            if add {
                // Reject a cycle: blocker already (transitively via direct edge) blocked by blocked.
                let creates_cycle: bool = tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM todo_blockers
                         WHERE blocked_todo_id=?1 AND blocker_todo_id=?2)",
                        params![request.blocker_todo_id, request.blocked_todo_id],
                        |r| r.get::<_, i64>(0),
                    )
                    .optional()?
                    .map(|v| v == 1)
                    .unwrap_or(false);
                if creates_cycle {
                    tx.commit()?;
                    return Ok(Some(TaskOutcome::RevisionConflict));
                }
                tx.execute(
                    "INSERT OR IGNORE INTO todo_blockers (blocked_todo_id, blocker_todo_id, created_at)
                     VALUES (?1,?2,?3)",
                    params![request.blocked_todo_id, request.blocker_todo_id, request.now.to_rfc3339()],
                )?;
            } else {
                tx.execute(
                    "DELETE FROM todo_blockers WHERE blocked_todo_id=?1 AND blocker_todo_id=?2",
                    params![request.blocked_todo_id, request.blocker_todo_id],
                )?;
            }
            let new_rev = revision + 1;
            tx.execute(
                "UPDATE todo_items SET revision=?2, updated_at=?3 WHERE id=?1",
                params![blocked, new_rev, request.now.to_rfc3339()],
            )?;
            let to_state = if add { "BLOCKER_ADDED" } else { "BLOCKER_REMOVED" };
            insert_receipt(
                &tx, &request.receipt_id, "TODO_BLOCKER", blocked, &request.idempotency_key,
                &request.request_hash, "EDGE", to_state, new_rev, None, request.now,
            )?;
            tx.commit()?;
            Ok(Some(TaskOutcome::Transitioned {
                todo_id: blocked.clone(),
                state: TodoState::Waiting,
                revision: new_rev,
            }))
        })
        .await
        .map_err(Into::<CoreError>::into)?;
    result.ok_or_else(skipped_err)
}

// ---- Query port ----

#[async_trait]
impl TaskQueryPort for SqliteStorage {
    async fn list_candidates(
        &self,
        filter: CandidateFilter,
    ) -> Result<Vec<TaskCandidate>, CoreError> {
        self.with_conn_read(move |conn| {
            let limit = filter.limit.unwrap_or(200).min(1000) as i64;
            let mut out = Vec::new();
            let mut stmt = conn.prepare(
                "SELECT id, dedupe_key FROM task_candidates
                 ORDER BY created_at DESC LIMIT ?1",
            )?;
            let ids: Vec<(String, String)> = stmt
                .query_map([limit], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?
                .collect::<Result<_, _>>()?;
            drop(stmt);
            for (_, dedupe_key) in ids {
                if let Some(c) = load_candidate_conn(conn, &dedupe_key)? {
                    let keep = filter
                        .states
                        .as_ref()
                        .map(|s| s.contains(&c.state))
                        .unwrap_or(true);
                    if keep {
                        out.push(c);
                    }
                }
            }
            Ok(out)
        })
        .await
        .map_err(Into::into)
    }

    async fn get_candidate(&self, id: &str) -> Result<Option<TaskCandidate>, CoreError> {
        let id = id.to_string();
        self.with_conn_read(move |conn| {
            let dedupe_key: Option<String> = conn
                .query_row(
                    "SELECT dedupe_key FROM task_candidates WHERE id=?1",
                    [&id],
                    |r| r.get(0),
                )
                .optional()?;
            match dedupe_key {
                Some(dk) => load_candidate_conn(conn, &dk),
                None => Ok(None),
            }
        })
        .await
        .map_err(Into::into)
    }

    async fn list_todos(&self, filter: TodoFilter) -> Result<Vec<TodoItem>, CoreError> {
        self.with_conn_read(move |conn| {
            let limit = filter.limit.unwrap_or(200).min(1000) as i64;
            let mut stmt = conn.prepare(
                "SELECT id, state, title, body, due, owner_ref, origin_candidate_id,
                        supersedes_todo_id, revision, created_at, updated_at
                 FROM todo_items ORDER BY updated_at DESC LIMIT ?1",
            )?;
            let rows = stmt
                .query_map([limit], row_to_todo)?
                .collect::<Result<Vec<_>, _>>()?;
            let out = rows
                .into_iter()
                .collect::<Result<Vec<TodoItem>, StorageError>>()?
                .into_iter()
                .filter(|t| {
                    filter
                        .states
                        .as_ref()
                        .map(|s| s.contains(&t.state))
                        .unwrap_or(true)
                })
                .collect();
            Ok(out)
        })
        .await
        .map_err(Into::into)
    }

    async fn get_todo(&self, id: &str) -> Result<Option<TodoItem>, CoreError> {
        let id = id.to_string();
        self.with_conn_read(move |conn| {
            let row = conn
                .query_row(
                    "SELECT id, state, title, body, due, owner_ref, origin_candidate_id,
                            supersedes_todo_id, revision, created_at, updated_at
                     FROM todo_items WHERE id=?1",
                    [&id],
                    row_to_todo,
                )
                .optional()?;
            match row {
                Some(inner) => Ok(Some(inner?)),
                None => Ok(None),
            }
        })
        .await
        .map_err(Into::into)
    }

    async fn list_blockers(&self, todo_id: &str) -> Result<Vec<TaskBlocker>, CoreError> {
        let todo_id = todo_id.to_string();
        self.with_conn_read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT blocked_todo_id, blocker_todo_id, created_at
                 FROM todo_blockers WHERE blocked_todo_id=?1 ORDER BY created_at",
            )?;
            let rows = stmt
                .query_map([&todo_id], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            rows.into_iter()
                .map(|(blocked, blocker, created)| {
                    Ok(TaskBlocker {
                        blocked_todo_id: blocked,
                        blocker_todo_id: blocker,
                        created_at: parse_ts(&created)?,
                    })
                })
                .collect::<Result<Vec<_>, StorageError>>()
        })
        .await
        .map_err(Into::into)
    }
}

/// Row → `TodoItem`, returning a nested `Result` so timestamp decode errors
/// surface without aborting the `query_map` closure signature.
#[allow(clippy::type_complexity)]
fn row_to_todo(r: &rusqlite::Row<'_>) -> rusqlite::Result<Result<TodoItem, StorageError>> {
    let id: String = r.get(0)?;
    let state_s: String = r.get(1)?;
    let title: String = r.get(2)?;
    let body: Option<String> = r.get(3)?;
    let due: Option<String> = r.get(4)?;
    let owner_ref: Option<String> = r.get(5)?;
    let origin_candidate_id: String = r.get(6)?;
    let supersedes_todo_id: Option<String> = r.get(7)?;
    let revision: i64 = r.get(8)?;
    let created_at: String = r.get(9)?;
    let updated_at: String = r.get(10)?;
    Ok((|| {
        let state = TodoState::from_sql_str(&state_s)
            .ok_or_else(|| StorageError::Internal(format!("unknown todo state {state_s}")))?;
        Ok(TodoItem {
            id,
            state,
            title,
            body,
            due: parse_ts_opt(due)?,
            owner_ref,
            origin_candidate_id,
            supersedes_todo_id,
            revision,
            created_at: parse_ts(&created_at)?,
            updated_at: parse_ts(&updated_at)?,
        })
    })())
}

fn load_candidate(
    tx: &Transaction<'_>,
    dedupe_key: &str,
) -> Result<Option<TaskCandidate>, StorageError> {
    load_candidate_conn(tx, dedupe_key)
}

/// Load a candidate + its source ref by dedupe key.
fn load_candidate_conn(
    conn: &Connection,
    dedupe_key: &str,
) -> Result<Option<TaskCandidate>, StorageError> {
    let cand = conn
        .query_row(
            "SELECT id, state, title, body, proposed_due, proposed_owner_ref,
                    expires_at, dedupe_key, revision, created_at, updated_at
             FROM task_candidates WHERE dedupe_key=?1",
            [dedupe_key],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, String>(7)?,
                    r.get::<_, i64>(8)?,
                    r.get::<_, String>(9)?,
                    r.get::<_, String>(10)?,
                ))
            },
        )
        .optional()?;
    let Some((id, state_s, title, body, pdue, powner, expires, dk, rev, created, updated)) = cand
    else {
        return Ok(None);
    };
    let source_ref = load_source_ref(conn, &id)?
        .ok_or_else(|| StorageError::Internal(format!("candidate {id} missing source ref")))?;
    let state = CandidateState::from_sql_str(&state_s)
        .ok_or_else(|| StorageError::Internal(format!("unknown candidate state {state_s}")))?;
    Ok(Some(TaskCandidate {
        id,
        state,
        title,
        body,
        proposed_due: parse_ts_opt(pdue)?,
        proposed_owner_ref: powner,
        expires_at: parse_ts(&expires)?,
        source_ref,
        dedupe_key: dk,
        revision: rev,
        created_at: parse_ts(&created)?,
        updated_at: parse_ts(&updated)?,
    }))
}

fn load_source_ref(
    conn: &Connection,
    candidate_id: &str,
) -> Result<Option<TaskSourceRef>, StorageError> {
    let row = conn
        .query_row(
            "SELECT source_kind, extension_id, install_id, account_subject_ref,
                    upstream_object_id, upstream_revision, upstream_etag, occurred_at,
                    observed_at, dedupe_namespace, content_hash, lifecycle, source_outcome
             FROM task_source_refs WHERE candidate_id=?1",
            [candidate_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, Option<String>>(7)?,
                    r.get::<_, String>(8)?,
                    r.get::<_, String>(9)?,
                    r.get::<_, String>(10)?,
                    r.get::<_, String>(11)?,
                    r.get::<_, Option<String>>(12)?,
                ))
            },
        )
        .optional()?;
    let Some((kind, ext, install, acct, obj, rev, etag, occurred, observed, ns, hash, lc, outcome)) =
        row
    else {
        return Ok(None);
    };
    Ok(Some(TaskSourceRef {
        source_kind: SourceKind::from_sql_str(&kind),
        extension_id: ext,
        install_id: install,
        account_subject_ref: acct,
        upstream_object_id: obj,
        upstream_revision: rev,
        upstream_etag: etag,
        occurred_at: parse_ts_opt(occurred)?,
        observed_at: parse_ts(&observed)?,
        dedupe_namespace: ns,
        content_hash: hash,
        lifecycle: SourceLifecycle::from_sql_str(&lc),
        source_outcome: outcome.and_then(|o| SourceOutcome::from_sql_str(&o)),
    }))
}

fn skipped_err() -> CoreError {
    StorageError::Internal("task mutation skipped during erasure".to_string()).into()
}

/// Convenience accessor so `ingest_candidate` can re-load the winning row.
trait DedupeKeyLookup {
    fn dedupe_key_lookup(&self) -> String;
}
impl DedupeKeyLookup for TaskCandidate {
    fn dedupe_key_lookup(&self) -> String {
        self.dedupe_key.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::models::task::compute_dedupe_key;

    fn storage() -> SqliteStorage {
        SqliteStorage::open_in_memory(30).unwrap()
    }

    fn candidate(id: &str, dedupe_namespace: &str, content_hash: &str) -> TaskCandidate {
        let now = Utc::now();
        let source_ref = TaskSourceRef {
            source_kind: SourceKind::LocalCurrentScene,
            extension_id: None,
            install_id: None,
            account_subject_ref: None,
            upstream_object_id: None,
            upstream_revision: None,
            upstream_etag: None,
            occurred_at: None,
            observed_at: now,
            dedupe_namespace: dedupe_namespace.to_string(),
            content_hash: content_hash.to_string(),
            lifecycle: SourceLifecycle::Active,
            source_outcome: None,
        };
        TaskCandidate {
            id: id.to_string(),
            state: CandidateState::Proposed,
            title: Some("Follow up on the report".to_string()),
            body: Some("body text".to_string()),
            proposed_due: None,
            proposed_owner_ref: None,
            expires_at: now + chrono::Duration::days(1),
            dedupe_key: compute_dedupe_key(&source_ref),
            source_ref,
            revision: 1,
            created_at: now,
            updated_at: now,
        }
    }

    fn confirm_req(cid: &str, rev: i64, key: &str) -> ConfirmCandidateRequest {
        ConfirmCandidateRequest {
            candidate_id: cid.to_string(),
            expected_revision: rev,
            idempotency_key: key.to_string(),
            request_hash: format!("hash-{key}"),
            new_todo_id: format!("todo_{key}"),
            receipt_id: format!("tmut_{key}"),
            confirmed_due: None,
            confirmed_owner_ref: None,
            confirmed_title: None,
            confirmed_body: None,
            now: Utc::now(),
        }
    }

    #[tokio::test]
    async fn ingest_dedupes_identical_delivery() {
        let s = storage();
        let c = candidate("tcand_1", "ns-1", "sha256:aaa");
        let first = s
            .ingest_candidate(IngestCandidateRequest {
                candidate: c.clone(),
            })
            .await
            .unwrap();
        assert!(first.created);
        // Same dedupe_key, different id: returns the existing row, no new insert.
        let mut c2 = candidate("tcand_2", "ns-1", "sha256:aaa");
        c2.dedupe_key = c.dedupe_key.clone();
        let second = s
            .ingest_candidate(IngestCandidateRequest { candidate: c2 })
            .await
            .unwrap();
        assert!(!second.created);
        assert_eq!(second.candidate.id, "tcand_1");
    }

    #[tokio::test]
    async fn confirm_creates_one_todo_clears_content_and_replays() {
        let s = storage();
        let c = candidate("tcand_1", "ns-1", "sha256:aaa");
        s.ingest_candidate(IngestCandidateRequest { candidate: c })
            .await
            .unwrap();

        let out = s
            .confirm_candidate(confirm_req("tcand_1", 1, "k1"))
            .await
            .unwrap();
        let TaskOutcome::Confirmed {
            todo_id, revision, ..
        } = out
        else {
            panic!("expected Confirmed, got {out:?}");
        };
        assert_eq!(todo_id, "todo_k1");
        assert_eq!(revision, 2);

        // Candidate content is cleared; state confirmed.
        let cand = s.get_candidate("tcand_1").await.unwrap().unwrap();
        assert_eq!(cand.state, CandidateState::Confirmed);
        assert!(cand.title.is_none() && cand.body.is_none());

        // Exactly one to-do exists, carrying the copied title.
        let todos = s.list_todos(TodoFilter::default()).await.unwrap();
        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0].title, "Follow up on the report");

        // Replaying the same key + hash returns the original result, no 2nd to-do.
        let replay = s
            .confirm_candidate(confirm_req("tcand_1", 1, "k1"))
            .await
            .unwrap();
        assert!(matches!(replay, TaskOutcome::Confirmed { revision: 2, .. }));
        assert_eq!(s.list_todos(TodoFilter::default()).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn confirm_second_key_after_winner_is_already_transitioned() {
        let s = storage();
        s.ingest_candidate(IngestCandidateRequest {
            candidate: candidate("tcand_1", "ns-1", "sha256:aaa"),
        })
        .await
        .unwrap();
        s.confirm_candidate(confirm_req("tcand_1", 1, "k1"))
            .await
            .unwrap();
        // A different key racing after the win: candidate is already CONFIRMED.
        let out = s
            .confirm_candidate(confirm_req("tcand_1", 1, "k2"))
            .await
            .unwrap();
        assert!(matches!(out, TaskOutcome::AlreadyTransitioned { .. }));
        assert_eq!(s.list_todos(TodoFilter::default()).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn confirm_wrong_revision_conflicts() {
        let s = storage();
        s.ingest_candidate(IngestCandidateRequest {
            candidate: candidate("tcand_1", "ns-1", "sha256:aaa"),
        })
        .await
        .unwrap();
        let out = s
            .confirm_candidate(confirm_req("tcand_1", 99, "k1"))
            .await
            .unwrap();
        assert!(matches!(out, TaskOutcome::RevisionConflict));
    }

    #[tokio::test]
    async fn idempotency_mismatch_on_reused_key_different_hash() {
        let s = storage();
        s.ingest_candidate(IngestCandidateRequest {
            candidate: candidate("tcand_1", "ns-1", "sha256:aaa"),
        })
        .await
        .unwrap();
        s.confirm_candidate(confirm_req("tcand_1", 1, "k1"))
            .await
            .unwrap();
        let mut req = confirm_req("tcand_1", 1, "k1");
        req.request_hash = "different-hash".to_string();
        let out = s.confirm_candidate(req).await.unwrap();
        assert!(matches!(out, TaskOutcome::IdempotencyMismatch));
    }

    #[tokio::test]
    async fn dismiss_clears_content_and_is_terminal() {
        let s = storage();
        s.ingest_candidate(IngestCandidateRequest {
            candidate: candidate("tcand_1", "ns-1", "sha256:aaa"),
        })
        .await
        .unwrap();
        let out = s
            .dismiss_candidate(DismissCandidateRequest {
                candidate_id: "tcand_1".to_string(),
                expected_revision: 1,
                idempotency_key: "d1".to_string(),
                request_hash: "h".to_string(),
                receipt_id: "tmut_d1".to_string(),
                reason: Some("not_relevant".to_string()),
                now: Utc::now(),
            })
            .await
            .unwrap();
        assert!(matches!(out, TaskOutcome::Dismissed { revision: 2, .. }));
        let cand = s.get_candidate("tcand_1").await.unwrap().unwrap();
        assert_eq!(cand.state, CandidateState::Dismissed);
        assert!(cand.title.is_none());
        // Confirming a dismissed candidate is an idempotent no-op, never a to-do.
        let out = s
            .confirm_candidate(confirm_req("tcand_1", 2, "k1"))
            .await
            .unwrap();
        assert!(matches!(out, TaskOutcome::AlreadyTransitioned { .. }));
        assert!(s
            .list_todos(TodoFilter::default())
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn todo_transition_matrix_and_conflicts() {
        let s = storage();
        s.ingest_candidate(IngestCandidateRequest {
            candidate: candidate("tcand_1", "ns-1", "sha256:aaa"),
        })
        .await
        .unwrap();
        s.confirm_candidate(confirm_req("tcand_1", 1, "k1"))
            .await
            .unwrap();
        let tid = "todo_k1";

        let mk = |target: TodoState, rev: i64, key: &str| TransitionTodoRequest {
            todo_id: tid.to_string(),
            target,
            expected_revision: rev,
            idempotency_key: key.to_string(),
            request_hash: format!("h-{key}"),
            receipt_id: format!("tmut_{key}"),
            now: Utc::now(),
        };

        // confirmed(rev1) -> in_progress(rev2)
        let out = s
            .transition_todo(mk(TodoState::InProgress, 1, "t1"))
            .await
            .unwrap();
        assert!(matches!(
            out,
            TaskOutcome::Transitioned {
                state: TodoState::InProgress,
                revision: 2,
                ..
            }
        ));
        // wrong revision now conflicts
        let out = s
            .transition_todo(mk(TodoState::Done, 1, "t2"))
            .await
            .unwrap();
        assert!(matches!(out, TaskOutcome::RevisionConflict));
        // in_progress(rev2) -> done(rev3)
        let out = s
            .transition_todo(mk(TodoState::Done, 2, "t3"))
            .await
            .unwrap();
        assert!(matches!(
            out,
            TaskOutcome::Transitioned {
                state: TodoState::Done,
                revision: 3,
                ..
            }
        ));
        // done is terminal: transitioning to same state is an idempotent no-op
        let out = s
            .transition_todo(mk(TodoState::Done, 3, "t4"))
            .await
            .unwrap();
        assert!(matches!(out, TaskOutcome::AlreadyTransitioned { .. }));
    }

    #[tokio::test]
    async fn reconcile_expires_proposed_past_ttl_and_clears_content() {
        let s = storage();
        let mut c = candidate("tcand_1", "ns-1", "sha256:aaa");
        c.expires_at = Utc::now() - chrono::Duration::hours(1); // already past TTL
        s.ingest_candidate(IngestCandidateRequest { candidate: c })
            .await
            .unwrap();
        let report = s.reconcile_tasks(Utc::now()).await.unwrap();
        assert_eq!(report.expired_candidates, 1);
        assert_eq!(report.integrity_errors, 0);
        let cand = s.get_candidate("tcand_1").await.unwrap().unwrap();
        assert_eq!(cand.state, CandidateState::Expired);
        assert!(cand.title.is_none());
    }

    #[tokio::test]
    async fn full_erasure_removes_task_tables() {
        let s = storage();
        s.ingest_candidate(IngestCandidateRequest {
            candidate: candidate("tcand_1", "ns-1", "sha256:aaa"),
        })
        .await
        .unwrap();
        s.confirm_candidate(confirm_req("tcand_1", 1, "k1"))
            .await
            .unwrap();
        s.delete_all_data().unwrap();
        assert!(s.get_candidate("tcand_1").await.unwrap().is_none());
        assert!(s
            .list_todos(TodoFilter::default())
            .await
            .unwrap()
            .is_empty());
    }
}
