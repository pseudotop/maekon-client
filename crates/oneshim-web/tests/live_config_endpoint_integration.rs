//! Live-config endpoint integration tests (Task 9.5).
//!
//! Per plan L3604 / spec §9.2 L1415-1417 / D29. Covers the
//! `GET /api/external-grpc/live-config` endpoint shipped in Task 7.1
//! (commit `eaefccd4`).
//!
//! Test 1 (`live_config_endpoint_returns_current_snapshot`) builds an
//! `AppState` with a `LiveExternalConfig` + `ExternalMetrics` wired into
//! `DiagnosticsState`, dispatches `GET /api/external-grpc/live-config`
//! through the production Axum router via `tower::ServiceExt::oneshot`,
//! and asserts:
//!   - HTTP 200
//!   - JSON body parses into `LiveConfigResponse`
//!   - `streaming_enabled`, `config_reload_task_alive`, and the four
//!     `LoadPolicyView` thresholds match the injected values
//!
//! Test 2 (`live_config_endpoint_503_when_external_disabled`) builds an
//! `AppState` without wiring `external_grpc_live` (i.e. `None` — external
//! gRPC is compiled but disabled at runtime). Asserts:
//!   - HTTP 503
//!   - JSON error body has the expected `{ error, status: 503 }` shape
//!     (per `oneshim_api_contracts::error::ErrorResponse`)
//!
//! Feature gate: requires `grpc-dashboard-external` for the live-config
//! handler's enabled path. Without the feature the handler always 503s
//! anyway, so these tests gate on the feature to keep the contract
//! observable from the test surface.

#![cfg(feature = "grpc-dashboard-external")]

use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Method, Request, StatusCode};
use oneshim_api_contracts::error::ErrorResponse;
use oneshim_api_contracts::external_grpc::LiveConfigResponse;
use oneshim_core::config::LoadThresholds;
use oneshim_storage::sqlite::SqliteStorage;
use serde::Deserialize;
use tokio::sync::broadcast;
use tower::ServiceExt;

use oneshim_web::app_state::AppState;
use oneshim_web::grpc::external::live_config::{LiveExternalConfig, LiveSnapshot};
use oneshim_web::grpc::external::metrics::ExternalMetrics;
use oneshim_web::grpc::LoadPolicy;
use oneshim_web::WebServer;

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Build a fresh `AppState` backed by in-memory SQLite + a fresh broadcast
/// channel. `DiagnosticsState.external_grpc_live` and
/// `DiagnosticsState.external_grpc_metrics` are `None` by default — the
/// "external disabled" baseline used by Test 2.
fn fresh_state() -> AppState {
    let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("in-memory SQLite"));
    let (event_tx, _) = broadcast::channel(16);
    AppState::with_core(storage, event_tx)
}

/// Build the full production Axum router (loopback gating included) and
/// attach a `MockConnectInfo` so `require_loopback_client` middleware passes.
///
/// Mirrors `audit_query_surface_integration.rs::loopback_app` so the two
/// integration test files use identical router setup.
fn loopback_app(state: AppState) -> axum::Router {
    WebServer::build_router(state).layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))))
}

/// `LoadThresholds` with deliberately distinct (non-default) values so Test 1's
/// assertions prove the handler reads from the injected `LiveExternalConfig`
/// (not from a default fallback).
fn injected_thresholds() -> LoadThresholds {
    LoadThresholds {
        cpu_low_pct: 33.0,
        cpu_medium_pct: 66.0,
        cpu_high_pct: 88.0,
        min_free_mem_gb: 1.5,
    }
}

// ── Test 1: GET /api/external-grpc/live-config → 200 + matching snapshot ──

