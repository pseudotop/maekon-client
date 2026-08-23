//! Audit query surface integration tests (Task 9.3).
//!
//! Per plan L3602 / spec §9.2 L1403-1405. Covers D25 `entries_by_command_id`
//! port method + `GET /api/audit/export` REST endpoint filtering.
//!
//! Test 1 exercises the `AuditLogPort::entries_by_command_id` contract via
//! the production `AuditLogAdapter` (the only `AuditLogPort` impl wired in
//! production today). It verifies that with 3 matching + 2 non-matching
//! entries, exactly 3 are returned, newest-first.
//!
//! Test 2 exercises the `GET /api/audit/export?command_id=...` REST endpoint
//! end-to-end through the production Axum router via `tower::ServiceExt`.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{HeaderValue, Method, Request, StatusCode};
use chrono::Utc;
use maekon_api_contracts::audit_export::AuditExportEntryDto;
use maekon_api_contracts::automation::AuditEntryDto;
use maekon_automation::audit::{AuditLogAdapter, AuditLogger, AuditQuery};
use maekon_core::models::audit::{AuditEntry, AuditLevel, AuditStatus};
use maekon_core::ports::audit_log::AuditLogPort;
use maekon_storage::sqlite::SqliteStorage;
use tokio::sync::{broadcast, RwLock};
use tower::ServiceExt;

use maekon_web::app_state::AppState;
use maekon_web::WebServer;

// ── Helpers ──────────────────────────────────────────────────────────────────

const LOCAL_AUTH_HEADER: &str = "x-local-auth";
const TEST_LOCAL_AUTH_TOKEN: &str = "test-local-auth-token-e20-41";

/// Build an `AuditLogAdapter` wrapping a fresh in-memory `AuditLogger`.
///
/// Buffer capacity 100 is large enough for any of these tests (5/4 entries).
fn fresh_audit_adapter() -> Arc<AuditLogAdapter> {
    let logger = Arc::new(RwLock::new(AuditLogger::new(100, 10)));
    Arc::new(AuditLogAdapter::new(logger))
}

/// Seed `n` entries with the given `command_id` via `log_start_if`.
///
/// Each entry receives a distinct `action_type` of the form `"seed-action-{i}"`.
/// Returns the action types in **insertion order** so callers can derive the
/// expected newest-first order by reversing the returned vec.
///
/// No timestamp sleep is needed: production ordering is VecDeque insertion
/// order (`self.buffer.iter().rev()`), never timestamps.
async fn seed_entries(adapter: &Arc<AuditLogAdapter>, command_id: &str, n: usize) -> Vec<String> {
    let mut action_types = Vec::with_capacity(n);
    for i in 0..n {
        let action_type = format!("seed-action-{i}");
        adapter
            .log_start_if(AuditLevel::Basic, command_id, "session-test", &action_type)
            .await;
        action_types.push(action_type);
    }
    action_types
}

/// Build an `AppState` with the supplied audit adapter wired into
/// `state.automation.audit_logger`. Backed by in-memory SQLite + a fresh
/// broadcast channel; nothing else from production is required for the
/// `/api/audit/export` route.
fn app_state_with_audit(audit: Arc<dyn AuditLogPort>) -> AppState {
    let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("in-memory SQLite"));
    let (event_tx, _) = broadcast::channel(128);
    let mut state = AppState::with_core(storage, event_tx);
    state.automation.audit_logger = Some(audit);
    state
}

/// Build the full production Axum router (loopback gating included) and attach
/// a `MockConnectInfo` so `require_loopback_client` middleware passes. The
/// per-session local-auth gate is also satisfied because the production router
/// now rejects every `/api` request without a session token.
fn loopback_app(mut state: AppState) -> axum::Router {
    state.auth.local_auth_token = Some(Arc::from(TEST_LOCAL_AUTH_TOKEN));
    WebServer::build_router(state)
        .layer(axum::middleware::map_request(
            |mut req: axum::extract::Request| async move {
                req.headers_mut().insert(
                    LOCAL_AUTH_HEADER,
                    HeaderValue::from_static(TEST_LOCAL_AUTH_TOKEN),
                );
                req
            },
        ))
        .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))))
}

// ── Test 1: AuditLogPort::entries_by_command_id ──────────────────────────────

