//! Context source sync contract tests — synthetic fake connector (ADR-030 §9, #8587).
//!
//! The #8587 acceptance criteria require the fake-server tests to cover
//! pagination, duplicate, reordering, retry, crash, and cancellation. Here we
//! combine a real store (`SqliteStorage`) with a scripted fake `ContextSourcePort`
//! to verify the orchestrator contract.
//!
//! **Note**: this is a synthetic fixture. It does not imply real provider
//! (Google/Microsoft) support — the real-account smoke of #8590 owns that boundary.

use std::sync::Mutex;

use async_trait::async_trait;
use chrono::Utc;
use maekon_core::error::CoreError;
use maekon_core::models::work_context::{
    ContextSourceDescriptor, ContextSourcePage, ContextSourceRecord, DataClassification, Lifecycle,
    RevisionModel, SourceObjectIdentity, WorkContextKind,
};
use maekon_core::ports::work_context::{
    AccountStatus, ContextSourcePort, SourceHealth, SyncOutcome, SyncRequest, WorkContextStorePort,
};
use maekon_core::services::context_sync::{run_sync, CancelFlag, StopReason, SyncPlan};
use maekon_storage::sqlite::SqliteStorage;
use tempfile::TempDir;

const DEDUPE_KEY: &[u8] = &[7u8; 32];

fn record(remote_id: &str, order: i64, content: &str, lifecycle: Lifecycle) -> ContextSourceRecord {
    ContextSourceRecord {
        identity: SourceObjectIdentity {
            extension_id: "com.maekon.calendar".into(),
            install_id: "inst_1".into(),
            account_subject_ref: "acct_1".into(),
            remote_type: "event".into(),
            remote_id: remote_id.into(),
        },
        kind: WorkContextKind::Meeting,
        classification: DataClassification::Internal,
        remote_revision: Some(order.to_string()),
        etag: None,
        source_order: Some(order),
        content_hash: format!("hash_{content}"),
        occurred_at: None,
        source_updated_at: None,
        observed_at: Utc::now(),
        relations: vec![],
        lifecycle,
        raw_payload_handle: None,
    }
}

/// A scripted fake connector. Each `sync` call returns the next result in the queue.
struct FakeConnector {
    /// Queue of results to return (a page or an unhealthy state).
    scripted: Mutex<Vec<SyncOutcome>>,
    /// Number of observed sync calls (for retry/replay verification).
    calls: Mutex<u32>,
}

impl FakeConnector {
    fn new(script: Vec<SyncOutcome>) -> Self {
        Self {
            scripted: Mutex::new(script),
            calls: Mutex::new(0),
        }
    }

    fn call_count(&self) -> u32 {
        *self.calls.lock().unwrap()
    }
}

fn page(records: Vec<ContextSourceRecord>, next: Option<&str>, has_more: bool) -> SyncOutcome {
    SyncOutcome::Page(ContextSourcePage {
        records,
        next_cursor: next.map(String::from),
        has_more,
        page_digest: "digest".into(),
        access_epoch_id: 1,
    })
}

#[async_trait]
impl ContextSourcePort for FakeConnector {
    async fn discover(&self) -> Result<Vec<ContextSourceDescriptor>, CoreError> {
        Ok(vec![ContextSourceDescriptor {
            extension_id: "com.maekon.calendar".into(),
            install_id: "inst_1".into(),
            remote_type: "event".into(),
            revision_model: RevisionModel::Monotonic,
            has_explicit_delete_signal: true,
            supports_undelete: false,
            max_page_records: 100,
        }])
    }

    async fn account_status(&self, _: &str, _: &str) -> Result<AccountStatus, CoreError> {
        Ok(AccountStatus::Connected)
    }

    async fn sync(&self, _request: SyncRequest) -> Result<SyncOutcome, CoreError> {
        *self.calls.lock().unwrap() += 1;
        let mut q = self.scripted.lock().unwrap();
        if q.is_empty() {
            // When the script is exhausted, an empty drained page.
            return Ok(page(vec![], None, false));
        }
        Ok(q.remove(0))
    }

    async fn health(&self, _: &str) -> Result<SourceHealth, CoreError> {
        Ok(SourceHealth::Healthy)
    }

    async fn revoke(&self, _: &str, _: &str) -> Result<(), CoreError> {
        Ok(())
    }
}

/// A connector that cannot deterministically handle deletion/access loss — it is
/// `content_hash_only` with no explicit delete signal, so it cannot be advertised
/// (§5, revision I6). If `sync` is called it panics, proving "it was refused
/// before collection".
struct NonAdvertisableConnector;

#[async_trait]
impl ContextSourcePort for NonAdvertisableConnector {
    async fn discover(&self) -> Result<Vec<ContextSourceDescriptor>, CoreError> {
        Ok(vec![ContextSourceDescriptor {
            extension_id: "com.maekon.calendar".into(),
            install_id: "inst_1".into(),
            remote_type: "event".into(),
            revision_model: RevisionModel::ContentHashOnly,
            has_explicit_delete_signal: false,
            supports_undelete: false,
            max_page_records: 100,
        }])
    }

