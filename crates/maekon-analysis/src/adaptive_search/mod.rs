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
use maekon_core::binary_quantizer::BinaryQuantizer;
use maekon_core::models::embedding::{SearchFilters, SearchResult};
#[cfg(feature = "hnsw")]
use maekon_core::ports::ann_index::AnnIndex;
use maekon_core::ports::vector_index::VectorIndex;
use maekon_core::ports::vector_store::VectorStore;
use maekon_core::quantization::ScalarQuantizer;
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
    pub(super) cached_vector_count: AtomicU64,
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
    pub(super) async fn join_metadata(
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
impl maekon_core::ports::adaptive_search::AdaptiveSearchPort for AdaptiveSearchCoordinator {
    async fn search(
        &self,
        query_f32: &[f32],
        limit: usize,
        time_decay_hours: f32,
        filters: &SearchFilters,
    ) -> Result<Vec<SearchResult>, maekon_core::error::CoreError> {
        self.search(query_f32, limit, time_decay_hours, filters)
            .await
            .map_err(Into::into)
    }

    async fn refresh_count(&self) -> Result<(), maekon_core::error::CoreError> {
        self.refresh_count().await.map_err(Into::into)
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
