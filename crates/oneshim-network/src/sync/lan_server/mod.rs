//! LanPeerServer -- Axum HTTPS peer server for LAN sync (Phase 3b-2).
//!
//! Lightweight server that serves changesets to authenticated LAN peers.
//! Uses self-signed TLS certificates (via `axum-server` + `rustls`) and
//! HMAC challenge-response authentication before allowing data exchange.
//!
//! ## Endpoints
//!
//! | Method | Path               | Auth     | Description                          |
//! |--------|--------------------|----------|--------------------------------------|
//! | GET    | `/sync/info`       | None     | Device info + protocol version       |
//! | POST   | `/sync/challenge`  | None     | Request an HMAC nonce for auth       |
//! | POST   | `/sync/verify`     | None     | Submit HMAC response, receive token  |
//! | GET    | `/sync/pull`       | Token    | Return encrypted changesets since HLC|
//! | POST   | `/sync/push`       | Token    | Receive encrypted changeset from peer|
//!
//! ## Authentication Flow
//!
//! 1. Client POSTs `/sync/challenge` with `{ "device_id": "..." }`
//! 2. Server returns `{ "nonce": "<hex>" }`
//! 3. Client computes `HMAC-SHA256(nonce, Argon2id(passphrase, peer_salt))`
//! 4. Client POSTs `/sync/verify` with `{ "device_id": "...", "nonce": "<hex>", "response": "<hex>" }`
//! 5. Server verifies and returns `{ "session_token": "<hex>", "expires_in_secs": 3600 }`
//! 6. Client includes `Authorization: Bearer <token>` header on `/sync/push` and `/sync/pull`
//!
//! ## TLS
//!
//! When valid PEM cert/key are provided, the server binds with TLS via `axum-server`.
//! If the cert/key are invalid or empty (e.g., in tests), falls back to plain HTTP.
//!
//! Requires the `lan-sync` feature flag.

mod handlers;
mod session;
mod tls;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use tracing::{debug, error, info, warn};

use oneshim_core::error::CoreError;
use oneshim_core::models::sync::ChangeSet;

use handlers::build_router;
use session::SessionStore;
use tls::try_build_tls_config;

pub use handlers::{
    ChallengeRequest, ChallengeResponse, DeviceInfoResponse, PullQuery, VerifyRequest,
    VerifyResponse,
};

const PROTOCOL_VERSION: &str = "1";

/// Maximum request body size: 16 MiB.
const MAX_BODY_SIZE: usize = 16 * 1024 * 1024;

/// Maximum number of outbound changesets held in memory.
/// When full, the oldest entries are evicted to make room.
const MAX_OUTBOUND_QUEUE: usize = 1000;

/// Session token TTL: 1 hour.
const SESSION_TTL: Duration = Duration::from_secs(3600);

/// Maximum number of active sessions (prevents memory exhaustion).
const MAX_SESSIONS: usize = 100;

/// Maximum number of pending nonces (prevents memory exhaustion).
const MAX_PENDING_NONCES: usize = 200;

/// Nonce TTL: 60 seconds (nonces must be used quickly).
const NONCE_TTL: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// Server state
// ---------------------------------------------------------------------------

/// Shared state for the Axum server.
#[derive(Clone)]
struct ServerState {
    device_id: String,
    device_name: String,
    fingerprint: String,
    passphrase: String,
    session_store: SessionStore,
    /// Changesets received via push, keyed by origin_device_id.
    /// In a full implementation this would be backed by the storage layer.
    received_changesets: Arc<RwLock<Vec<ChangeSet>>>,
    /// Pending outbound changesets for peers to pull.
    /// Populated by the SyncEngine when local changes are extracted.
    outbound_changesets: Arc<RwLock<Vec<ChangeSet>>>,
}