    async fn account_status(&self, _: &str, _: &str) -> Result<AccountStatus, CoreError> {
        Ok(AccountStatus::Connected)
    }

    async fn sync(&self, _request: SyncRequest) -> Result<SyncOutcome, CoreError> {
        panic!("a non-advertisable connector must never be synced (I6)");
    }

    async fn health(&self, _: &str) -> Result<SourceHealth, CoreError> {
        Ok(SourceHealth::Healthy)
    }

    async fn revoke(&self, _: &str, _: &str) -> Result<(), CoreError> {
        Ok(())
    }
}

async fn store() -> (TempDir, SqliteStorage) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("wctx.db");
    let s = SqliteStorage::open(&path, 30, None).unwrap();
    s.begin_access_epoch("inst_1", "acct_1", Utc::now())
        .await
        .unwrap();
    (dir, s)
}

fn plan(max_pages: u32) -> SyncPlan {
    SyncPlan {
        install_id: "inst_1".into(),
        account_subject_ref: "acct_1".into(),
        access_epoch_id: 1,
        ingest_run_id: "run_1".into(),
        max_records: 100,
        max_pages,
        dedupe_key: DEDUPE_KEY.to_vec(),
        revision_model: RevisionModel::Monotonic,
    }
}

#[tokio::test]
async fn paginates_until_drained() {
    let (_d, s) = store().await;
    let conn = FakeConnector::new(vec![
        page(
            vec![record("a", 1, "a", Lifecycle::Active)],
            Some("c1"),
            true,
        ),
        page(
            vec![record("b", 1, "b", Lifecycle::Active)],
            Some("c2"),
            true,
        ),
        page(vec![record("c", 1, "c", Lifecycle::Active)], None, false),
    ]);
    let summary = run_sync(&conn, &s, &plan(10), &CancelFlag::new(), Utc::now())
        .await
        .unwrap();
    assert_eq!(summary.stop_reason, StopReason::Drained);
    assert_eq!(summary.pages_committed, 3);
    assert_eq!(summary.records_seen, 3);
    assert_eq!(s.list_projectable(100).await.unwrap().len(), 3);
}

#[tokio::test]
async fn duplicate_page_delivery_does_not_double_insert() {
    let (_d, s) = store().await;
    // Re-deliver the same record across two pages (at-least-once).
    let conn = FakeConnector::new(vec![
        page(
            vec![record("a", 1, "a", Lifecycle::Active)],
            Some("c1"),
            true,
        ),
        page(vec![record("a", 1, "a", Lifecycle::Active)], None, false),
    ]);
    let summary = run_sync(&conn, &s, &plan(10), &CancelFlag::new(), Utc::now())
        .await
        .unwrap();
    assert_eq!(summary.stop_reason, StopReason::Drained);
    // Observed twice but only one projection.
    assert_eq!(s.list_projectable(100).await.unwrap().len(), 1);
}

#[tokio::test]
async fn reordered_delivery_keeps_the_higher_revision() {
    let (_d, s) = store().await;
    // Revision 2 arrives first, revision 1 (stale) arrives later.
    let conn = FakeConnector::new(vec![
        page(
            vec![record("a", 2, "v2", Lifecycle::Active)],
            Some("c1"),
            true,
        ),
        page(vec![record("a", 1, "v1", Lifecycle::Active)], None, false),
    ]);
    run_sync(&conn, &s, &plan(10), &CancelFlag::new(), Utc::now())
        .await
        .unwrap();
    // A stale revision does not overwrite the latest — delivery order does not determine visibility.
    let proj = s.list_projectable(100).await.unwrap();
    assert_eq!(proj.len(), 1);
    assert_eq!(proj[0].source_order, Some(2));
}

#[tokio::test]
async fn rate_limited_surfaces_typed_health_without_raw_body() {
    let (_d, s) = store().await;
    let conn = FakeConnector::new(vec![
        page(
            vec![record("a", 1, "a", Lifecycle::Active)],
            Some("c1"),
            true,
        ),
        SyncOutcome::Unhealthy(SourceHealth::RateLimited {
            retry_after_secs: Some(30),
        }),
    ]);
    let summary = run_sync(&conn, &s, &plan(10), &CancelFlag::new(), Utc::now())
        .await
        .unwrap();
    match summary.stop_reason {
        StopReason::Unhealthy(SourceHealth::RateLimited { retry_after_secs }) => {
            assert_eq!(retry_after_secs, Some(30));
        }
        other => panic!("expected rate-limited health, got {other:?}"),
    }
    // The first page was committed.
    assert_eq!(summary.pages_committed, 1);
}

