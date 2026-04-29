//! Adaptive search coordinator that auto-selects the optimal vector search strategy
//! based on collection size and configuration.
//!
//! Strategies:
//! - `BruteForceInt8`: Full scan with INT8 cosine similarity (< 5K vectors)
//! - `Hnsw`: HNSW approximate nearest neighbor search (5K - 10K vectors, feature = "hnsw")
//! - `IvfInt8`: IVF partitioned scan with INT8 cosine (10K - 100K vectors)
//! - `IvfBinaryRerank`: IVF + 2-bit Hamming filter + INT8 re-rank (>= 100K vectors)

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;

use crate::error::AnalysisError;
#[cfg(feature = "hnsw")]
use chrono::Utc;
use oneshim_core::binary_quantizer::BinaryQuantizer;
use oneshim_core::models::embedding::{SearchFilters, SearchResult};
#[cfg(feature = "hnsw")]
use oneshim_core::ports::ann_index::AnnIndex;
use oneshim_core::ports::vector_index::VectorIndex;
use oneshim_core::ports::vector_store::VectorStore;
use oneshim_core::quantization::ScalarQuantizer;
use tracing::debug;
#[cfg(feature = "hnsw")]
use tracing::{info, warn};

/// Search strategies selected by the coordinator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchStrategy {
    BruteForceInt8,
    /// HNSW approximate nearest neighbor search.
    /// Only available when the `hnsw` feature is enabled.
    #[cfg(feature = "hnsw")]
    Hnsw,
    IvfInt8,
    IvfBinaryRerank,
}

/// Configuration for the adaptive search coordinator.
pub struct SearchConfig {
    /// Vector count below which brute-force is used. Default: 10_000.
    pub brute_force_threshold: u64,
    /// Vector count below which IVF-only is used (above = IVF+binary). Default: 100_000.
    pub ivf_threshold: u64,
    /// Vector count threshold for HNSW strategy. Default: 5_000.
    /// When count >= hnsw_threshold && count < brute_force_threshold
    /// and an AnnIndex is available, HNSW is selected.
    pub hnsw_threshold: u64,
    /// Oversample factor for 2-bit binary filter stage. Default: 10.
    pub oversample_factor: usize,
    /// Number of IVF partitions to probe. 0 = auto. Default: 0.
    pub default_nprobe: usize,
    /// Force a specific strategy. None = "auto". Values: "brute_force", "hnsw", "ivf", "ivf_binary".
    pub forced_strategy: Option<String>,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            brute_force_threshold: 10_000,
            ivf_threshold: 100_000,
            hnsw_threshold: 5_000,
            oversample_factor: 10,
            default_nprobe: 0,
            forced_strategy: None,
        }
    }
}

/// Auto-selects the optimal search strategy based on collection size.
pub struct AdaptiveSearchCoordinator {
    vector_store: Arc<dyn VectorStore>,
    vector_index: Arc<dyn VectorIndex>,
    config: SearchConfig,
    /// Cached active vector count, refreshed periodically by the scheduler.
    cached_vector_count: AtomicU64,
    /// Optional HNSW index for approximate nearest neighbor search.
    #[cfg(feature = "hnsw")]
    ann_index: Option<Arc<dyn AnnIndex>>,
}

impl AdaptiveSearchCoordinator {
    pub fn new(
        vector_store: Arc<dyn VectorStore>,
        vector_index: Arc<dyn VectorIndex>,
        config: SearchConfig,
    ) -> Self {
        Self {
            vector_store,
            vector_index,
            config,
            cached_vector_count: AtomicU64::new(0),
            #[cfg(feature = "hnsw")]
            ann_index: None,
        }
    }

    /// Attach an HNSW ANN index to enable the Hnsw search strategy.
    #[cfg(feature = "hnsw")]
    pub fn with_ann_index(mut self, ann: Arc<dyn AnnIndex>) -> Self {
        self.ann_index = Some(ann);
        self
    }

    /// Refresh the cached vector count from the store.
    /// Called from the scheduler aggregate loop (not the search hot path).
    pub async fn refresh_count(&self) -> Result<(), AnalysisError> {
        let count = self.vector_store.count_active_vectors().await?;
        self.cached_vector_count.store(count, Ordering::Relaxed);
        Ok(())
    }

    /// Determine the search strategy based on config and cached vector count.
    /// This is a sync method — reads an atomic counter, no I/O.
    pub fn determine_strategy(&self) -> SearchStrategy {
        if let Some(ref forced) = self.config.forced_strategy {
            return match forced.as_str() {
                "brute_force" => SearchStrategy::BruteForceInt8,
                #[cfg(feature = "hnsw")]
                "hnsw" => SearchStrategy::Hnsw,
                "ivf" => SearchStrategy::IvfInt8,
                "ivf_binary" => SearchStrategy::IvfBinaryRerank,
                _ => SearchStrategy::BruteForceInt8,
            };
        }

        let count = self.cached_vector_count.load(Ordering::Relaxed);

        // HNSW tier: hnsw_threshold <= count < brute_force_threshold, requires ann_index
        #[cfg(feature = "hnsw")]
        if count >= self.config.hnsw_threshold
            && count < self.config.brute_force_threshold
            && self.ann_index.is_some()
        {
            return SearchStrategy::Hnsw;
        }

        if count < self.config.brute_force_threshold {
            SearchStrategy::BruteForceInt8
        } else if count < self.config.ivf_threshold {
            SearchStrategy::IvfInt8
        } else {
            SearchStrategy::IvfBinaryRerank
        }
    }