/// LAN peer server state.
pub struct LanPeerServer {
    device_id: String,
    device_name: String,
    passphrase: String,
    port: u16,
    fingerprint: String,
    tls_enabled: bool,
    /// Changesets received from peers via POST /sync/push.
    received_changesets: Arc<RwLock<Vec<ChangeSet>>>,
    /// Changesets queued for peers to pull via GET /sync/pull.
    outbound_changesets: Arc<RwLock<Vec<ChangeSet>>>,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    server_handle: Option<tokio::task::JoinHandle<()>>,
    running: bool,
}

impl LanPeerServer {
    /// Create a new peer server (not yet started).
    pub fn new(
        device_id: String,
        device_name: String,
        passphrase: String,
        fingerprint: String,
    ) -> Self {
        Self {
            device_id,
            device_name,
            passphrase,
            port: 0,
            fingerprint,
            tls_enabled: false,
            received_changesets: Arc::new(RwLock::new(Vec::new())),
            outbound_changesets: Arc::new(RwLock::new(Vec::new())),
            shutdown_tx: None,
            server_handle: None,
            running: false,
        }
    }

    /// Start the HTTPS server on the specified port (0 = ephemeral OS-assigned).
    ///
    /// Returns the actual bound port. The server runs in a background tokio task
    /// and can be stopped via `stop()`.
    ///
    /// ## TLS
    ///
    /// If valid PEM-encoded `cert_pem` and `key_pem` are provided, the server
    /// binds with TLS via `axum-server` + `rustls`. Otherwise falls back to
    /// plain HTTP (e.g., in unit tests or when cert generation fails).
    pub async fn start(
        &mut self,
        cert_pem: &[u8],
        key_pem: &[u8],
        port: u16,
    ) -> Result<u16, CoreError> {
        if self.running {
            return Ok(self.port);
        }

        let state = ServerState {
            device_id: self.device_id.clone(),
            device_name: self.device_name.clone(),
            fingerprint: self.fingerprint.clone(),
            passphrase: self.passphrase.clone(),
            session_store: SessionStore::new(),
            received_changesets: Arc::clone(&self.received_changesets),
            outbound_changesets: Arc::clone(&self.outbound_changesets),
        };

        let app = build_router(state);
        let addr = SocketAddr::from(([0, 0, 0, 0], port));

        // Try TLS, fall back to plain HTTP on failure
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let tls_config = try_build_tls_config(cert_pem, key_pem).await;
        let tls_enabled = tls_config.is_some();

        let actual_port = if let Some(config) = tls_config {
            // TLS path via axum-server.
            // Bind a TcpListener first to discover the actual port (important
            // when the requested port is 0), then hand the std listener to
            // axum-server for TLS wrapping.
            let listener =
                tokio::net::TcpListener::bind(addr)
                    .await
                    .map_err(|e| CoreError::Internal {
                        code: oneshim_core::error_codes::InternalCode::Generic,
                        message: format!("failed to bind LAN server on {addr}: {e}"),
                    })?;
            let actual_addr = listener.local_addr().map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("failed to get local addr: {e}"),
            })?;
            let bound_port = actual_addr.port();

