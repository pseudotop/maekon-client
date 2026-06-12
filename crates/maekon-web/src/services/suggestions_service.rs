use maekon_api_contracts::suggestions::{SuggestionDto, SuggestionFeedbackRequest};

use crate::error::ApiError;
use crate::services::suggestions_assembler::assemble_suggestion;
use crate::services::web_contexts::StorageWebContext;

#[derive(Clone)]
pub struct SuggestionsQueryService {
    ctx: StorageWebContext,
}

impl SuggestionsQueryService {
    pub fn new(ctx: StorageWebContext) -> Self {
        Self { ctx }
    }

    /// Return the most recent non-dismissed suggestions (up to `limit`).
    pub async fn list_suggestions(&self, limit: usize) -> Result<Vec<SuggestionDto>, ApiError> {
        self.ctx
            .storage
            .list_suggestions(limit)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))
            .map(|rows| rows.into_iter().map(assemble_suggestion).collect())
    }
}

#[derive(Clone)]
pub struct SuggestionsCommandService {
    ctx: StorageWebContext,
}

impl SuggestionsCommandService {
    pub fn new(ctx: StorageWebContext) -> Self {
        Self { ctx }
    }

    /// Dismiss a suggestion by its UUID `suggestion_id`.
    /// Returns `true` if the suggestion was found and dismissed.
    pub async fn dismiss(&self, suggestion_id: &str) -> Result<bool, ApiError> {
        self.ctx
            .storage
            .dismiss_unified_suggestion(suggestion_id)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))
    }

    /// Record user feedback on a unified suggestion by UUID.
    pub async fn submit_feedback(
        &self,
        suggestion_id: &str,
        request: &SuggestionFeedbackRequest,
    ) -> Result<bool, ApiError> {
        match request.action.as_str() {
            "shown" => self
                .ctx
                .storage
                .mark_unified_suggestion_shown(suggestion_id)
                .await
                .map_err(|e| ApiError::Internal(e.to_string())),
            "dismissed" => self.dismiss(suggestion_id).await,
            "acted" => self
                .ctx
                .storage
                .mark_unified_suggestion_acted(suggestion_id)
                .await
                .map_err(|e| ApiError::Internal(e.to_string())),
            other => Err(ApiError::BadRequest(format!(
                "unsupported suggestion feedback action: {other}"
            ))),
        }
    }
}
