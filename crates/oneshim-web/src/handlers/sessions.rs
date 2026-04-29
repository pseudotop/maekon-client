use axum::extract::{Path, State};
use axum::Json;
use oneshim_api_contracts::sessions::SessionResponse;

use crate::error::ApiError;
use crate::services::sessions_service::SessionsQueryService;
use crate::services::web_contexts::StorageWebContext;

/// GET /api/sessions
pub async fn list_sessions(
    State(context): State<StorageWebContext>,
) -> Result<Json<Vec<SessionResponse>>, ApiError> {
    Ok(Json(SessionsQueryService::new(context).list_sessions()?))
}

/// GET /api/sessions/:id
pub async fn get_session(
    State(context): State<StorageWebContext>,
    Path(session_id): Path<String>,
) -> Result<Json<SessionResponse>, ApiError> {
    Ok(Json(
        SessionsQueryService::new(context)
            .get_session(&session_id)
            .await?,
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
    fn session_response_serializes() {
        let session = SessionResponse {
            session_id: "test_123".to_string(),
            started_at: "2024-01-01T00:00:00Z".to_string(),
            ended_at: None,
            total_events: 100,
            total_frames: 50,
            total_idle_secs: 300,
            active_duration_secs: None,
        };
        let json = serde_json::to_string(&session).unwrap();
        assert!(json.contains("test_123"));
    }

    #[tokio::test]
    async fn get_session_returns_not_found_for_nonexistent() {
        let app = loopback_app(test_app_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/sessions/nonexistent-session-id")
                    .body(Body::empty())
                    .expect("request build"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