            // Convert to std listener for axum-server
            let std_listener = listener.into_std().map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("failed to convert listener: {e}"),
            })?;

            let axum_handle = axum_server::Handle::new();
            let shutdown_handle = axum_handle.clone();

            let tls_server = axum_server::from_tcp_rustls(std_listener, config).map_err(|e| {
                CoreError::Internal {
                    code: oneshim_core::error_codes::InternalCode::Generic,
                    message: format!("TLS server init: {e}"),
                }
            })?;

            let handle = tokio::task::spawn(async move {
                // Shutdown listener: wait for the oneshot then trigger graceful shutdown
                tokio::spawn(async move {
                    if let Err(e) = shutdown_rx.await {
                        debug!("operation failed: {e}");
                    }
                    shutdown_handle.graceful_shutdown(Some(Duration::from_secs(2)));
                });

                let serve_result = tls_server
                    .handle(axum_handle)
                    .serve(app.into_make_service())
                    .await;

                if let Err(e) = serve_result {
                    error!("LAN peer server (TLS) error: {e}");
                }
            });

            self.server_handle = Some(handle);
            bound_port
        } else {
            // Plain HTTP fallback
            let listener =
                tokio::net::TcpListener::bind(addr)
                    .await
                    .map_err(|e| CoreError::Internal {
                        code: oneshim_core::error_codes::InternalCode::Generic,
                        message: format!("failed to bind LAN server on {addr}: {e}"),
                    })?;

            let actual_port = listener
                .local_addr()
                .map_err(|e| CoreError::Internal {
                    code: oneshim_core::error_codes::InternalCode::Generic,
                    message: format!("failed to get local addr: {e}"),
                })?
                .port();

            let handle = tokio::task::spawn(async move {
                let serve_result = axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        if let Err(e) = shutdown_rx.await {
                            debug!("operation failed: {e}");
                        }
                    })
                    .await;

                if let Err(e) = serve_result {
                    error!("LAN peer server (HTTP) error: {e}");
                }
            });

            self.server_handle = Some(handle);
            actual_port
        };

        self.port = actual_port;
        self.shutdown_tx = Some(shutdown_tx);
        self.tls_enabled = tls_enabled;
        self.running = true;

        info!(
            port = actual_port,
            tls = tls_enabled,
            device_id = %self.device_id,
            "LAN peer server started"
        );

        Ok(actual_port)
    }

    /// Stop the server gracefully.
    ///
    /// Sends a shutdown signal and spawns a background task to abort
    /// the server if it does not complete within a grace period. This
    /// avoids the race between the oneshot signal and an immediate
    /// `abort()` while remaining safe to call from both sync and async
    /// contexts.
    pub fn stop(&mut self) {
        if !self.running {
            return;
        }

        // Signal graceful shutdown
        if let Some(tx) = self.shutdown_tx.take() {
            if let Err(_e) = tx.send(()) {
                debug!("channel send failed: receiver dropped");
            }
        }

        // Give the server task a grace period before aborting.
        // Spawn a fire-and-forget task so we don't block the caller.
        if let Some(handle) = self.server_handle.take() {
            if let Ok(_rt) = tokio::runtime::Handle::try_current() {
                tokio::spawn(async move {
                    match tokio::time::timeout(std::time::Duration::from_secs(2), handle).await {
                        Ok(_) => debug!("LAN peer server task completed gracefully"),
                        Err(_) => warn!("LAN peer server task did not complete in time"),
                    }
                });
            } else {
                // No tokio runtime available (e.g., during Drop) -- abort immediately
                handle.abort();
            }
        }

        self.running = false;
        info!("LAN peer server stopped");
    }

    /// Get the bound port.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Get the certificate fingerprint.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// Check if server is running.
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Check if TLS is enabled.
    pub fn is_tls_enabled(&self) -> bool {
        self.tls_enabled
    }

    /// Enqueue a changeset for peers to pull.
    ///
    /// Called by the SyncEngine when local changes are extracted.
    /// If the queue exceeds `MAX_OUTBOUND_QUEUE`, the oldest entries
    /// are evicted to make room.
    pub fn enqueue_outbound(&self, changeset: ChangeSet) {
        let mut queue = self.outbound_changesets.write();
        queue.push(changeset);
        if queue.len() > MAX_OUTBOUND_QUEUE {
            let excess = queue.len() - MAX_OUTBOUND_QUEUE;
            queue.drain(..excess);
            debug!(
                evicted = excess,
                "outbound queue exceeded capacity, evicted oldest entries"
            );
        }
    }

    /// Drain received changesets (from peers pushing to us).
    ///
    /// Called by the SyncEngine to process incoming peer data.
    pub fn drain_received(&self) -> Vec<ChangeSet> {
        let mut store = self.received_changesets.write();
        std::mem::take(&mut *store)
    }
}

impl Drop for LanPeerServer {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests;