/// Spec §9.2 L1403 — D25 port method:
///
/// Insert 3 audit entries with the same `command_id` + 2 with different
/// command_ids. Calling `AuditLogPort::entries_by_command_id(cmd, 10)` must
/// return exactly the 3 matching rows, newest first.
///
/// Drives the production `AuditLogAdapter` (the wired prod impl) — not a mock.
///
/// NOTE: This test covers the buffer-only fast path. Storage fall-through
/// (Task 0.3.1) is exercised by
/// [`audit_entries_by_command_id_falls_through_to_storage_when_buffer_empty`]
/// below, which seeds entries directly into SqliteStorage and queries via
/// the same `AuditLogPort::entries_by_command_id` surface.
#[tokio::test]
async fn audit_entries_by_command_id_returns_matching_rows() {
    let adapter = fresh_audit_adapter();

    // Seed 3 entries with the target command_id, then 2 with a different one.
    // Each entry has a distinct action_type ("seed-action-{i}") for direct
    // ordering verification. No sleep needed: production ordering is
    // VecDeque::iter().rev() — insertion order, not timestamp order.
    let target_actions = seed_entries(&adapter, "cmd-target-123", 3).await;
    let _other_actions = seed_entries(&adapter, "cmd-other-456", 2).await;

    // Query: limit 10 (well above the 3 matching rows).
    let results: Vec<AuditEntry> = adapter.entries_by_command_id("cmd-target-123", 10).await;

    // Exactly 3 rows match the target command_id.
    assert_eq!(
        results.len(),
        3,
        "expected exactly 3 entries matching cmd-target-123, got {}",
        results.len()
    );

    // All returned rows have the target command_id (no leakage from cmd-other-456).
    for entry in &results {
        assert_eq!(
            entry.command_id, "cmd-target-123",
            "result leaked non-matching command_id: {:?}",
            entry.command_id
        );
    }

    // Newest-first ordering: directly assert reverse-insertion-order via
    // action_type identifiers. Production impl uses VecDeque::iter().rev(),
    // so newest-first == reverse insertion order — no timestamp proxy needed.
    let expected: Vec<&str> = target_actions.iter().rev().map(String::as_str).collect();
    let actual: Vec<&str> = results.iter().map(|e| e.action_type.as_str()).collect();
    assert_eq!(
        actual, expected,
        "newest-first must equal reverse insertion order; expected {:?}, got {:?}",
        expected, actual
    );
}

// ── Test 2: GET /api/audit/export?command_id=X ───────────────────────────────

/// Spec §9.2 L1404-1405 — D25 REST endpoint:
///
/// Pre-populate the audit log with rows for two different command_ids; call
/// `GET /api/audit/export?command_id=cmd-target-789`; assert that all returned
/// rows have the target `command_id` (no leakage from the other command_id).
///
/// Drives the production Axum router end-to-end via `tower::ServiceExt::oneshot`,
/// proving that:
///   1. The route `/api/audit/export` is reachable through `WebServer::build_router`.
///   2. `command_id` is parsed from the query string.
///   3. The handler dispatches to `AuditLogPort::entries_by_command_id` (not
///      `recent_entries`) when the param is present + non-empty.
///   4. Only matching rows are serialized into the JSON body.
#[tokio::test]
async fn audit_export_rest_endpoint_filters_by_command_id() {
    let adapter = fresh_audit_adapter();

    // Seed 2 matching + 2 non-matching rows.
    let _matching_actions = seed_entries(&adapter, "cmd-target-789", 2).await;
    let _other_actions = seed_entries(&adapter, "cmd-other-321", 2).await;

    let state = app_state_with_audit(adapter as Arc<dyn AuditLogPort>);
    let app = loopback_app(state);

    // GET /api/audit/export?command_id=cmd-target-789
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/audit/export?command_id=cmd-target-789")
                .body(Body::empty())
                .expect("build GET request"),
        )
        .await
        .expect("oneshot");

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "GET /api/audit/export with command_id filter must return 200"
    );

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    let entries: Vec<AuditExportEntryDto> = serde_json::from_slice(&body_bytes)
        .expect("response body must be Vec<AuditExportEntryDto> JSON");

    // Exactly 2 rows match the target command_id.
    assert_eq!(
        entries.len(),
        2,
        "expected exactly 2 entries matching cmd-target-789, got {}",
        entries.len()
    );

    // No leakage from cmd-other-321.
    for entry in &entries {
        assert_eq!(
            entry.command_id, "cmd-target-789",
            "REST response leaked non-matching command_id: {:?}",
            entry.command_id
        );
    }
}

// ── Test 3: Settings + automation audit harness endpoints ───────────────────

