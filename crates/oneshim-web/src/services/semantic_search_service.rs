//! Semantic search service — mode routing, embedding, hybrid scoring.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tracing::{debug, warn};

use oneshim_api_contracts::search::SemanticSearchResult;
use oneshim_core::error::CoreError;
use oneshim_core::models::embedding::{SearchFilters, SearchResult};
use oneshim_core::models::storage_records::SegmentDetailRecord;
use oneshim_core::ports::adaptive_search::AdaptiveSearchPort;
use oneshim_core::ports::embedding_provider::EmbeddingProvider;
use oneshim_core::ports::text_search::TextSearchProvider;
use oneshim_core::ports::vector_store::VectorStore;

use crate::AppState;

/// Sanitize query text before embedding (masks email-like tokens).
pub(crate) fn sanitize_query(text: &str) -> String {
    text.split_whitespace()
        .map(|token| {
            if token.contains('@') && token.contains('.') {
                "[EMAIL]"
            } else {
                token
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Resolve search mode from optional parameter. Defaults to "hybrid".
pub(crate) fn resolve_mode(mode: Option<&str>) -> &str {
    match mode {
        Some("semantic") => "semantic",
        Some("keyword") => "keyword",
        _ => "hybrid",
    }
}

/// Embed a sanitized query. Errors from the provider pass through unchanged,
/// preserving their typed wire code (ADR-019 §1) so callers / telemetry can
/// distinguish e.g. `provider.analysis_failed` from `config.invalid`.
async fn embed_query(
    embedding_provider: &Arc<dyn EmbeddingProvider>,
    sanitized: &str,
) -> Result<Vec<f32>, CoreError> {
    embedding_provider.embed(sanitized).await
}

/// Build the set of segment IDs that should receive an FTS-boost in hybrid mode.
///
/// Returns an empty set when `hybrid` is false, when `TextSearchProvider` is
/// unconfigured, or when the FTS call fails (errors are logged at `debug`).
async fn fts_boost_set(
    state: &AppState,
    sanitized: &str,
    limit: usize,
    hybrid: bool,
) -> HashSet<String> {
    if !hybrid {
        return HashSet::new();
    }
    let Some(ref text_search) = state.analysis.text_search else {
        debug!("Hybrid mode requested but TextSearchProvider not configured");
        return HashSet::new();
    };
    match text_search.search_fts(sanitized, limit).await {
        Ok(fts_results) => fts_results.into_iter().map(|r| r.segment_id).collect(),
        Err(e) => {
            debug!("FTS fallback skipped (text search error): {e}");
            HashSet::new()
        }
    }
}

/// Fetch segment metadata for the given IDs, logging storage errors at `warn`.
fn fetch_segment_details(
    state: &AppState,
    segment_ids: &[String],
) -> HashMap<String, SegmentDetailRecord> {
    state
        .core
        .storage
        .get_segment_details(segment_ids)
        .unwrap_or_else(|e| {
            warn!("Failed to fetch segment details: {e}");
            HashMap::new()
        })
}

/// Convert `SearchResult`s into `SemanticSearchResult`s, applying FTS boost and
/// enriching with segment details when available.
fn map_vector_results(
    results: Vec<SearchResult>,
    fts_boost_ids: &HashSet<String>,
    segment_details: &HashMap<String, SegmentDetailRecord>,
) -> Vec<SemanticSearchResult> {
    results
        .into_iter()
        .map(|r| {
            let detail = segment_details.get(&r.segment_id);
            let score_boost = if fts_boost_ids.contains(&r.segment_id) {
                0.1
            } else {
                0.0
            };
            SemanticSearchResult {
                segment_id: r.segment_id,
                content_type: format!("{:?}", r.content_type),
                content_label: r.content_label,
                original_text: r.original_text,
                score: (r.score + score_boost).min(1.0),
                similarity: r.similarity,
                time_decay: r.time_decay,
                timestamp: r.timestamp.to_rfc3339(),
                segment_start: detail.map(|d| d.start_time.clone()),
                segment_end: detail.map(|d| d.end_time.clone()),
                duration_secs: detail.map(|d| d.duration_secs),
                llm_summary: detail.and_then(|d| d.llm_summary.clone()),
                dominant_category: detail.map(|d| d.dominant_category.clone()),
                regime_label: detail.and_then(|d| d.regime_label.clone()),
            }
        })
        .collect()
}

/// Execute a search with mode dispatch and provider availability checks.
///
/// Iter-96: return `CoreError` instead of `String` so the typed
/// `err.code()` survives through to handlers. Provider-unavailable
/// conditions become `ServiceUnavailable` (wire code
/// `service.unavailable`); inner search failures preserve their own
/// wire codes (e.g. `provider.analysis_failed`, `internal.io`).
pub async fn execute(
    state: &AppState,
    query: &str,
    limit: usize,
    mode: &str,
) -> Result<Vec<SemanticSearchResult>, CoreError> {
    match mode {
        "keyword" => {
            let ts = state.analysis.text_search.as_ref().ok_or_else(|| {
                CoreError::ServiceUnavailable {
                    code: oneshim_core::error_codes::ServiceCode::Unavailable,
                    message:
                        "Keyword search is not available (text search provider not configured)"
                            .to_string(),
                }
            })?;
            keyword_search(ts, state, query, limit).await
        }
        _ => {
            // Prefer adaptive search (IVF/HNSW auto-selection) when available.
            if let Some(ref adaptive) = state.analysis.adaptive_search {
                let ep = state.analysis.embedding_provider.as_ref().ok_or_else(|| {
                    CoreError::ServiceUnavailable {
                        code: oneshim_core::error_codes::ServiceCode::Unavailable,
                        message:
                            "Semantic search is not available (embedding provider not configured)"
                                .to_string(),
                    }
                })?;
                return adaptive_vector_search(adaptive, ep, state, query, limit, mode == "hybrid")
                    .await;
            }

            // Fallback: direct brute-force via VectorStore
            let vs = state.analysis.vector_store.as_ref().ok_or_else(|| {
                CoreError::ServiceUnavailable {
                    code: oneshim_core::error_codes::ServiceCode::Unavailable,
                    message: "Semantic search is not available (embedding pipeline not configured)"
                        .to_string(),
                }
            })?;
            let ep = state.analysis.embedding_provider.as_ref().ok_or_else(|| {
                CoreError::ServiceUnavailable {
                    code: oneshim_core::error_codes::ServiceCode::Unavailable,
                    message: "Semantic search is not available (embedding provider not configured)"
                        .to_string(),
                }
            })?;
            vector_search(vs, ep, state, query, limit, mode == "hybrid").await
        }
    }
}

/// Execute keyword-only search via TextSearchProvider.
pub async fn keyword_search(
    text_search: &Arc<dyn TextSearchProvider>,
    state: &AppState,
    query: &str,
    limit: usize,
) -> Result<Vec<SemanticSearchResult>, CoreError> {
    let sanitized = sanitize_query(query);
    let fts_results = text_search.search_fts(&sanitized, limit).await?;

    let segment_ids: Vec<String> = fts_results.iter().map(|r| r.segment_id.clone()).collect();
    let segment_details = state
        .core
        .storage
        .get_segment_details(&segment_ids)
        .unwrap_or_else(|e| {
            warn!("Failed to fetch segment details: {e}");
            HashMap::new()
        });

    Ok(fts_results
        .into_iter()
        .map(|r| {
            let detail = segment_details.get(&r.segment_id);
            SemanticSearchResult {
                segment_id: r.segment_id,
                content_type: r.content_type,
                content_label: None,
                original_text: r.matched_text,
                score: r.rank,
                similarity: 0.0,
                time_decay: 0.0,
                timestamp: detail.map(|d| d.start_time.clone()).unwrap_or_default(),
                segment_start: detail.map(|d| d.start_time.clone()),
                segment_end: detail.map(|d| d.end_time.clone()),
                duration_secs: detail.map(|d| d.duration_secs),
                llm_summary: detail.and_then(|d| d.llm_summary.clone()),
                dominant_category: detail.map(|d| d.dominant_category.clone()),
                regime_label: detail.and_then(|d| d.regime_label.clone()),
            }
        })
        .collect())
}

/// Execute semantic/hybrid search via VectorStore + EmbeddingProvider.
pub async fn vector_search(
    vector_store: &Arc<dyn VectorStore>,
    embedding_provider: &Arc<dyn EmbeddingProvider>,
    state: &AppState,
    query: &str,
    limit: usize,
    hybrid: bool,
) -> Result<Vec<SemanticSearchResult>, CoreError> {
    let sanitized = sanitize_query(query);
    let query_vector = embed_query(embedding_provider, &sanitized).await?;
    let vector_results = vector_store.search(&query_vector, limit, 168.0).await?;
    let fts_boost_ids = fts_boost_set(state, &sanitized, limit, hybrid).await;
    let segment_ids: Vec<String> = vector_results
        .iter()
        .map(|r| r.segment_id.clone())
        .collect();
    let segment_details = fetch_segment_details(state, &segment_ids);
    Ok(map_vector_results(
        vector_results,
        &fts_boost_ids,
        &segment_details,
    ))
}

/// Execute search via AdaptiveSearchPort (IVF/HNSW auto-selection).
pub async fn adaptive_vector_search(
    adaptive: &Arc<dyn AdaptiveSearchPort>,
    embedding_provider: &Arc<dyn EmbeddingProvider>,
    state: &AppState,
    query: &str,
    limit: usize,
    hybrid: bool,
) -> Result<Vec<SemanticSearchResult>, CoreError> {
    let sanitized = sanitize_query(query);
    let query_vector = embed_query(embedding_provider, &sanitized).await?;
    let results = adaptive
        .search(&query_vector, limit, 168.0, &SearchFilters::default())
        .await?;
    let fts_boost_ids = fts_boost_set(state, &sanitized, limit, hybrid).await;
    let segment_ids: Vec<String> = results.iter().map(|r| r.segment_id.clone()).collect();
    let segment_details = fetch_segment_details(state, &segment_ids);
    Ok(map_vector_results(
        results,
        &fts_boost_ids,
        &segment_details,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_query_masks_email() {
        let q = "emails from user@example.com about auth";
        let sanitized = sanitize_query(q);
        assert!(!sanitized.contains("user@example.com"));
        assert!(sanitized.contains("[EMAIL]"));
    }

    #[test]
    fn sanitize_query_passes_through_clean_text() {
        let q = "what did I work on yesterday";
        assert_eq!(sanitize_query(q), q);
    }

    #[test]
    fn resolve_mode_defaults_to_hybrid() {
        assert_eq!(resolve_mode(None), "hybrid");
        assert_eq!(resolve_mode(Some("unknown")), "hybrid");
    }

    #[test]
    fn resolve_mode_recognizes_valid_modes() {
        assert_eq!(resolve_mode(Some("semantic")), "semantic");
        assert_eq!(resolve_mode(Some("keyword")), "keyword");
    }

    // ── Fallback routing tests (Item 2f) ────────────────────────────────

    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use oneshim_core::models::embedding::EmbeddingMetadata;
    use oneshim_storage::sqlite::SqliteStorage;
    use tokio::sync::broadcast;

    use crate::RealtimeEvent;

    struct CountingAdaptive {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl AdaptiveSearchPort for CountingAdaptive {
        async fn search(
            &self,
            _q: &[f32],
            _limit: usize,
            _decay: f32,
            _filters: &SearchFilters,
        ) -> Result<Vec<SearchResult>, CoreError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(Vec::new())
        }
        async fn refresh_count(&self) -> Result<(), CoreError> {
            Ok(())
        }
    }

    struct CountingVectorStore {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl VectorStore for CountingVectorStore {
        async fn store(&self, _v: Vec<f32>, _m: EmbeddingMetadata) -> Result<(), CoreError> {
            Ok(())
        }
        async fn search(
            &self,
            _q: &[f32],
            _limit: usize,
            _decay: f32,
        ) -> Result<Vec<SearchResult>, CoreError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(Vec::new())
        }
        async fn search_filtered(
            &self,
            _q: &[f32],
            _limit: usize,
            _decay: f32,
            _f: &SearchFilters,
        ) -> Result<Vec<SearchResult>, CoreError> {
            Ok(Vec::new())
        }
        async fn enforce_retention(&self, _days: u32) -> Result<u64, CoreError> {
            Ok(0)
        }
        async fn mark_stale(&self, _id: &str) -> Result<u64, CoreError> {
            Ok(0)
        }
        async fn update_vector(&self, _id: i64, _v: Vec<f32>, _m: &str) -> Result<(), CoreError> {
            Ok(())
        }
        async fn get_current_model_id(&self) -> Result<Option<String>, CoreError> {
            Ok(None)
        }
        async fn get_stale_vectors(&self, _limit: usize) -> Result<Vec<(i64, String)>, CoreError> {
            Ok(vec![])
        }
    }

    struct ZeroEmbedding;

    #[async_trait]
    impl EmbeddingProvider for ZeroEmbedding {
        async fn embed(&self, _text: &str) -> Result<Vec<f32>, CoreError> {
            Ok(vec![0.0_f32; 4])
        }
        fn dimensions(&self) -> usize {
            4
        }
        fn model_id(&self) -> &str {
            "test-zero"
        }
    }

    fn build_state() -> AppState {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("open_in_memory"));
        let (tx, _rx) = broadcast::channel::<RealtimeEvent>(10);
        AppState::with_core(storage, tx)
    }

    #[tokio::test]
    async fn execute_prefers_adaptive_when_available() {
        let adaptive = Arc::new(CountingAdaptive {
            calls: AtomicUsize::new(0),
        });
        let brute = Arc::new(CountingVectorStore {
            calls: AtomicUsize::new(0),
        });
        let mut state = build_state();
        state.analysis.adaptive_search = Some(adaptive.clone());
        state.analysis.vector_store = Some(brute.clone());
        state.analysis.embedding_provider = Some(Arc::new(ZeroEmbedding));

        execute(&state, "hello", 10, "semantic").await.unwrap();
        assert_eq!(adaptive.calls.load(Ordering::SeqCst), 1);
        assert_eq!(brute.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn execute_falls_back_to_vector_store_when_adaptive_missing() {
        let brute = Arc::new(CountingVectorStore {
            calls: AtomicUsize::new(0),
        });
        let mut state = build_state();
        state.analysis.vector_store = Some(brute.clone());
        state.analysis.embedding_provider = Some(Arc::new(ZeroEmbedding));

        execute(&state, "hello", 10, "semantic").await.unwrap();
        assert_eq!(brute.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn execute_errors_when_vector_store_missing() {
        let state = build_state();
        let err = execute(&state, "hello", 10, "semantic").await.unwrap_err();
        // Iter-96: previously asserted on the stringified error; now asserts
        // on the typed wire code (service.unavailable) so the test protects
        // against future drift that would silently re-stringify.
        assert_eq!(err.code(), "service.unavailable");
        assert!(
            err.to_string().to_lowercase().contains("not available"),
            "unexpected error: {err}"
        );
    }
}
