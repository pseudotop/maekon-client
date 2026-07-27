//! Google Calendar connector fake-server contract tests (MK-EXT-01.C01 #8590).
//!
//! Fakes `events.list` with an HTTP-level fake Google Calendar server (`mockito`)
//! and drives the real `SqliteStorage` + real `HttpCalendarEventsApi` + `run_sync`
//! orchestrator to verify the fake-server checklist of the #8590 acceptance criteria:
//!
//! pagination · duplicate · reordering · retry(Retry-After) · crash-before-commit ·
//! cancellation · syncToken expiry(410)→CursorExpired→full resync ·
//! recurring/timezone/all-day/moved-occurrence identity · sensitive-field exclusion ·
//! 401/403/429/5xx typed health(+no body/token logging) · restart(2-process) incremental resume ·
//! fail-closed after revocation · refined-projection persistence(reviewable + source/freshness).
//!
//! **NOTE (HONESTY)**: This is a synthetic fixture. A real Google account smoke test
//! (connect · sync · revoke · delete · rate-limit) is a human-performed release evidence
//! gate and cannot be substituted here. Passing this test does not mean "Google Calendar support".

// The mockito `ServerGuard` must stay alive while the connector sends HTTP requests
// (an early drop brings the mock server down and the request fails). The
// significant_drop_tightening "drop earlier" suggestion breaks the tests here, so we allow it file-wide.
#![allow(clippy::significant_drop_tightening)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use mockito::Matcher;
use serde_json::{json, Value};
use tempfile::TempDir;

use maekon_core::error::CoreError;
use maekon_core::models::work_context::{
    compute_revision_fingerprint, compute_source_object_key, RevisionModel, WorkContextEnvelope,
    WORK_CONTEXT_SCHEMA_VERSION,
};
use maekon_core::ports::oauth::{
    OAuthConnectionStatus, OAuthFlowHandle, OAuthFlowStatus, OAuthPort, RefreshResult,
};
use maekon_core::ports::work_context::{
    AccountStatus, CommitContent, CommitOutcome, CommitPageRequest, ContextSourcePort,
    CursorAdvance, SourceHealth, SyncOutcome, SyncRequest, WorkContextStorePort,
};
use maekon_core::services::context_sync::{run_sync, CancelFlag, StopReason, SyncPlan};
use maekon_network::integration::google_calendar::{
    event_to_commit_content, event_to_record, GoogleCalendarConfig, GoogleCalendarConnector,
    GoogleCalendarMapCtx, GoogleEvent, HttpCalendarEventsApi, GOOGLE_CALENDAR_EXTENSION_ID,
    GOOGLE_CALENDAR_PROVIDER_ID, GOOGLE_CALENDAR_REMOTE_TYPE,
};
use maekon_storage::sqlite::SqliteStorage;

const DEDUPE_KEY: &[u8] = &[7u8; 32];
const INSTALL: &str = "inst_1";
const ACCOUNT: &str = "acct_1";
const EPOCH: i64 = 1;

// ── OAuth test double ────────────────────────────────────────────────────

/// Minimal `OAuthPort` double. Models only token presence/absence and revocation.
struct FakeOAuthPort {
    token: Mutex<Option<String>>,
    revoked: AtomicBool,
}

impl FakeOAuthPort {
    fn connected(token: &str) -> Arc<Self> {
        Arc::new(Self {
            token: Mutex::new(Some(token.to_string())),
            revoked: AtomicBool::new(false),
        })
    }
}

#[async_trait]
impl OAuthPort for FakeOAuthPort {
    async fn start_flow(&self, _provider_id: &str) -> Result<OAuthFlowHandle, CoreError> {
        Ok(OAuthFlowHandle {
            flow_id: "flow".into(),
            auth_url: "https://accounts.google.com/o/oauth2/v2/auth".into(),
        })
    }
    async fn flow_status(&self, _flow_id: &str) -> Result<OAuthFlowStatus, CoreError> {
        Ok(OAuthFlowStatus::Completed)
    }
    async fn cancel_flow(&self, _flow_id: &str) -> Result<(), CoreError> {
        Ok(())
    }
    async fn get_access_token(&self, _provider_id: &str) -> Result<Option<String>, CoreError> {
        Ok(self.token.lock().unwrap().clone())
    }
    async fn revoke(&self, _provider_id: &str) -> Result<(), CoreError> {
        *self.token.lock().unwrap() = None;
        self.revoked.store(true, Ordering::SeqCst);
        Ok(())
    }
    async fn connection_status(
        &self,
        provider_id: &str,
    ) -> Result<OAuthConnectionStatus, CoreError> {
        Ok(OAuthConnectionStatus {
            provider_id: provider_id.to_string(),
            connected: self.token.lock().unwrap().is_some(),
            expires_at: None,
            scopes: vec!["https://www.googleapis.com/auth/calendar.events.readonly".to_string()],
            api_base_url: Some("https://www.googleapis.com/calendar/v3".into()),
            has_refresh_token: true,
        })
    }
    async fn refresh_access_token(
        &self,
        _provider_id: &str,
        _min_valid_for_secs: i64,
    ) -> Result<RefreshResult, CoreError> {
        Ok(RefreshResult::AlreadyFresh {
            expires_at: String::new(),
        })
    }
}

