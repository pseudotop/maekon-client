use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use maekon_api_contracts::suggestions::{SuggestionDto, SuggestionFeedbackRequest};
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
        SuggestionsQueryService::new(context)
            .list_suggestions(50)
            .await?,
    ))
}

/// POST /api/suggestions/:id/dismiss — dismiss a suggestion by its UUID.
pub async fn dismiss_suggestion(
    State(context): State<StorageWebContext>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    debug!("POST /api/suggestions/{}/dismiss", id);
    let found = SuggestionsCommandService::new(context).dismiss(&id).await?;
    if found {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound(format!(
            "suggestion {id} not found or already dismissed"
        )))
    }
}

/// POST /api/suggestions/:id/feedback — record shown/dismissed/acted feedback.
pub async fn submit_suggestion_feedback(
    State(context): State<StorageWebContext>,
    Path(id): Path<String>,
    Json(request): Json<SuggestionFeedbackRequest>,
) -> Result<StatusCode, ApiError> {
    debug!(
        "POST /api/suggestions/{}/feedback action={}",
        id, request.action
    );
    let found = SuggestionsCommandService::new(context)
        .submit_feedback(&id, &request)
        .await?;
    if found {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound(format!("suggestion {id} not found")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};

    use chrono::Utc;
    use maekon_core::models::suggestion::{Priority, Suggestion, SuggestionSource, SuggestionType};
    use tower::ServiceExt;

    use crate::test_local_auth::{
        authed_loopback_router as loopback_app, test_app_state, test_app_state_with_storage,
    };

    fn sample_suggestion(suggestion_id: &str) -> Suggestion {
        Suggestion {
            suggestion_id: suggestion_id.to_string(),
            suggestion_type: SuggestionType::WorkGuidance,
            content: "Review the current window before acting.".to_string(),
            priority: Priority::Medium,
            confidence_score: 0.84,
            relevance_score: 0.91,
            is_actionable: true,
            created_at: Utc::now(),
            expires_at: None,
            source: SuggestionSource::LlmLocal,
            reasoning: Some("Visible context matched the request.".to_string()),
            context_scope: None,
        }
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

    #[tokio::test]
    async fn post_suggestion_feedback_marks_unified_suggestion_shown() {
        let (state, storage) = test_app_state_with_storage();
        storage
            .save_rule_suggestion_sync(&sample_suggestion("suggestion-feedback-1"))
            .expect("save suggestion");

        let app = loopback_app(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/suggestions/suggestion-feedback-1/feedback")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"action":"shown"}"#))
                    .expect("request build"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let rows = storage.list_suggestions(10).expect("list suggestions");
        let row = rows
            .into_iter()
            .find(|row| row.suggestion_id == "suggestion-feedback-1")
            .expect("saved suggestion");
        assert!(row.shown_at.is_some());
        assert!(row.dismissed_at.is_none());
        assert!(row.acted_at.is_none());
    }
}