    /// Compute nprobe: use configured value or auto-select.
    fn compute_nprobe(&self) -> usize {
        if self.config.default_nprobe > 0 {
            return self.config.default_nprobe;
        }
        // Auto: sqrt(n_clusters) ≈ 4th-root(n_vectors), minimum 1
        let count = self.cached_vector_count.load(Ordering::Relaxed) as f64;
        let n_clusters = count.sqrt();
        let nprobe = (n_clusters / 10.0).ceil() as usize;
        nprobe.max(1)
    }

    /// Convert HNSW results (key, distance) into SearchResult by looking up
    /// metadata from the vector store and applying time decay.
    #[cfg(feature = "hnsw")]
    async fn join_metadata(
        &self,
        hnsw_results: Vec<(u64, f32)>,
        time_decay_hours: f32,
    ) -> Result<Vec<SearchResult>, AnalysisError> {
        if hnsw_results.is_empty() {
            return Ok(Vec::new());
        }

        let keys: Vec<u64> = hnsw_results.iter().map(|(k, _)| *k).collect();
        let metadata_map = self.vector_store.get_metadata_by_ids(&keys).await?;

        let now = Utc::now();
        let mut results: Vec<SearchResult> = hnsw_results
            .into_iter()
            .filter_map(|(key, distance)| {
                let meta = metadata_map.get(&key)?;
                // usearch cosine distance: 0.0 = identical, 2.0 = opposite
                // Convert to similarity: similarity = 1.0 - distance
                let similarity = (1.0 - distance).max(0.0);
                let age_hours = (now - meta.timestamp).num_seconds().max(0) as f32 / 3600.0;
                let time_decay = if time_decay_hours > 0.0 {
                    (-age_hours / time_decay_hours).exp()
                } else {
                    1.0
                };
                let score = similarity * time_decay;
                Some(SearchResult {
                    segment_id: meta.segment_id.clone(),
                    content_type: meta.content_type.clone(),
                    content_label: meta.content_label.clone(),
                    score,
                    similarity,
                    time_decay,
                    timestamp: meta.timestamp,
                    original_text: meta.original_text.clone(),
                })
            })
            .collect();

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(results)
    }

    /// Search using the auto-selected (or forced) strategy.
    pub async fn search(
        &self,
        query_f32: &[f32],
        limit: usize,
        time_decay_hours: f32,
        filters: &SearchFilters,
    ) -> Result<Vec<SearchResult>, AnalysisError> {
        let strategy = self.determine_strategy();
        debug!(?strategy, "AdaptiveSearchCoordinator selected strategy");

        // HNSW path with graceful degradation
        #[cfg(feature = "hnsw")]
        if strategy == SearchStrategy::Hnsw {
            if let Some(ref ann) = self.ann_index {
                match ann.search(query_f32, limit).await {
                    Ok(hnsw_results) => {
                        return self.join_metadata(hnsw_results, time_decay_hours).await;
                    }
                    Err(e) => {
                        warn!("HNSW search failed, falling back to brute-force: {}", e);
                        // Fall through to brute-force below
                    }
                }
            }
            // Fallback: use brute-force INT8 search
            let quantized = ScalarQuantizer::quantize(query_f32)?;
            return self
                .vector_store
                .search_quantized(&quantized, limit, time_decay_hours, filters)
                .await;
        }

        let quantized = ScalarQuantizer::quantize(query_f32)?;

        match strategy {
            SearchStrategy::BruteForceInt8 => self
                .vector_store
                .search_quantized(&quantized, limit, time_decay_hours, filters)
                .await
                .map_err(AnalysisError::Core),
            #[cfg(feature = "hnsw")]
            SearchStrategy::Hnsw => {
                // Already handled above; this arm is unreachable but required
                // by the compiler for exhaustive matching.
                unreachable!("Hnsw strategy handled in early-return path above")
            }
            SearchStrategy::IvfInt8 => {
                let nprobe = self.compute_nprobe();
                self.vector_index
                    .search_ivf(&quantized, nprobe, limit, time_decay_hours, filters)
                    .await
                    .map_err(AnalysisError::Core)
            }
            SearchStrategy::IvfBinaryRerank => {
                let nprobe = self.compute_nprobe();
                let thresholds = self.vector_index.load_quantile_thresholds().await?;

                match thresholds {
                    Some(t) => {
                        let binary_code = BinaryQuantizer::encode(query_f32, &t)?;
                        self.vector_index
                            .search_ivf_binary(
                                &quantized,
                                &binary_code,
                                nprobe,
                                self.config.oversample_factor,
                                limit,
                                time_decay_hours,
                                filters,
                            )
                            .await
                            .map_err(AnalysisError::Core)
                    }
                    None => {
                        // Thresholds not built yet — fall back to IVF-only
                        debug!("quantile thresholds not available, falling back to IVF-only");
                        self.vector_index
                            .search_ivf(&quantized, nprobe, limit, time_decay_hours, filters)
                            .await
                            .map_err(AnalysisError::Core)
                    }
                }
            }
        }
    }

