//! Port for storing and searching embedding vectors with time-decay and metadata filtering.

use std::collections::HashMap;

use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::embedding::{EmbeddingMetadata, SearchFilters, SearchResult};
use crate::quantization::QuantizedVector;

/// Port for storing and searching embedding vectors.
/// Primary adapter: brute-force cosine similarity implementation in oneshim-storage.
///
/// # Errors
/// - `CoreError::Storage` (wire: `storage.failed`) for SQLite-backed
///   operations in the oneshim-storage adapter (iter-47 mass fix pattern).
/// - `CoreError::InvalidArguments` (wire: `validation.invalid_arguments`)
///   for caller-side input violations — empty/NaN vectors, dimension
///   mismatch (iter-95/106 pattern).
/// - `CoreError::Internal` (wire: `internal.generic`) for default impls
///   of optional methods (`store_quantized`, `search_quantized`,
///   `backfill_quantized`) that an adapter chooses not to override.
///   Production adapters that do override these emit domain-appropriate
///   variants instead.
#[async_trait]
pub trait VectorStore: Send + Sync {
    // --- Core CRUD ---

    /// Store a vector with its associated metadata.
    async fn store(&self, vector: Vec<f32>, metadata: EmbeddingMetadata) -> Result<(), CoreError>;

    /// Search for the top-k most similar vectors with time decay weighting.
    async fn search(
        &self,
        query_vector: &[f32],
        limit: usize,
        time_decay_hours: f32,
    ) -> Result<Vec<SearchResult>, CoreError>;

    /// Search with additional metadata filters (time range, content type, regime).
    async fn search_filtered(
        &self,
        query_vector: &[f32],
        limit: usize,
        time_decay_hours: f32,
        filters: &SearchFilters,
    ) -> Result<Vec<SearchResult>, CoreError>;

    /// Delete embedding vectors older than max_days. Returns count of deleted rows.
    async fn enforce_retention(&self, max_days: u32) -> Result<u64, CoreError>;

    /// Mark vectors produced by an old model as stale. Returns count of marked rows.
    async fn mark_stale(&self, old_model_id: &str) -> Result<u64, CoreError>;

    /// Update a re-embedded vector: replace the BLOB, model_id, and clear stale flag.
    async fn update_vector(
        &self,
        id: i64,
        vector: Vec<f32>,
        model_id: &str,
    ) -> Result<(), CoreError>;

    // --- Quantized (Phase A) ---

    /// Store a pre-quantized INT8 vector alongside its float32 original.
    ///
    /// When `skip_float32` is `true`, an empty BLOB is stored instead of
    /// the f32 data to save storage (Phase A.5 float32 retention control).
    async fn store_quantized(
        &self,
        _vector_f32: Vec<f32>,
        _vector_int8: &QuantizedVector,
        _metadata: EmbeddingMetadata,
        _skip_float32: bool,
    ) -> Result<(), CoreError> {
        Err(CoreError::Internal {
            code: crate::error_codes::InternalCode::Generic,
            message: "store_quantized not implemented".into(),
        })
    }

    /// Search using INT8 quantized cosine similarity (faster, approximate).
    /// Accepts SearchFilters for parity with search_filtered.
    async fn search_quantized(
        &self,
        _query_vector: &QuantizedVector,
        _limit: usize,
        _time_decay_hours: f32,
        _filters: &SearchFilters,
    ) -> Result<Vec<SearchResult>, CoreError> {
        Err(CoreError::Internal {
            code: crate::error_codes::InternalCode::Generic,
            message: "search_quantized not implemented".into(),
        })
    }

    /// Backfill INT8 quantization for existing float32-only vectors.
    /// Processes rows WHERE vector_int8 IS NULL LIMIT batch_size.
    /// Returns the number of rows backfilled.
    async fn backfill_quantized(&self, _batch_size: usize) -> Result<u64, CoreError> {
        Err(CoreError::Internal {
            code: crate::error_codes::InternalCode::Generic,
            message: "backfill_quantized not implemented".into(),
        })
    }

    /// Count rows that have not yet been quantized to INT8.
    /// Returns the number of rows WHERE vector_int8 IS NULL.
    /// Used to determine when float32 column removal is safe.
    async fn count_unquantized(&self) -> Result<u64, CoreError> {
        Ok(0)
    }

    // --- Metadata ---

    /// Get the model_id of the most recent non-stale vector, if any.
    async fn get_current_model_id(&self) -> Result<Option<String>, CoreError>;

    /// Fetch a batch of stale vectors for re-embedding.
    /// Returns (id, original_text) pairs. Limit controls batch size.
    async fn get_stale_vectors(&self, limit: usize) -> Result<Vec<(i64, String)>, CoreError>;

    /// Count the number of active (non-stale) vectors in the store.
    /// Used by AdaptiveSearchCoordinator to select search strategy.
    async fn count_active_vectors(&self) -> Result<u64, CoreError> {
        Ok(0)
    }

    /// Batch-fetch metadata for a set of vector row IDs.
    ///
    /// Used by the HNSW search path to join ANN results (key, distance)
    /// with full metadata from the embedding_vectors table.
    /// Returns a map from row id to metadata. IDs not found are silently skipped.
    async fn get_metadata_by_ids(
        &self,
        _ids: &[u64],
    ) -> Result<HashMap<u64, EmbeddingMetadata>, CoreError> {
        Ok(HashMap::new())
    }

    /// Fetch all active vectors as (row_id, f32_vector) pairs.
    ///
    /// Used for HNSW index rebuild when the persisted index file is
    /// corrupt or missing. Only returns rows where `is_stale = 0`
    /// and the f32 vector column is non-empty.
    async fn get_all_vectors_for_rebuild(&self) -> Result<Vec<(u64, Vec<f32>)>, CoreError> {
        Ok(Vec::new())
    }

    /// Return row IDs of vectors that would be deleted by `enforce_retention(max_days)`.
    ///
    /// Used by the HNSW integration to remove corresponding entries from the
    /// ANN index before the SQLite rows are deleted. The caller should invoke
    /// this *before* `enforce_retention` so the IDs are still available.
    async fn get_expired_ids(&self, _max_days: u32) -> Result<Vec<u64>, CoreError> {
        Ok(Vec::new())
    }

    /// Return the last-inserted row ID.
    ///
    /// Used by the embedding pipeline to obtain the key for a newly stored
    /// vector so it can be inserted into the HNSW index.
    async fn last_insert_id(&self) -> Result<u64, CoreError> {
        Ok(0)
    }
}
