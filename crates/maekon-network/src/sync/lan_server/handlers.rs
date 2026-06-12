use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use maekon_core::models::sync::ChangeSet;
use maekon_core::sync::Hlc;

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
/// Returns the authenticated peer's device_id (#5211) on a valid token, else 401.
fn extract_session_token(
    headers: &HeaderMap,
    session_store: &SessionStore,
) -> Result<String, StatusCode> {
    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let token = auth_header.strip_prefix("Bearer ").unwrap_or("");

    session_store
        .authenticated_device_id(token)
        .ok_or(StatusCode::UNAUTHORIZED)
}

/// #5174 LAN: whether `cross_device_sync` consent permits the server to exchange data.
/// `None` consent manager (transport-level tests / unmanaged) is treated as permitted —
/// production always wires a manager (build_sync_engine), where revoke/expiry returns false.
fn cross_device_sync_consented(state: &ServerState) -> bool {
    state
        .consent_manager
        .as_ref()
        .map(|cm| cm.effective_permissions().cross_device_sync)
        .unwrap_or(true)
}

fn first_row_origin_mismatch(changeset: &ChangeSet, expected_origin: &str) -> Option<&'static str> {
    let row_groups: [(&str, &Vec<serde_json::Value>); 7] = [
        ("activity_segments", &changeset.segments),
        ("regimes", &changeset.regimes),
        ("regime_overrides", &changeset.overrides),
        ("embedding_vectors", &changeset.embeddings),
        ("suggestions", &changeset.suggestions),
        ("trigger_params_snapshots", &changeset.param_snapshots),
        ("sync_preferences", &changeset.preferences),
    ];

    for (table, rows) in row_groups {
        for row in rows {
            if row.get("origin_device_id").and_then(|v| v.as_str()) != Some(expected_origin) {
                return Some(table);
            }
        }
    }

    None
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
    // #5174 LAN: consent gate. Serving this device's data to a peer is a real egress;
    // it requires cross_device_sync consent, not just a valid session token. A peer that
    // still holds a token must not pull data after consent is revoked/expired.
    if !cross_device_sync_consented(&state) {
        debug!("LAN pull refused: cross_device_sync consent not granted");
        return (
            StatusCode::FORBIDDEN,
            "cross_device_sync consent not granted",
        )
            .into_response();
    }

    let since = Hlc {
        wall_ms: params.since_wall_ms.unwrap_or(0),
        counter: params.since_counter.unwrap_or(0),
        device_id: params.device_id.unwrap_or_default(),
    };

    // #5174 LAN: storage-backed (production) extracts the device's real changes since the
    // peer's watermark; the in-memory buffer is the transport-test fallback. The response
    // is a JSON ARRAY (single element for the extractor path) to keep the wire contract +
    // the client's composite-merge (incl. the tombstone fix) unchanged.
    let changesets: Vec<ChangeSet> = if let Some(extractor) = &state.extractor {
        match extractor.get_changes_since(&since).await {
            Ok(cs) if cs.is_empty() => return StatusCode::NO_CONTENT.into_response(),
            Ok(cs) => vec![cs],
            Err(e) => {
                warn!(err.code = %e.code(), "LAN pull extract failed: {e}");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        }
    } else {
        let outbound = state.outbound_changesets.read();
        let newer: Vec<ChangeSet> = outbound
            .iter()
            .filter(|cs| {
                cs.watermark.wall_ms > since.wall_ms
                    || (cs.watermark.wall_ms == since.wall_ms
                        && cs.watermark.counter > since.counter)
            })
            .cloned()
            .collect();
        if newer.is_empty() {
            return StatusCode::NO_CONTENT.into_response();
        }
        newer
    };

    // Serialize and encrypt
    let json = match serde_json::to_vec(&changesets) {
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
        count = changesets.len(),
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
    // Authenticate (and capture the peer's proven device_id for the #5211 origin bind).
    let peer_device_id = match extract_session_token(&headers, &state.session_store) {
        Ok(id) => id,
        Err(status) => return (status, "unauthorized").into_response(),
    };
    // #5174 LAN: consent gate. Ingesting a peer's changeset into local SQLite is data
    // processing; it requires cross_device_sync consent. When revoked the device is out
    // of the mesh — it neither serves nor accepts data (the user's own pending erasure
    // still propagates OUTBOUND via the consent-bypassing client path, #5170).
    if !cross_device_sync_consented(&state) {
        debug!("LAN push refused: cross_device_sync consent not granted");
        return (
            StatusCode::FORBIDDEN,
            "cross_device_sync consent not granted",
        )
            .into_response();
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

    // #5211: a peer may only push rows whose origin matches the identity it authenticated
    // under (the SyncEngine sets changeset + row origins to self). This is defense-in-depth,
    // not a cryptographic device-identity guarantee: with only the shared mesh passphrase, a
    // passphrase holder can still claim a victim's device_id during /sync/challenge. A full
    // guarantee still needs per-device keys / per-row signatures.
    if changeset.origin_device_id != peer_device_id {
        warn!(
            claimed = %changeset.origin_device_id,
            authenticated = %peer_device_id,
            "LAN push rejected: changeset origin does not match the authenticated peer"
        );
        return (StatusCode::FORBIDDEN, "changeset origin mismatch").into_response();
    }
    // Tombstones keep the erased row's original origin so relay peers can carry
    // content-free erasures for offline receivers. Data rows remain peer-origin bound.
    if let Some(table) = first_row_origin_mismatch(&changeset, &peer_device_id) {
        warn!(
            table,
            authenticated = %peer_device_id,
            "LAN push rejected: row origin does not match the authenticated peer"
        );
        return (StatusCode::FORBIDDEN, "row origin mismatch").into_response();
    }

    debug!(
        origin = %changeset.origin_device_id,
        rows = changeset.row_count(),
        "received push from LAN peer"
    );

    // #5174 LAN: storage-backed (production) applies the push via the merger so the peer's
    // changes land in this device's SQLite (HLC LWW + tombstone suppression). The in-memory
    // buffer is the transport-test fallback.
    if let Some(merger) = &state.merger {
        if let Err(e) = merger.apply_changes(changeset).await {
            warn!(err.code = %e.code(), "LAN push merge failed: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    } else {
        state.received_changesets.write().push(changeset);
    }

    StatusCode::OK.into_response()
}
