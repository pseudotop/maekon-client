use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use oneshim_api_contracts::suggestions::SuggestionDto;
use tracing::debug;

use crate::error::ApiError;
use crate::services::suggestions_service::{SuggestionsCommandService, SuggestionsQueryService};
use crate::services::web_contexts::StorageWebContext;

/// GET /api/suggestions — list non-dismissed suggestions, newest first.
pub async fn list_suggestions(
    State(context): State<StorageWebContext>,
) -> Result<Json<Vec<SuggestionDto>>, ApiError> {
    debug!("GET /api/suggestions");
    Ok(Json(
        SuggestionsQueryService::new(context).list_suggestions(50)?,
    ))
}

/// POST /api/suggestions/:id/dismiss — dismiss a suggestion by its UUID.
pub async fn dismiss_suggestion(
    State(context): State<StorageWebContext>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    debug!("POST /api/suggestions/{}/dismiss", id);
    let found = SuggestionsCommandService::new(context).dismiss(&id)?;
    if found {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound(format!(
            "suggestion {id} not found or already dismissed"
        )))
    }
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
    fn suggestion_dto_serializes() {
        let dto = SuggestionDto {
            id: 1,
            suggestion_id: "abc-123".to_string(),
            suggestion_type: "WorkGuidance".to_string(),
            source: "LLM_LOCAL".to_string(),
            content: "Take a break".to_string(),
            priority: "Medium".to_string(),
            confidence_score: 0.85,
            relevance_score: 0.9,
            is_actionable: true,
            reasoning: Some("High focus duration".to_string()),
            shown_at: None,
            dismissed_at: None,
            acted_at: None,
            created_at: "2026-03-18T10:00:00Z".to_string(),
            expires_at: None,
        };
        let json = serde_json::to_string(&dto).unwrap();
        assert!(json.contains("LLM_LOCAL"));
        assert!(json.contains("Take a break"));
    }

    #[tokio::test]
    async fn get_suggestions_returns_empty_list_initially() {
        let app = loopback_app(test_app_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/suggestions")
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
        assert!(parsed.is_array());
        assert_eq!(parsed.as_array().expect("array").len(), 0);
    }
}
