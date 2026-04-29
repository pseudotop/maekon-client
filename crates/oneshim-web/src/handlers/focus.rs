use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use oneshim_api_contracts::focus::{
    FocusMetricsResponse, InterruptionDto, LocalSuggestionDto, SuggestionFeedbackRequest,
    WorkSessionDto,
};
use tracing::debug;

use crate::error::ApiError;
use crate::handlers::TimeRangeQuery;
use crate::services::focus_service::{FocusCommandService, FocusQueryService};
use crate::services::web_contexts::StorageWebContext;

pub async fn get_focus_metrics(
    State(context): State<StorageWebContext>,
) -> Result<Json<FocusMetricsResponse>, ApiError> {
    debug!("GET /api/focus/metrics");
    Ok(Json(FocusQueryService::new(context).get_focus_metrics()?))
}

pub async fn get_work_sessions(
    State(context): State<StorageWebContext>,
    axum::extract::Query(query): axum::extract::Query<TimeRangeQuery>,
) -> Result<Json<Vec<WorkSessionDto>>, ApiError> {
    debug!("GET /api/focus/sessions");
    Ok(Json(
        FocusQueryService::new(context).get_work_sessions(&query)?,
    ))
}

pub async fn get_interruptions(
    State(context): State<StorageWebContext>,
    axum::extract::Query(query): axum::extract::Query<TimeRangeQuery>,
) -> Result<Json<Vec<InterruptionDto>>, ApiError> {
    debug!("GET /api/focus/interruptions");
    Ok(Json(
        FocusQueryService::new(context).get_interruptions(&query)?,
    ))
}

pub async fn get_suggestions(
    State(context): State<StorageWebContext>,
) -> Result<Json<Vec<LocalSuggestionDto>>, ApiError> {
    debug!("GET /api/focus/suggestions");
    Ok(Json(FocusQueryService::new(context).get_suggestions()?))
}

pub async fn submit_suggestion_feedback(
    State(context): State<StorageWebContext>,
    Path(id): Path<i64>,
    Json(request): Json<SuggestionFeedbackRequest>,
) -> Result<StatusCode, ApiError> {
    debug!(
        "POST /api/focus/suggestions/{}/feedback action={}",
        id, request.action
    );
    FocusCommandService::new(context).submit_suggestion_feedback(id, &request)?;
    Ok(StatusCode::NO_CONTENT)
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
        let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
        let (event_tx, _) = broadcast::channel(16);
        AppState::with_core(storage, event_tx)
    }

    fn loopback_app() -> axum::Router {
        let state = test_app_state();
        crate::WebServer::build_router(state)
            .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))))
    }

    #[tokio::test]
    async fn get_focus_metrics_returns_200() {
        let app = loopback_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/focus/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        // Verify response structure: { today: { date: "..." }, history: [] }
        assert!(parsed["today"]["date"].is_string());
        assert!(parsed["history"].is_array());
    }

    #[tokio::test]
    async fn get_work_sessions_returns_list() {
        let app = loopback_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/focus/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        // Empty database returns an empty JSON array
        assert!(parsed.is_array());
        assert_eq!(parsed.as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn get_interruptions_returns_list() {
        let app = loopback_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/focus/interruptions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(parsed.is_array());
        assert_eq!(parsed.as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn get_suggestions_returns_list() {
        let app = loopback_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/focus/suggestions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(parsed.is_array());
        assert_eq!(parsed.as_array().unwrap().len(), 0);
    }

    #[test]
    fn focus_metrics_response_serializes() {
        use oneshim_api_contracts::focus::FocusMetricsDto;

        let response = FocusMetricsResponse {
            today: FocusMetricsDto {
                date: "2026-03-21".to_string(),
                total_active_secs: 28800,
                deep_work_secs: 14400,
                communication_secs: 3600,
                context_switches: 42,
                interruption_count: 5,
                avg_focus_duration_secs: 1800,
                max_focus_duration_secs: 3600,
                focus_score: 78.5,
            },
            history: vec![],
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("2026-03-21"));
        assert!(json.contains("78.5"));
    }

    #[test]
    fn work_session_dto_serializes() {
        let dto = WorkSessionDto {
            id: 1,
            started_at: "2026-03-21T09:00:00Z".to_string(),
            ended_at: Some("2026-03-21T10:30:00Z".to_string()),
            primary_app: "VS Code".to_string(),
            category: "coding".to_string(),
            state: "completed".to_string(),
            interruption_count: 2,
            deep_work_secs: 4800,
            duration_secs: 5400,
        };
        let json = serde_json::to_string(&dto).unwrap();
        assert!(json.contains("VS Code"));
        assert!(json.contains("coding"));
    }

    #[test]
    fn interruption_dto_serializes() {
        let dto = InterruptionDto {
            id: 1,
            interrupted_at: "2026-03-21T09:30:00Z".to_string(),
            from_app: "VS Code".to_string(),
            from_category: "coding".to_string(),
            to_app: "Slack".to_string(),
            to_category: "communication".to_string(),
            resumed_at: Some("2026-03-21T09:35:00Z".to_string()),
            resumed_to_app: Some("VS Code".to_string()),
            duration_secs: Some(300),
        };
        let json = serde_json::to_string(&dto).unwrap();
        assert!(json.contains("Slack"));
        assert!(json.contains("VS Code"));
    }
}