/// CRT-PRV-HARNESS-AUDIT-001 — reusable endpoint readiness harness.
///
/// Verifies the same production Axum router exposes `/api/settings` and
/// `/api/automation/audit` before and after a native-action-window audit marker.
/// The native window action itself is covered by OS probes; this guard keeps the
/// reusable debug/audit endpoints deterministic in CI and local private runs.
#[tokio::test]
async fn settings_and_automation_audit_endpoints_survive_native_action_window_probe() {
    let adapter = fresh_audit_adapter();
    adapter
        .log_complete_with_time(
            AuditLevel::Basic,
            "cmd-audit-baseline",
            "session-harness-audit",
            "baseline endpoint readiness marker",
            3,
        )
        .await;

    let state = app_state_with_audit(adapter.clone() as Arc<dyn AuditLogPort>);
    let app = loopback_app(state);

    let settings_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/settings")
                .body(Body::empty())
                .expect("build settings request"),
        )
        .await
        .expect("settings endpoint");

    assert_eq!(settings_response.status(), StatusCode::OK);
    let settings_body = axum::body::to_bytes(settings_response.into_body(), usize::MAX)
        .await
        .expect("read settings body");
    let settings: serde_json::Value =
        serde_json::from_slice(&settings_body).expect("settings endpoint must return JSON");
    assert_eq!(
        settings.get("web_port").and_then(serde_json::Value::as_u64),
        Some(u64::from(maekon_core::config::DEFAULT_WEB_PORT))
    );

    let baseline_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/automation/audit?limit=50")
                .body(Body::empty())
                .expect("build audit baseline request"),
        )
        .await
        .expect("audit baseline endpoint");

    assert_eq!(baseline_response.status(), StatusCode::OK);
    let baseline_body = axum::body::to_bytes(baseline_response.into_body(), usize::MAX)
        .await
        .expect("read audit baseline body");
    let baseline_entries: Vec<AuditEntryDto> =
        serde_json::from_slice(&baseline_body).expect("audit baseline must return JSON array");
    assert!(baseline_entries
        .iter()
        .any(|entry| entry.command_id == "cmd-audit-baseline"));

    adapter
        .log_complete_with_time(
            AuditLevel::Basic,
            "cmd-native-action-window",
            "session-harness-audit",
            "native action window marker",
            8,
        )
        .await;

    let after_response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/automation/audit?limit=50")
                .body(Body::empty())
                .expect("build audit after request"),
        )
        .await
        .expect("audit after endpoint");

    assert_eq!(after_response.status(), StatusCode::OK);
    let after_body = axum::body::to_bytes(after_response.into_body(), usize::MAX)
        .await
        .expect("read audit after body");
    let after_entries: Vec<AuditEntryDto> =
        serde_json::from_slice(&after_body).expect("audit after must return JSON array");
    assert!(after_entries
        .iter()
        .any(|entry| entry.command_id == "cmd-native-action-window"));

    if let Some(evidence_dir) = std::env::var_os("MAEKON_AUDIT_ENDPOINT_EVIDENCE_DIR") {
        let evidence_dir = std::path::PathBuf::from(evidence_dir);
        std::fs::create_dir_all(&evidence_dir).expect("create audit endpoint evidence dir");
        std::fs::write(
            evidence_dir.join("settings-endpoint.json"),
            settings_body.as_ref(),
        )
        .expect("write settings endpoint evidence");
        std::fs::write(
            evidence_dir.join("audit-baseline.json"),
            baseline_body.as_ref(),
        )
        .expect("write audit baseline evidence");
        std::fs::write(evidence_dir.join("audit-after.json"), after_body.as_ref())
            .expect("write audit after evidence");
        let summary = serde_json::json!({
            "ok": true,
            "settings_endpoint_json": true,
            "audit_baseline_json": true,
            "audit_after_json": true,
            "native_action_window_marker_observed": true,
            "host_mouse_events": false,
            "host_keyboard_events": false
        });
        std::fs::write(
            evidence_dir.join("summary.json"),
            serde_json::to_vec_pretty(&summary).expect("serialize evidence summary"),
        )
        .expect("write audit endpoint summary");
    }
}

// ── Test 4: Storage fall-through ─────────────────────────────────────────────

