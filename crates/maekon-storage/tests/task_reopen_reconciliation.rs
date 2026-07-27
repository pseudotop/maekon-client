//! Two-process reopen + restart-reconciliation integration tests for the durable
//! task lifecycle (ADR-028 §7, #8577).
//!
//! These exercise the on-disk persistence + reopen boundary that in-memory unit
//! tests cannot: state and provenance must survive a close/reopen, expired
//! candidates reconcile on startup without inventing user intent, and a confirmed
//! to-do is never duplicated or lost across the boundary.

use chrono::{Duration, Utc};
use maekon_core::models::task::{
    compute_dedupe_key, CandidateState, SourceKind, SourceLifecycle, TaskCandidate, TaskOutcome,
    TaskSourceRef, TodoState,
};
use maekon_core::ports::task_store::{
    ConfirmCandidateRequest, IngestCandidateRequest, TaskCommandPort, TaskQueryPort, TodoFilter,
};
use maekon_storage::sqlite::SqliteStorage;
use tempfile::TempDir;

fn candidate(id: &str, ns: &str, hash: &str, expires_in: Duration) -> TaskCandidate {
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
        dedupe_namespace: ns.to_string(),
        content_hash: hash.to_string(),
        lifecycle: SourceLifecycle::Active,
        source_outcome: None,
    };
    TaskCandidate {
        id: id.to_string(),
        state: CandidateState::Proposed,
        title: Some("Prepare the weekly report".to_string()),
        body: Some("sanitized body".to_string()),
        proposed_due: None,
        proposed_owner_ref: None,
        expires_at: now + expires_in,
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
async fn confirmed_todo_and_provenance_survive_reopen() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("tasks.db");

    // First "process": ingest + confirm a candidate.
    {
        let s = SqliteStorage::open(&path, 30, None).unwrap();
        s.ingest_candidate(IngestCandidateRequest {
            candidate: candidate("tcand_1", "ns-1", "sha256:aaa", Duration::days(1)),
        })
        .await
        .unwrap();
        let out = s
            .confirm_candidate(confirm_req("tcand_1", 1, "k1"))
            .await
            .unwrap();
        assert!(matches!(out, TaskOutcome::Confirmed { .. }));
    }

    // Second "process": reopen the same file. The confirmed to-do + minimized
    // provenance must be intact, and the candidate content cleared.
    {
        let s = SqliteStorage::open(&path, 30, None).unwrap();
        let todos = s.list_todos(TodoFilter::default()).await.unwrap();
        assert_eq!(todos.len(), 1, "exactly one to-do survives reopen");
        assert_eq!(todos[0].state, TodoState::Confirmed);
        assert_eq!(todos[0].title, "Prepare the weekly report");

        let cand = s.get_candidate("tcand_1").await.unwrap().unwrap();
        assert_eq!(cand.state, CandidateState::Confirmed);
        assert!(cand.title.is_none(), "confirmed candidate content cleared");
        // Provenance kind survives even though content was cleared.
        assert_eq!(cand.source_ref.source_kind, SourceKind::LocalCurrentScene);

        // Replaying the confirm after reopen is an idempotent no-op — never a
        // second to-do (ADR-028 §7 restart replay).
        let replay = s
            .confirm_candidate(confirm_req("tcand_1", 1, "k1"))
            .await
            .unwrap();
        assert!(matches!(replay, TaskOutcome::Confirmed { .. }));
        assert_eq!(s.list_todos(TodoFilter::default()).await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn reconciliation_expires_past_ttl_candidate_after_reopen() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("tasks.db");

    // First process: ingest a candidate whose TTL is already in the past.
    {
        let s = SqliteStorage::open(&path, 30, None).unwrap();
        s.ingest_candidate(IngestCandidateRequest {
            candidate: candidate("tcand_stale", "ns-2", "sha256:bbb", Duration::hours(-1)),
        })
        .await
        .unwrap();
        // It is still proposed at this point — expiry is decided at reconciliation.
        let cand = s.get_candidate("tcand_stale").await.unwrap().unwrap();
        assert_eq!(cand.state, CandidateState::Proposed);
    }

    // Second process: startup reconciliation expires it and clears its content,
    // without inventing a confirmation.
    {
        let s = SqliteStorage::open(&path, 30, None).unwrap();
        let report = s.reconcile_tasks(Utc::now()).await.unwrap();
        assert_eq!(report.expired_candidates, 1);
        assert_eq!(report.integrity_errors, 0);

        let cand = s.get_candidate("tcand_stale").await.unwrap().unwrap();
        assert_eq!(cand.state, CandidateState::Expired);
        assert!(cand.title.is_none());
        // No to-do was ever synthesized from the expired candidate.
        assert!(s
            .list_todos(TodoFilter::default())
            .await
            .unwrap()
            .is_empty());
    }
}

#[tokio::test]
async fn wall_clock_rollback_does_not_unexpire() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("tasks.db");
    let s = SqliteStorage::open(&path, 30, None).unwrap();
    s.ingest_candidate(IngestCandidateRequest {
        candidate: candidate("tcand_1", "ns-1", "sha256:aaa", Duration::hours(-1)),
    })
    .await
    .unwrap();

    // Reconcile at "now" advances the persisted floor and expires the candidate.
    let now = Utc::now();
    let first = s.reconcile_tasks(now).await.unwrap();
    assert_eq!(first.expired_candidates, 1);

    // A later reconcile with a rolled-back wall clock must not resurrect anything;
    // effective_now = max(rolled_back, persisted_floor) stays at the floor.
    let rolled_back = now - Duration::days(2);
    let second = s.reconcile_tasks(rolled_back).await.unwrap();
    assert_eq!(
        second.expired_candidates, 0,
        "already-expired stays expired"
    );
    assert!(second.reconciled_at >= now, "floor never moves backward");
    let cand = s.get_candidate("tcand_1").await.unwrap().unwrap();
    assert_eq!(cand.state, CandidateState::Expired);
}

/// edit-then-confirm (#8892): when confirming with confirmed_title/body populated,
/// the todo carries the **edited values** rather than the candidate's original,
/// clamped to the bounds (200/2000 chars). This pins down at the storage level the
/// backend override path that was previously verified only via a frontend fake.
#[tokio::test]
async fn confirm_with_edited_title_and_body_persists_the_edit() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("tasks.db");

    let s = SqliteStorage::open(&path, 30, None).unwrap();
    s.ingest_candidate(IngestCandidateRequest {
        candidate: candidate("tcand_e", "ns_e", "he", Duration::days(7)),
    })
    .await
    .unwrap();

    // Confirm with edited values.
    let mut req = confirm_req("tcand_e", 1, "ke");
    req.confirmed_title = Some("Edited next step".to_string());
    req.confirmed_body = Some("Edited body text".to_string());
    let out = s.confirm_candidate(req).await.unwrap();
    assert!(matches!(out, TaskOutcome::Confirmed { .. }));

    // The todo carries the edited values, not the candidate's original ("Prepare the weekly report").
    let todos = s.list_todos(TodoFilter::default()).await.unwrap();
    assert_eq!(todos.len(), 1);
    assert_eq!(todos[0].title, "Edited next step");
    assert_eq!(todos[0].body.as_deref(), Some("Edited body text"));
}

/// When confirmed_title exceeds the bound (200 chars) it is clamped, and an empty
/// body override does not overwrite the candidate's original (None or empty → original kept).
#[tokio::test]
async fn confirm_edit_clamps_title_and_ignores_empty_override() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("tasks.db");

    let s = SqliteStorage::open(&path, 30, None).unwrap();
    s.ingest_candidate(IngestCandidateRequest {
        candidate: candidate("tcand_c", "ns_c", "hc", Duration::days(7)),
    })
    .await
    .unwrap();

    let mut req = confirm_req("tcand_c", 1, "kc");
    req.confirmed_title = Some("x".repeat(500)); // Exceeds the 200-char cap.
    req.confirmed_body = None; // No override → keep the candidate's original.
    s.confirm_candidate(req).await.unwrap();

    let todos = s.list_todos(TodoFilter::default()).await.unwrap();
    assert_eq!(todos.len(), 1);
    assert!(
        todos[0].title.chars().count() <= 200,
        "title clamped to bound"
    );
    // Since there is no body override, the candidate's sanitized body is kept.
    assert_eq!(todos[0].body.as_deref(), Some("sanitized body"));
}
