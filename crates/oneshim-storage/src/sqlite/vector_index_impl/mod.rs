//! SQLite-backed implementation of the VectorIndex port.
//!
//! Stores IVF centroids, cluster assignments, and binary codes in SQLite tables
//! (created by V16 migration). Supports full rebuild and incremental updates.

mod build;
mod metadata;
mod search;

#[cfg(test)]
mod tests;

use oneshim_core::models::embedding::{EmbeddingContentType, SearchFilters, SearchResult};
use oneshim_core::quantization::{QuantizedVector, ScalarQuantizer};
use rusqlite::Connection;
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;

use crate::error::StorageError;

/// SQLite-backed vector index supporting IVF clustering and binary code search.
pub struct SqliteVectorIndex {
    conn: Arc<Mutex<Connection>>,
    /// Cached centroids for query-time probe selection.
    /// Uses `tokio::sync::RwLock` so the guard is `Send` — safe to hold
    /// briefly inside async methods without blocking the executor.
    centroid_cache: RwLock<Option<Vec<CachedCentroid>>>,
}

/// Cached centroid data for query-time use.
struct CachedCentroid {
    id: usize,
    vector: QuantizedVector,
}

impl SqliteVectorIndex {
    /// Create a new index implementation sharing the same DB connection.
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self {
            conn,
            centroid_cache: RwLock::new(None),
        }
    }

    /// Execute a closure with the SQLite connection via spawn_blocking.
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

    /// Load centroids from DB into cache if not already cached.
    async fn ensure_cache(&self) -> Result<(), StorageError> {
        {
            let cache = self.centroid_cache.read().await;
            if cache.is_some() {
                return Ok(());
            }
        }

        // Scope the entire SQLite operation so conn + stmt are dropped
        // before the tokio RwLock write-await below.
        let centroids = {
            let conn = self
                .conn
                .lock()
                .map_err(|e| StorageError::Internal(format!("SQLite lock poisoned: {e}")))?;

            let mut stmt = conn
                .prepare("SELECT id, centroid_int8, centroid_scale, centroid_offset FROM ivf_centroids ORDER BY id")
                .map_err(|e| StorageError::Internal(format!("Failed to prepare centroid query: {e}")))?;

            let rows: Vec<CachedCentroid> = stmt
                .query_map([], |row| {
                    let id: i64 = row.get(0)?;
                    let blob: Vec<u8> = row.get(1)?;
                    let scale: f32 = row.get(2)?;
                    let offset: f32 = row.get(3)?;
                    Ok(CachedCentroid {
                        id: id as usize,
                        vector: QuantizedVector {
                            data: blob.iter().map(|&b| b as i8).collect(),
                            scale,
                            offset,
                        },
                    })
                })
                .map_err(|e| StorageError::Internal(format!("Failed to query centroids: {e}")))?
                .filter_map(|r| r.ok())
                .collect();
            rows
        };

        let mut cache = self.centroid_cache.write().await;
        *cache = Some(centroids);
        Ok(())
    }

    /// Invalidate the centroid cache (e.g., after rebuild).
    async fn invalidate_cache(&self) {
        let mut cache = self.centroid_cache.write().await;
        *cache = None;
    }
}

fn parse_content_type(s: &str) -> EmbeddingContentType {
    match s {
        "SEGMENT_SUMMARY" => EmbeddingContentType::SegmentSummary,
        _ => EmbeddingContentType::ContentActivity,
    }
}

/// A row of data fetched for scoring and ranking.
type ScoringRow = (
    String,
    String,
    Option<String>,
    String,
    Vec<i8>,
    f32,
    f32,
    String,
);

/// Build search results from INT8 rows with cosine similarity + time decay.
///
/// Uses `cosine_similarity_int8_unchecked` because all rows originate from
/// the same model/index and share the query's dimensionality.
fn score_and_rank(
    rows: Vec<ScoringRow>,
    query: &QuantizedVector,
    limit: usize,
    time_decay_hours: f32,
) -> Vec<SearchResult> {
    let now = chrono::Utc::now();
    let mut scored: Vec<SearchResult> = rows
        .into_iter()
        .map(
            |(
                segment_id,
                content_type,
                content_label,
                original_text,
                data,
                scale,
                offset,
                ts_str,
            )| {
                let row_qv = QuantizedVector {
                    data,
                    scale,
                    offset,
                };
                let similarity = ScalarQuantizer::cosine_similarity_int8_unchecked(query, &row_qv);

                let timestamp = chrono::DateTime::parse_from_rfc3339(&ts_str)
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or(now);
                let age_hours = (now - timestamp).num_seconds().max(0) as f32 / 3600.0;
                let time_decay = if time_decay_hours > 0.0 {
                    (-age_hours / time_decay_hours).exp()
                } else {
                    1.0
                };
                let score = similarity * time_decay;

                SearchResult {
                    segment_id,
                    content_type: parse_content_type(&content_type),
                    content_label,
                    score,
                    similarity,
                    time_decay,
                    timestamp,
                    original_text,
                }
            },
        )
        .collect();

    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(limit);
    scored
}

/// Build a WHERE clause and params for the common filter conditions.
fn build_filter_conditions(
    filters: &SearchFilters,
) -> (Vec<String>, Vec<Box<dyn rusqlite::types::ToSql>>) {
    let mut conditions = vec!["ev.is_stale = 0".to_string()];
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(ref after) = filters.after {
        conditions.push(format!("ev.timestamp >= ?{}", param_values.len() + 1));
        param_values.push(Box::new(after.to_rfc3339()));
    }
    if let Some(ref before) = filters.before {
        conditions.push(format!("ev.timestamp <= ?{}", param_values.len() + 1));
        param_values.push(Box::new(before.to_rfc3339()));
    }
    if let Some(ref content_types) = filters.content_types {
        if !content_types.is_empty() {
            let placeholders: Vec<String> = content_types
                .iter()
                .map(|_| {
                    let idx = param_values.len() + 1;
                    format!("?{idx}")
                })
                .collect();
            conditions.push(format!("ev.content_type IN ({})", placeholders.join(", ")));
            for ct in content_types {
                let s = match ct {
                    EmbeddingContentType::SegmentSummary => "SEGMENT_SUMMARY",
                    EmbeddingContentType::ContentActivity => "CONTENT_ACTIVITY",
                };
                param_values.push(Box::new(s.to_string()));
            }
        }
    }
    if !filters.excluded_segment_ids.is_empty() {
        let base_idx = param_values.len();
        let placeholders: Vec<String> = filters
            .excluded_segment_ids
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", base_idx + i + 1))
            .collect();
        conditions.push(format!(
            "ev.segment_id NOT IN ({})",
            placeholders.join(", ")
        ));
        for seg_id in &filters.excluded_segment_ids {
            param_values.push(Box::new(seg_id.clone()));
        }
    }

    (conditions, param_values)
}