#[tokio::test]
async fn unauthorized_health_should_not_retry() {
    // An auth failure is useless to retry until the user intervenes — prevents rate-limit exhaustion.
    assert!(!SourceHealth::Unauthorized.should_retry());
    assert!(SourceHealth::Unauthorized.needs_user_action());
    assert!(SourceHealth::RateLimited {
        retry_after_secs: None
    }
    .should_retry());
}

#[tokio::test]
async fn crash_before_commit_replays_from_the_same_cursor() {
    let (_d, s) = store().await;
    // First run: commits only one page and stops on budget exhaustion (a stand-in for a crash).
    let conn1 = FakeConnector::new(vec![page(
        vec![record("a", 1, "a", Lifecycle::Active)],
        Some("c1"),
        true,
    )]);
    let s1 = run_sync(&conn1, &s, &plan(1), &CancelFlag::new(), Utc::now())
        .await
        .unwrap();
    assert_eq!(s1.stop_reason, StopReason::PageBudgetReached);
    assert_eq!(s1.pages_committed, 1);

    // Second run: resumes from cursor c1. Even if the same record is replayed, no duplication.
    let conn2 = FakeConnector::new(vec![
        page(
            vec![record("a", 1, "a", Lifecycle::Active)],
            Some("c1"),
            true,
        ), // replay
        page(vec![record("b", 1, "b", Lifecycle::Active)], None, false),
    ]);
    let s2 = run_sync(&conn2, &s, &plan(10), &CancelFlag::new(), Utc::now())
        .await
        .unwrap();
    assert_eq!(s2.stop_reason, StopReason::Drained);
    // `a` stays one even when replayed, `b` is new — 2 in total.
    assert_eq!(s.list_projectable(100).await.unwrap().len(), 2);
}

#[tokio::test]
async fn cancellation_stops_at_the_next_page_boundary() {
    let (_d, s) = store().await;
    let cancel = CancelFlag::new();
    // Set cancellation in advance — stops immediately at the first page boundary.
    cancel.cancel();
    let conn = FakeConnector::new(vec![page(
        vec![record("a", 1, "a", Lifecycle::Active)],
        Some("c1"),
        true,
    )]);
    let summary = run_sync(&conn, &s, &plan(10), &cancel, Utc::now())
        .await
        .unwrap();
    assert_eq!(summary.stop_reason, StopReason::Cancelled);
    assert_eq!(summary.pages_committed, 0);
    // The connector was never called — cancellation comes before the network.
    assert_eq!(conn.call_count(), 0);
    assert_eq!(s.list_projectable(100).await.unwrap().len(), 0);
}

#[tokio::test]
async fn page_budget_bounds_unlimited_collection() {
    let (_d, s) = store().await;
    // The provider gives has_more=true forever — without a budget, an infinite loop.
    let conn = FakeConnector::new(vec![
        page(
            vec![record("a", 1, "a", Lifecycle::Active)],
            Some("c1"),
            true,
        ),
        page(
            vec![record("b", 1, "b", Lifecycle::Active)],
            Some("c2"),
            true,
        ),
        page(
            vec![record("c", 1, "c", Lifecycle::Active)],
            Some("c3"),
            true,
        ),
    ]);
    let summary = run_sync(&conn, &s, &plan(2), &CancelFlag::new(), Utc::now())
        .await
        .unwrap();
    // Stops at budget 2.
    assert_eq!(summary.stop_reason, StopReason::PageBudgetReached);
    assert_eq!(summary.pages_committed, 2);
}

#[tokio::test]
async fn non_advertisable_connector_is_refused_before_any_sync() {
    // I6/§5: a non-advertisable connector is refused immediately after discover and
    // pulls no pages at all — proving `is_advertisable` is a real gate, not a dead pure function.
    let (_d, s) = store().await;
    let conn = NonAdvertisableConnector;
    let summary = run_sync(&conn, &s, &plan(10), &CancelFlag::new(), Utc::now())
        .await
        .unwrap();
    assert_eq!(summary.stop_reason, StopReason::NotAdvertisable);
    assert_eq!(summary.pages_committed, 0);
    assert_eq!(summary.records_seen, 0);
    assert_eq!(s.list_projectable(100).await.unwrap().len(), 0);
}

#[tokio::test]
async fn delete_tombstone_removes_a_previously_active_record() {
    let (_d, s) = store().await;
    let conn = FakeConnector::new(vec![
        page(
            vec![record("a", 1, "a", Lifecycle::Active)],
            Some("c1"),
            true,
        ),
        page(
            vec![record("a", 2, "gone", Lifecycle::Deleted)],
            None,
            false,
        ),
    ]);
    run_sync(&conn, &s, &plan(10), &CancelFlag::new(), Utc::now())
        .await
        .unwrap();
    // A deleted object is no longer projected.
    assert_eq!(s.list_projectable(100).await.unwrap().len(), 0);
}
