use chrono::{DateTime, Utc};
use maekon_core::models::embedding::{EmbeddingContentType, SearchResult};
use maekon_core::quantization::QuantizedVector;
use rusqlite::Connection;
use tracing::warn;

/// Convert a slice of f32 values to a little-endian byte vector.
pub fn f32_vec_to_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Convert a byte slice back to a `Vec<f32>` (little-endian).
pub fn bytes_to_f32_vec(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Convert a slice of i8 values to a byte vector (for SQLite BLOB storage).
pub fn i8_vec_to_bytes(v: &[i8]) -> Vec<u8> {
    v.iter().map(|&b| b as u8).collect()
}

/// Convert a byte slice back to a `Vec<i8>`.
pub fn bytes_to_i8_vec(b: &[u8]) -> Vec<i8> {
    b.iter().map(|&b| b as i8).collect()
}

/// Compute cosine similarity between two vectors.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}

/// Row fetched from the embedding_vectors table for brute-force search.
pub struct VectorRow {
    pub segment_id: String,
    pub content_type: String,
    pub content_label: Option<String>,
    pub original_text: String,
    pub vector: Vec<f32>,
    pub timestamp: DateTime<Utc>,
}

/// Row fetched for INT8 brute-force search.
pub struct QuantizedVectorRow {
    pub segment_id: String,
    pub content_type: String,
    pub content_label: Option<String>,
    pub original_text: String,
    pub vector_int8: Vec<i8>,
    pub quant_scale: f32,
    pub quant_offset: f32,
    pub timestamp: DateTime<Utc>,
}

pub fn parse_content_type(s: &str) -> EmbeddingContentType {
    match s {
        "SEGMENT_SUMMARY" => EmbeddingContentType::SegmentSummary,
        _ => EmbeddingContentType::ContentActivity,
    }
}

/// Map a single SQLite row (with columns segment_id, content_type, content_label,
/// original_text, vector, timestamp at positions 0..5) to a `VectorRow`.
pub fn map_vector_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<VectorRow> {
    let ts_str: String = row.get(5)?;
    let timestamp = DateTime::parse_from_rfc3339(&ts_str)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    let blob: Vec<u8> = row.get(4)?;
    Ok(VectorRow {
        segment_id: row.get(0)?,
        content_type: row.get(1)?,
        content_label: row.get(2)?,
        original_text: row.get(3)?,
        vector: bytes_to_f32_vec(&blob),
        timestamp,
    })
}

/// Map a SQLite row (segment_id, content_type, content_label, original_text,
/// vector_int8, quant_scale, quant_offset, timestamp at positions 0..7)
/// to a QuantizedVectorRow.
pub fn map_quantized_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<QuantizedVectorRow> {
    let ts_str: String = row.get(7)?;
    let timestamp = DateTime::parse_from_rfc3339(&ts_str)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    let blob: Vec<u8> = row.get(4)?;
    Ok(QuantizedVectorRow {
        segment_id: row.get(0)?,
        content_type: row.get(1)?,
        content_label: row.get(2)?,
        original_text: row.get(3)?,
        vector_int8: bytes_to_i8_vec(&blob),
        quant_scale: row.get(5)?,
        quant_offset: row.get(6)?,
        timestamp,
    })
}

pub fn content_type_to_str(ct: &EmbeddingContentType) -> &'static str {
    match ct {
        EmbeddingContentType::SegmentSummary => "SEGMENT_SUMMARY",
        EmbeddingContentType::ContentActivity => "CONTENT_ACTIVITY",
    }
}

/// Execute brute-force search on rows, applying cosine similarity + time decay.
pub fn brute_force_search(
    rows: Vec<VectorRow>,
    query_vector: &[f32],
    limit: usize,
    time_decay_hours: f32,
) -> Vec<SearchResult> {
    let now = Utc::now();
    let mut scored: Vec<SearchResult> = rows
        .into_iter()
        .map(|row| {
            let similarity = cosine_similarity(query_vector, &row.vector);
            let age_hours = (now - row.timestamp).num_seconds().max(0) as f32 / 3600.0;
            let time_decay = if time_decay_hours > 0.0 {
                (-age_hours / time_decay_hours).exp()
            } else {
                1.0
            };
            let score = similarity * time_decay;
            SearchResult {
                segment_id: row.segment_id,
                content_type: parse_content_type(&row.content_type),
                content_label: row.content_label,
                score,
                similarity,
                time_decay,
                timestamp: row.timestamp,
                original_text: row.original_text,
            }
        })
        .collect();

    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(limit);
    scored
}

