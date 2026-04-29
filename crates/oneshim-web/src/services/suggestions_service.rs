use oneshim_api_contracts::suggestions::SuggestionDto;

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
    pub fn list_suggestions(&self, limit: usize) -> Result<Vec<SuggestionDto>, ApiError> {
        self.ctx
            .storage
            .list_suggestions(limit)
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
    pub fn dismiss(&self, suggestion_id: &str) -> Result<bool, ApiError> {
        self.ctx
            .storage
            .dismiss_unified_suggestion(suggestion_id)
            .map_err(|e| ApiError::Internal(e.to_string()))
    }
}