    /// Load or rebuild the HNSW index from disk.
    ///
    /// Attempts `ann_index.load()` first. If that fails (corrupt file, missing
    /// file, version mismatch), fetches all vectors from SQLite and rebuilds
    /// the index from scratch.
    ///
    /// Call at startup (scheduler initialization) before the first search.
    #[cfg(feature = "hnsw")]
    pub async fn load_or_rebuild_hnsw(&self) -> Result<(), AnalysisError> {
        let ann = match self.ann_index {
            Some(ref a) => a,
            None => {
                debug!("No ANN index configured, skipping HNSW load");
                return Ok(());
            }
        };

        // Try loading from persisted file
        match ann.load().await {
            Ok(()) => {
                info!(size = ann.len(), "HNSW index loaded from disk");
                return Ok(());
            }
            Err(e) => {
                warn!("HNSW index load failed ({}), rebuilding from SQLite...", e);
            }
        }

        // Rebuild: fetch all vectors from SQLite
        let all_vectors = self.vector_store.get_all_vectors_for_rebuild().await?;
        if all_vectors.is_empty() {
            info!("No vectors in SQLite, HNSW index will remain empty");
            return Ok(());
        }

        let total = all_vectors.len();
        info!(total, "Rebuilding HNSW index from SQLite vectors");

        for (key, vector) in &all_vectors {
            ann.add(*key, vector).await?;
        }

        // Persist the rebuilt index
        ann.save().await?;
        info!(size = ann.len(), "HNSW index rebuilt and saved");
        Ok(())
    }

    /// Expose cached count for testing.
    #[cfg(test)]
    pub fn set_cached_count(&self, count: u64) {
        self.cached_vector_count.store(count, Ordering::Relaxed);
    }
}

#[async_trait]
impl oneshim_core::ports::adaptive_search::AdaptiveSearchPort for AdaptiveSearchCoordinator {
    async fn search(
        &self,
        query_f32: &[f32],
        limit: usize,
        time_decay_hours: f32,
        filters: &SearchFilters,
    ) -> Result<Vec<SearchResult>, oneshim_core::error::CoreError> {
        self.search(query_f32, limit, time_decay_hours, filters)
            .await
            .map_err(Into::into)
    }

