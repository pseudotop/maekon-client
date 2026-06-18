mod helpers;
mod trait_impl;

#[cfg(test)]
mod tests;

use rusqlite::Connection;
use std::sync::atomic::AtomicU32;
use std::sync::Arc;

use crate::error::StorageError;
use crate::sqlite::hlc_clock::HlcClock;
use crate::sqlite::vector_index_impl::SqliteVectorIndex;
use crate::sqlite::GuardedConnection;

pub use helpers::{
    brute_force_search, brute_force_search_quantized, bytes_to_f32_vec, bytes_to_i8_vec,
    content_type_to_str, corpus_dims_guard, cosine_similarity, f32_vec_to_bytes, i8_vec_to_bytes,
    map_quantized_row, map_vector_row, parse_content_type, QuantizedVectorRow, VectorRow,
};

// Sentinel value meaning "cache not yet populated" for corpus_dims_cache.
// Zero is safe because valid embedding dimensions are always >= 1.
const DIMS_CACHE_UNSET: u32 = 0;

/// `app_meta` key under which the corpus vector dimension is persisted.
pub(super) const CORPUS_DIMS_META_KEY: &str = "embedding_corpus_dims";

/// SQLite-backed vector store with IVF-routed and brute-force fallback search.
///
/// Vectors are stored as little-endian f32 BLOBs in the `embedding_vectors` table.
/// When an IVF index has been built (`ivf_built_at IS NOT NULL`), `search` and
/// `search_filtered` route through `SqliteVectorIndex::search_ivf` and supplement the
/// result with any unindexed-delta vectors via a targeted brute-force scan.  When no
/// IVF index is available, or when IVF returns an error, the implementation falls back
/// to the original full brute-force path.
///
/// # IVF routing gate (#5642)
///
/// The gate checks two conditions before taking the IVF path:
///   1. `get_index_meta().ivf_built_at.is_some()` — index was built at least once.
///   2. Corpus dimension matches query dimension — detected by comparing
///      `corpus_dims_cache` (set at write time) to `query_vector.len()`.
///
/// The meta query is a cheap pair of COUNT queries and a few key lookups; no centroid
/// loading happens until `search_ivf` is called and `ensure_cache` fires.
///
/// # Unindexed delta correctness (#5642)
///
/// `search_ivf_impl` uses `INNER JOIN ivf_assignments`, so vectors inserted after the
/// last `build_ivf_index` run (cluster-unassigned) are invisible to the IVF path.  When
/// `unindexed_count > 0` the implementation fetches those rows separately via a
/// targeted brute-force query and merges both result sets before truncating to `limit`.
///
/// # Corpus dimension guard (#5755 FLAG-DIM-768)
///
/// `corpus_dims_cache` is an in-process cache of the last-known corpus dimension
/// recorded in `app_meta` under `CORPUS_DIMS_META_KEY`. It avoids a `SELECT` on
/// every `store`/`store_quantized` call after the first write. The value is
/// `DIMS_CACHE_UNSET` (0) until the first vector is stored.
pub struct SqliteVectorStore {
    conn: Arc<GuardedConnection>,
    /// Persistent monotonic HLC clock for stamping local `embedding_vectors` writes so
    /// they propagate via cross-device sync (F0/#5186). Its own instance is fine — the
    /// durable floor lives in the `hlc_clock` table reached through the **same**
    /// `GuardedConnection` mutex as `SqliteStorage`, so RMW stays serialized + monotonic.
    clock: Arc<HlcClock>,
    /// In-process cache of the persisted corpus vector dimension (number of f32/i8 elements).
    /// Stored as `u32`; `DIMS_CACHE_UNSET` (0) means "not yet loaded from app_meta".
    /// Updated atomically after every dimension transition. See corpus_dims_guard.
    corpus_dims_cache: Arc<AtomicU32>,
    /// IVF index adapter sharing the same GuardedConnection.
    /// Used by the IVF routing path in `search` and `search_filtered`.
    pub(super) ivf_index: Arc<SqliteVectorIndex>,
}

impl SqliteVectorStore {
    /// Create a new `SqliteVectorStore` sharing the same [`GuardedConnection`]
    /// as `SqliteStorage` (#4928 — a barrier-free handle is not allowed).
    pub fn new(conn: Arc<GuardedConnection>) -> Self {
        let ivf_index = Arc::new(SqliteVectorIndex::new(conn.clone()));
        Self {
            conn,
            clock: Arc::new(HlcClock::new()),
            corpus_dims_cache: Arc::new(AtomicU32::new(DIMS_CACHE_UNSET)),
            ivf_index,
        }
    }

    /// Funnel that isolates a **write** closure onto spawn_blocking.
    /// When deletion_flag is set, the closure is not run and `Ok(T::default())`
    /// is returned instead.
    async fn with_conn<F, T>(&self, f: F) -> Result<T, StorageError>
    where
        F: FnOnce(&Connection) -> Result<T, StorageError> + Send + 'static,
        T: Default + Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || conn.write_lock().run(T::default(), f))
            .await
            .map_err(|e| StorageError::Internal(format!("spawn_blocking join error: {e}")))?
    }

    /// Funnel that isolates a **read** closure onto spawn_blocking
    /// (independent of deletion_flag).
    async fn with_conn_read<F, T>(&self, f: F) -> Result<T, StorageError>
    where
        F: FnOnce(&Connection) -> Result<T, StorageError> + Send + 'static,
        T: Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || conn.read_lock().run(f))
            .await
            .map_err(|e| StorageError::Internal(format!("spawn_blocking join error: {e}")))?
    }
}
