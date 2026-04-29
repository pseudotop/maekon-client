//! Reference sync server for testing RemoteSyncTransport.
//!
//! A minimal in-memory Axum HTTP server that implements the three endpoints
//! expected by `RemoteSyncTransport`:
//!
//! | Method | Path          | Description                                   |
//! |--------|---------------|-----------------------------------------------|
//! | POST   | `/sync/push`  | Receive encrypted changeset, store in memory  |
//! | GET    | `/sync/pull`  | Return changesets newer than HLC watermark    |
//! | GET    | `/sync/peers` | Return list of known peers                    |
//!
//! **This is a development/testing tool, not a production server.**
//! All state is in-memory and lost on restart.
//!
//! # Usage
//!
//! ```rust,ignore
//! use oneshim_network::sync::reference_server::run_reference_server;
//!
//! let handle = run_reference_server(0, "my-secret-token").await.unwrap();
//! let port = handle.port();
//! // ... use RemoteSyncTransport pointed at http://127.0.0.1:{port} ...
//! handle.shutdown().await;
//! ```

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use parking_lot::Mutex;
use serde::Deserialize;
use tracing::{debug, info, warn};

use oneshim_core::models::sync::{ChangeSet, PeerInfo};
use oneshim_core::sync::Hlc;

use super::sync_crypto;

/// Maximum request body size: 16 MiB.
const MAX_BODY_SIZE: usize = 16 * 1024 * 1024;

/// Shared server state.
#[derive(Clone)]
struct ServerState {
    /// Bearer token or API key accepted for authentication.
    auth_token: String,
    /// Passphrase for AES-256-GCM encryption/decryption.
    passphrase: String,
    /// Changesets stored per origin device ID.
    store: Arc<Mutex<HashMap<String, Vec<ChangeSet>>>>,
    /// Known peers (populated from push metadata).
    peers: Arc<Mutex<HashMap<String, PeerInfo>>>,
}

/// Query parameters for `GET /sync/pull`.
#[derive(Debug, Deserialize)]
struct PullQuery {
    since_wall_ms: Option<u64>,
    since_counter: Option<u32>,
    device_id: Option<String>,
}

/// Handle to a running reference server.
///
/// Dropping the handle does NOT stop the server; call `shutdown()` explicitly.
pub struct ReferenceServerHandle {
    port: u16,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    server_handle: Option<tokio::task::JoinHandle<()>>,
}

impl ReferenceServerHandle {
    /// The port the server is listening on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The base URL for connecting clients (e.g., `http://127.0.0.1:12345`).
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Gracefully shut down the server.
    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            if let Err(e) = tx.send(()) {
                debug!("channel send failed: {e:?}");
            }
        }
        if let Some(handle) = self.server_handle.take() {
            if let Err(e) = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await {
                debug!("timeout failed: {e}");
            }
        }
    }
}

/// Start a reference sync server on the given port.
///
/// - `port`: TCP port to bind. Pass `0` for an ephemeral OS-assigned port.
/// - `auth_token`: The token value accepted for both Bearer and X-Api-Key auth.
/// - `passphrase`: Passphrase for AES-256-GCM encrypt/decrypt of changeset bodies.
///
/// Returns a `ReferenceServerHandle` with the actual bound port.
pub async fn run_reference_server(
    port: u16,
    auth_token: &str,
    passphrase: &str,
) -> Result<ReferenceServerHandle, std::io::Error> {
    let state = ServerState {
        auth_token: auth_token.to_string(),
        passphrase: passphrase.to_string(),
        store: Arc::new(Mutex::new(HashMap::new())),
        peers: Arc::new(Mutex::new(HashMap::new())),
    };

    let app = Router::new()
        .route("/sync/push", post(handle_push))
        .route("/sync/pull", get(handle_pull))
        .route("/sync/peers", get(handle_peers))
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY_SIZE))
        .with_state(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let actual_port = listener.local_addr()?.port();

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

    let server_handle = tokio::spawn(async move {
        let result = axum::serve(listener, app)
            .with_graceful_shutdown(async {
                if let Err(e) = shutdown_rx.await {
                    debug!("operation failed: {e}");
                }
            })
            .await;
        if let Err(e) = result {
            warn!("reference sync server error: {e}");
        }
    });

    info!(port = actual_port, "reference sync server started");

    Ok(ReferenceServerHandle {
        port: actual_port,
        shutdown_tx: Some(shutdown_tx),
        server_handle: Some(server_handle),
    })
}

