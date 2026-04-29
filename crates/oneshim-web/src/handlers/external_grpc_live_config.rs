//! `GET /api/external-grpc/live-config` endpoint (spec §5.11 / D29).
//!
//! Returns the current LiveSnapshot + LoadPolicy threshold summary +
//! config_reload_task_alive boolean. Returns 503 when external gRPC is
//! disabled or not compiled in.

#[cfg(feature = "grpc-dashboard-external")]
use std::sync::atomic::Ordering;

use axum::{extract::State, Json};
use oneshim_api_contracts::external_grpc::LiveConfigResponse;
#[cfg(feature = "grpc-dashboard-external")]
use oneshim_api_contracts::external_grpc::LoadPolicyView;

use crate::error::ApiError;
use crate::AppState;

/// Handler compiled when `grpc-dashboard-external` is enabled.
///
/// Returns a 503 when `DiagnosticsState.external_grpc_live` is `None`
/// (i.e. external gRPC is compiled in but disabled at runtime via config).
#[cfg(feature = "grpc-dashboard-external")]
pub async fn get_live_config(
    State(state): State<AppState>,
) -> Result<Json<LiveConfigResponse>, ApiError> {
    let Some(live) = &state.diagnostics.external_grpc_live else {
        return Err(ApiError::ServiceUnavailable(
            "external gRPC not enabled".into(),
        ));
    };
    let snap = live.snapshot();
    let policy = &snap.load_policy;
    let t = policy.thresholds();
    let task_alive = state
        .diagnostics
        .external_grpc_metrics
        .as_ref()
        .map(|m| m.config_reload_task_alive.load(Ordering::Relaxed))
        .unwrap_or(false);

    Ok(Json(LiveConfigResponse {
        streaming_enabled: snap.streaming_enabled,
        load_policy_snapshot: LoadPolicyView {
            cpu_low_pct: t.cpu_low_pct,
            cpu_medium_pct: t.cpu_medium_pct,
            cpu_high_pct: t.cpu_high_pct,
            min_free_mem_gb: t.min_free_mem_gb,
            started_at_elapsed_ms: policy.started_at().elapsed().as_millis() as u64,
            in_warmup: policy.is_in_warmup(),
        },
        config_reload_task_alive: task_alive,
    }))
}

/// Stub handler for builds without `grpc-dashboard-external` — always returns 503.
#[cfg(not(feature = "grpc-dashboard-external"))]
pub async fn get_live_config(
    State(_state): State<AppState>,
) -> Result<Json<LiveConfigResponse>, ApiError> {
    Err(ApiError::ServiceUnavailable(
        "external gRPC not compiled (missing grpc-dashboard-external feature)".into(),
    ))
}

#[cfg(all(test, feature = "grpc-dashboard-external"))]
mod tests {
    use super::*;
    use crate::grpc::external::live_config::{LiveExternalConfig, LiveSnapshot};
    use crate::grpc::external::metrics::ExternalMetrics;
    use crate::grpc::LoadPolicy;
    use oneshim_core::config::LoadThresholds;
    use oneshim_storage::sqlite::SqliteStorage;
    use std::sync::Arc;
    use tokio::sync::broadcast;

    fn fixture_state() -> AppState {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("in-memory sqlite"));
        let (event_tx, _) = broadcast::channel(16);
        AppState::with_core(storage, event_tx)
    }

    fn fixture_live() -> Arc<LiveExternalConfig> {
        Arc::new(LiveExternalConfig::new(LiveSnapshot {
            streaming_enabled: true,
            load_policy: Arc::new(LoadPolicy::new(LoadThresholds::default())),
        }))
    }

    #[tokio::test]
    async fn returns_503_when_external_disabled() {
        // DiagnosticsState.external_grpc_live is None by default.
        let state = fixture_state();
        let err = get_live_config(State(state)).await.unwrap_err();
        match err {
            ApiError::ServiceUnavailable(_) => {} // expected
            other => panic!("expected ServiceUnavailable, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn returns_live_snapshot_when_enabled() {
        let live = fixture_live();
        let metrics = Arc::new(ExternalMetrics::new());
        metrics
            .config_reload_task_alive
            .store(true, Ordering::Relaxed);

        let mut state = fixture_state();
        state.diagnostics.external_grpc_live = Some(live);
        state.diagnostics.external_grpc_metrics = Some(metrics);

        let resp = get_live_config(State(state)).await.unwrap().0;
        assert!(resp.streaming_enabled);
        assert!(resp.config_reload_task_alive);
        // Default LoadThresholds have non-zero cpu thresholds.
        assert!(resp.load_policy_snapshot.cpu_low_pct > 0.0);
    }

    #[tokio::test]
    async fn config_reload_task_alive_false_when_metrics_none() {
        let live = fixture_live();

        let mut state = fixture_state();
        state.diagnostics.external_grpc_live = Some(live);
        // external_grpc_metrics remains None

        let resp = get_live_config(State(state)).await.unwrap().0;
        assert!(!resp.config_reload_task_alive);
    }
}
