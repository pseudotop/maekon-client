use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use oneshim_core::models::sync::ChangeSet;
use oneshim_core::sync::Hlc;

use super::session::SessionStore;
use super::{ServerState, MAX_BODY_SIZE, PROTOCOL_VERSION, SESSION_TTL};
use crate::sync::lan_crypto;
use crate::sync::sync_crypto;

/// Device info returned by `GET /sync/info`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfoResponse {
    pub device_id: String,
    pub device_name: String,
    pub fingerprint: String,
    pub protocol_version: String,
}

/// Request body for `POST /sync/challenge`.
#[derive(Debug, Serialize, Deserialize)]
pub struct ChallengeRequest {
    pub device_id: String,
}

/// Response from `POST /sync/challenge`.
#[derive(Debug, Serialize, Deserialize)]
pub struct ChallengeResponse {
    pub nonce: String,
}

/// Request body for `POST /sync/verify`.
#[derive(Debug, Serialize, Deserialize)]
pub struct VerifyRequest {
    pub device_id: String,
    pub nonce: String,
    pub response: String,
}

/// Response from `POST /sync/verify`.
#[derive(Debug, Serialize, Deserialize)]
pub struct VerifyResponse {
    pub session_token: String,
    pub expires_in_secs: u64,
}

/// Query parameters for `GET /sync/pull`.
#[derive(Debug, Deserialize)]
pub struct PullQuery {
    /// Wall-clock milliseconds of the HLC watermark.
    pub since_wall_ms: Option<u64>,
    /// Counter component of the HLC watermark.
    pub since_counter: Option<u32>,
    /// Requesting device's ID.
    pub device_id: Option<String>,
}

pub(super) fn build_router(state: ServerState) -> Router {
    Router::new()
        // Public endpoints (no auth required)
        .route("/sync/info", get(handle_info))
        .route("/sync/challenge", post(handle_challenge))
        .route("/sync/verify", post(handle_verify))
        // Protected endpoints (session token required)
        .route("/sync/pull", get(handle_pull))
        .route("/sync/push", post(handle_push))
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY_SIZE))
        .with_state(state)
}

/// GET /sync/info -- return device info and protocol version.
async fn handle_info(State(state): State<ServerState>) -> Json<DeviceInfoResponse> {
    Json(DeviceInfoResponse {
        device_id: state.device_id.clone(),
        device_name: state.device_name.clone(),
        fingerprint: state.fingerprint.clone(),
        protocol_version: PROTOCOL_VERSION.to_string(),
    })
}

/// POST /sync/challenge -- generate and return a random nonce.
///
/// The peer must compute `HMAC-SHA256(nonce, derived_key)` and submit it
/// via `/sync/verify` to obtain a session token.
async fn handle_challenge(
    State(state): State<ServerState>,
    Json(req): Json<ChallengeRequest>,
) -> impl IntoResponse {
    if req.device_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "device_id required"})),
        )
            .into_response();
    }

    let nonce = state.session_store.create_nonce(&req.device_id);
    let nonce_hex = hex::encode(&nonce);

    debug!(
        peer_device_id = %req.device_id,
        "challenge nonce issued"
    );

    Json(ChallengeResponse { nonce: nonce_hex }).into_response()
}

/// POST /sync/verify -- verify the HMAC challenge response and issue a session token.
async fn handle_verify(
    State(state): State<ServerState>,
    Json(req): Json<VerifyRequest>,
) -> impl IntoResponse {
    if req.device_id.is_empty() || req.nonce.is_empty() || req.response.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "device_id, nonce, and response required"})),
        )
            .into_response();
    }

    // Consume the pending nonce (one-time use)
    let (nonce_bytes, expected_peer_id) = match state.session_store.take_nonce(&req.nonce) {
        Some(v) => v,
        None => {
            warn!(
                peer_device_id = %req.device_id,
                "verify failed: unknown or expired nonce"
            );
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "invalid or expired nonce"})),
            )
                .into_response();
        }
    };

    // Verify the device_id matches
    if req.device_id != expected_peer_id {
        warn!(
            expected = %expected_peer_id,
            actual = %req.device_id,
            "verify failed: device_id mismatch"
        );
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "device_id mismatch"})),
        )
            .into_response();
    }

    // Decode the HMAC response
    let response_bytes = match hex::decode(&req.response) {
        Ok(b) => b,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid hex response"})),
            )
                .into_response();
        }
    };

    // Verify the HMAC
    let valid = match lan_crypto::verify_challenge_response(
        &nonce_bytes,
        &response_bytes,
        &state.passphrase,
        &state.device_id, // local device (server)
        &req.device_id,   // peer device (client)
    ) {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, "HMAC verification error");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "verification error"})),
            )
                .into_response();
        }
    };

    if !valid {
        warn!(
            peer_device_id = %req.device_id,
            "verify failed: HMAC mismatch (wrong passphrase?)"
        );
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "authentication failed"})),
        )
            .into_response();
    }

    // Issue a session token
    let token = state.session_store.create_session(&req.device_id);

    info!(
        peer_device_id = %req.device_id,
        "peer authenticated via HMAC challenge-response"
    );

    Json(VerifyResponse {
        session_token: token,
        expires_in_secs: SESSION_TTL.as_secs(),
    })
    .into_response()
}