// ── Fixture helpers ──────────────────────────────────────────────────────────

fn connector(base_url: &str, oauth: Arc<FakeOAuthPort>) -> GoogleCalendarConnector {
    let oauth_dyn: Arc<dyn OAuthPort> = oauth.clone();
    let api = Arc::new(
        HttpCalendarEventsApi::new(base_url.to_string(), oauth_dyn, GOOGLE_CALENDAR_PROVIDER_ID)
            .expect("hardened calendar client builds"),
    );
    GoogleCalendarConnector::new(api, oauth, GoogleCalendarConfig::new(INSTALL))
}

async fn store() -> (TempDir, SqliteStorage) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("wctx.db");
    let s = SqliteStorage::open(&path, 30, None).unwrap();
    s.begin_access_epoch(INSTALL, ACCOUNT, Utc::now())
        .await
        .unwrap();
    (dir, s)
}

fn plan(max_pages: u32) -> SyncPlan {
    SyncPlan {
        install_id: INSTALL.into(),
        account_subject_ref: ACCOUNT.into(),
        access_epoch_id: EPOCH,
        ingest_run_id: "run_1".into(),
        max_records: 100,
        max_pages,
        dedupe_key: DEDUPE_KEY.to_vec(),
        revision_model: RevisionModel::Monotonic,
    }
}

/// Timed-event JSON.
fn event_json(id: &str, updated: &str, summary: &str) -> Value {
    json!({
        "id": id,
        "status": "confirmed",
        "etag": format!("\"etag-{id}-{updated}\""),
        "summary": summary,
        "updated": updated,
        "start": {"dateTime": "2026-07-22T10:00:00+09:00"},
        "end": {"dateTime": "2026-07-22T11:00:00+09:00"}
    })
}

fn page_body(items: Vec<Value>, next_page: Option<&str>, next_sync: Option<&str>) -> String {
    let mut m = serde_json::Map::new();
    m.insert("items".into(), Value::Array(items));
    if let Some(p) = next_page {
        m.insert("nextPageToken".into(), json!(p));
    }
    if let Some(s) = next_sync {
        m.insert("nextSyncToken".into(), json!(s));
    }
    Value::Object(m).to_string()
}

