use std::collections::HashMap;
use std::sync::Arc;

use tracing::debug;

use crate::error::AnalysisError;
use oneshim_core::models::embedding::SearchResult;
use oneshim_core::ports::embedding_provider::EmbeddingProvider;
use oneshim_core::ports::text_search::TextSearchProvider;
use oneshim_core::ports::vector_store::VectorStore;

use crate::PiiFilter;

/// Search mode selector for the hybrid search service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    /// Vector + FTS5 Reciprocal Rank Fusion.
    Hybrid,
    /// Vector-only semantic search.
    Semantic,
    /// FTS5-only keyword search.
    Keyword,
}

/// Hybrid search service combining vector similarity and FTS5 keyword search
/// using Reciprocal Rank Fusion (RRF) to merge results.
pub struct HybridSearchService {
    text_search: Arc<dyn TextSearchProvider>,
    vector_store: Arc<dyn VectorStore>,
    embedding_provider: Arc<dyn EmbeddingProvider>,
    pii_filter: PiiFilter,
    /// Weight for vector search scores in RRF. Default: 0.6.
    alpha: f32,
    /// Weight for FTS5 search scores in RRF. Default: 0.4.
    beta: f32,
    /// RRF constant (controls how much rank position matters). Default: 60.0.
    k: f32,
}

impl HybridSearchService {
    pub fn new(
        text_search: Arc<dyn TextSearchProvider>,
        vector_store: Arc<dyn VectorStore>,
        embedding_provider: Arc<dyn EmbeddingProvider>,
        pii_filter: PiiFilter,
    ) -> Self {
        Self {
            text_search,
            vector_store,
            embedding_provider,
            pii_filter,
            alpha: 0.6,
            beta: 0.4,
            k: 60.0,
        }
    }

    /// Create with custom RRF parameters.
    pub fn with_params(
        text_search: Arc<dyn TextSearchProvider>,
        vector_store: Arc<dyn VectorStore>,
        embedding_provider: Arc<dyn EmbeddingProvider>,
        pii_filter: PiiFilter,
        alpha: f32,
        beta: f32,
        k: f32,
    ) -> Self {
        Self {
            text_search,
            vector_store,
            embedding_provider,
            pii_filter,
            alpha,
            beta,
            k,
        }
    }

    /// Execute a search in the given mode.
    pub async fn search(
        &self,
        query: &str,
        mode: SearchMode,
        limit: usize,
    ) -> Result<Vec<SearchResult>, AnalysisError> {
        let filtered_query = (self.pii_filter)(query);

        match mode {
            SearchMode::Semantic => self.vector_search(&filtered_query, limit).await,
            SearchMode::Keyword => self.keyword_search(&filtered_query, limit).await,
            SearchMode::Hybrid => self.hybrid_search(&filtered_query, limit).await,
        }
    }

    /// Vector-only semantic search: embed the query, then search the vector store.
    async fn vector_search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>, AnalysisError> {
        let query_vector = self.embedding_provider.embed(query).await?;
        let results = self.vector_store.search(&query_vector, limit, 0.0).await?;
        debug!(count = results.len(), "Vector search results");
        Ok(results)
    }

    /// FTS5-only keyword search: convert TextSearchResult to SearchResult.
    async fn keyword_search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>, AnalysisError> {
        let fts_results = self.text_search.search_fts(query, limit).await?;
        debug!(count = fts_results.len(), "FTS5 keyword search results");

        let results = fts_results
            .into_iter()
            .map(|r| SearchResult {
                segment_id: r.segment_id,
                content_type: oneshim_core::models::embedding::EmbeddingContentType::SegmentSummary,
                content_label: None,
                score: r.rank,
                similarity: r.rank,
                time_decay: 1.0,
                timestamp: chrono::Utc::now(),
                original_text: r.matched_text,
            })
            .collect();

        Ok(results)
    }