/// Spec §9.2 L1415 — Task 9.5 positive path:
///
/// Build an `AppState` with a `LiveExternalConfig` carrying known values
/// (streaming_enabled=true + custom thresholds) + `ExternalMetrics` with
/// `config_reload_task_alive=true`. Dispatch
/// `GET /api/external-grpc/live-config` through the production router.
///
/// Assert:
/// - HTTP 200
/// - JSON body parses into `LiveConfigResponse`
/// - `streaming_enabled == true` (matches injection)
/// - `config_reload_task_alive == true` (matches injection)
/// - The four `LoadPolicyView` threshold fields match `injected_thresholds()`
///   bit-for-bit (proves the handler reads from the wired `LiveExternalConfig`,
///   not a default fallback)
/// - `started_at_elapsed_ms` is populated (any u64 value is acceptable since
///   wall-clock between fixture build + handler dispatch is non-deterministic;
///   the key invariant is the field is present in the JSON and parses)
#[tokio::test]
async fn live_config_endpoint_returns_current_snapshot() {
    let thresholds = injected_thresholds();
    let live = Arc::new(LiveExternalConfig::new(LiveSnapshot {
        streaming_enabled: true,
        load_policy: Arc::new(LoadPolicy::new(thresholds.clone())),
    }));
    let metrics = Arc::new(ExternalMetrics::new());
    metrics
        .config_reload_task_alive
        .store(true, Ordering::Relaxed);

    let mut state = fresh_state();
    state.diagnostics.external_grpc_live = Some(live);
    state.diagnostics.external_grpc_metrics = Some(metrics);

    let app = loopback_app(state);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/external-grpc/live-config")
                .body(Body::empty())
                .expect("build GET request"),
        )
        .await
        .expect("oneshot");

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "GET /api/external-grpc/live-config (enabled) must return 200"
    );

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    let body: LiveConfigResponse = serde_json::from_slice(&body_bytes).expect(
        "response body must deserialize into LiveConfigResponse \
         (the api-contracts DTO shape)",
    );

    // streaming_enabled propagates from the injected LiveSnapshot.
    assert!(
        body.streaming_enabled,
        "streaming_enabled must reflect the injected LiveSnapshot value (true)"
    );

    // config_reload_task_alive propagates from the injected ExternalMetrics.
    assert!(
        body.config_reload_task_alive,
        "config_reload_task_alive must reflect the injected ExternalMetrics value (true)"
    );

    // All four LoadPolicyView thresholds must equal the injected values
    // bit-for-bit — proves the handler reads from the wired LiveExternalConfig
    // (not a default fallback).
    let view = &body.load_policy_snapshot;
    assert_eq!(
        view.cpu_low_pct, thresholds.cpu_low_pct,
        "cpu_low_pct must match injected LoadThresholds"
    );
    assert_eq!(
        view.cpu_medium_pct, thresholds.cpu_medium_pct,
        "cpu_medium_pct must match injected LoadThresholds"
    );
    assert_eq!(
        view.cpu_high_pct, thresholds.cpu_high_pct,
        "cpu_high_pct must match injected LoadThresholds"
    );
    assert_eq!(
        view.min_free_mem_gb, thresholds.min_free_mem_gb,
        "min_free_mem_gb must match injected LoadThresholds"
    );

    // Bounded assertion on started_at_elapsed_ms — fresh LoadPolicy::new()
    // uses Instant::now(); within CI dispatch latency this should be << 60s.
    // Ruling out a saturating-zero or negative-cast regression.
    assert!(
        view.started_at_elapsed_ms < 60_000,
        "started_at_elapsed_ms should be sub-minute since fixture creation, got {}",
        view.started_at_elapsed_ms
    );

    // Deterministic assertion on in_warmup — WARMUP=30s; test creates the
    // policy and dispatches within milliseconds, so it must be in_warmup=true.
    // Test would have to take >30s to flake.
    assert!(
        view.in_warmup,
        "fresh LoadPolicy must be in warmup window (WARMUP=30s)"
    );
}

// ── Test 2: GET /api/external-grpc/live-config (disabled) → 503 ──────────

/// Spec §9.2 L1417 — Task 9.5 negative path:
///
/// Build an `AppState` whose `DiagnosticsState.external_grpc_live` is `None`
/// (the default — i.e. external gRPC is compiled in but disabled at runtime).
/// Dispatch `GET /api/external-grpc/live-config` through the production
/// router.
///
/// Assert:
/// - HTTP 503
/// - JSON error body matches the standard `ErrorResponse` shape
///   (`{ error: String, status: 503 }`) per `ApiError::ServiceUnavailable`'s
///   `IntoResponse` impl
#[tokio::test]
async fn live_config_endpoint_503_when_external_disabled() {
    // fresh_state() leaves both external_grpc_live + external_grpc_metrics as None.
    let state = fresh_state();
    let app = loopback_app(state);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/external-grpc/live-config")
                .body(Body::empty())
                .expect("build GET request"),
        )
        .await
        .expect("oneshot");

    assert_eq!(
        response.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "GET /api/external-grpc/live-config with external_grpc_live=None \
         must return 503"
    );

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");

    // ErrorResponse is `#[derive(Serialize)]` only — local mirror with
    // `Deserialize` so we can validate the JSON shape from the wire side.
    #[derive(Debug, Deserialize)]
    struct ErrorResponseDe {
        error: String,
        status: u16,
    }

    // Compile-time proof that ErrorResponse retains { error, status }.
    // The destructure pattern fails to compile if either field is renamed
    // or removed — catches both rename and removal regressions.
    let ErrorResponse {
        error: _,
        status: _,
    } = ErrorResponse {
        error: String::new(),
        status: 503,
    };

    let body: ErrorResponseDe = serde_json::from_slice(&body_bytes).expect(
        "503 response body must deserialize into ErrorResponse \
         ({ error: String, status: u16 })",
    );

    assert_eq!(
        body.status, 503,
        "ErrorResponse.status must equal the HTTP status (503)"
    );
    assert!(
        !body.error.is_empty(),
        "ErrorResponse.error must be non-empty (handler emits a descriptive message)"
    );
}
