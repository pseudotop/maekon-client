//! Integration test: WebServer startup, HTTP request, and graceful shutdown.
//!
//! Verifies that `WebServer` can:
//! 1. Build a router from a minimal `AppState` with in-memory SQLite
//! 2. Bind to an ephemeral port
//! 3. Respond to a GET /api/metrics request with 200
//! 4. Shut down cleanly via the `watch` channel

use oneshim_core::config::WebConfig;
use oneshim_storage::sqlite::SqliteStorage;
use oneshim_web::WebServer;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use tokio::sync::{oneshot, watch};
use tracing::debug;

#[tokio::test]
async fn web_server_starts_responds_and_shuts_down() {
    let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());

    // Use port 0 via a high ephemeral base to avoid collisions; the server's
    // fallback logic will find an available port.
    let config = WebConfig {
        port: 19090,
        ..WebConfig::default()
    };

    let bound_port_state = Arc::new(AtomicU16::new(0));
    let (bound_port_tx, bound_port_rx) = oneshot::channel();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let server = WebServer::new(storage, config)
        .with_bound_port_state(bound_port_state.clone())
        .with_bound_port_notifier(bound_port_tx);

    // Start the server in a background task
    let server_handle = tokio::spawn(async move { server.run(shutdown_rx).await });

    // Wait for the server to bind (with timeout)
    let port = tokio::time::timeout(std::time::Duration::from_secs(5), bound_port_rx)
        .await
        .expect("timed out waiting for server to bind")
        .expect("bound_port_rx channel dropped");

    assert!(port > 0, "bound port should be non-zero");
    assert_eq!(bound_port_state.load(Ordering::Relaxed), port);

    // Send a real HTTP request to the focus/metrics endpoint (returns a JSON object)
    let url = format!("http://127.0.0.1:{}/api/focus/metrics", port);
    let response = reqwest::get(&url)
        .await
        .expect("HTTP GET /api/focus/metrics failed");

    assert_eq!(
        response.status().as_u16(),
        200,
        "expected 200 from /api/focus/metrics"
    );

    // Verify the response body is valid JSON with the expected structure
    let body = response.text().await.expect("failed to read response body");
    let parsed: serde_json::Value =
        serde_json::from_str(&body).expect("response body is not valid JSON");
    assert!(
        parsed.is_object(),
        "focus/metrics response should be a JSON object"
    );
    assert!(
        parsed["today"]["date"].is_string(),
        "response should contain today.date"
    );

    // Graceful shutdown
    if let Err(e) = shutdown_tx.send(true) {
        debug!("channel send failed: {e}");
    }
    let server_result = tokio::time::timeout(std::time::Duration::from_secs(5), server_handle)
        .await
        .expect("timed out waiting for server shutdown")
        .expect("server task panicked");

    assert!(
        server_result.is_ok(),
        "server exited with error: {:?}",
        server_result.err()
    );
}

#[tokio::test]
async fn web_server_router_resolves_focus_routes() {
    // Verify that the router can be built and routes are registered correctly
    // without starting TCP, using tower::ServiceExt::oneshot.
    use axum::body::Body;
    use axum::extract::connect_info::MockConnectInfo;
    use axum::http::{Request, StatusCode};
    use std::net::SocketAddr;
    use tower::ServiceExt;

    let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
    let (event_tx, _) = tokio::sync::broadcast::channel(16);

    let state = oneshim_web::AppState::with_core(storage, event_tx);

    let app = WebServer::build_router(state)
        .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))));

    // Verify focus/metrics route
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/focus/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Verify coaching/history route
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/coaching/history")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Verify coaching/goals route
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/coaching/goals")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}
