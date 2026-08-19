use axum::extract::{Query, State};
use axum::Json;
use maekon_api_contracts::search::{
    SemanticSearchCapabilities, SemanticSearchQuery, SemanticSearchResult,
};
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
    // Iter-96: CoreError -> ApiError via semantic From impl (preserves wire
    // codes). Previously the service stringified errors and the handler
    // collapsed every failure to ApiError::ServiceUnavailable, which sent
    // 503 even for client-side validation errors.
    //
    // #7574: execute() now returns SearchExecutionError, which distinguishes
    // the mode=semantic capability boundary (SemanticNotConfigured -> 501)
    // from ordinary CoreError failures (still mapped 503/400/etc. as above).
    let results = semantic_search_service::execute(&state, &params.q, limit, mode)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(results))
}

/// GET /api/semantic-search/capabilities (#7600)
///
/// Lets the frontend honestly label the "Semantic" search toggle BEFORE
/// firing a search: when the vector/embedding pipeline is not wired up (the
/// shipped release build compiles `maekon-embedding` OUT, so this is always
/// `false` there), disable the toggle or annotate it as keyword-only instead
/// of silently letting `mode=hybrid` degrade to FTS results labeled
/// "semantic", or letting `mode=semantic` return an unexplained HTTP 501.
pub async fn semantic_search_capabilities(
    State(state): State<AppState>,
) -> Json<SemanticSearchCapabilities> {
    let semantic_available = state.analysis.embedding_provider.is_some()
        && (state.analysis.adaptive_search.is_some() || state.analysis.vector_store.is_some());
    Json(SemanticSearchCapabilities { semantic_available })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::extract::State;
    use maekon_api_contracts::search::{SemanticSearchQuery, SemanticSearchResult};
    use maekon_core::error::CoreError;
    use maekon_core::models::embedding::{SearchFilters, SearchResult};
    use maekon_core::ports::adaptive_search::AdaptiveSearchPort;
    use maekon_core::ports::embedding_provider::EmbeddingProvider;

    use super::*;

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

    // ── semantic_search_capabilities (#7600) ─────────────────────────────────

    struct StubEmbedding;

    #[async_trait]
    impl EmbeddingProvider for StubEmbedding {
        async fn embed(&self, _text: &str) -> Result<Vec<f32>, CoreError> {
            Ok(vec![0.0_f32; 4])
        }
        fn dimensions(&self) -> usize {
            4
        }
        fn model_id(&self) -> &str {
            "test-stub"
        }
    }

    struct StubAdaptive;

    #[async_trait]
    impl AdaptiveSearchPort for StubAdaptive {
        async fn search(
            &self,
            _q: &[f32],
            _limit: usize,
            _decay: f32,
            _filters: &SearchFilters,
        ) -> Result<Vec<SearchResult>, CoreError> {
            Ok(Vec::new())
        }
        async fn refresh_count(&self) -> Result<(), CoreError> {
            Ok(())
        }
    }

    // #7738 D-4: funnel through the canonical test-state helper.
    fn build_state() -> AppState {
        crate::test_local_auth::test_app_state_with_event_capacity(10)
    }

    /// Neither an embedding provider nor a vector search backend wired up
    /// (the default/degraded state, and the shipped release build reality
    /// since `maekon-embedding` is compiled out) — must report `false`.
    #[tokio::test]
    async fn capabilities_report_unavailable_when_nothing_is_wired() {
        let state = build_state();

        let Json(capabilities) = semantic_search_capabilities(State(state)).await;

        assert!(!capabilities.semantic_available);
    }

    /// An embedding provider alone (no vector backend) is not enough to run a
    /// vector search — must still report `false`.
    #[tokio::test]
    async fn capabilities_report_unavailable_with_embedding_but_no_vector_backend() {
        let mut state = build_state();
        state.analysis.embedding_provider = Some(Arc::new(StubEmbedding));

        let Json(capabilities) = semantic_search_capabilities(State(state)).await;

        assert!(!capabilities.semantic_available);
    }

    /// Both an embedding provider AND a vector search backend (adaptive)
    /// wired up — must report `true`.
    #[tokio::test]
    async fn capabilities_report_available_with_embedding_and_adaptive_search() {
        let mut state = build_state();
        state.analysis.embedding_provider = Some(Arc::new(StubEmbedding));
        state.analysis.adaptive_search = Some(Arc::new(StubAdaptive));

        let Json(capabilities) = semantic_search_capabilities(State(state)).await;

        assert!(capabilities.semantic_available);
    }
}