/// Execute brute-force search on INT8 quantized rows.
///
/// Rows whose stored INT8 vector dimension (`vector_int8.len()`) differs from the
/// query dimension are **skipped** silently. This is the read-side guard for the
/// mixed-dimension problem (#5755 FLAG-DIM-768): when the embedding model changes
/// from e.g. 384 → 768 dimensions, old-model rows that have not yet been
/// re-embedded carry a different length. Including them in the dot-product via
/// `zip`-truncation would produce silently incorrect scores.
///
/// IVF search is **not** touched here — it already returns `CoreError::InvalidArguments`
/// on centroid/query dimension mismatch (intended; see vector_index_impl/search.rs).
pub fn brute_force_search_quantized(
    rows: Vec<QuantizedVectorRow>,
    query_vector: &QuantizedVector,
    limit: usize,
    time_decay_hours: f32,
) -> Vec<SearchResult> {
    let now = Utc::now();
    let query_dims = query_vector.data.len();
    let mut skipped: usize = 0;

    let mut scored: Vec<SearchResult> = rows
        .into_iter()
        .filter_map(|row| {
            // Read-side dimension guard: skip rows whose stored dimension does not
            // match the query. These are stale-model rows pending re-embedding.
            if row.vector_int8.len() != query_dims {
                skipped += 1;
                return None;
            }
            let row_qv = QuantizedVector {
                data: row.vector_int8,
                scale: row.quant_scale,
                offset: row.quant_offset,
            };
            let similarity =
                maekon_core::quantization::ScalarQuantizer::cosine_similarity_int8_unchecked(
                    query_vector,
                    &row_qv,
                );
            let age_hours = (now - row.timestamp).num_seconds().max(0) as f32 / 3600.0;
            let time_decay = if time_decay_hours > 0.0 {
                (-age_hours / time_decay_hours).exp()
            } else {
                1.0
            };
            let score = similarity * time_decay;
            Some(SearchResult {
                segment_id: row.segment_id,
                content_type: parse_content_type(&row.content_type),
                content_label: row.content_label,
                score,
                similarity,
                time_decay,
                timestamp: row.timestamp,
                original_text: row.original_text,
            })
        })
        .collect();

    if skipped > 0 {
        // Log once per search call (outside the loop). Callers see this in debug output;
        // the system.rs sweep loop will eventually re-embed these rows.
        warn!(
            skipped,
            query_dims, "brute_force_search_quantized: skipped dim-mismatched rows (stale-model vectors pending re-embed)"
        );
    }

    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(limit);
    scored
}