/// Extract and validate the session token from the Authorization header.
///
/// Expects: `Authorization: Bearer <token_hex>`
fn extract_session_token(
    headers: &HeaderMap,
    session_store: &SessionStore,
) -> Result<(), StatusCode> {
    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let token = auth_header.strip_prefix("Bearer ").unwrap_or("");

    if token.is_empty() || !session_store.validate_token(token) {
        return Err(StatusCode::UNAUTHORIZED);
    }

    Ok(())
}

/// GET /sync/pull -- return encrypted changesets newer than the given HLC.
///
/// Requires a valid session token via `Authorization: Bearer <token>`.
/// Query parameters: `since_wall_ms`, `since_counter`, `device_id`.
/// Response: AES-256-GCM encrypted JSON array of changesets, or 204 if none.
async fn handle_pull(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(params): Query<PullQuery>,
) -> impl IntoResponse {
    // Authenticate
    if let Err(status) = extract_session_token(&headers, &state.session_store) {
        return (status, "unauthorized").into_response();
    }

    let since = Hlc {
        wall_ms: params.since_wall_ms.unwrap_or(0),
        counter: params.since_counter.unwrap_or(0),
        device_id: params.device_id.unwrap_or_default(),
    };

    // Filter outbound changesets newer than the given watermark
    let outbound = state.outbound_changesets.read();
    let newer: Vec<&ChangeSet> = outbound
        .iter()
        .filter(|cs| {
            cs.watermark.wall_ms > since.wall_ms
                || (cs.watermark.wall_ms == since.wall_ms && cs.watermark.counter > since.counter)
        })
        .collect();

    if newer.is_empty() {
        return StatusCode::NO_CONTENT.into_response();
    }

    // Serialize and encrypt
    let json = match serde_json::to_vec(&newer) {
        Ok(j) => j,
        Err(e) => {
            warn!("failed to serialize changesets: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let encrypted = match sync_crypto::encrypt(&state.passphrase, &json) {
        Ok(enc) => enc,
        Err(e) => {
            warn!("failed to encrypt changesets: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    debug!(
        count = newer.len(),
        bytes = encrypted.len(),
        "serving pull request"
    );

    (
        StatusCode::OK,
        [("content-type", "application/octet-stream")],
        encrypted,
    )
        .into_response()
}

/// POST /sync/push -- receive an encrypted changeset from a peer.
///
/// Requires a valid session token via `Authorization: Bearer <token>`.
/// Request body: AES-256-GCM encrypted JSON changeset.
/// Response: 200 OK on success, 400 on decryption/parse failure.
async fn handle_push(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    // Authenticate
    if let Err(status) = extract_session_token(&headers, &state.session_store) {
        return (status, "unauthorized").into_response();
    }

    if body.len() > MAX_BODY_SIZE {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload exceeds 16 MiB limit",
        )
            .into_response();
    }

    if body.is_empty() {
        return (StatusCode::BAD_REQUEST, "empty body").into_response();
    }

    // Decrypt
    let plaintext = match sync_crypto::decrypt(&state.passphrase, &body) {
        Ok(pt) => pt,
        Err(e) => {
            warn!("push decryption failed (wrong passphrase?): {e}");
            return (StatusCode::BAD_REQUEST, "decryption failed").into_response();
        }
    };

    // Deserialize
    let changeset: ChangeSet = match serde_json::from_slice(&plaintext) {
        Ok(cs) => cs,
        Err(e) => {
            warn!("push deserialization failed: {e}");
            return (StatusCode::BAD_REQUEST, "invalid changeset JSON").into_response();
        }
    };

    debug!(
        origin = %changeset.origin_device_id,
        rows = changeset.row_count(),
        "received push from LAN peer"
    );

    state.received_changesets.write().push(changeset);

    StatusCode::OK.into_response()
}
