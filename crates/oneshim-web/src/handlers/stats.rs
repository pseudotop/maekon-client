use axum::extract::{Query, State};
use axum::Json;
#[cfg(test)]
use oneshim_api_contracts::stats::{AppUsageEntry, HeatmapCell};
use oneshim_api_contracts::stats::{
    AppUsageResponse, DailySummaryResponse, DateQuery, GuiHeatmapCell, GuiHeatmapQuery,
    HeatmapQuery, HeatmapResponse,
};

use crate::error::ApiError;
use crate::services::stats_service::StatsQueryService;
use crate::services::web_contexts::StorageWebContext;

/// GET /api/stats/summary?date=YYYY-MM-DD
pub async fn get_summary(
    State(context): State<StorageWebContext>,
    Query(params): Query<DateQuery>,
) -> Result<Json<DailySummaryResponse>, ApiError> {
    Ok(Json(
        StatsQueryService::new(context).get_summary(&params).await?,
    ))
}

/// GET /api/stats/apps?date=YYYY-MM-DD
pub async fn get_app_usage(
    State(context): State<StorageWebContext>,
    Query(params): Query<DateQuery>,
) -> Result<Json<AppUsageResponse>, ApiError> {
    Ok(Json(
        StatsQueryService::new(context)
            .get_app_usage(&params)
            .await?,
    ))
}

/// GET /api/stats/heatmap?days=7
pub async fn get_heatmap(
    State(context): State<StorageWebContext>,
    Query(params): Query<HeatmapQuery>,
) -> Result<Json<HeatmapResponse>, ApiError> {
    Ok(Json(
        StatsQueryService::new(context)
            .get_heatmap(params.days)
            .await?,
    ))
}

/// GET /api/stats/gui-heatmap?start=...&end=...
pub async fn get_gui_heatmap(
    State(context): State<StorageWebContext>,
    Query(params): Query<GuiHeatmapQuery>,
) -> Result<Json<Vec<GuiHeatmapCell>>, ApiError> {
    let now = chrono::Utc::now();
    let start = params
        .start
        .unwrap_or_else(|| now.format("%Y-%m-%dT00:00:00Z").to_string());
    let end = params.end.unwrap_or_else(|| now.to_rfc3339());

    let density = context
        .storage
        .query_gui_interaction_density(&start, &end)
        .map_err(ApiError::from)?;

    let cells: Vec<GuiHeatmapCell> = density
        .into_iter()
        .map(|(hour, count)| GuiHeatmapCell { hour, count })
        .collect();

    Ok(Json(cells))
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
    fn summary_response_serializes() {
        let response = DailySummaryResponse {
            date: "2024-01-30".to_string(),
            total_active_secs: 28800,
            total_idle_secs: 3600,
            top_apps: vec![AppUsageEntry {
                name: "VS Code".to_string(),
                duration_secs: 14400,
                event_count: 2880,
                frame_count: 100,
            }],
            cpu_avg: 35.2,
            memory_avg_percent: 68.5,
            frames_captured: 1234,
            events_logged: 567,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("VS Code"));
    }

    #[test]
    fn heatmap_response_serializes() {
        let response = HeatmapResponse {
            from_date: "2024-01-23".to_string(),
            to_date: "2024-01-30".to_string(),
            cells: vec![
                HeatmapCell {
                    day: 0,
                    hour: 9,
                    value: 42,
                },
                HeatmapCell {
                    day: 0,
                    hour: 10,
                    value: 58,
                },
            ],
            max_value: 58,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"day\":0"));
        assert!(json.contains("\"hour\":9"));
        assert!(json.contains("\"value\":42"));
        assert!(json.contains("\"max_value\":58"));
    }

    #[tokio::test]
    async fn get_heatmap_returns_array_structure() {
        let app = loopback_app(test_app_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/stats/heatmap?days=7")
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

        // Should have from_date, to_date, cells, max_value
        assert!(parsed.get("from_date").is_some());
        assert!(parsed.get("to_date").is_some());
        assert!(parsed.get("cells").is_some());
        assert!(parsed.get("max_value").is_some());

        // 7 days * 24 hours = 168 cells
        let cells = parsed["cells"].as_array().expect("cells array");
        assert_eq!(cells.len(), 168);
    }

    #[tokio::test]
    async fn get_gui_heatmap_returns_structure() {
        let app = loopback_app(test_app_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/stats/gui-heatmap")
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

        // Empty database should return an empty array
        assert!(parsed.is_array());
    }
}