// ---------------------------------------------------------------------------
// Auth middleware helper
// ---------------------------------------------------------------------------

/// Validate Bearer token or X-Api-Key header against the configured token.
fn check_auth(headers: &HeaderMap, expected: &str) -> Result<(), StatusCode> {
    // Check Bearer token
    if let Some(auth) = headers.get("authorization") {
        if let Ok(value) = auth.to_str() {
            if let Some(token) = value.strip_prefix("Bearer ") {
                if token == expected {
                    return Ok(());
                }
            }
        }
        return Err(StatusCode::UNAUTHORIZED);
    }

    // Check API key
    if let Some(api_key) = headers.get("x-api-key") {
        if let Ok(value) = api_key.to_str() {
            if value == expected {
                return Ok(());
            }
        }
        return Err(StatusCode::UNAUTHORIZED);
    }

    Err(StatusCode::UNAUTHORIZED)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /sync/push -- receive an encrypted changeset and store it.
async fn handle_push(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    if let Err(status) = check_auth(&headers, &state.auth_token) {
        return (status, "Unauthorized").into_response();
    }

    if body.is_empty() {
        return (StatusCode::BAD_REQUEST, "empty body").into_response();
    }

    // Decrypt
    let plaintext = match sync_crypto::decrypt(&state.passphrase, &body) {
        Ok(pt) => pt,
        Err(e) => {
            warn!("reference server push decrypt failed: {e}");
            return (StatusCode::BAD_REQUEST, "decryption failed").into_response();
        }
    };

    // Deserialize
    let changeset: ChangeSet = match serde_json::from_slice(&plaintext) {
        Ok(cs) => cs,
        Err(e) => {
            warn!("reference server push deserialize failed: {e}");
            return (StatusCode::BAD_REQUEST, "invalid changeset JSON").into_response();
        }
    };

    let device_id = changeset.origin_device_id.clone();
    let device_name = changeset.origin_device_name.clone();
    let watermark = changeset.watermark.clone();

    debug!(
        origin = %device_id,
        rows = changeset.row_count(),
        "reference server received push"
    );

    // Store changeset per device
    {
        let mut store = state.store.lock();
        store.entry(device_id.clone()).or_default().push(changeset);
    }

    // Update peer info
    {
        let mut peers = state.peers.lock();
        let now = chrono::Utc::now().to_rfc3339();
        peers.insert(
            device_id.clone(),
            PeerInfo {
                device_id,
                device_name,
                last_sync_at: now,
                watermark,
            },
        );
    }

    StatusCode::OK.into_response()
}

/// GET /sync/pull -- return a single merged changeset of all rows newer than watermark.
///
/// The reference server merges all stored changesets (from all devices except the
/// requester) that have a watermark newer than the `since` parameter into one
/// response changeset. The response body is AES-256-GCM encrypted.
///
/// Returns 204 if no new data is available.
async fn handle_pull(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(params): Query<PullQuery>,
) -> impl IntoResponse {
    if let Err(status) = check_auth(&headers, &state.auth_token) {
        return (status, "Unauthorized").into_response();
    }

    let since_wall = params.since_wall_ms.unwrap_or(0);
    let since_counter = params.since_counter.unwrap_or(0);
    let requesting_device = params.device_id.unwrap_or_default();

    let store = state.store.lock();

    // Collect changesets from all devices (except requester) newer than watermark
    let mut merged = ChangeSet {
        origin_device_id: "reference-server".to_string(),
        origin_device_name: "Reference Sync Server".to_string(),
        watermark: Hlc {
            wall_ms: since_wall,
            counter: since_counter,
            device_id: "reference-server".to_string(),
        },
        ..Default::default()
    };

    let mut found_newer = false;

    for (device_id, changesets) in store.iter() {
        // Skip the requesting device's own changesets
        if *device_id == requesting_device {
            continue;
        }

        for cs in changesets {
            let is_newer = cs.watermark.wall_ms > since_wall
                || (cs.watermark.wall_ms == since_wall && cs.watermark.counter > since_counter);

            if is_newer {
                found_newer = true;
                // Merge rows into the response changeset
                merged.segments.extend(cs.segments.iter().cloned());
                merged.regimes.extend(cs.regimes.iter().cloned());
                merged.overrides.extend(cs.overrides.iter().cloned());
                merged.embeddings.extend(cs.embeddings.iter().cloned());
                merged.suggestions.extend(cs.suggestions.iter().cloned());
                merged
                    .param_snapshots
                    .extend(cs.param_snapshots.iter().cloned());
                merged.preferences.extend(cs.preferences.iter().cloned());

                // Track the highest watermark
                if cs.watermark > merged.watermark {
                    merged.watermark = cs.watermark.clone();
                }
            }
        }
    }

    drop(store);

    if !found_newer {
        return StatusCode::NO_CONTENT.into_response();
    }

    // Serialize and encrypt
    let json = match serde_json::to_vec(&merged) {
        Ok(j) => j,
        Err(e) => {
            warn!("reference server serialize failed: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let encrypted = match sync_crypto::encrypt(&state.passphrase, &json) {
        Ok(enc) => enc,
        Err(e) => {
            warn!("reference server encrypt failed: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    debug!(
        rows = merged.row_count(),
        bytes = encrypted.len(),
        "reference server serving pull"
    );

    (
        StatusCode::OK,
        [("content-type", "application/octet-stream")],
        encrypted,
    )
        .into_response()
}

/// GET /sync/peers -- return list of known peers as JSON.
async fn handle_peers(State(state): State<ServerState>, headers: HeaderMap) -> impl IntoResponse {
    if let Err(status) = check_auth(&headers, &state.auth_token) {
        return (status, "Unauthorized").into_response();
    }

    let peers = state.peers.lock();
    let peer_list: Vec<PeerInfo> = peers.values().cloned().collect();

    debug!(count = peer_list.len(), "reference server serving peers");

    (StatusCode::OK, Json(peer_list)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use oneshim_core::models::sync::ChangeSetKind;

    const TEST_TOKEN: &str = "test-token-abc";
    const TEST_PASSPHRASE: &str = "test-passphrase-xyz";

    fn test_changeset(device_id: &str, wall_ms: u64) -> ChangeSet {
        ChangeSet {
            kind: ChangeSetKind::Data,
            origin_device_id: device_id.to_string(),
            origin_device_name: format!("Device {device_id}"),
            watermark: Hlc {
                wall_ms,
                counter: 1,
                device_id: device_id.to_string(),
            },
            segments: vec![serde_json::json!({"id": format!("seg-{device_id}")})],
            ..Default::default()
        }
    }

    async fn start_server() -> ReferenceServerHandle {
        run_reference_server(0, TEST_TOKEN, TEST_PASSPHRASE)
            .await
            .unwrap()
    }

    fn bearer_client() -> reqwest::Client {
        reqwest::Client::new()
    }

    #[tokio::test]
    async fn push_requires_auth() {
        let handle = start_server().await;
        let client = bearer_client();

        // No auth header => 401
        let resp = client
            .post(format!("{}/sync/push", handle.base_url()))
            .body(vec![1, 2, 3])
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 401);

        // Wrong token => 401
        let resp = client
            .post(format!("{}/sync/push", handle.base_url()))
            .header("Authorization", "Bearer wrong-token")
            .body(vec![1, 2, 3])
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 401);

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn push_accepts_bearer_token() {
        let handle = start_server().await;
        let client = bearer_client();

        let cs = test_changeset("dev-a", 100);
        let json = serde_json::to_vec(&cs).unwrap();
        let encrypted = sync_crypto::encrypt(TEST_PASSPHRASE, &json).unwrap();

        let resp = client
            .post(format!("{}/sync/push", handle.base_url()))
            .header("Authorization", format!("Bearer {TEST_TOKEN}"))
            .body(encrypted)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn push_accepts_api_key() {
        let handle = start_server().await;
        let client = bearer_client();

        let cs = test_changeset("dev-a", 100);
        let json = serde_json::to_vec(&cs).unwrap();
        let encrypted = sync_crypto::encrypt(TEST_PASSPHRASE, &json).unwrap();

        let resp = client
            .post(format!("{}/sync/push", handle.base_url()))
            .header("X-Api-Key", TEST_TOKEN)
            .body(encrypted)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn pull_returns_204_when_empty() {
        let handle = start_server().await;
        let client = bearer_client();

        let resp = client
            .get(format!(
                "{}/sync/pull?since_wall_ms=0&since_counter=0",
                handle.base_url()
            ))
            .header("Authorization", format!("Bearer {TEST_TOKEN}"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 204);

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn push_then_pull_roundtrip() {
        let handle = start_server().await;
        let client = bearer_client();

        // Device A pushes
        let cs_a = test_changeset("dev-a", 100);
        let json = serde_json::to_vec(&cs_a).unwrap();
        let encrypted = sync_crypto::encrypt(TEST_PASSPHRASE, &json).unwrap();

        let resp = client
            .post(format!("{}/sync/push", handle.base_url()))
            .header("Authorization", format!("Bearer {TEST_TOKEN}"))
            .body(encrypted)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        // Device B pulls (since=0 should see A's data)
        let resp = client
            .get(format!(
                "{}/sync/pull?since_wall_ms=0&since_counter=0&device_id=dev-b",
                handle.base_url()
            ))
            .header("Authorization", format!("Bearer {TEST_TOKEN}"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let body = resp.bytes().await.unwrap();
        let decrypted = sync_crypto::decrypt(TEST_PASSPHRASE, &body).unwrap();
        let pulled: ChangeSet = serde_json::from_slice(&decrypted).unwrap();
        assert_eq!(pulled.segments.len(), 1);

        // Device A pulls its own data => should get 204 (filtered out)
        let resp = client
            .get(format!(
                "{}/sync/pull?since_wall_ms=0&since_counter=0&device_id=dev-a",
                handle.base_url()
            ))
            .header("Authorization", format!("Bearer {TEST_TOKEN}"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 204);

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn peers_populated_by_push() {
        let handle = start_server().await;
        let client = bearer_client();

        // Initially empty
        let resp = client
            .get(format!("{}/sync/peers", handle.base_url()))
            .header("Authorization", format!("Bearer {TEST_TOKEN}"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let peers: Vec<PeerInfo> = resp.json().await.unwrap();
        assert!(peers.is_empty());

        // Push from two devices
        for dev in &["dev-a", "dev-b"] {
            let cs = test_changeset(dev, 100);
            let json = serde_json::to_vec(&cs).unwrap();
            let encrypted = sync_crypto::encrypt(TEST_PASSPHRASE, &json).unwrap();
            client
                .post(format!("{}/sync/push", handle.base_url()))
                .header("Authorization", format!("Bearer {TEST_TOKEN}"))
                .body(encrypted)
                .send()
                .await
                .unwrap();
        }

        // Now should have 2 peers
        let resp = client
            .get(format!("{}/sync/peers", handle.base_url()))
            .header("Authorization", format!("Bearer {TEST_TOKEN}"))
            .send()
            .await
            .unwrap();
        let peers: Vec<PeerInfo> = resp.json().await.unwrap();
        assert_eq!(peers.len(), 2);

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn pull_watermark_filtering() {
        let handle = start_server().await;
        let client = bearer_client();

        // Push two changesets with different watermarks
        for (dev, wall_ms) in &[("dev-a", 100u64), ("dev-a", 200)] {
            let cs = test_changeset(dev, *wall_ms);
            let json = serde_json::to_vec(&cs).unwrap();
            let encrypted = sync_crypto::encrypt(TEST_PASSPHRASE, &json).unwrap();
            client
                .post(format!("{}/sync/push", handle.base_url()))
                .header("Authorization", format!("Bearer {TEST_TOKEN}"))
                .body(encrypted)
                .send()
                .await
                .unwrap();
        }

        // Pull with since_wall_ms=150 should only see the wall_ms=200 changeset
        let resp = client
            .get(format!(
                "{}/sync/pull?since_wall_ms=150&since_counter=0&device_id=dev-b",
                handle.base_url()
            ))
            .header("Authorization", format!("Bearer {TEST_TOKEN}"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let body = resp.bytes().await.unwrap();
        let decrypted = sync_crypto::decrypt(TEST_PASSPHRASE, &body).unwrap();
        let pulled: ChangeSet = serde_json::from_slice(&decrypted).unwrap();
        // Only 1 segment from the wall_ms=200 changeset
        assert_eq!(pulled.segments.len(), 1);

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn base_url_format() {
        let handle = start_server().await;
        assert!(handle.base_url().starts_with("http://127.0.0.1:"));
        assert!(handle.port() > 0);
        handle.shutdown().await;
    }
}