    /// Hybrid search: run both vector and keyword searches in parallel,
    /// then merge using Reciprocal Rank Fusion (RRF).
    async fn hybrid_search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>, AnalysisError> {
        let over_fetch = limit * 2;

        // Run both searches in parallel
        let (vector_results, fts_results) = tokio::join!(
            self.vector_search(query, over_fetch),
            self.keyword_search(query, over_fetch),
        );

        let vector_results = vector_results.unwrap_or_default();
        let fts_results = fts_results.unwrap_or_default();

        debug!(
            vector_count = vector_results.len(),
            fts_count = fts_results.len(),
            "Hybrid search: merging results via RRF"
        );

        // Build rank maps (segment_id -> 1-based rank position)
        let vector_ranks: HashMap<String, usize> = vector_results
            .iter()
            .enumerate()
            .map(|(i, r)| (r.segment_id.clone(), i + 1))
            .collect();

        let fts_ranks: HashMap<String, usize> = fts_results
            .iter()
            .enumerate()
            .map(|(i, r)| (r.segment_id.clone(), i + 1))
            .collect();

        // Missing rank sentinel
        let missing_rank = over_fetch + 1;

        // Collect all unique segment_ids with their best SearchResult
        let mut best_results: HashMap<String, SearchResult> = HashMap::new();
        for r in vector_results.into_iter().chain(fts_results) {
            best_results
                .entry(r.segment_id.clone())
                .and_modify(|existing| {
                    // Keep the one with higher similarity (prefer vector result quality)
                    if r.similarity > existing.similarity {
                        *existing = r.clone();
                    }
                })
                .or_insert(r);
        }

        // Compute RRF scores and attach to results
        let mut scored: Vec<(f32, SearchResult)> = best_results
            .into_iter()
            .map(|(seg_id, result)| {
                let rank_v = *vector_ranks.get(&seg_id).unwrap_or(&missing_rank);
                let rank_f = *fts_ranks.get(&seg_id).unwrap_or(&missing_rank);

                let rrf_score =
                    self.alpha / (self.k + rank_v as f32) + self.beta / (self.k + rank_f as f32);

                let mut result = result;
                result.score = rrf_score;
                (rrf_score, result)
            })
            .collect();

        // Sort by RRF score descending
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        // Return top `limit`
        let results: Vec<SearchResult> = scored.into_iter().take(limit).map(|(_, r)| r).collect();

        debug!(count = results.len(), "Hybrid search: final merged results");
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::Utc;
    use oneshim_core::error::CoreError;
    use oneshim_core::models::embedding::{EmbeddingContentType, SearchResult as VectorResult};
    use oneshim_core::ports::text_search::TextSearchResult;

    // ── Mock TextSearchProvider ──────────────────────────────────

    struct MockTextSearch {
        results: Vec<TextSearchResult>,
    }

    #[async_trait]
    impl TextSearchProvider for MockTextSearch {
        async fn search_fts(
            &self,
            _query: &str,
            limit: usize,
        ) -> Result<Vec<TextSearchResult>, CoreError> {
            Ok(self.results.iter().take(limit).cloned().collect())
        }

        async fn sync_segment(
            &self,
            _segment_id: &str,
            _searchable_text: &str,
        ) -> Result<(), CoreError> {
            Ok(())
        }
    }

    // ── Mock VectorStore ─────────────────────────────────────────

    struct MockVectorStore {
        results: Vec<VectorResult>,
    }

    #[async_trait]
    impl VectorStore for MockVectorStore {
        async fn store(
            &self,
            _vector: Vec<f32>,
            _metadata: oneshim_core::models::embedding::EmbeddingMetadata,
        ) -> Result<(), CoreError> {
            Ok(())
        }

        async fn search(
            &self,
            _query_vector: &[f32],
            limit: usize,
            _time_decay_hours: f32,
        ) -> Result<Vec<VectorResult>, CoreError> {
            Ok(self.results.iter().take(limit).cloned().collect())
        }

        async fn search_filtered(
            &self,
            _query_vector: &[f32],
            limit: usize,
            _time_decay_hours: f32,
            _filters: &oneshim_core::models::embedding::SearchFilters,
        ) -> Result<Vec<VectorResult>, CoreError> {
            Ok(self.results.iter().take(limit).cloned().collect())
        }

        async fn enforce_retention(&self, _max_days: u32) -> Result<u64, CoreError> {
            Ok(0)
        }
        async fn mark_stale(&self, _old_model_id: &str) -> Result<u64, CoreError> {
            Ok(0)
        }
        async fn get_current_model_id(&self) -> Result<Option<String>, CoreError> {
            Ok(None)
        }
        async fn get_stale_vectors(&self, _limit: usize) -> Result<Vec<(i64, String)>, CoreError> {
            Ok(vec![])
        }
        async fn update_vector(
            &self,
            _id: i64,
            _vector: Vec<f32>,
            _model_id: &str,
        ) -> Result<(), CoreError> {
            Ok(())
        }
    }

    // ── Mock EmbeddingProvider ────────────────────────────────────

    struct MockEmbeddingProvider;

    #[async_trait]
    impl EmbeddingProvider for MockEmbeddingProvider {
        async fn embed(&self, _text: &str) -> Result<Vec<f32>, CoreError> {
            Ok(vec![0.1, 0.2, 0.3])
        }
        fn dimensions(&self) -> usize {
            3
        }
        fn model_id(&self) -> &str {
            "mock-model"
        }
    }

    // ── Helpers ──────────────────────────────────────────────────

    fn identity_filter() -> PiiFilter {
        Box::new(|s: &str| s.to_string())
    }

    fn make_vector_result(segment_id: &str, similarity: f32) -> VectorResult {
        VectorResult {
            segment_id: segment_id.to_string(),
            content_type: EmbeddingContentType::SegmentSummary,
            content_label: None,
            score: similarity,
            similarity,
            time_decay: 1.0,
            timestamp: Utc::now(),
            original_text: format!("vector text for {segment_id}"),
        }
    }

    fn make_fts_result(segment_id: &str, rank: f32) -> TextSearchResult {
        TextSearchResult {
            segment_id: segment_id.to_string(),
            content_type: "segment".to_string(),
            matched_text: format!("keyword text for {segment_id}"),
            rank,
        }
    }

    fn build_service(
        vector_results: Vec<VectorResult>,
        fts_results: Vec<TextSearchResult>,
    ) -> HybridSearchService {
        HybridSearchService::new(
            Arc::new(MockTextSearch {
                results: fts_results,
            }),
            Arc::new(MockVectorStore {
                results: vector_results,
            }),
            Arc::new(MockEmbeddingProvider),
            identity_filter(),
        )
    }

    // ── Tests ────────────────────────────────────────────────────

    #[tokio::test]
    async fn hybrid_merges_both_sources() {
        let vector_results = vec![
            make_vector_result("seg-1", 0.9),
            make_vector_result("seg-2", 0.7),
        ];
        let fts_results = vec![make_fts_result("seg-3", 5.0), make_fts_result("seg-2", 4.0)];

        let service = build_service(vector_results, fts_results);
        let results = service.search("test query", SearchMode::Hybrid, 10).await;

        assert!(results.is_ok());
        let results = results.unwrap();
        // seg-1, seg-2, seg-3 should all be present
        let ids: Vec<&str> = results.iter().map(|r| r.segment_id.as_str()).collect();
        assert!(ids.contains(&"seg-1"));
        assert!(ids.contains(&"seg-2"));
        assert!(ids.contains(&"seg-3"));
    }

    #[tokio::test]
    async fn semantic_mode_ignores_fts() {
        let vector_results = vec![make_vector_result("seg-vec", 0.9)];
        let fts_results = vec![make_fts_result("seg-fts", 5.0)];

        let service = build_service(vector_results, fts_results);
        let results = service
            .search("test query", SearchMode::Semantic, 10)
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].segment_id, "seg-vec");
    }

    #[tokio::test]
    async fn keyword_mode_ignores_vector() {
        let vector_results = vec![make_vector_result("seg-vec", 0.9)];
        let fts_results = vec![make_fts_result("seg-fts", 5.0)];

        let service = build_service(vector_results, fts_results);
        let results = service
            .search("test query", SearchMode::Keyword, 10)
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].segment_id, "seg-fts");
    }

    #[tokio::test]
    async fn deduplication_keeps_highest_score() {
        // seg-2 appears in both result sets — should appear only once in output
        let vector_results = vec![
            make_vector_result("seg-1", 0.9),
            make_vector_result("seg-2", 0.8),
        ];
        let fts_results = vec![make_fts_result("seg-2", 5.0), make_fts_result("seg-3", 4.0)];

        let service = build_service(vector_results, fts_results);
        let results = service
            .search("test query", SearchMode::Hybrid, 10)
            .await
            .unwrap();

        // seg-2 should appear exactly once
        let seg2_count = results.iter().filter(|r| r.segment_id == "seg-2").count();
        assert_eq!(seg2_count, 1);
    }

    #[tokio::test]
    async fn rrf_scoring_correct_with_known_ranks() {
        // seg-A: vector rank 1, fts rank 2
        // seg-B: vector rank 2, fts rank 1
        // With alpha=0.6, beta=0.4, k=60:
        //   seg-A: 0.6/(60+1) + 0.4/(60+2) = 0.6/61 + 0.4/62
        //   seg-B: 0.6/(60+2) + 0.4/(60+1) = 0.6/62 + 0.4/61
        let vector_results = vec![
            make_vector_result("seg-A", 0.95),
            make_vector_result("seg-B", 0.85),
        ];
        let fts_results = vec![
            make_fts_result("seg-B", 10.0),
            make_fts_result("seg-A", 8.0),
        ];

        let service = build_service(vector_results, fts_results);
        let results = service
            .search("test query", SearchMode::Hybrid, 10)
            .await
            .unwrap();

        assert!(results.len() >= 2);

        let score_a = results
            .iter()
            .find(|r| r.segment_id == "seg-A")
            .unwrap()
            .score;
        let score_b = results
            .iter()
            .find(|r| r.segment_id == "seg-B")
            .unwrap()
            .score;

        let expected_a = 0.6 / (60.0 + 1.0) + 0.4 / (60.0 + 2.0);
        let expected_b = 0.6 / (60.0 + 2.0) + 0.4 / (60.0 + 1.0);

        assert!(
            (score_a - expected_a).abs() < 1e-6,
            "seg-A score {score_a} != expected {expected_a}"
        );
        assert!(
            (score_b - expected_b).abs() < 1e-6,
            "seg-B score {score_b} != expected {expected_b}"
        );

        // seg-A should rank higher (alpha=0.6 favors vector, seg-A is vector rank 1)
        assert!(score_a > score_b);
    }

    #[tokio::test]
    async fn hybrid_respects_limit() {
        let vector_results = vec![
            make_vector_result("seg-1", 0.9),
            make_vector_result("seg-2", 0.8),
            make_vector_result("seg-3", 0.7),
        ];
        let fts_results = vec![make_fts_result("seg-4", 5.0), make_fts_result("seg-5", 4.0)];

        let service = build_service(vector_results, fts_results);
        let results = service
            .search("test query", SearchMode::Hybrid, 3)
            .await
            .unwrap();

        assert!(results.len() <= 3);
    }

    #[tokio::test]
    async fn empty_results_produce_empty_output() {
        let service = build_service(vec![], vec![]);
        let results = service
            .search("test query", SearchMode::Hybrid, 10)
            .await
            .unwrap();
        assert!(results.is_empty());
    }
}