/// Check and update the corpus vector dimension recorded in `app_meta`.
///
/// Called from `store` and `store_quantized` before each INSERT. This is the
/// write-side guard for the mixed-dimension problem (#5755 FLAG-DIM-768).
///
/// # Behaviour
///
/// - **First write** (key absent): records `new_dims` → `Ok(())`.
/// - **Matching dims**: returns `Ok(())` immediately (hot-path, no write).
/// - **Dimension transition** (old != new):
///   1. Updates `app_meta` to `new_dims`.
///   2. Emits a single `warn!` with old/new dims and a re-embed notice.
///   3. Marks ALL non-stale rows as `is_stale = 1` via
///      `UPDATE embedding_vectors SET is_stale = 1 WHERE is_stale = 0`.
///      This is intentionally broad: every row produced by the old dimension
///      model is incorrect for the new query space regardless of model_id.
///      The sweep loop in `src-tauri/src/scheduler/loops/system.rs` picks up
///      these rows via `get_stale_vectors` → `embed_batch` → `update_vector`
///      (clears `is_stale = 0`). This contract is the existing stale-model
///      re-embedding pipeline; we reuse it exactly.
///
/// # Why inline inside the write closure?
///
/// `get_meta`/`set_meta` on `SqliteStorage` use the same `GuardedConnection`
/// mutex that `with_conn` already holds. Calling them from outside that closure
/// would require a second `spawn_blocking` + mutex re-entry, which deadlocks on
/// the non-reentrant `parking_lot::Mutex`. This function is called from inside
/// `with_conn` so the mutex is already held by the current `spawn_blocking` task.
///
/// # In-process cache coherence
///
/// The caller (`store` / `store_quantized` in `trait_impl.rs`) maintains an
/// `AtomicU32` cache of the corpus dims and stores `new_dims` unconditionally
/// after this function returns `Ok(())` — including the transition path. That
/// is correct: once the transition has run (meta key updated, old rows marked
/// stale), `new_dims` IS the corpus dimension, so subsequent same-dim writes
/// skip the DB round-trip and a write with yet another dimension misses the
/// cache and re-enters this guard.
pub fn corpus_dims_guard(
    conn: &Connection,
    new_dims: usize,
) -> Result<(), crate::error::StorageError> {
    let key = super::CORPUS_DIMS_META_KEY;
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM app_meta WHERE key = ?1",
            rusqlite::params![key],
            |row| row.get(0),
        )
        .ok();

    match stored {
        None => {
            // First write: record dimension.
            conn.execute(
                "INSERT OR REPLACE INTO app_meta (key, value) VALUES (?1, ?2)",
                rusqlite::params![key, new_dims.to_string()],
            )
            .map_err(|e| {
                crate::error::StorageError::Internal(format!(
                    "corpus_dims_guard: failed to record initial dims: {e}"
                ))
            })?;
        }
        Some(ref s) => {
            let stored_dims: usize = s.parse().unwrap_or(0);
            if stored_dims != new_dims {
                // Dimension transition: update app_meta.
                conn.execute(
                    "INSERT OR REPLACE INTO app_meta (key, value) VALUES (?1, ?2)",
                    rusqlite::params![key, new_dims.to_string()],
                )
                .map_err(|e| {
                    crate::error::StorageError::Internal(format!(
                        "corpus_dims_guard: failed to update dims: {e}"
                    ))
                })?;

                warn!(
                    old_dims = stored_dims,
                    new_dims,
                    "Embedding corpus dimension changed — marking all active vectors stale for re-embedding"
                );

                // Mark ALL active (non-stale) rows stale so the scheduler sweep
                // (system.rs: get_stale_vectors → embed_batch → update_vector)
                // re-embeds them with the new-dimension model.
                //
                // Batched UPDATE to avoid a single long-running full-table write that
                // would hold the GuardedConnection mutex for its entire duration and
                // starve all other DB operations (#5987).
                //
                // Each iteration marks at most STALE_BATCH_SIZE rows. The loop exits
                // when an iteration reports rows_affected == 0 (no more active rows).
                //
                // Net effect is identical to the previous unbounded single UPDATE
                // (all is_stale=0 rows become is_stale=1). The mutex is still held for
                // the full duration of all iterations because this closure runs inside
                // the with_conn spawn_blocking block — interleaving requires a
                // structurally different call site. The chunk size keeps each individual
                // SQLite B-tree write-lock short, which reduces per-iteration latency
                // and allows SQLite WAL readers to make progress between statements.
                const STALE_BATCH_SIZE: i64 = 1000;
                let mut total_marked: usize = 0;
                loop {
                    let n = conn
                        .execute(
                            "UPDATE embedding_vectors SET is_stale = 1 \
                             WHERE rowid IN \
                               (SELECT rowid FROM embedding_vectors \
                                WHERE is_stale = 0 \
                                LIMIT ?1)",
                            rusqlite::params![STALE_BATCH_SIZE],
                        )
                        .map_err(|e| {
                            crate::error::StorageError::Internal(format!(
                                "corpus_dims_guard: bulk stale-mark batch failed: {e}"
                            ))
                        })?;
                    total_marked += n;
                    if n == 0 {
                        break;
                    }
                }

                warn!(
                    marked = total_marked,
                    "corpus_dims_guard: marked old-dim vectors stale (pending re-embed)"
                );
            }
        }
    }
    Ok(())
}
