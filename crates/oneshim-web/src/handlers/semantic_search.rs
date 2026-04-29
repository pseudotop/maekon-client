use axum::extract::{Query, State};
use axum::Json;
use oneshim_api_contracts::search::{SemanticSearchQuery, SemanticSearchResult};
use tracing::debug;

use crate::error::ApiError;
use crate::services::semantic_search_service::{self, resolve_mode};
use crate::AppState;

/// GET /api/semantic-search?q=auth+module&limit=10&mode=hybrid
pub async fn semantic_search(
    State(state): State<AppState>,
    Query(params): Query<SemanticSearchQuery>,
) -> Result<Json<Vec<SemanticSearchResult>>, ApiError> {
    let mode = resolve_mode(params.mode.as_deref());
    debug!("GET /api/semantic-search q={} mode={}", params.q, mode);

    let limit = params.limit.unwrap_or(10).min(50);
    // Iter-96: CoreError → ApiError via semantic From impl (preserves wire
    // codes). Previously the service stringified errors and the handler
    // collapsed every failure to ApiError::ServiceUnavailable, which sent
    // 503 even for client-side validation errors.
    let results = semantic_search_service::execute(&state, &params.q, limit, mode)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(results))
}

#[cfg(test)]
mod tests {
    use oneshim_api_contracts::search::{SemanticSearchQuery, SemanticSearchResult};

    #[test]
    fn semantic_search_query_defaults() {
        let json = r#"{"q": "auth module"}"#;
        let query: SemanticSearchQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.q, "auth module");
        assert!(query.limit.is_none());
        assert!(query.mode.is_none());
    }

    #[test]
    fn semantic_search_result_serializes() {
        let result = SemanticSearchResult {
            segment_id: "seg-001".to_string(),
            content_type: "SegmentSummary".to_string(),
            content_label: Some("VSCode: main.rs".to_string()),
            original_text: "Focused coding on auth module".to_string(),
            score: 0.85,
            similarity: 0.9,
            time_decay: 0.95,
            timestamp: "2026-03-18T10:00:00+00:00".to_string(),
            segment_start: Some("2026-03-18T09:30:00+00:00".to_string()),
            segment_end: Some("2026-03-18T10:00:00+00:00".to_string()),
            duration_secs: Some(1800),
            llm_summary: Some("Focused development session".to_string()),
            dominant_category: Some("Development".to_string()),
            regime_label: None,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("seg-001"));
    }
}