/// Registers a successful (2xx) events.list mock. `query` is a regex matching the raw query.
async fn mock_ok(server: &mut mockito::ServerGuard, query: &str, body: String) -> mockito::Mock {
    server
        .mock("GET", "/calendars/primary/events")
        .match_query(Matcher::Regex(query.to_string()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(body)
        .create_async()
        .await
}

// ── run_sync contract tests (mockito + real store) ────────────────────────────

#[tokio::test]
async fn paginates_multi_page_until_sync_token_drains() {
    let mut server = mockito::Server::new_async().await;
    let oauth = FakeOAuthPort::connected("tok");
    let conn = connector(&server.url(), oauth);
    let (_d, s) = store().await;

    let _m0 = mock_ok(
        &mut server,
        "timeMin",
        page_body(
            vec![event_json("a", "2026-07-22T09:00:00Z", "A")],
            Some("p1"),
            None,
        ),
    )
    .await;
    let _m1 = mock_ok(
        &mut server,
        "pageToken=p1",
        page_body(
            vec![event_json("b", "2026-07-22T09:00:00Z", "B")],
            Some("p2"),
            None,
        ),
    )
    .await;
    let _m2 = mock_ok(
        &mut server,
        "pageToken=p2",
        page_body(
            vec![event_json("c", "2026-07-22T09:00:00Z", "C")],
            None,
            Some("s1"),
        ),
    )
    .await;

    let summary = run_sync(&conn, &s, &plan(10), &CancelFlag::new(), Utc::now())
        .await
        .unwrap();
    assert_eq!(summary.stop_reason, StopReason::Drained);
    assert_eq!(summary.pages_committed, 3);
    assert_eq!(summary.records_seen, 3);
    assert_eq!(s.list_projectable(100).await.unwrap().len(), 3);
    // The last page's syncToken is stored as the incremental cursor.
    let cur = s.get_cursor(INSTALL, ACCOUNT).await.unwrap().unwrap();
    assert_eq!(cur.cursor.as_deref(), Some("sync:s1"));
}

#[tokio::test]
async fn duplicate_page_delivery_does_not_double_insert() {
    let mut server = mockito::Server::new_async().await;
    let conn = connector(&server.url(), FakeOAuthPort::connected("tok"));
    let (_d, s) = store().await;

    let _m0 = mock_ok(
        &mut server,
        "timeMin",
        page_body(
            vec![event_json("a", "2026-07-22T09:00:00Z", "A")],
            Some("p1"),
            None,
        ),
    )
    .await;
    // Re-send the same record a on the next page (at-least-once).
    let _m1 = mock_ok(
        &mut server,
        "pageToken=p1",
        page_body(
            vec![event_json("a", "2026-07-22T09:00:00Z", "A")],
            None,
            Some("s1"),
        ),
    )
    .await;

    run_sync(&conn, &s, &plan(10), &CancelFlag::new(), Utc::now())
        .await
        .unwrap();
    // Observed twice but projected once (the local uniqueness key makes replay idempotent).
    assert_eq!(s.list_projectable(100).await.unwrap().len(), 1);
}

#[tokio::test]
async fn reordered_delivery_keeps_higher_updated_revision() {
    let mut server = mockito::Server::new_async().await;
    let conn = connector(&server.url(), FakeOAuthPort::connected("tok"));
    let (_d, s) = store().await;

    // The higher revision (newer updated) arrives first, the stale revision (older updated) later.
    let high = "2026-07-22T09:00:00Z";
    let low = "2026-07-20T09:00:00Z";
    let _m0 = mock_ok(
        &mut server,
        "timeMin",
        page_body(vec![event_json("a", high, "A-new")], Some("p1"), None),
    )
    .await;
    let _m1 = mock_ok(
        &mut server,
        "pageToken=p1",
        page_body(vec![event_json("a", low, "A-old")], None, Some("s1")),
    )
    .await;

    run_sync(&conn, &s, &plan(10), &CancelFlag::new(), Utc::now())
        .await
        .unwrap();
    let proj = s.list_projectable(100).await.unwrap();
    assert_eq!(proj.len(), 1);
    // The stale revision does not overwrite the newest one.
    let expected = DateTime::parse_from_rfc3339(high)
        .unwrap()
        .timestamp_millis();
    assert_eq!(proj[0].source_order, Some(expected));
}

#[tokio::test]
async fn cancelled_event_tombstones_previously_active_record() {
    let mut server = mockito::Server::new_async().await;
    let conn = connector(&server.url(), FakeOAuthPort::connected("tok"));
    let (_d, s) = store().await;

    let _m0 = mock_ok(
        &mut server,
        "timeMin",
        page_body(
            vec![event_json("a", "2026-07-20T09:00:00Z", "A")],
            Some("p1"),
            None,
        ),
    )
    .await;
    // status=cancelled + higher updated → explicit delete signal → tombstone.
    let cancelled = json!({
        "id": "a", "status": "cancelled", "updated": "2026-07-22T09:00:00Z"
    });
    let _m1 = mock_ok(
        &mut server,
        "pageToken=p1",
        page_body(vec![cancelled], None, Some("s1")),
    )
    .await;

    run_sync(&conn, &s, &plan(10), &CancelFlag::new(), Utc::now())
        .await
        .unwrap();
    // The cancelled object is no longer projected.
    assert_eq!(s.list_projectable(100).await.unwrap().len(), 0);
}

#[tokio::test]
async fn crash_before_commit_leaves_cursor_and_resumes() {
    let mut server = mockito::Server::new_async().await;
    let conn = connector(&server.url(), FakeOAuthPort::connected("tok"));
    let (_d, s) = store().await;

    let _m0 = mock_ok(
        &mut server,
        "timeMin",
        page_body(
            vec![event_json("a", "2026-07-22T09:00:00Z", "A")],
            Some("p1"),
            None,
        ),
    )
    .await;
    let _m1 = mock_ok(
        &mut server,
        "pageToken=p1",
        page_body(
            vec![event_json("b", "2026-07-22T09:00:00Z", "B")],
            None,
            Some("s1"),
        ),
    )
    .await;

    // Run 1: with a budget of 1, commit only page1 and stop (stands in for a crash before committing page2).
    let s1 = run_sync(&conn, &s, &plan(1), &CancelFlag::new(), Utc::now())
        .await
        .unwrap();
    assert_eq!(s1.stop_reason, StopReason::PageBudgetReached);
    assert_eq!(s1.pages_committed, 1);
    assert_eq!(s.list_projectable(100).await.unwrap().len(), 1);
    // The cursor stays at the next cursor of the committed page1 (page:p1) — page2 is not committed.
    let cur = s.get_cursor(INSTALL, ACCOUNT).await.unwrap().unwrap();
    assert_eq!(cur.cursor.as_deref(), Some("page:p1"));

    // Run 2: resume from page:p1 and drain. Total 2 with no duplicates.
    let s2 = run_sync(&conn, &s, &plan(10), &CancelFlag::new(), Utc::now())
        .await
        .unwrap();
    assert_eq!(s2.stop_reason, StopReason::Drained);
    assert_eq!(s.list_projectable(100).await.unwrap().len(), 2);
}

#[tokio::test]
async fn cancellation_before_network_pulls_nothing() {
    let mut server = mockito::Server::new_async().await;
    let conn = connector(&server.url(), FakeOAuthPort::connected("tok"));
    let (_d, s) = store().await;

    // Register no mock so any request fails, and verify 0 calls with expect(0).
    let guard = server
        .mock("GET", Matcher::Any)
        .expect(0)
        .create_async()
        .await;

    let cancel = CancelFlag::new();
    cancel.cancel();
    let summary = run_sync(&conn, &s, &plan(10), &cancel, Utc::now())
        .await
        .unwrap();
    assert_eq!(summary.stop_reason, StopReason::Cancelled);
    assert_eq!(summary.pages_committed, 0);
    assert_eq!(s.list_projectable(100).await.unwrap().len(), 0);
    // Cancellation precedes the network — events.list was never called at all.
    guard.assert_async().await;
}

#[tokio::test]
async fn rate_limited_surfaces_typed_health_and_first_page_commits() {
    let mut server = mockito::Server::new_async().await;
    let conn = connector(&server.url(), FakeOAuthPort::connected("tok"));
    let (_d, s) = store().await;

    let _m0 = mock_ok(
        &mut server,
        "timeMin",
        page_body(
            vec![event_json("a", "2026-07-22T09:00:00Z", "A")],
            Some("p1"),
            None,
        ),
    )
    .await;
    // The second page request is 429 + Retry-After.
    let _m1 = server
        .mock("GET", "/calendars/primary/events")
        .match_query(Matcher::Regex("pageToken=p1".into()))
        .with_status(429)
        .with_header("retry-after", "30")
        .with_body(r#"{"error":{"errors":[{"reason":"rateLimitExceeded"}]}}"#)
        .create_async()
        .await;

    let summary = run_sync(&conn, &s, &plan(10), &CancelFlag::new(), Utc::now())
        .await
        .unwrap();
    match summary.stop_reason {
        StopReason::Unhealthy(SourceHealth::RateLimited { retry_after_secs }) => {
            assert_eq!(retry_after_secs, Some(30));
        }
        other => panic!("expected rate-limited health, got {other:?}"),
    }
    // The first page is already committed.
    assert_eq!(summary.pages_committed, 1);
    assert_eq!(s.list_projectable(100).await.unwrap().len(), 1);
}

#[tokio::test]
async fn sync_token_expiry_surfaces_cursor_expired_then_full_resync_recovers() {
    let mut server = mockito::Server::new_async().await;
    let conn = connector(&server.url(), FakeOAuthPort::connected("tok"));
    let (_d, s) = store().await;

    // Initial full sync → collect 2 + syncToken s1.
    let _init = mock_ok(
        &mut server,
        "timeMin",
        page_body(
            vec![
                event_json("a", "2026-07-22T09:00:00Z", "A"),
                event_json("b", "2026-07-22T09:00:00Z", "B"),
            ],
            None,
            Some("s1"),
        ),
    )
    .await;
    // An incremental query with s1 returns 410 Gone (expired).
    let _expired = server
        .mock("GET", "/calendars/primary/events")
        .match_query(Matcher::Regex("syncToken=s1".into()))
        .with_status(410)
        .with_body(r#"{"error":{"code":410,"message":"Sync token is no longer valid"}}"#)
        .create_async()
        .await;

    // Run 1: drain the initial full sync.
    let r1 = run_sync(&conn, &s, &plan(10), &CancelFlag::new(), Utc::now())
        .await
        .unwrap();
    assert_eq!(r1.stop_reason, StopReason::Drained);
    assert_eq!(s.list_projectable(100).await.unwrap().len(), 2);

    // Run 2: incremental with the stored s1 → 410 → surfaces CursorExpired.
    let r2 = run_sync(&conn, &s, &plan(10), &CancelFlag::new(), Utc::now())
        .await
        .unwrap();
    assert_eq!(
        r2.stop_reason,
        StopReason::Unhealthy(SourceHealth::CursorExpired)
    );

    // Run 3: the connector discards the expired syncToken and self-heals with an Initial full resync.
    let r3 = run_sync(&conn, &s, &plan(10), &CancelFlag::new(), Utc::now())
        .await
        .unwrap();
    assert_eq!(r3.stop_reason, StopReason::Drained);
    // Re-collected but still 2 with no duplicates.
    assert_eq!(s.list_projectable(100).await.unwrap().len(), 2);
    // The cursor advanced from the expired s1 → a new s1 (re-issued) (recovered after 410).
    let cur = s.get_cursor(INSTALL, ACCOUNT).await.unwrap().unwrap();
    assert_eq!(cur.cursor.as_deref(), Some("sync:s1"));
}

#[tokio::test]
async fn restart_resumes_from_sync_token_without_duplicates() {
    // 2-process restart: process 1 exits after the initial sync, process 2 reopens and resumes incrementally.
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("wctx.db");

    let mut server = mockito::Server::new_async().await;
    let _init = mock_ok(
        &mut server,
        "timeMin",
        page_body(
            vec![
                event_json("a", "2026-07-22T09:00:00Z", "A"),
                event_json("b", "2026-07-22T09:00:00Z", "B"),
            ],
            None,
            Some("s1"),
        ),
    )
    .await;
    // Incremental: b is updated (higher updated) + new c. a is not included.
    let _incr = mock_ok(
        &mut server,
        "syncToken=s1",
        page_body(
            vec![
                event_json("b", "2026-07-23T09:00:00Z", "B-updated"),
                event_json("c", "2026-07-23T09:00:00Z", "C"),
            ],
            None,
            Some("s2"),
        ),
    )
    .await;

    // Process 1.
    {
        let s = SqliteStorage::open(&path, 30, None).unwrap();
        s.begin_access_epoch(INSTALL, ACCOUNT, Utc::now())
            .await
            .unwrap();
        let conn = connector(&server.url(), FakeOAuthPort::connected("tok"));
        run_sync(&conn, &s, &plan(10), &CancelFlag::new(), Utc::now())
            .await
            .unwrap();
        assert_eq!(s.list_projectable(100).await.unwrap().len(), 2);
    }

    // Process 2: reopen (new store + new connector). Do not call begin_access_epoch again
    // (bumping the epoch causes EpochMismatch) — reuse the on-disk epoch=1 and cursor sync:s1.
    {
        let s = SqliteStorage::open(&path, 30, None).unwrap();
        let conn = connector(&server.url(), FakeOAuthPort::connected("tok"));
        let summary = run_sync(&conn, &s, &plan(10), &CancelFlag::new(), Utc::now())
            .await
            .unwrap();
        assert_eq!(summary.stop_reason, StopReason::Drained);
        // a (kept) + b (once, updated) + c (new) = 3. b is not duplicated.
        let proj = s.list_projectable(100).await.unwrap();
        assert_eq!(proj.len(), 3);
        let cur = s.get_cursor(INSTALL, ACCOUNT).await.unwrap().unwrap();
        assert_eq!(cur.cursor.as_deref(), Some("sync:s2"));
    }
}

#[tokio::test]
async fn recurring_timezone_all_day_and_moved_occurrence_identities() {
    let mut server = mockito::Server::new_async().await;
    let conn = connector(&server.url(), FakeOAuthPort::connected("tok"));
    let (_d, s) = store().await;

    // 2 recurring instances (unique id) + all-day event + moved occurrence (same id, higher updated).
    let occ1 = json!({
        "id": "master_20260722T010000Z", "status": "confirmed",
        "summary": "Standup", "updated": "2026-07-20T00:00:00Z",
        "recurringEventId": "master",
        "start": {"dateTime": "2026-07-22T10:00:00+09:00"}
    });
    let occ2 = json!({
        "id": "master_20260723T010000Z", "status": "confirmed",
        "summary": "Standup", "updated": "2026-07-20T00:00:00Z",
        "recurringEventId": "master",
        "start": {"dateTime": "2026-07-23T10:00:00+09:00"}
    });
    let all_day = json!({
        "id": "holiday_1", "status": "confirmed", "summary": "Company holiday",
        "updated": "2026-07-20T00:00:00Z", "start": {"date": "2026-07-24"}
    });
    let _init = mock_ok(
        &mut server,
        "timeMin",
        page_body(vec![occ1, occ2, all_day], None, Some("s1")),
    )
    .await;
    // occ1 is moved: same instance id, changed start + higher updated → revision bump.
    let occ1_moved = json!({
        "id": "master_20260722T010000Z", "status": "confirmed",
        "summary": "Standup", "updated": "2026-07-21T00:00:00Z",
        "recurringEventId": "master",
        "start": {"dateTime": "2026-07-22T14:00:00+09:00"}
    });
    let _incr = mock_ok(
        &mut server,
        "syncToken=s1",
        page_body(vec![occ1_moved], None, Some("s2")),
    )
    .await;

    // Run 1: initial.
    run_sync(&conn, &s, &plan(10), &CancelFlag::new(), Utc::now())
        .await
        .unwrap();
    let proj = s.list_projectable(100).await.unwrap();
    // 2 distinct occurrences + 1 all-day = 3 separate identities.
    assert_eq!(proj.len(), 3);

    // Run 2: the moved occurrence is accepted as a higher revision of the same identity (not new).
    run_sync(&conn, &s, &plan(10), &CancelFlag::new(), Utc::now())
        .await
        .unwrap();
    let proj2 = s.list_projectable(100).await.unwrap();
    assert_eq!(
        proj2.len(),
        3,
        "옮겨진 occurrence 는 새 정체성을 만들지 않는다"
    );
    // The moved occurrence's revision (updated millis) has increased.
    let moved = proj2
        .iter()
        .find(|e| e.identity.remote_id == "master_20260722T010000Z")
        .unwrap();
    let expected = DateTime::parse_from_rfc3339("2026-07-21T00:00:00Z")
        .unwrap()
        .timestamp_millis();
    assert_eq!(moved.source_order, Some(expected));
    // The occurrence links master only as an opaque Parent relation.
    assert_eq!(moved.relations.len(), 1);
    assert_eq!(moved.relations[0].opaque_source_id, "master");
}

#[tokio::test]
async fn sensitive_fields_are_absent_from_envelope() {
    let mut server = mockito::Server::new_async().await;
    let conn = connector(&server.url(), FakeOAuthPort::connected("tok"));
    let (_d, s) = store().await;

    // An event carrying description/attendees/location.
    let ev = json!({
        "id": "evt_secret", "status": "confirmed",
        "summary": "1:1 sync", "updated": "2026-07-22T09:00:00Z",
        "start": {"dateTime": "2026-07-22T10:00:00+09:00"},
        "description": "SECRET_AGENDA_do_not_leak",
        "location": "SECRET_ROOM_do_not_leak",
        "attendees": [{"email": "leak-me@example.com"}]
    });
    let _m0 = mock_ok(
        &mut server,
        "timeMin",
        page_body(vec![ev], None, Some("s1")),
    )
    .await;

    run_sync(&conn, &s, &plan(10), &CancelFlag::new(), Utc::now())
        .await
        .unwrap();
    let proj = s.list_projectable(100).await.unwrap();
    assert_eq!(proj.len(), 1);
    let json_env = serde_json::to_string(&proj[0]).unwrap();
    // No sensitive raw text anywhere in the envelope (structural exclusion).
    assert!(
        !json_env.contains("SECRET_AGENDA_do_not_leak"),
        "description leaked"
    );
    assert!(
        !json_env.contains("SECRET_ROOM_do_not_leak"),
        "location leaked"
    );
    assert!(
        !json_env.contains("leak-me@example.com"),
        "attendee email leaked"
    );
}

#[tokio::test]
async fn revoke_removes_credentials_and_fails_closed() {
    let mut server = mockito::Server::new_async().await;
    let oauth = FakeOAuthPort::connected("tok");
    let conn = connector(&server.url(), oauth.clone());
    let (_d, s) = store().await;

    // Before revocation: check the connection status.
    assert_eq!(
        conn.account_status(INSTALL, ACCOUNT).await.unwrap(),
        AccountStatus::Connected
    );

    // After revocation, events.list must never be called.
    let net = server
        .mock("GET", Matcher::Any)
        .expect(0)
        .create_async()
        .await;

    conn.revoke(INSTALL, ACCOUNT).await.unwrap();
    // The credentials have been removed.
    assert!(oauth
        .get_access_token("google_calendar")
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        conn.account_status(INSTALL, ACCOUNT).await.unwrap(),
        AccountStatus::Revoked
    );

    // Subsequent sync is fail-closed (Unauthorized) and does not touch the network.
    let summary = run_sync(&conn, &s, &plan(10), &CancelFlag::new(), Utc::now())
        .await
        .unwrap();
    assert_eq!(
        summary.stop_reason,
        StopReason::Unhealthy(SourceHealth::Unauthorized)
    );
    assert_eq!(summary.pages_committed, 0);
    net.assert_async().await;
}

// ── Connector-level HTTP classification tests ─────────────────────────────────────────

/// Builds a request that makes the connector do one Initial collection.
fn initial_request() -> SyncRequest {
    SyncRequest {
        install_id: INSTALL.into(),
        account_subject_ref: ACCOUNT.into(),
        cursor: None,
        access_epoch_id: EPOCH,
        max_records: 100,
    }
}

async fn assert_status_maps_to_health(status: usize, body: &str, expected: SourceHealth) {
    let mut server = mockito::Server::new_async().await;
    let conn = connector(&server.url(), FakeOAuthPort::connected("tok"));
    let _m = server
        .mock("GET", "/calendars/primary/events")
        .match_query(Matcher::Any)
        .with_status(status)
        .with_body(body)
        .create_async()
        .await;
    match conn.sync(initial_request()).await.unwrap() {
        SyncOutcome::Unhealthy(h) => assert_eq!(h, expected, "status {status}"),
        SyncOutcome::Page(_) => panic!("status {status} should surface unhealthy, got a page"),
    }
}

#[tokio::test]
async fn http_401_maps_to_unauthorized() {
    assert_status_maps_to_health(
        401,
        r#"{"error":{"errors":[{"reason":"authError"}]}}"#,
        SourceHealth::Unauthorized,
    )
    .await;
}

#[tokio::test]
async fn http_403_permission_maps_to_forbidden() {
    assert_status_maps_to_health(
        403,
        r#"{"error":{"errors":[{"reason":"insufficientPermissions"}]}}"#,
        SourceHealth::Forbidden,
    )
    .await;
}

#[tokio::test]
async fn http_403_rate_limit_reason_maps_to_rate_limited() {
    assert_status_maps_to_health(
        403,
        r#"{"error":{"errors":[{"reason":"userRateLimitExceeded"}]}}"#,
        SourceHealth::RateLimited {
            retry_after_secs: None,
        },
    )
    .await;
}

#[tokio::test]
async fn http_500_maps_to_provider_unavailable() {
    assert_status_maps_to_health(500, "upstream boom", SourceHealth::ProviderUnavailable).await;
}

#[tokio::test]
async fn missing_token_fails_closed_without_network() {
    let mut server = mockito::Server::new_async().await;
    // No token.
    let oauth = Arc::new(FakeOAuthPort {
        token: Mutex::new(None),
        revoked: AtomicBool::new(false),
    });
    let conn = connector(&server.url(), oauth);
    let net = server
        .mock("GET", Matcher::Any)
        .expect(0)
        .create_async()
        .await;
    match conn.sync(initial_request()).await.unwrap() {
        SyncOutcome::Unhealthy(SourceHealth::Unauthorized) => {}
        other => panic!("missing token must fail closed, got {other:?}"),
    }
    // With no token it does not even make a network call (fail-closed).
    net.assert_async().await;
}

#[tokio::test]
async fn offline_transport_error_surfaces_offline() {
    // Trigger a connection refusal with an unreachable endpoint (closed port). The transport
    // error surfaces as typed Offline, and the reqwest error Display (which may include a
    // token-bearing URL) is not carried in the return value (SourceHealth has no body field).
    let conn = connector("http://127.0.0.1:1", FakeOAuthPort::connected("tok"));
    match conn.sync(initial_request()).await.unwrap() {
        SyncOutcome::Unhealthy(SourceHealth::Offline) => {}
        other => panic!("transport error must surface Offline, got {other:?}"),
    }
}

// ── Refined-projection persistence (reviewable + source/freshness) ────────────────

#[allow(clippy::too_many_arguments)]
fn envelope_from_event(
    event: &GoogleEvent,
    ctx: &GoogleCalendarMapCtx,
    epoch: i64,
    now: DateTime<Utc>,
) -> WorkContextEnvelope {
    let rec = event_to_record(event, ctx, now);
    let sok = compute_source_object_key(DEDUPE_KEY, &rec.identity);
    let fingerprint = compute_revision_fingerprint(
        RevisionModel::Monotonic,
        rec.remote_revision.as_deref(),
        rec.etag.as_deref(),
        rec.source_updated_at,
        &rec.content_hash,
        rec.lifecycle,
    );
    WorkContextEnvelope {
        envelope_id: format!("wctx_{sok}_{}_{epoch}", rec.source_order.unwrap_or(0)),
        schema_version: WORK_CONTEXT_SCHEMA_VERSION,
        access_epoch_id: epoch,
        source_object_key: sok,
        identity: rec.identity,
        revision_model: RevisionModel::Monotonic,
        remote_revision: rec.remote_revision,
        etag: rec.etag,
        source_order: rec.source_order,
        content_hash: rec.content_hash,
        revision_fingerprint: fingerprint,
        kind: rec.kind,
        classification: rec.classification,
        retention_class: None,
        occurred_at: rec.occurred_at,
        source_updated_at: rec.source_updated_at,
        observed_at: rec.observed_at,
        ingested_at: now,
        relations: rec.relations,
        access_snapshot: None,
        consent_snapshot: None,
        ingest_run_id: "run_proj".into(),
        prior_envelope_id: None,
        source_cursor_digest: None,
        projection_ref: None,
        raw_blob_ref: None,
        lifecycle: rec.lifecycle,
    }
}

#[tokio::test]
async fn refined_projection_is_reviewable_with_source_and_freshness_no_sensitive() {
    let (_d, s) = store().await;
    let now = Utc::now();
    let ctx = GoogleCalendarMapCtx {
        extension_id: GOOGLE_CALENDAR_EXTENSION_ID.into(),
        install_id: INSTALL.into(),
        account_subject_ref: ACCOUNT.into(),
        remote_type: GOOGLE_CALENDAR_REMOTE_TYPE.into(),
    };

    // An event carrying description (sensitive) too. No consent (default) → project title only.
    let ev: GoogleEvent = serde_json::from_value(json!({
        "id": "evt_review", "status": "confirmed",
        "summary": "Quarterly planning",
        "updated": "2026-07-22T09:00:00Z",
        "start": {"dateTime": "2026-07-22T10:00:00+09:00"},
        "description": "SECRET_do_not_leak"
    }))
    .unwrap();

    let env = envelope_from_event(&ev, &ctx, EPOCH, now);
    let sok = env.source_object_key.clone();
    let content: CommitContent =
        event_to_commit_content(&ev, &sok, /* sensitive_consent */ false)
            .expect("title-only projection is non-empty");

    let outcome = s
        .commit_page(CommitPageRequest {
            install_id: INSTALL.into(),
            account_subject_ref: ACCOUNT.into(),
            access_epoch_id: EPOCH,
            ingest_run_id: "run_proj".into(),
            envelopes: vec![env],
            contents: vec![content],
            cursor: CursorAdvance {
                install_id: INSTALL.into(),
                account_subject_ref: ACCOUNT.into(),
                expected_cursor: None,
                next_cursor: Some("sync:s1".into()),
            },
            now,
        })
        .await
        .unwrap();
    assert!(matches!(outcome, CommitOutcome::Committed { .. }));

    // The refined projection is stored — title only, description excluded.
    let proj = s.read_projection(&sok, EPOCH).await.unwrap().unwrap();
    assert_eq!(proj.sanitized_title.as_deref(), Some("Quarterly planning"));
    assert_eq!(proj.sanitized_summary, None);

    // Exposed as reviewable timeline evidence while preserving source/freshness.
    let timeline = s.list_work_context_timeline(100).await.unwrap();
    assert_eq!(timeline.len(), 1);
    let item = &timeline[0];
    // Distinguished from PC events by the source-family label (work_context).
    use maekon_core::models::work_context_projection::SourceFamily;
    assert_eq!(item.source_family, SourceFamily::WorkContext);
    // freshness: the display time aligns with the event occurrence time (occurred_at, UTC-normalized).
    assert_eq!(item.display_time.to_rfc3339(), "2026-07-22T01:00:00+00:00");
    assert_eq!(item.sanitized_title.as_deref(), Some("Quarterly planning"));
    // The sensitive raw text is nowhere in the projection or timeline.
    let dump = serde_json::to_string(&timeline).unwrap();
    assert!(!dump.contains("SECRET_do_not_leak"));
}
