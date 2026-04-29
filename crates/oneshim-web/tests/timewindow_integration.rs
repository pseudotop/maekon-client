//! E2E test verifying TimeWindow flows correctly through REST → handler → service → storage layer.
//!
//! Per Phase 2 iter-1 C3: assertions limited to status code + error message
//! substring (ApiError IntoResponse emits `{ error, status }` only — NO `code`
//! field in the response body).

use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Method, Request, StatusCode};
use chrono::{DateTime, Utc};
use oneshim_core::models::frame::FrameMetadata;
use oneshim_storage::sqlite::SqliteStorage;
use serde_json::Value;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::broadcast;
use tower::ServiceExt;

use oneshim_web::app_state::AppState;
use oneshim_web::WebServer;

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Build a fresh in-memory `SqliteStorage` plus the loopback Axum router.
/// The storage handle is returned so callers can seed test data before issuing
/// HTTP requests.
fn loopback_app_with_storage() -> (axum::Router, Arc<SqliteStorage>) {
    let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("in-memory SQLite"));
    // Keep one concrete handle for seeding (concrete inherent methods like
    // save_frame_metadata are not on the WebStorage trait).
    let storage_for_seed = Arc::clone(&storage);
    let (event_tx, _) = broadcast::channel(16);
    // `storage` (Arc<SqliteStorage>) coerces to Arc<dyn WebStorage> on call.
    let state = AppState::with_core(storage, event_tx);
    let router = WebServer::build_router(state)
        .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))));
    (router, storage_for_seed)
}

/// Seed a single frame at the given RFC3339 timestamp with default
/// non-period fields. Importance >= 0 so the default frames-endpoint filter
/// (`min_importance` unset → 0.0) admits the row.
fn seed_frame(storage: &SqliteStorage, ts: &str) {
    let timestamp = DateTime::parse_from_rfc3339(ts)
        .expect("trusted test ts")
        .with_timezone(&Utc);
    let meta = FrameMetadata {
        timestamp,
        trigger_type: "test".to_string(),
        app_name: "TestApp".to_string(),
        window_title: "Test Window".to_string(),
        resolution: (1920, 1080),
        importance: 0.5,
    };
    storage
        .save_frame_metadata(&meta, None, None)
        .expect("seed frame");
}

async fn read_body_json(response: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    serde_json::from_slice(&bytes).expect("body must be valid JSON")
}

// ── Test 1: closed-closed boundary inclusion ────────────────────────────────

/// Per spec §5.1: `TimeWindow [start, end]` is closed-closed — both bounds
/// included. Seed 4 frames; query `[t1, t3]` should return frames at t1 +
/// middle + t3 (3 frames), NOT t4 (after end).
#[tokio::test]
async fn frames_endpoint_with_explicit_window_returns_correct_count() {
    let (app, storage) = loopback_app_with_storage();
    seed_frame(&storage, "2026-04-01T00:00:00+00:00");
    seed_frame(&storage, "2026-04-15T00:00:00+00:00");
    seed_frame(&storage, "2026-04-25T00:00:00+00:00");
    seed_frame(&storage, "2026-04-30T00:00:00+00:00"); // after upper boundary

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/frames?from=2026-04-01T00:00:00%2B00:00&to=2026-04-25T00:00:00%2B00:00")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("oneshot");

    assert_eq!(response.status(), StatusCode::OK);
    let body = read_body_json(response).await;
    let total = body["pagination"]["total"]
        .as_u64()
        .expect("pagination.total present");
    assert_eq!(total, 3, "closed-closed should include both boundaries");
}

// ── Test 2: DeleteRangeRequest preserves external from/to JSON shape ────────

/// Per Phase 2 iter-1 C9 Option C: DeleteRangeRequest keeps `from` + `to`
/// String fields trivially. Frontend JSON shape unchanged after refactor —
/// endpoint still accepts `{"from", "to", "data_types"}` body.
#[tokio::test]
async fn delete_range_request_preserves_external_from_to_shape() {
    let (app, _storage) = loopback_app_with_storage();
    let body = r#"{"from":"2026-04-01T00:00:00+00:00","to":"2026-04-25T00:00:00+00:00","data_types":["frames"]}"#;

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/api/data/range")
                .header("Content-Type", "application/json")
                .body(Body::from(body))
                .expect("build request"),
        )
        .await
        .expect("oneshot");

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "frontend from/to keys still accepted; deletion succeeds on empty store"
    );
}

// ── Test 3: inverted bounds → 400 BadRequest ────────────────────────────────

/// Per spec §5.1 + §7.2: invalid TimeWindow construction (start > end) maps
/// via CoreError::TimeWindow → ApiError::BadRequest → HTTP 400. Body schema
/// is `{ error, status }` — no `code` field per Phase 2 iter-1 C3.
#[tokio::test]
async fn invalid_time_window_returns_400() {
    let (app, _storage) = loopback_app_with_storage();

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/frames?from=2026-04-25T00:00:00%2B00:00&to=2026-04-01T00:00:00%2B00:00")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("oneshot");

    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "inverted bounds → 400"
    );
    let body = read_body_json(response).await;
    let err_msg = body["error"]
        .as_str()
        .expect("error field present")
        .to_lowercase();
    assert!(
        err_msg.contains("must be <=") || err_msg.contains("start") || err_msg.contains("inverted"),
        "error message should mention bound inversion; got: {err_msg}"
    );
    assert_eq!(
        body["status"], 400,
        "ErrorResponse.status mirrors HTTP code"
    );
    assert!(
        body.get("code").is_none(),
        "ErrorResponse must NOT carry a `code` field per ApiError schema"
    );
}

// ── Test 4: invalid RFC3339 timestamp → 400 BadRequest ──────────────────────

/// Per Phase 2 iter-9 NEW-C1: invalid RFC3339 timestamps now propagate as
/// HTTP 400 BadRequest (was 200 OK with default-window data via silent
/// from_datetime fallback).
#[tokio::test]
async fn invalid_iso8601_timestamp_returns_400() {
    let (app, _storage) = loopback_app_with_storage();

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/frames?from=not-a-date&to=2026-04-25T00:00:00%2B00:00")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("oneshot");

    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "RFC3339 parse failure → 400"
    );
}
