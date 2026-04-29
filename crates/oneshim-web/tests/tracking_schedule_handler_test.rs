//! TDD-red integration tests for the tracking-schedule REST handlers.
//!
//! Plan ref: §3.3 A.15
//!
//! These four tests describe the expected contract of the three endpoints:
//!
//! - `GET  /api/tracking-schedule`         → 200 with default config JSON
//! - `PUT  /api/tracking-schedule` (valid) → 200 echo; subsequent GET echoes
//! - `PUT  /api/tracking-schedule` (bad)   → 400 with wire code `validation.invalid_arguments`
//! - `GET  /api/tracking-schedule/status`  → 200 with `{ active_now: bool, ... }`
//!
//! Currently RED: the stub handlers in A.15 return 501 Not Implemented.
//! A.16 will supply the real logic and make all four tests green.

use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Method, Request, StatusCode};
use oneshim_storage::sqlite::SqliteStorage;
use std::net::SocketAddr;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::broadcast;
use tower::ServiceExt;

use oneshim_core::config_manager::ConfigManager;
use oneshim_web::app_state::AppState;
use oneshim_web::WebServer;

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Build an `AppState` backed by in-memory SQLite + a temp-file `ConfigManager`
/// so config mutations in one test cannot bleed into others (each test gets its
/// own `TempDir`).
///
/// The `TempDir` is returned alongside the state so the caller keeps the
/// directory alive for the duration of the test.
fn app_state_with_config() -> (AppState, TempDir) {
    let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("in-memory SQLite"));
    let (event_tx, _) = broadcast::channel(16);
    let mut state = AppState::with_core(storage, event_tx);

    let dir = TempDir::new().expect("tempdir");
    let config_path = dir.path().join("config.json");
    let manager = ConfigManager::with_path(config_path).expect("ConfigManager");
    state.core.config_manager = Some(manager);

    (state, dir)
}

/// Build the full Axum router (same as production, minus TCP binding) and
/// attach a loopback `MockConnectInfo` so `require_loopback_client` passes.
fn loopback_app(state: AppState) -> axum::Router {
    WebServer::build_router(state).layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))))
}

// ── Test 1: GET /api/tracking-schedule → 200 with default config ─────────────

/// On a fresh install, `GET /api/tracking-schedule` must return 200 with a
/// JSON body representing the default `TrackingScheduleConfig`:
/// `{ "enabled": false, "windows": [], "timezone": "Local" }`.
#[tokio::test]
async fn get_returns_default_config() {
    let (state, _dir) = app_state_with_config();
    let app = loopback_app(state);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/tracking-schedule")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("oneshot");

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "GET /api/tracking-schedule must return 200 on fresh install"
    );

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    let parsed: serde_json::Value =
        serde_json::from_slice(&body_bytes).expect("body must be valid JSON");

    assert_eq!(
        parsed["enabled"],
        serde_json::Value::Bool(false),
        "default config must have enabled=false"
    );
    assert!(
        parsed["windows"].is_array(),
        "default config must have windows as an array"
    );
    assert_eq!(
        parsed["windows"].as_array().unwrap().len(),
        0,
        "default config must have an empty windows array"
    );
    assert_eq!(
        parsed["timezone"],
        serde_json::Value::String("Local".to_string()),
        "default config must have timezone=Local"
    );
}

// ── Test 2: PUT persists; subsequent GET echoes ───────────────────────────────

/// `PUT /api/tracking-schedule` with a valid body must:
///   1. Return 200 with the echo of the submitted config.
///   2. Persist the change so a subsequent `GET` returns the same value.
#[tokio::test]
async fn put_persists_config() {
    let (state, _dir) = app_state_with_config();
    let app = loopback_app(state);

    let new_config = serde_json::json!({
        "enabled": true,
        "windows": [{
            "start": "22:00",
            "end": "08:00",
            "days_of_week": ["Mon", "Tue", "Wed", "Thu", "Fri"],
            "label": "Night quiet hours"
        }],
        "timezone": "Local"
    });

    // PUT the new config.
    let put_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/api/tracking-schedule")
                .header("content-type", "application/json")
                .body(Body::from(new_config.to_string()))
                .expect("build PUT request"),
        )
        .await
        .expect("PUT oneshot");

    assert_eq!(
        put_response.status(),
        StatusCode::OK,
        "PUT with valid config must return 200"
    );

    let put_body = axum::body::to_bytes(put_response.into_body(), usize::MAX)
        .await
        .expect("read PUT body");
    let put_parsed: serde_json::Value =
        serde_json::from_slice(&put_body).expect("PUT body must be valid JSON");

    assert_eq!(
        put_parsed["enabled"],
        serde_json::Value::Bool(true),
        "PUT response must echo enabled=true"
    );
    assert_eq!(
        put_parsed["windows"]
            .as_array()
            .expect("windows array")
            .len(),
        1,
        "PUT response must echo the one window"
    );
    assert_eq!(
        put_parsed["windows"][0]["label"],
        serde_json::Value::String("Night quiet hours".to_string()),
        "PUT response must echo the window label"
    );

    // GET must now return the persisted value.
    let get_response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/tracking-schedule")
                .body(Body::empty())
                .expect("build GET request"),
        )
        .await
        .expect("GET oneshot");

    assert_eq!(
        get_response.status(),
        StatusCode::OK,
        "GET after PUT must return 200"
    );

    let get_body = axum::body::to_bytes(get_response.into_body(), usize::MAX)
        .await
        .expect("read GET body");
    let get_parsed: serde_json::Value =
        serde_json::from_slice(&get_body).expect("GET body must be valid JSON");

    assert_eq!(
        get_parsed["enabled"],
        serde_json::Value::Bool(true),
        "GET after PUT must reflect persisted enabled=true"
    );
    assert_eq!(
        get_parsed["windows"][0]["start"],
        serde_json::Value::String("22:00".to_string()),
        "GET after PUT must reflect persisted window start"
    );
}