/// Recent and command-scoped queries fall through to SqliteStorage when the
/// in-memory buffer doesn't contain matching rows.
///
/// Mirrors the production wiring in `src-tauri::audit_query::SqliteAuditQuery`:
/// builds an `AuditLogger` whose `with_query` handle delegates directly to
/// `SqliteStorage::entries_by_command_id`. Seeds 3 audit entries directly into
/// SQLite (bypassing the in-memory buffer entirely) and verifies the
/// both `AuditLogPort::recent_entries` and `entries_by_command_id` return all
/// 3 in timestamp-DESC order.
///
/// This exercises the production fall-through path end-to-end: the adapter
/// reaches into `AuditLogger::entries_by_command_id`, finds an empty buffer,
/// queries the attached `AuditQuery` handle (the SqliteStorage wrapper),
/// re-sorts by timestamp DESC, and returns the merged result.
#[tokio::test]
async fn audit_entries_by_command_id_falls_through_to_storage_when_buffer_empty() {
    // SQLite-backed AuditQuery wrapper for this test — mirrors the production
    // `SqliteAuditQuery` in `src-tauri::audit_query` (cannot be imported from
    // the binary crate, so we replicate the trivial 5-line bridge here).
    struct StorageQuery {
        storage: Arc<SqliteStorage>,
    }
    impl AuditQuery for StorageQuery {
        fn stats(&self) -> maekon_core::models::audit::AuditStats {
            self.storage.audit_stats()
        }

        fn recent_entries(&self, limit: usize) -> Vec<AuditEntry> {
            self.storage.recent_audit_entries(limit)
        }

        fn entries_by_command_id(&self, command_id: &str, limit: usize) -> Vec<AuditEntry> {
            self.storage.entries_by_command_id(command_id, limit)
        }
    }

    // In-memory SQLite + 3 entries with target command_id at known offsets.
    let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("in-memory SQLite"));
    // #11474: one clock reading for the whole loop. Calling `Utc::now()` per
    // iteration made the 100ms offsets relative to a moving base, so a loop
    // body slower than 100ms produced `t_1 - 100ms > t_0` and inverted the
    // order the assertions below depend on.
    //
    // Measured, not argued. The delay was varied and the failure tracked it:
    //
    //   no probe, loaded runner   left "storage-only-1"  adjacent flip
    //     (public run 32638488008, while the same job pruned a target
    //      directory over its 8 GiB soft limit)
    //   150ms probe every pass    left "storage-only-2"  full reversal
    //     (public run 32648945810, throwaway branch)
    //
    // A cause that did not depend on elapsed time -- storage returning rows
    // unordered, say -- would not scale with the delay like that.
    //
    // Anchoring to one base fixes the spacing at 100ms no matter how slow the
    // runner is.
    let seeded_at = Utc::now();
    for i in 0..3_i64 {
        let entry = AuditEntry {
            entry_id: format!("storage-only-{i}"),
            // Newer i = larger offset back; results[0] should be i=0.
            timestamp: seeded_at - chrono::Duration::milliseconds(i * 100),
            session_id: "sess-storage".to_string(),
            command_id: "cmd-storage-fallthrough".to_string(),
            action_type: format!("storage-act-{i}"),
            status: AuditStatus::Completed,
            details: None,
            execution_time_ms: Some(10),
        };
        storage.save_audit_entry(&entry);
    }

    // AuditLogger with EMPTY buffer + storage-backed query handle. This
    // mirrors the production wiring in src-tauri (with_persistence +
    // with_query); we drop with_persistence since this test seeds storage
    // directly.
    let query: Arc<dyn AuditQuery> = Arc::new(StorageQuery {
        storage: storage.clone(),
    });
    let logger = AuditLogger::new(100, 10).with_query(query);
    let adapter = AuditLogAdapter::new(Arc::new(RwLock::new(logger)));

    let recent = adapter.recent_entries(10).await;
    assert_eq!(
        recent.len(),
        3,
        "unfiltered audit views must include storage-only privacy rows"
    );
    assert_eq!(recent[0].entry_id, "storage-only-0");
    assert_eq!(recent[1].entry_id, "storage-only-1");
    assert_eq!(recent[2].entry_id, "storage-only-2");

    let stats = adapter.stats().await;
    assert_eq!(stats.total, 3);
    assert_eq!(stats.completed, 3);

    // Query through the adapter — exercises the full AuditLogPort surface.
    let results = adapter
        .entries_by_command_id("cmd-storage-fallthrough", 10)
        .await;

    // All 3 storage rows surface (buffer is empty so dedup is a no-op).
    assert_eq!(
        results.len(),
        3,
        "expected 3 entries from storage fall-through, got {}",
        results.len()
    );

    // All rows have the target command_id (no leakage from any other rows).
    for entry in &results {
        assert_eq!(entry.command_id, "cmd-storage-fallthrough");
    }

    // Newest-first: results[0] = i=0 (offset 0), results[1] = i=1 (-100ms),
    // results[2] = i=2 (-200ms). entry_id == "storage-only-{i}".
    assert_eq!(results[0].entry_id, "storage-only-0");
    assert_eq!(results[1].entry_id, "storage-only-1");
    assert_eq!(results[2].entry_id, "storage-only-2");

    // Defensive: assert monotonic timestamp DESC.
    for w in results.windows(2) {
        assert!(
            w[0].timestamp >= w[1].timestamp,
            "expected newest-first ordering after storage fall-through"
        );
    }
}
