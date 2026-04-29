mod helpers;
mod trait_impl;

#[cfg(test)]
mod tests;

use rusqlite::Connection;
use std::sync::{Arc, Mutex};

use crate::error::StorageError;

pub use helpers::{
    brute_force_search, brute_force_search_quantized, bytes_to_f32_vec, bytes_to_i8_vec,
    content_type_to_str, cosine_similarity, f32_vec_to_bytes, i8_vec_to_bytes, map_quantized_row,
    map_vector_row, parse_content_type, QuantizedVectorRow, VectorRow,
};

/// SQLite-backed vector store with brute-force cosine similarity search.
///
/// Vectors are stored as little-endian f32 BLOBs in the `embedding_vectors` table.
/// Search is performed in-memory via brute-force cosine similarity with optional
/// exponential time decay weighting.
pub struct SqliteVectorStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteVectorStore {
    /// Create a new `SqliteVectorStore` sharing the same connection as `SqliteStorage`.
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    /// Wrap a synchronous closure on the connection via `spawn_blocking`.
    async fn with_conn<F, T>(&self, f: F) -> Result<T, StorageError>
    where
        F: FnOnce(&Connection) -> Result<T, StorageError> + Send + 'static,
        T: Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let guard = conn
                .lock()
                .map_err(|e| StorageError::Internal(format!("SQLite lock poisoned: {e}")))?;
            f(&guard)
        })
        .await
        .map_err(|e| StorageError::Internal(format!("spawn_blocking join error: {e}")))?
    }
}