// ── Test 3: PUT with invalid HH:MM → 400 with wire code ──────────────────────

/// `PUT /api/tracking-schedule` with an invalid `HH:MM` value (e.g. `"25:00"`)
/// must return 400 and a JSON error body containing the wire code
/// `"validation.invalid_arguments"`.
#[tokio::test]
async fn put_rejects_invalid_hhmm() {
    let (state, _dir) = app_state_with_config();
    let app = loopback_app(state);

    // "25:00" is an invalid hour — must be rejected.
    let bad_config = serde_json::json!({
        "enabled": true,
        "windows": [{
            "start": "25:00",
            "end": "08:00",
            "days_of_week": ["Mon"],
            "label": "Bad window"
        }],
        "timezone": "Local"
    });

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/api/tracking-schedule")
                .header("content-type", "application/json")
                .body(Body::from(bad_config.to_string()))
                .expect("build PUT request"),
        )
        .await
        .expect("oneshot");

    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "PUT with invalid HH:MM must return 400"
    );

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    let parsed: serde_json::Value =
        serde_json::from_slice(&body_bytes).expect("error body must be valid JSON");

    // Wire code must be present somewhere in the error field.
    let error_text = parsed["error"]
        .as_str()
        .expect("error body must have an 'error' string field");
    assert!(
        error_text.contains("validation.invalid_arguments"),
        "error body must contain wire code 'validation.invalid_arguments'; got: {error_text}"
    );
}

// ── Test 4: GET /api/tracking-schedule/status → 200 with active_now ──────────

/// `GET /api/tracking-schedule/status` with an always-active window configured
/// must return 200 with `active_now: true` and valid optional timestamp fields.
#[tokio::test]
async fn get_status_reflects_configured_windows() {
    let (state, _dir) = app_state_with_config();

    // Configure an always-active window (00:00–23:59 every day) so that
    // `active_now` is deterministically true regardless of when the test runs.
    if let Some(ref manager) = state.core.config_manager {
        manager
            .update_with(|cfg| {
                let json = r#"{
                    "enabled": true,
                    "windows": [{
                        "start": "00:00",
                        "end": "23:59",
                        "days_of_week": ["Mon","Tue","Wed","Thu","Fri","Sat","Sun"],
                        "label": "Always on"
                    }],
                    "timezone": "Local"
                }"#;
                cfg.tracking_schedule =
                    serde_json::from_str(json).expect("valid always-on schedule");
                Ok(())
            })
            .expect("update tracking schedule");
    }

    let app = loopback_app(state);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/tracking-schedule/status")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("oneshot");

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "GET /api/tracking-schedule/status must return 200"
    );

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    let parsed: serde_json::Value =
        serde_json::from_slice(&body_bytes).expect("status body must be valid JSON");

    assert_eq!(
        parsed["active_now"],
        serde_json::Value::Bool(true),
        "active_now must be true when an always-active window is configured"
    );
    assert!(
        parsed["ends_at"].is_string() || parsed["ends_at"].is_null(),
        "ends_at must be a string (RFC 3339) or null"
    );
    assert!(
        parsed["next_starts_at"].is_string() || parsed["next_starts_at"].is_null(),
        "next_starts_at must be a string (RFC 3339) or null"
    );
    assert!(parsed["label"].is_string(), "label must be a string field");
    assert_eq!(
        parsed["label"],
        serde_json::Value::String("Always on".to_string()),
        "label must reflect the active window label"
    );
}
