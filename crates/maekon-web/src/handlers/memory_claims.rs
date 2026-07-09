//! Memory-graph claims browser endpoints (T1.3, #7911).
//!
//! - `GET  /api/memory/claims` — list the durable ADR-023 claim nodes the agent
//!   accumulates about the user, with kind / status / date-range filters and a
//!   clamped page. Retracted claims are excluded unless explicitly requested
//!   (`?status=retracted` / `?status=all`).
//! - `POST /api/memory/claims/{id}/retract` — user retraction. Flips a claim to
//!   `ClaimStatus::Retracted` (hiding it from every read surface + retrieval)
//!   while preserving the node and its provenance edges. Idempotent, and never a
//!   delete.
//!
//! Both read the already-wired async
//! [`MemoryGraphPort`](maekon_core::ports::memory_graph_port) at
//! `state.core.memory_graph` (the SAME concrete `SqliteStorage` the scheduler's
//! claim promoter writes into — Port Instance Sharing). Before this surface the
//! claim nodes had no user-facing read path and no way to reach `Retracted`.

use axum::extract::{Path, Query, State};
use axum::Json;
use tracing::debug;

use maekon_api_contracts::memory_claims::{
    ClaimListQuery, ClaimListResponse, RetractClaimResponse,
};

use crate::error::ApiError;
use crate::services::memory_claims_service;
use crate::AppState;

/// `GET /api/memory/claims` handler.
///
/// # Errors
/// - `503 Service Unavailable`: when `core.memory_graph` is None (a standalone
///   web-server build without a durable SQLite storage backing).
/// - propagates storage failures as `ApiError` (wire `storage.failed`).
pub async fn list_claims(
    State(state): State<AppState>,
    Query(query): Query<ClaimListQuery>,
) -> Result<Json<ClaimListResponse>, ApiError> {
    let Some(mg) = state.core.memory_graph.as_ref() else {
        return Err(ApiError::ServiceUnavailable(
            "memory graph not configured".into(),
        ));
    };
    let response = memory_claims_service::build_claim_list_response(mg, &query).await?;
    Ok(Json(response))
}

/// `POST /api/memory/claims/{id}/retract` handler.
///
/// Idempotent: retracting an already-retracted claim is a `200` no-op
/// (`already_retracted = true`). Never deletes — retraction is a status change
/// preserving provenance.
///
/// # Errors
/// - `503 Service Unavailable`: when `core.memory_graph` is None.
/// - `404 Not Found`: when no claim with `id` exists.
pub async fn retract_claim(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<RetractClaimResponse>, ApiError> {
    debug!("POST /api/memory/claims/{}/retract", id);
    let Some(mg) = state.core.memory_graph.as_ref() else {
        return Err(ApiError::ServiceUnavailable(
            "memory graph not configured".into(),
        ));
    };
    let now_secs = chrono::Utc::now().timestamp();
    match memory_claims_service::retract_claim(mg, &id, now_secs).await? {
        Some(response) => Ok(Json(response)),
        None => Err(ApiError::NotFound(format!("claim {id} not found"))),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use maekon_core::models::memory_graph::{ClaimKind, ClaimStatus, MemoryClaim};
    use maekon_core::ports::memory_graph_port::MemoryGraphPort;

    use super::*;

    fn claim(id: &str, status: ClaimStatus) -> MemoryClaim {
        MemoryClaim {
            claim_id: id.to_string(),
            kind: ClaimKind::Reflective,
            text: format!("belief {id}"),
            source: "digest_highlight".to_string(),
            confidence: 0.8,
            status,
            created_at: 1_000,
            updated_at: 1_000,
        }
    }

    /// Real `SqliteStorage` seeded with an active + a retracted claim, wired as
    /// `state.core.memory_graph` (Port Instance Sharing).
    async fn state_with_seeded_graph() -> AppState {
        let (mut state, storage) = crate::test_local_auth::test_app_state_with_storage();
        let mg: Arc<dyn MemoryGraphPort> = storage.clone();
        mg.save_claim(&claim("clm_active", ClaimStatus::Active))
            .await
            .unwrap();
        mg.save_claim(&claim("clm_retracted", ClaimStatus::Retracted))
            .await
            .unwrap();
        state.core.memory_graph = Some(mg);
        state
    }

    /// Test 1: 503 when no memory graph is wired (mirrors egress-ledger's
    /// `returns_503_when_reader_not_configured`).
    #[tokio::test]
    async fn list_returns_503_when_memory_graph_not_configured() {
        let mut state = crate::test_local_auth::test_app_state();
        state.core.memory_graph = None;

        let err = list_claims(State(state), Query(ClaimListQuery::default()))
            .await
            .unwrap_err();

        assert!(
            matches!(err, ApiError::ServiceUnavailable(_)),
            "expected ServiceUnavailable, got {err:?}"
        );
    }

    /// Test 2: default list over the REAL SqliteStorage excludes retracted.
    #[tokio::test]
    async fn list_excludes_retracted_by_default() {
        let state = state_with_seeded_graph().await;

        let Json(body) = list_claims(State(state), Query(ClaimListQuery::default()))
            .await
            .expect("handler should succeed");

        assert_eq!(body.total, 1);
        assert_eq!(body.claims.len(), 1);
        assert_eq!(body.claims[0].claim_id, "clm_active");
    }

    /// Test 3: retract flips active → retracted, and the second call is a 200
    /// no-op (idempotent).
    #[tokio::test]
    async fn retract_flips_then_is_idempotent() {
        let state = state_with_seeded_graph().await;

        let Json(first) = retract_claim(State(state.clone()), Path("clm_active".to_string()))
            .await
            .expect("first retract succeeds");
        assert!(!first.already_retracted);
        assert_eq!(first.claim.status, "retracted");

        let Json(second) = retract_claim(State(state), Path("clm_active".to_string()))
            .await
            .expect("second retract succeeds");
        assert!(second.already_retracted, "second retract is a no-op");
        assert_eq!(second.claim.status, "retracted");
    }

    /// Test 4: retracting an unknown claim id is a 404.
    #[tokio::test]
    async fn retract_unknown_claim_is_404() {
        let state = state_with_seeded_graph().await;

        let err = retract_claim(State(state), Path("clm_nope".to_string()))
            .await
            .unwrap_err();

        assert!(
            matches!(err, ApiError::NotFound(_)),
            "expected NotFound, got {err:?}"
        );
    }
}