    async fn refresh_count(&self) -> Result<(), oneshim_core::error::CoreError> {
        self.refresh_count().await.map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use oneshim_core::binary_quantizer::{BinaryCode, QuantileThresholds};
    use oneshim_core::error::CoreError;
    use oneshim_core::models::embedding::{EmbeddingMetadata, SearchResult};
    use oneshim_core::ports::vector_index::IndexMeta;
    use oneshim_core::quantization::QuantizedVector;
    use std::sync::atomic::AtomicBool;

    // ── Mock VectorStore ───────────────────────────────────────────

    struct MockVectorStore {
        brute_force_called: AtomicBool,
        active_count: u64,
    }

    impl MockVectorStore {
        fn new(active_count: u64) -> Self {
            Self {
                brute_force_called: AtomicBool::new(false),
                active_count,
            }
        }
    }

    #[async_trait]
    impl VectorStore for MockVectorStore {
        async fn store(
            &self,
            _vector: Vec<f32>,
            _metadata: EmbeddingMetadata,
        ) -> Result<(), CoreError> {
            Ok(())
        }
        async fn search(
            &self,
            _query: &[f32],
            _limit: usize,
            _time_decay: f32,
        ) -> Result<Vec<SearchResult>, CoreError> {
            Ok(vec![])
        }
        async fn search_filtered(
            &self,
            _query: &[f32],
            _limit: usize,
            _time_decay: f32,
            _filters: &SearchFilters,
        ) -> Result<Vec<SearchResult>, CoreError> {
            Ok(vec![])
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
        async fn search_quantized(
            &self,
            _query: &QuantizedVector,
            _limit: usize,
            _time_decay: f32,
            _filters: &SearchFilters,
        ) -> Result<Vec<SearchResult>, CoreError> {
            self.brute_force_called.store(true, Ordering::Relaxed);
            Ok(vec![])
        }
        async fn count_active_vectors(&self) -> Result<u64, CoreError> {
            Ok(self.active_count)
        }
    }

    /// Mock VectorStore that also returns metadata and vectors for HNSW tests.
    #[cfg(feature = "hnsw")]
    struct MockVectorStoreWithMetadata {
        brute_force_called: AtomicBool,
        active_count: u64,
        metadata: std::sync::Mutex<HashMap<u64, EmbeddingMetadata>>,
        vectors: std::sync::Mutex<Vec<(u64, Vec<f32>)>>,
    }

    #[cfg(feature = "hnsw")]
    impl MockVectorStoreWithMetadata {
        fn new(
            active_count: u64,
            metadata: HashMap<u64, EmbeddingMetadata>,
            vectors: Vec<(u64, Vec<f32>)>,
        ) -> Self {
            Self {
                brute_force_called: AtomicBool::new(false),
                active_count,
                metadata: std::sync::Mutex::new(metadata),
                vectors: std::sync::Mutex::new(vectors),
            }
        }
    }

    #[cfg(feature = "hnsw")]
    #[async_trait]
    impl VectorStore for MockVectorStoreWithMetadata {
        async fn store(&self, _v: Vec<f32>, _m: EmbeddingMetadata) -> Result<(), CoreError> {
            Ok(())
        }
        async fn search(
            &self,
            _q: &[f32],
            _l: usize,
            _t: f32,
        ) -> Result<Vec<SearchResult>, CoreError> {
            Ok(vec![])
        }
        async fn search_filtered(
            &self,
            _q: &[f32],
            _l: usize,
            _t: f32,
            _f: &SearchFilters,
        ) -> Result<Vec<SearchResult>, CoreError> {
            Ok(vec![])
        }
        async fn enforce_retention(&self, _d: u32) -> Result<u64, CoreError> {
            Ok(0)
        }
        async fn mark_stale(&self, _m: &str) -> Result<u64, CoreError> {
            Ok(0)
        }
        async fn get_current_model_id(&self) -> Result<Option<String>, CoreError> {
            Ok(None)
        }
        async fn get_stale_vectors(&self, _l: usize) -> Result<Vec<(i64, String)>, CoreError> {
            Ok(vec![])
        }
        async fn update_vector(&self, _i: i64, _v: Vec<f32>, _m: &str) -> Result<(), CoreError> {
            Ok(())
        }
        async fn search_quantized(
            &self,
            _q: &QuantizedVector,
            _l: usize,
            _t: f32,
            _f: &SearchFilters,
        ) -> Result<Vec<SearchResult>, CoreError> {
            self.brute_force_called.store(true, Ordering::Relaxed);
            Ok(vec![])
        }
        async fn count_active_vectors(&self) -> Result<u64, CoreError> {
            Ok(self.active_count)
        }
        async fn get_metadata_by_ids(
            &self,
            ids: &[u64],
        ) -> Result<HashMap<u64, EmbeddingMetadata>, CoreError> {
            let map = self.metadata.lock().unwrap();
            Ok(ids
                .iter()
                .filter_map(|id| map.get(id).map(|m| (*id, m.clone())))
                .collect())
        }
        async fn get_all_vectors_for_rebuild(&self) -> Result<Vec<(u64, Vec<f32>)>, CoreError> {
            Ok(self.vectors.lock().unwrap().clone())
        }
    }

    // ── Mock VectorIndex ──────────────────────────────────────────

    struct MockVectorIndex {
        ivf_called: AtomicBool,
        ivf_binary_called: AtomicBool,
    }

    impl MockVectorIndex {
        fn new() -> Self {
            Self {
                ivf_called: AtomicBool::new(false),
                ivf_binary_called: AtomicBool::new(false),
            }
        }
    }

    #[async_trait]
    #[allow(clippy::too_many_arguments)]
    impl VectorIndex for MockVectorIndex {
        async fn search_ivf(
            &self,
            _query: &QuantizedVector,
            _nprobe: usize,
            _limit: usize,
            _time_decay: f32,
            _filters: &SearchFilters,
        ) -> Result<Vec<SearchResult>, CoreError> {
            self.ivf_called.store(true, Ordering::Relaxed);
            Ok(vec![])
        }

        async fn search_ivf_binary(
            &self,
            _query: &QuantizedVector,
            _query_binary: &BinaryCode,
            _nprobe: usize,
            _oversample: usize,
            _limit: usize,
            _time_decay: f32,
            _filters: &SearchFilters,
        ) -> Result<Vec<SearchResult>, CoreError> {
            self.ivf_binary_called.store(true, Ordering::Relaxed);
            Ok(vec![])
        }

        async fn load_quantile_thresholds(&self) -> Result<Option<QuantileThresholds>, CoreError> {
            Ok(Some(QuantileThresholds {
                q25: vec![0.0; 3],
                q50: vec![0.5; 3],
                q75: vec![1.0; 3],
                dimensions: 3,
            }))
        }

        async fn get_index_meta(&self) -> Result<IndexMeta, CoreError> {
            Ok(IndexMeta {
                ivf_built_at: None,
                ivf_vector_count: 0,
                binary_built_at: None,
                total_vector_count: 0,
                unindexed_count: 0,
            })
        }
    }

    // ── Tests ─────────────────────────────────────────────────────

    #[test]
    fn strategy_auto_brute_force() {
        let store = Arc::new(MockVectorStore::new(0));
        let index = Arc::new(MockVectorIndex::new());
        let coordinator = AdaptiveSearchCoordinator::new(store, index, SearchConfig::default());
        coordinator.set_cached_count(5_000);
        assert_eq!(
            coordinator.determine_strategy(),
            SearchStrategy::BruteForceInt8
        );
    }

    #[test]
    fn strategy_auto_ivf() {
        let store = Arc::new(MockVectorStore::new(0));
        let index = Arc::new(MockVectorIndex::new());
        let coordinator = AdaptiveSearchCoordinator::new(store, index, SearchConfig::default());
        coordinator.set_cached_count(50_000);
        assert_eq!(coordinator.determine_strategy(), SearchStrategy::IvfInt8);
    }

    #[test]
    fn strategy_auto_ivf_binary() {
        let store = Arc::new(MockVectorStore::new(0));
        let index = Arc::new(MockVectorIndex::new());
        let coordinator = AdaptiveSearchCoordinator::new(store, index, SearchConfig::default());
        coordinator.set_cached_count(200_000);
        assert_eq!(
            coordinator.determine_strategy(),
            SearchStrategy::IvfBinaryRerank
        );
    }

    #[test]
    fn strategy_forced_brute_force() {
        let store = Arc::new(MockVectorStore::new(0));
        let index = Arc::new(MockVectorIndex::new());
        let config = SearchConfig {
            forced_strategy: Some("brute_force".to_string()),
            ..Default::default()
        };
        let coordinator = AdaptiveSearchCoordinator::new(store, index, config);
        coordinator.set_cached_count(999_999); // Would be IvfBinaryRerank in auto mode
        assert_eq!(
            coordinator.determine_strategy(),
            SearchStrategy::BruteForceInt8
        );
    }

    #[test]
    fn strategy_forced_ivf() {
        let store = Arc::new(MockVectorStore::new(0));
        let index = Arc::new(MockVectorIndex::new());
        let config = SearchConfig {
            forced_strategy: Some("ivf".to_string()),
            ..Default::default()
        };
        let coordinator = AdaptiveSearchCoordinator::new(store, index, config);
        coordinator.set_cached_count(100);
        assert_eq!(coordinator.determine_strategy(), SearchStrategy::IvfInt8);
    }

    #[test]
    fn strategy_forced_ivf_binary() {
        let store = Arc::new(MockVectorStore::new(0));
        let index = Arc::new(MockVectorIndex::new());
        let config = SearchConfig {
            forced_strategy: Some("ivf_binary".to_string()),
            ..Default::default()
        };
        let coordinator = AdaptiveSearchCoordinator::new(store, index, config);
        coordinator.set_cached_count(0);
        assert_eq!(
            coordinator.determine_strategy(),
            SearchStrategy::IvfBinaryRerank
        );
    }

    #[tokio::test]
    async fn refresh_count_updates_atomic() {
        let store = Arc::new(MockVectorStore::new(42_000));
        let index = Arc::new(MockVectorIndex::new());
        let coordinator = AdaptiveSearchCoordinator::new(store, index, SearchConfig::default());
        assert_eq!(coordinator.cached_vector_count.load(Ordering::Relaxed), 0);

        coordinator.refresh_count().await.unwrap();
        assert_eq!(
            coordinator.cached_vector_count.load(Ordering::Relaxed),
            42_000
        );
    }

    #[tokio::test]
    async fn search_delegates_to_brute_force() {
        let store = Arc::new(MockVectorStore::new(100));
        let index = Arc::new(MockVectorIndex::new());
        let coordinator =
            AdaptiveSearchCoordinator::new(store.clone(), index.clone(), SearchConfig::default());
        coordinator.set_cached_count(100);

        let _ = coordinator
            .search(&[0.1, 0.2, 0.3], 5, 168.0, &SearchFilters::default())
            .await;

        assert!(store.brute_force_called.load(Ordering::Relaxed));
        assert!(!index.ivf_called.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn search_delegates_to_ivf() {
        let store = Arc::new(MockVectorStore::new(50_000));
        let index = Arc::new(MockVectorIndex::new());
        let coordinator =
            AdaptiveSearchCoordinator::new(store.clone(), index.clone(), SearchConfig::default());
        coordinator.set_cached_count(50_000);

        let _ = coordinator
            .search(&[0.1, 0.2, 0.3], 5, 168.0, &SearchFilters::default())
            .await;

        assert!(!store.brute_force_called.load(Ordering::Relaxed));
        assert!(index.ivf_called.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn search_delegates_to_ivf_binary() {
        let store = Arc::new(MockVectorStore::new(200_000));
        let index = Arc::new(MockVectorIndex::new());
        let coordinator =
            AdaptiveSearchCoordinator::new(store.clone(), index.clone(), SearchConfig::default());
        coordinator.set_cached_count(200_000);

        let _ = coordinator
            .search(&[0.1, 0.2, 0.3], 5, 168.0, &SearchFilters::default())
            .await;

        assert!(!store.brute_force_called.load(Ordering::Relaxed));
        assert!(index.ivf_binary_called.load(Ordering::Relaxed));
    }

    // ── Backward compatibility tests (Task 22) ──────────────────

    #[tokio::test]
    async fn brute_force_config_skips_indexing() {
        // Forced brute_force strategy should always delegate to vector_store,
        // even when cached_vector_count is very high.
        let store = Arc::new(MockVectorStore::new(999_999));
        let index = Arc::new(MockVectorIndex::new());
        let config = SearchConfig {
            forced_strategy: Some("brute_force".to_string()),
            ..Default::default()
        };
        let coordinator = AdaptiveSearchCoordinator::new(store.clone(), index.clone(), config);
        coordinator.set_cached_count(999_999);

        // Strategy should be brute force regardless of count
        assert_eq!(
            coordinator.determine_strategy(),
            SearchStrategy::BruteForceInt8
        );

        // Search should go to vector_store.search_quantized, not index
        let _ = coordinator
            .search(&[0.1, 0.2, 0.3], 5, 168.0, &SearchFilters::default())
            .await;

        assert!(store.brute_force_called.load(Ordering::Relaxed));
        assert!(!index.ivf_called.load(Ordering::Relaxed));
        assert!(!index.ivf_binary_called.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn retriever_without_coordinator_works_unchanged() {
        // VectorRetriever constructed without a coordinator should still work
        // via the original brute-force / quantized path.
        use crate::assembler::PiiFilter;
        use crate::vector_retriever::VectorRetriever;
        use oneshim_core::ports::embedding_provider::EmbeddingProvider;

        struct MockEmbed;

        #[async_trait]
        impl EmbeddingProvider for MockEmbed {
            async fn embed(&self, _text: &str) -> Result<Vec<f32>, CoreError> {
                Ok(vec![0.1, 0.2, 0.3])
            }
            fn dimensions(&self) -> usize {
                3
            }
            fn model_id(&self) -> &str {
                "mock"
            }
        }

        let store = Arc::new(MockVectorStore::new(0));
        let noop_filter: PiiFilter = Box::new(|text: &str| text.to_string());

        // Construct WITHOUT coordinator — the pre-Phase-C path
        let retriever = VectorRetriever::new(
            Arc::new(MockEmbed),
            store.clone(),
            noop_filter,
            5,
            168.0,
            true, // quantization_enabled
        );

        // search_quantized should be called (no coordinator to intercept)
        let results = retriever
            .retrieve_for_context("VSCode", "main.rs", None)
            .await
            .unwrap();

        assert!(results.is_empty()); // mock returns empty
        assert!(store.brute_force_called.load(Ordering::Relaxed));
    }

    // ── HNSW-specific tests (feature = "hnsw") ──────────────────

    #[cfg(feature = "hnsw")]
    mod hnsw_tests {
        use super::*;
        use chrono::Utc;
        use oneshim_core::models::embedding::EmbeddingContentType;

        /// Mock ANN index that returns configurable results or errors.
        struct MockAnnIndex {
            search_results: std::sync::Mutex<Option<Vec<(u64, f32)>>>,
            search_fail: AtomicBool,
            add_called: std::sync::atomic::AtomicUsize,
            save_called: AtomicBool,
            load_fail: AtomicBool,
            size: std::sync::atomic::AtomicUsize,
        }

        impl MockAnnIndex {
            fn new(results: Vec<(u64, f32)>) -> Self {
                Self {
                    search_results: std::sync::Mutex::new(Some(results)),
                    search_fail: AtomicBool::new(false),
                    add_called: std::sync::atomic::AtomicUsize::new(0),
                    save_called: AtomicBool::new(false),
                    load_fail: AtomicBool::new(false),
                    size: std::sync::atomic::AtomicUsize::new(0),
                }
            }

            fn with_search_fail(mut self) -> Self {
                self.search_fail = AtomicBool::new(true);
                self
            }

            fn with_load_fail(mut self) -> Self {
                self.load_fail = AtomicBool::new(true);
                self
            }
        }

        #[async_trait]
        impl AnnIndex for MockAnnIndex {
            async fn add(&self, _key: u64, _vector: &[f32]) -> Result<(), CoreError> {
                self.add_called.fetch_add(1, Ordering::Relaxed);
                self.size.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            async fn search(
                &self,
                _query: &[f32],
                _k: usize,
            ) -> Result<Vec<(u64, f32)>, CoreError> {
                if self.search_fail.load(Ordering::Relaxed) {
                    return Err(CoreError::Internal {
                        code: oneshim_core::error_codes::InternalCode::Generic,
                        message: "mock HNSW search failure".into(),
                    });
                }
                Ok(self
                    .search_results
                    .lock()
                    .unwrap()
                    .clone()
                    .unwrap_or_default())
            }
            async fn remove(&self, _key: u64) -> Result<(), CoreError> {
                Ok(())
            }
            async fn save(&self) -> Result<(), CoreError> {
                self.save_called.store(true, Ordering::Relaxed);
                Ok(())
            }
            async fn load(&self) -> Result<(), CoreError> {
                if self.load_fail.load(Ordering::Relaxed) {
                    return Err(CoreError::Internal {
                        code: oneshim_core::error_codes::InternalCode::Generic,
                        message: "mock HNSW load failure".into(),
                    });
                }
                Ok(())
            }
            fn len(&self) -> usize {
                self.size.load(Ordering::Relaxed)
            }
            fn capacity(&self) -> usize {
                50_000
            }
        }

        fn make_test_metadata(key: u64) -> EmbeddingMetadata {
            EmbeddingMetadata {
                segment_id: format!("seg-{key}"),
                content_type: EmbeddingContentType::ContentActivity,
                content_label: Some(format!("label-{key}")),
                timestamp: Utc::now(),
                original_text: format!("text-{key}"),
                model_id: "test-model".to_string(),
            }
        }

        #[test]
        fn strategy_auto_selects_hnsw_in_threshold_range() {
            let store = Arc::new(MockVectorStore::new(0));
            let index = Arc::new(MockVectorIndex::new());
            let ann = Arc::new(MockAnnIndex::new(vec![]));
            let coordinator = AdaptiveSearchCoordinator::new(store, index, SearchConfig::default())
                .with_ann_index(ann);
            // 5_000 is at hnsw_threshold, below brute_force_threshold (10_000)
            coordinator.set_cached_count(5_000);
            assert_eq!(coordinator.determine_strategy(), SearchStrategy::Hnsw);
        }

        #[test]
        fn strategy_auto_selects_hnsw_at_7500() {
            let store = Arc::new(MockVectorStore::new(0));
            let index = Arc::new(MockVectorIndex::new());
            let ann = Arc::new(MockAnnIndex::new(vec![]));
            let coordinator = AdaptiveSearchCoordinator::new(store, index, SearchConfig::default())
                .with_ann_index(ann);
            coordinator.set_cached_count(7_500);
            assert_eq!(coordinator.determine_strategy(), SearchStrategy::Hnsw);
        }

        #[test]
        fn strategy_falls_back_to_brute_force_without_ann_index() {
            let store = Arc::new(MockVectorStore::new(0));
            let index = Arc::new(MockVectorIndex::new());
            // No .with_ann_index() — ann_index is None
            let coordinator = AdaptiveSearchCoordinator::new(store, index, SearchConfig::default());
            coordinator.set_cached_count(7_500);
            // Without ann_index, should select BruteForceInt8 even in HNSW range
            assert_eq!(
                coordinator.determine_strategy(),
                SearchStrategy::BruteForceInt8
            );
        }

        #[test]
        fn strategy_below_hnsw_threshold_uses_brute_force() {
            let store = Arc::new(MockVectorStore::new(0));
            let index = Arc::new(MockVectorIndex::new());
            let ann = Arc::new(MockAnnIndex::new(vec![]));
            let coordinator = AdaptiveSearchCoordinator::new(store, index, SearchConfig::default())
                .with_ann_index(ann);
            coordinator.set_cached_count(4_999);
            assert_eq!(
                coordinator.determine_strategy(),
                SearchStrategy::BruteForceInt8
            );
        }

        #[test]
        fn strategy_forced_hnsw() {
            let store = Arc::new(MockVectorStore::new(0));
            let index = Arc::new(MockVectorIndex::new());
            let config = SearchConfig {
                forced_strategy: Some("hnsw".to_string()),
                ..Default::default()
            };
            let coordinator = AdaptiveSearchCoordinator::new(store, index, config);
            coordinator.set_cached_count(0);
            assert_eq!(coordinator.determine_strategy(), SearchStrategy::Hnsw);
        }

        #[tokio::test]
        async fn search_hnsw_delegates_to_ann_index_and_joins_metadata() {
            let mut metadata = HashMap::new();
            metadata.insert(1, make_test_metadata(1));
            metadata.insert(2, make_test_metadata(2));
            let store = Arc::new(MockVectorStoreWithMetadata::new(7_000, metadata, vec![]));
            let index = Arc::new(MockVectorIndex::new());
            let ann = Arc::new(MockAnnIndex::new(vec![(1, 0.1), (2, 0.3)]));

            let coordinator =
                AdaptiveSearchCoordinator::new(store.clone(), index, SearchConfig::default())
                    .with_ann_index(ann);
            coordinator.set_cached_count(7_000);

            let results = coordinator
                .search(&[1.0, 0.0, 0.0], 5, 168.0, &SearchFilters::default())
                .await
                .unwrap();

            // Should have 2 results from HNSW, not brute-force
            assert_eq!(results.len(), 2);
            assert!(!store.brute_force_called.load(Ordering::Relaxed));
            // Results should be ordered by score (distance-based similarity * time_decay)
            assert!(results[0].score >= results[1].score);
            // Check distance→similarity conversion: similarity = 1.0 - distance
            assert!((results[0].similarity - 0.9).abs() < 0.01); // 1.0 - 0.1
        }

        #[tokio::test]
        async fn search_hnsw_falls_back_on_failure() {
            let store = Arc::new(MockVectorStore::new(7_000));
            let index = Arc::new(MockVectorIndex::new());
            let ann = Arc::new(MockAnnIndex::new(vec![]).with_search_fail());

            let coordinator =
                AdaptiveSearchCoordinator::new(store.clone(), index, SearchConfig::default())
                    .with_ann_index(ann);
            coordinator.set_cached_count(7_000);

            // HNSW search fails → should gracefully fall back to brute-force
            let _results = coordinator
                .search(&[0.1, 0.2, 0.3], 5, 168.0, &SearchFilters::default())
                .await
                .unwrap();

            assert!(
                store.brute_force_called.load(Ordering::Relaxed),
                "should have fallen back to brute-force"
            );
        }

        #[tokio::test]
        async fn join_metadata_converts_distance_to_similarity() {
            let mut metadata = HashMap::new();
            metadata.insert(10, make_test_metadata(10));
            let store = Arc::new(MockVectorStoreWithMetadata::new(0, metadata, vec![]));
            let index = Arc::new(MockVectorIndex::new());
            let ann = Arc::new(MockAnnIndex::new(vec![]));

            let coordinator = AdaptiveSearchCoordinator::new(store, index, SearchConfig::default())
                .with_ann_index(ann);

            let results = coordinator
                .join_metadata(vec![(10, 0.25)], 0.0)
                .await
                .unwrap();

            assert_eq!(results.len(), 1);
            // distance 0.25 → similarity 0.75
            assert!((results[0].similarity - 0.75).abs() < f32::EPSILON);
            // time_decay_hours = 0.0 → time_decay = 1.0
            assert!((results[0].time_decay - 1.0).abs() < f32::EPSILON);
            assert_eq!(results[0].segment_id, "seg-10");
        }

        #[tokio::test]
        async fn join_metadata_skips_missing_ids() {
            let mut metadata = HashMap::new();
            metadata.insert(1, make_test_metadata(1));
            // Key 99 is NOT in the metadata map
            let store = Arc::new(MockVectorStoreWithMetadata::new(0, metadata, vec![]));
            let index = Arc::new(MockVectorIndex::new());
            let ann = Arc::new(MockAnnIndex::new(vec![]));

            let coordinator = AdaptiveSearchCoordinator::new(store, index, SearchConfig::default())
                .with_ann_index(ann);

            let results = coordinator
                .join_metadata(vec![(1, 0.1), (99, 0.2)], 0.0)
                .await
                .unwrap();

            // Only key 1 should appear, key 99 silently skipped
            assert_eq!(results.len(), 1);
            assert_eq!(results[0].segment_id, "seg-1");
        }

        #[tokio::test]
        async fn join_metadata_empty_input() {
            let store = Arc::new(MockVectorStoreWithMetadata::new(0, HashMap::new(), vec![]));
            let index = Arc::new(MockVectorIndex::new());
            let ann = Arc::new(MockAnnIndex::new(vec![]));

            let coordinator = AdaptiveSearchCoordinator::new(store, index, SearchConfig::default())
                .with_ann_index(ann);

            let results = coordinator.join_metadata(vec![], 0.0).await.unwrap();
            assert!(results.is_empty());
        }

        #[tokio::test]
        async fn load_or_rebuild_loads_successfully() {
            let store = Arc::new(MockVectorStoreWithMetadata::new(0, HashMap::new(), vec![]));
            let index = Arc::new(MockVectorIndex::new());
            let ann = Arc::new(MockAnnIndex::new(vec![]));

            let coordinator = AdaptiveSearchCoordinator::new(store, index, SearchConfig::default())
                .with_ann_index(ann.clone());

            coordinator.load_or_rebuild_hnsw().await.unwrap();
            // load() succeeded, no rebuild needed
            assert_eq!(ann.add_called.load(Ordering::Relaxed), 0);
        }

        #[tokio::test]
        async fn load_or_rebuild_rebuilds_on_corrupt() {
            let mut metadata = HashMap::new();
            metadata.insert(1, make_test_metadata(1));
            metadata.insert(2, make_test_metadata(2));
            let vectors = vec![(1, vec![1.0, 0.0, 0.0]), (2, vec![0.0, 1.0, 0.0])];
            let store = Arc::new(MockVectorStoreWithMetadata::new(2, metadata, vectors));
            let index = Arc::new(MockVectorIndex::new());
            let ann = Arc::new(MockAnnIndex::new(vec![]).with_load_fail());

            let coordinator = AdaptiveSearchCoordinator::new(store, index, SearchConfig::default())
                .with_ann_index(ann.clone());

            coordinator.load_or_rebuild_hnsw().await.unwrap();
            // load() failed → should have rebuilt from 2 vectors
            assert_eq!(ann.add_called.load(Ordering::Relaxed), 2);
            assert!(ann.save_called.load(Ordering::Relaxed));
        }

        #[tokio::test]
        async fn load_or_rebuild_empty_sqlite() {
            let store = Arc::new(MockVectorStoreWithMetadata::new(0, HashMap::new(), vec![]));
            let index = Arc::new(MockVectorIndex::new());
            let ann = Arc::new(MockAnnIndex::new(vec![]).with_load_fail());

            let coordinator = AdaptiveSearchCoordinator::new(store, index, SearchConfig::default())
                .with_ann_index(ann.clone());

            coordinator.load_or_rebuild_hnsw().await.unwrap();
            // load() failed but no vectors in SQLite → no add/save
            assert_eq!(ann.add_called.load(Ordering::Relaxed), 0);
            assert!(!ann.save_called.load(Ordering::Relaxed));
        }

        #[tokio::test]
        async fn load_or_rebuild_no_ann_index() {
            let store = Arc::new(MockVectorStore::new(0));
            let index = Arc::new(MockVectorIndex::new());
            // No ann_index attached
            let coordinator = AdaptiveSearchCoordinator::new(store, index, SearchConfig::default());

            // Should return Ok(()) silently when no ann_index
            coordinator.load_or_rebuild_hnsw().await.unwrap();
        }
    }
}
