use axum::extract::{Query, State};
use axum::Json;
#[cfg(test)]
use oneshim_api_contracts::metrics::MetricsResponse;
use oneshim_api_contracts::metrics::{HourlyMetricsResponse, HourlyQuery};

use crate::error::ApiError;
use crate::services::metrics_service::MetricsQueryService;
use crate::services::web_contexts::StorageWebContext;

use super::TimeRangeQuery;

/// GET /api/metrics?from=&to=&limit=
pub async fn get_metrics(
    State(context): State<StorageWebContext>,
    Query(params): Query<TimeRangeQuery>,
) -> Result<Json<Vec<oneshim_api_contracts::metrics::MetricsResponse>>, ApiError> {
    Ok(Json(
        MetricsQueryService::new(context)
            .get_metrics(&params)
            .await?,
    ))
}

/// GET /api/metrics/hourly?hours=24
pub async fn get_hourly_metrics(
    State(context): State<StorageWebContext>,
    Query(params): Query<HourlyQuery>,
) -> Result<Json<Vec<HourlyMetricsResponse>>, ApiError> {
    Ok(Json(
        MetricsQueryService::new(context).get_hourly_metrics(&params)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppState;
    use axum::body::Body;
    use axum::extract::connect_info::MockConnectInfo;
    use axum::http::{Request, StatusCode};

    use oneshim_storage::sqlite::SqliteStorage;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use tokio::sync::broadcast;
    use tower::ServiceExt;

    fn test_app_state() -> AppState {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("in-memory sqlite"));
        let (event_tx, _) = broadcast::channel(16);
        AppState::with_core(storage, event_tx)
    }

    fn loopback_app(state: AppState) -> axum::Router {
        crate::WebServer::build_router(state)
            .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))))
    }

    #[test]
    fn metrics_response_memory_percent() {
        let response = MetricsResponse {
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            cpu_usage: 50.0,
            memory_used: 8_000_000_000,
            memory_total: 16_000_000_000,
            memory_percent: 50.0,
            disk_used: 0,
            disk_total: 0,
            network_upload: 0,
            network_download: 0,
        };
        assert_eq!(response.memory_percent, 50.0);
    }

    #[tokio::test]
    async fn get_detailed_metrics_returns_expected_shape() {
        let app = loopback_app(test_app_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/metrics?limit=10")
                    .body(Body::empty())
                    .expect("request build"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        let parsed: serde_json::Value = serde_json::from_slice(&body).expect("json parse");

        // Empty database returns an empty JSON array
        assert!(parsed.is_array());
        assert_eq!(parsed.as_array().expect("array").len(), 0);
    }
}
