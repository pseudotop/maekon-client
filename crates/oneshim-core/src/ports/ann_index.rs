//! Port trait for approximate nearest neighbor (ANN) index operations.
//!
//! Defines the contract for HNSW-based vector indexing with persistence.
//! The primary adapter is `HnswAdapter` in oneshim-analysis (feature = "hnsw").

use async_trait::async_trait;

use crate::error::CoreError;

/// Approximate Nearest Neighbor index port.
///
/// Provides key-addressable vector insertion, k-NN search, removal,
/// and file-based persistence. All mutations are expected to be
/// internally synchronized (interior mutability).
///
/// # Implementations
///
/// - `HnswAdapter` (oneshim-analysis, feature = "hnsw") — usearch HNSW index
///
/// # Errors
/// - `CoreError::Analysis` (wire: `provider.analysis_failed`) for HNSW
///   library failures (usearch add/search/remove), capacity growth
///   errors, file I/O during save/load.
/// - `CoreError::Internal` (wire: `internal.generic`) for tokio
///   spawn_blocking JoinError — HNSW calls run on a blocking thread
///   pool to avoid stalling tokio workers during SIMD vector math.
#[async_trait]
pub trait AnnIndex: Send + Sync {
    /// Insert a vector under the given key.
    ///
    /// If `len() > capacity() * 80 / 100`, the implementation should
    /// grow its internal capacity before inserting.
    async fn add(&self, key: u64, vector: &[f32]) -> Result<(), CoreError>;

    /// Return the `k` nearest neighbors as `(key, distance)` pairs,
    /// ordered by ascending distance.
    async fn search(&self, query: &[f32], k: usize) -> Result<Vec<(u64, f32)>, CoreError>;

    /// Remove the vector associated with `key`.
    async fn remove(&self, key: u64) -> Result<(), CoreError>;

    /// Persist the index to its configured data path (atomic write).
    async fn save(&self) -> Result<(), CoreError>;

    /// Load the index from its configured data path.
    async fn load(&self) -> Result<(), CoreError>;

    /// Number of vectors currently stored.
    fn len(&self) -> usize;

    /// Total reserved capacity (number of vectors the index can hold
    /// before the next reallocation).
    fn capacity(&self) -> usize;

    /// Returns `true` if the index contains no vectors.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
