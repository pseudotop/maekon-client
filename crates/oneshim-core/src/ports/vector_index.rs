//! Port trait for vector index operations (IVF + binary code).
//!
//! Defines the contract for building and searching indexed vector collections.
//! The primary adapter is `SqliteVectorIndex` in oneshim-storage.

use async_trait::async_trait;

use crate::binary_quantizer::{BinaryCode, QuantileThresholds};
use crate::error::CoreError;
use crate::models::embedding::{SearchFilters, SearchResult};
use crate::quantization::QuantizedVector;

/// Metadata about the current state of the vector index.
#[derive(Debug, Clone)]
pub struct IndexMeta {
    /// Timestamp when the IVF index was last built (ISO 8601), or None if never built.
    pub ivf_built_at: Option<String>,
    /// Number of vectors included in the last IVF build.
    pub ivf_vector_count: u64,
    /// Timestamp when binary codes were last built, or None if never built.
    pub binary_built_at: Option<String>,
    /// Total number of active (non-stale) vectors.
    pub total_vector_count: u64,
    /// Number of vectors not yet assigned to an IVF cluster.
    pub unindexed_count: u64,
}

/// Port for vector index build and search operations.
///
/// All methods have default implementations returning `CoreError::Internal { code: crate::error_codes::InternalCode::Generic, message: "not implemented" }`
/// so that test mocks compile without change.
///
/// # Errors
/// - `CoreError::Storage` (wire: `storage.failed`) — production adapters
///   (`SqliteVectorIndex`) surface SQLite errors during build/search/
///   centroid persistence (iter-47 mass fix pattern).
/// - `CoreError::Internal` (wire: `internal.generic`) — trait default
///   impls return "not implemented"; k-means convergence failure;
///   dimension-mismatch between query vector and stored centroids.
/// - Build operations (`build_ivf_index`, `build_binary_codes`) on an
///   empty collection return `Ok(0)` rather than an error — callers
///   infer no-op from the zero count.
/// - `get_index_meta` on a never-built index returns `Ok(IndexMeta { ivf_built_at: None, .. })`, not Err.
#[async_trait]
#[allow(clippy::too_many_arguments)]
pub trait VectorIndex: Send + Sync {
    /// Build (or rebuild) the IVF cluster index.
    ///
    /// Loads all non-stale INT8 vectors, runs k-means++/Lloyd's with the given
    /// parameters, and persists centroids + assignments.
    /// Returns the number of clusters created.
    async fn build_ivf_index(
        &self,
        _n_clusters: usize,
        _n_iterations: usize,
    ) -> Result<usize, CoreError> {
        Err(CoreError::Internal {
            code: crate::error_codes::InternalCode::Generic,
            message: "build_ivf_index not implemented".into(),
        })
    }

    /// Build (or rebuild) 2-bit binary codes for all indexed vectors.
    ///
    /// Computes quantile thresholds across the collection, encodes each vector,
    /// and persists the codes. Returns the number of codes generated.
    async fn build_binary_codes(&self) -> Result<u64, CoreError> {
        Err(CoreError::Internal {
            code: crate::error_codes::InternalCode::Generic,
            message: "build_binary_codes not implemented".into(),
        })
    }

    /// Search using IVF partitioning with INT8 cosine similarity.
    ///
    /// Probes the nearest `nprobe` clusters and performs brute-force INT8
    /// cosine similarity within those partitions.
    async fn search_ivf(
        &self,
        _query_vector: &QuantizedVector,
        _nprobe: usize,
        _limit: usize,
        _time_decay_hours: f32,
        _filters: &SearchFilters,
    ) -> Result<Vec<SearchResult>, CoreError> {
        Err(CoreError::Internal {
            code: crate::error_codes::InternalCode::Generic,
            message: "search_ivf not implemented".into(),
        })
    }

    /// Search using IVF + 2-bit binary Hamming filter + INT8 re-ranking.
    ///
    /// 1. Probes nearest `nprobe` clusters
    /// 2. Hamming distance filter keeps top `limit * oversample_factor` candidates
    /// 3. INT8 cosine similarity re-ranks the survivors
    async fn search_ivf_binary(
        &self,
        _query_vector: &QuantizedVector,
        _query_binary: &BinaryCode,
        _nprobe: usize,
        _oversample_factor: usize,
        _limit: usize,
        _time_decay_hours: f32,
        _filters: &SearchFilters,
    ) -> Result<Vec<SearchResult>, CoreError> {
        Err(CoreError::Internal {
            code: crate::error_codes::InternalCode::Generic,
            message: "search_ivf_binary not implemented".into(),
        })
    }

    /// Assign a single vector to its nearest IVF cluster (incremental update).
    async fn assign_to_cluster(
        &self,
        _vector_id: i64,
        _vector: &QuantizedVector,
    ) -> Result<(), CoreError> {
        Err(CoreError::Internal {
            code: crate::error_codes::InternalCode::Generic,
            message: "assign_to_cluster not implemented".into(),
        })
    }

    /// Store a single binary code for a vector (incremental update).
    async fn store_binary_code(
        &self,
        _vector_id: i64,
        _code: &BinaryCode,
    ) -> Result<(), CoreError> {
        Err(CoreError::Internal {
            code: crate::error_codes::InternalCode::Generic,
            message: "store_binary_code not implemented".into(),
        })
    }

    /// Get metadata about the current index state.
    async fn get_index_meta(&self) -> Result<IndexMeta, CoreError> {
        Err(CoreError::Internal {
            code: crate::error_codes::InternalCode::Generic,
            message: "get_index_meta not implemented".into(),
        })
    }

    /// Count vectors that have not been assigned to any IVF cluster.
    async fn count_unindexed(&self) -> Result<u64, CoreError> {
        Err(CoreError::Internal {
            code: crate::error_codes::InternalCode::Generic,
            message: "count_unindexed not implemented".into(),
        })
    }

    /// Load the quantile thresholds used for binary encoding, if available.
    async fn load_quantile_thresholds(&self) -> Result<Option<QuantileThresholds>, CoreError> {
        Err(CoreError::Internal {
            code: crate::error_codes::InternalCode::Generic,
            message: "load_quantile_thresholds not implemented".into(),
        })
    }
}
