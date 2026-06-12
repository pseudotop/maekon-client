use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use maekon_core::error::CoreError;
use maekon_core::models::embedding::{EmbeddingMetadata, SearchFilters, SearchResult};
use maekon_core::ports::vector_index::VectorIndex;
use maekon_core::ports::vector_store::VectorStore;
use maekon_core::quantization::{QuantizedVector, ScalarQuantizer};
use rusqlite::{params, OptionalExtension};
use tracing::{debug, warn};

use crate::error::StorageError;

use super::helpers::{
    brute_force_search, brute_force_search_quantized, bytes_to_f32_vec, content_type_to_str,
    corpus_dims_guard, f32_vec_to_bytes, i8_vec_to_bytes, map_quantized_row, map_vector_row,
    parse_content_type,
};
use super::{SqliteVectorStore, DIMS_CACHE_UNSET};

// ── IVF routing helpers ───────────────────────────────────────────────────────

/// Merge two result sets and return the top-`limit` entries by score.
///
/// Called when IVF results must be supplemented by unindexed-delta brute-force
/// results. Both slices are already sorted descending; the merge is O(n log n).
fn merge_and_truncate(
    mut ivf: Vec<SearchResult>,
    mut delta: Vec<SearchResult>,
    limit: usize,
) -> Vec<SearchResult> {
    ivf.append(&mut delta);
    ivf.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    ivf.truncate(limit);
    ivf
}

/// Minimum nprobe when the cluster count is known.
///
/// For small corpora (n_clusters <= MIN_NPROBE) every cluster is probed, which
/// preserves the pre-#5811 behaviour.  For larger indexes the probe set is
/// `ceil(n_clusters * NPROBE_RATIO)` which gives 30 % coverage by default.
const MIN_NPROBE: usize = 10;

/// Fraction of clusters to probe when the corpus is larger than MIN_NPROBE.
const NPROBE_RATIO: f64 = 0.3;

/// Compute a cluster-proportional nprobe value.
///
/// Formula: `clamp(ceil(n_clusters * NPROBE_RATIO), MIN_NPROBE, n_clusters)`
///
/// - n_clusters == 0: returns 0 (no index built; routing gate will not fire).
/// - n_clusters <= MIN_NPROBE: returns n_clusters (probe every cluster).
/// - n_clusters > MIN_NPROBE: returns ceil(n_clusters * NPROBE_RATIO), floored
///   at MIN_NPROBE and capped at n_clusters.
///
/// The caller side (`search_ivf_impl`) already applies `.min(centroids.len())`
/// before the hot loop, so over-supplying is harmless.  This function is a pure
/// computation with no I/O; it can be called freely from async contexts.
pub(crate) fn compute_nprobe(n_clusters: usize) -> usize {
    if n_clusters == 0 {
        return 0;
    }
    let proportional = (n_clusters as f64 * NPROBE_RATIO).ceil() as usize;
    proportional.max(MIN_NPROBE).min(n_clusters)
}

#[async_trait]
impl VectorStore for SqliteVectorStore {
    async fn store(&self, vector: Vec<f32>, metadata: EmbeddingMetadata) -> Result<(), CoreError> {
        if vector.is_empty() {
            return Err(CoreError::InvalidArguments {
                code: maekon_core::error_codes::ValidationCode::InvalidArguments,
                message: "Cannot store empty f32 vector".to_string(),
            });
        }

        let new_dims = vector.len();
        let blob = f32_vec_to_bytes(&vector);
        let content_type_str = content_type_to_str(&metadata.content_type).to_string();
        let timestamp_str = metadata.timestamp.to_rfc3339();

        // Write-side corpus dimension guard (#5755 FLAG-DIM-768).
        // Hot path: if the in-process cache matches new_dims, skip the DB round-trip.
        // The cache is DIMS_CACHE_UNSET (0) until the first write establishes it.
        let dims_cache = self.corpus_dims_cache.clone();
        let clock = self.clock.clone();
        self.with_conn(move |conn| {
            let cached = dims_cache.load(Ordering::Relaxed);
            if cached == DIMS_CACHE_UNSET || cached as usize != new_dims {
                // Cache miss or transition: run the full guard (DB read + conditional write).
                corpus_dims_guard(conn, new_dims)?;
                // Update the in-process cache to the new dimension so subsequent
                // writes skip the DB round-trip on the hot path.
                dims_cache.store(new_dims as u32, Ordering::Relaxed);
            }

            // F0/#5186: stamp a monotonic HLC so this row propagates via cross-device sync.
            let hlc = clock
                .next(conn)
                .map_err(|e| StorageError::Internal(format!("hlc stamp (store): {e}")))?;
            conn.execute(
                "INSERT INTO embedding_vectors (segment_id, content_type, content_label, original_text, vector, model_id, timestamp, hlc_wall_ms, hlc_counter, origin_device_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    metadata.segment_id,
                    content_type_str,
                    metadata.content_label,
                    metadata.original_text,
                    blob,
                    metadata.model_id,
                    timestamp_str,
                    hlc.wall_ms,
                    hlc.counter,
                    hlc.device_id,
                ],
            )
            .map_err(|e| StorageError::Internal(format!("Failed to store embedding vector: {e}")))?;

            debug!(
                "Stored embedding vector for segment {} (type={})",
                metadata.segment_id, content_type_str
            );
            Ok(())
        })
        .await
        .map_err(Into::into)
    }

    async fn search(
        &self,
        query_vector: &[f32],
        limit: usize,
        time_decay_hours: f32,
    ) -> Result<Vec<SearchResult>, CoreError> {
        // ── IVF routing gate (#5642) ──────────────────────────────────────────
        // Attempt IVF-accelerated search when the index is built and dimensions
        // match the query.  Fall back to brute-force on any IVF error so that
        // the existing safety guarantee is preserved.
        //
        // 콜드-스타트 lazy-hydrate: 앱 재시작 시 corpus_dims_cache는 DIMS_CACHE_UNSET(0)으로
        // 초기화된다. write 경로(store/store_quantized)가 캐시를 채우기 전까지 IVF를 우회하는
        // 콜드-스타트 라우팅 미스를 막기 위해, 캐시가 UNSET이면 app_meta KV에서 1회 hydrate한다.
        let mut cached_dims = self.corpus_dims_cache.load(Ordering::Relaxed);
        if cached_dims == DIMS_CACHE_UNSET {
            self.try_hydrate_dims_cache().await;
            cached_dims = self.corpus_dims_cache.load(Ordering::Relaxed);
        }
        let query_dims = query_vector.len();
        let ivf_candidate = cached_dims != DIMS_CACHE_UNSET && cached_dims as usize == query_dims;

        if ivf_candidate {
            match self.ivf_index.get_index_meta().await {
                Ok(meta) if meta.ivf_built_at.is_some() => {
                    // Compute cluster-proportional nprobe (#5811).
                    // For small corpora (n_clusters <= MIN_NPROBE) every cluster is
                    // probed, preserving pre-#5811 behaviour.  For larger corpora
                    // 30 % of clusters are probed.  search_ivf_impl clamps to
                    // centroids.len() internally so over-supply is safe.
                    let nprobe = compute_nprobe(meta.n_clusters);
                    debug!(
                        nprobe,
                        n_clusters = meta.n_clusters,
                        "IVF routing: computed nprobe"
                    );

                    // Quantize the f32 query for the IVF path.
                    let quantized_query = match ScalarQuantizer::quantize(query_vector) {
                        Ok(q) => q,
                        Err(e) => {
                            warn!(
                                "IVF routing: quantize failed ({e}), falling back to brute-force"
                            );
                            return self
                                .brute_force_search_f32(query_vector, limit, time_decay_hours)
                                .await;
                        }
                    };

                    let default_filters = SearchFilters::default();
                    match self
                        .ivf_index
                        .search_ivf(
                            &quantized_query,
                            nprobe,
                            limit,
                            time_decay_hours,
                            &default_filters,
                        )
                        .await
                    {
                        Ok(ivf_results) => {
                            // Supplement with unindexed-delta vectors when present.
                            if meta.unindexed_count > 0 {
                                debug!(
                                    unindexed = meta.unindexed_count,
                                    "IVF search: supplementing with unindexed-delta brute-force"
                                );
                                let delta = self
                                    .brute_force_unindexed_f32(
                                        query_vector,
                                        limit,
                                        time_decay_hours,
                                    )
                                    .await
                                    .unwrap_or_else(|e| {
                                        warn!("IVF delta brute-force failed ({e}), using IVF-only results");
                                        vec![]
                                    });
                                return Ok(merge_and_truncate(ivf_results, delta, limit));
                            }
                            return Ok(ivf_results);
                        }
                        Err(e) => {
                            warn!("IVF search failed ({e}), falling back to brute-force");
                            // Fall through to brute-force below.
                        }
                    }
                }
                Ok(_) => {
                    // Index not yet built — use brute-force (normal early-lifecycle path).
                }
                Err(e) => {
                    warn!("get_index_meta failed ({e}), falling back to brute-force");
                }
            }
        }

        // ── Brute-force fallback (original behaviour) ─────────────────────────
        self.brute_force_search_f32(query_vector, limit, time_decay_hours)
            .await
    }

    async fn search_filtered(
        &self,
        query_vector: &[f32],
        limit: usize,
        time_decay_hours: f32,
        filters: &SearchFilters,
    ) -> Result<Vec<SearchResult>, CoreError> {
        // ── IVF routing gate (#5642) ──────────────────────────────────────────
        // Same gate as `search`: route through IVF when index is built and
        // dimensions match.  `search_ivf` already accepts `SearchFilters`, so
        // all filter predicates (time range, content_type, regime_id, excluded
        // segment IDs) are passed through unchanged.
        //
        // 콜드-스타트 lazy-hydrate: search()와 동일한 이유로 캐시가 UNSET이면 app_meta에서 1회 hydrate한다.
        let mut cached_dims = self.corpus_dims_cache.load(Ordering::Relaxed);
        if cached_dims == DIMS_CACHE_UNSET {
            self.try_hydrate_dims_cache().await;
            cached_dims = self.corpus_dims_cache.load(Ordering::Relaxed);
        }
        let query_dims = query_vector.len();
        let ivf_candidate = cached_dims != DIMS_CACHE_UNSET && cached_dims as usize == query_dims;

        if ivf_candidate {
            match self.ivf_index.get_index_meta().await {
                Ok(meta) if meta.ivf_built_at.is_some() => {
                    // Compute cluster-proportional nprobe (#5811).
                    let nprobe = compute_nprobe(meta.n_clusters);
                    debug!(
                        nprobe,
                        n_clusters = meta.n_clusters,
                        "IVF routing (filtered): computed nprobe"
                    );

                    let quantized_query = match ScalarQuantizer::quantize(query_vector) {
                        Ok(q) => q,
                        Err(e) => {
                            warn!("IVF routing (filtered): quantize failed ({e}), falling back to brute-force");
                            return self
                                .brute_force_search_filtered_f32(
                                    query_vector,
                                    limit,
                                    time_decay_hours,
                                    filters,
                                )
                                .await;
                        }
                    };

                    match self
                        .ivf_index
                        .search_ivf(&quantized_query, nprobe, limit, time_decay_hours, filters)
                        .await
                    {
                        Ok(ivf_results) => {
                            // Supplement with unindexed-delta vectors.
                            if meta.unindexed_count > 0 {
                                debug!(
                                    unindexed = meta.unindexed_count,
                                    "IVF filtered search: supplementing with unindexed-delta brute-force"
                                );
                                let delta = self
                                    .brute_force_unindexed_filtered_f32(
                                        query_vector,
                                        limit,
                                        time_decay_hours,
                                        filters,
                                    )
                                    .await
                                    .unwrap_or_else(|e| {
                                        warn!("IVF delta filtered brute-force failed ({e}), using IVF-only results");
                                        vec![]
                                    });
                                return Ok(merge_and_truncate(ivf_results, delta, limit));
                            }
                            return Ok(ivf_results);
                        }
                        Err(e) => {
                            warn!("IVF filtered search failed ({e}), falling back to brute-force");
                        }
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    warn!("get_index_meta failed (filtered) ({e}), falling back to brute-force");
                }
            }
        }

        // ── Brute-force fallback (original behaviour) ─────────────────────────
        self.brute_force_search_filtered_f32(query_vector, limit, time_decay_hours, filters)
            .await
    }

    async fn enforce_retention(&self, max_days: u32) -> Result<u64, CoreError> {
        self.with_conn(move |conn| {
            let cutoff = (Utc::now() - Duration::days(max_days as i64)).to_rfc3339();
            let deleted = conn
                .execute(
                    "DELETE FROM embedding_vectors WHERE timestamp < ?1",
                    params![cutoff],
                )
                .map_err(|e| {
                    StorageError::Internal(format!("Failed to enforce vector retention: {e}"))
                })?;
            debug!("Enforced vector retention: deleted {deleted} rows older than {max_days} days");
            Ok(deleted as u64)
        })
        .await
        .map_err(Into::into)
    }

    async fn mark_stale(&self, old_model_id: &str) -> Result<u64, CoreError> {
        let model_id = old_model_id.to_string();
        let clock = self.clock.clone();
        self.with_conn(move |conn| {
            // F0/#5186: `is_stale` is a synced column, so the staleness must propagate.
            // A bulk UPDATE can't bind a distinct HLC per row — one shared `next()` for
            // the batch is correct for LWW. **Preserve `origin_device_id`** (only bump the
            // HLC wall/counter): origin identifies the device whose data this is = the
            // erasure scope (`DeletionEvent` deletes `WHERE origin_device_id`). This UPDATE
            // can hit peer-synced embeddings (WHERE model_id), and embeddings are LWW so a
            // re-attribution to local would propagate and remove them from the originating
            // peer's erasure. (Mirrors the PR4 activity_segments origin-preservation fix.)
            let hlc = clock
                .next(conn)
                .map_err(|e| StorageError::Internal(format!("hlc stamp (mark_stale): {e}")))?;
            let updated = conn
                .execute(
                    "UPDATE embedding_vectors SET is_stale = 1, \
                     hlc_wall_ms = ?2, hlc_counter = ?3 \
                     WHERE model_id = ?1",
                    params![model_id, hlc.wall_ms, hlc.counter],
                )
                .map_err(|e| {
                    StorageError::Internal(format!("Failed to mark vectors stale: {e}"))
                })?;
            debug!("Marked {updated} vectors as stale for model_id={model_id}");
            Ok(updated as u64)
        })
        .await
        .map_err(Into::into)
    }

    async fn get_current_model_id(&self) -> Result<Option<String>, CoreError> {
        self.with_conn_read(move |conn| {
            let result: Option<String> = conn
                .query_row(
                    "SELECT model_id FROM embedding_vectors WHERE is_stale = 0 ORDER BY id DESC LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .ok();
            Ok(result)
        })
        .await
        .map_err(Into::into)
    }

    async fn get_stale_vectors(&self, limit: usize) -> Result<Vec<(i64, String)>, CoreError> {
        self.with_conn_read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, original_text FROM embedding_vectors WHERE is_stale = 1 LIMIT ?1",
                )
                .map_err(|e| {
                    StorageError::Internal(format!("Failed to prepare stale query: {e}"))
                })?;
            let rows: Vec<(i64, String)> = stmt
                .query_map(params![limit as i64], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(|e| StorageError::Internal(format!("Failed to query stale vectors: {e}")))?
                .filter_map(|r| match r {
                    Ok(row) => Some(row),
                    Err(e) => {
                        warn!("Skipping corrupted row in get_stale_vectors: {e}");
                        None
                    }
                })
                .collect();
            Ok(rows)
        })
        .await
        .map_err(Into::into)
    }

    async fn update_vector(
        &self,
        id: i64,
        vector: Vec<f32>,
        model_id: &str,
    ) -> Result<(), CoreError> {
        let blob = f32_vec_to_bytes(&vector);
        let model_id = model_id.to_string();
        let clock = self.clock.clone();
        self.with_conn(move |conn| {
            // F0/#5186: vector/model_id/is_stale are synced columns → bump HLC to propagate.
            // **Preserve `origin_device_id`** (only wall/counter bump) — origin = erasure
            // scope; re-embedding a peer's vector must not re-attribute it to local (mirrors
            // the PR4 activity_segments fix; embeddings are LWW so it would propagate).
            let hlc = clock
                .next(conn)
                .map_err(|e| StorageError::Internal(format!("hlc stamp (update_vector): {e}")))?;
            conn.execute(
                "UPDATE embedding_vectors SET vector = ?1, model_id = ?2, is_stale = 0, \
                 hlc_wall_ms = ?4, hlc_counter = ?5 WHERE id = ?3",
                params![blob, model_id, id, hlc.wall_ms, hlc.counter],
            )
            .map_err(|e| StorageError::Internal(format!("Failed to update vector: {e}")))?;
            Ok(())
        })
        .await
        .map_err(Into::into)
    }

    async fn store_quantized(
        &self,
        vector_f32: Vec<f32>,
        vector_int8: &QuantizedVector,
        metadata: EmbeddingMetadata,
        skip_float32: bool,
    ) -> Result<(), CoreError> {
        // Validate INT8 vector is non-empty before persisting.
        if vector_int8.data.is_empty() {
            return Err(CoreError::InvalidArguments {
                code: maekon_core::error_codes::ValidationCode::InvalidArguments,
                message: "Cannot store empty INT8 vector".to_string(),
            });
        }

        // Validate f32/INT8 dimension consistency when f32 is being stored.
        if !skip_float32 && vector_f32.len() != vector_int8.data.len() {
            return Err(CoreError::InvalidArguments {
                code: maekon_core::error_codes::ValidationCode::InvalidArguments,
                message: format!(
                    "Vector dimension mismatch: f32 has {}, INT8 has {}",
                    vector_f32.len(),
                    vector_int8.data.len()
                ),
            });
        }

        // The canonical dimension for a quantized row is the INT8 length.
        // For skip_float32 rows, vector_f32 is empty so INT8 is the only signal.
        let new_dims = vector_int8.data.len();

        // When skip_float32 is true, store an empty BLOB instead of the f32 data.
        // The column has a NOT NULL constraint (pre-existing schema), so we use
        // an empty vec rather than NULL. An empty BLOB is distinguishable from
        // a real vector (which always has len >= 4).
        let f32_blob: Vec<u8> = if skip_float32 {
            Vec::new()
        } else {
            f32_vec_to_bytes(&vector_f32)
        };
        let int8_blob = i8_vec_to_bytes(&vector_int8.data);
        let scale = vector_int8.scale;
        let offset = vector_int8.offset;
        let content_type_str = content_type_to_str(&metadata.content_type).to_string();
        let timestamp_str = metadata.timestamp.to_rfc3339();

        // Write-side corpus dimension guard (#5755 FLAG-DIM-768).
        let dims_cache = self.corpus_dims_cache.clone();
        let clock = self.clock.clone();
        self.with_conn(move |conn| {
            let cached = dims_cache.load(Ordering::Relaxed);
            if cached == DIMS_CACHE_UNSET || cached as usize != new_dims {
                corpus_dims_guard(conn, new_dims)?;
                dims_cache.store(new_dims as u32, Ordering::Relaxed);
            }

            // F0/#5186: stamp a monotonic HLC so this new row propagates via sync.
            let hlc = clock
                .next(conn)
                .map_err(|e| StorageError::Internal(format!("hlc stamp (store_quantized): {e}")))?;
            conn.execute(
                "INSERT INTO embedding_vectors (segment_id, content_type, content_label, original_text, vector, model_id, timestamp, vector_int8, quant_scale, quant_offset, hlc_wall_ms, hlc_counter, origin_device_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    metadata.segment_id,
                    content_type_str,
                    metadata.content_label,
                    metadata.original_text,
                    f32_blob,
                    metadata.model_id,
                    timestamp_str,
                    int8_blob,
                    scale,
                    offset,
                    hlc.wall_ms,
                    hlc.counter,
                    hlc.device_id,
                ],
            )
            .map_err(|e| StorageError::Internal(format!("Failed to store quantized vector: {e}")))?;

            debug!(
                "Stored quantized vector for segment {} (type={}, skip_f32={})",
                metadata.segment_id, content_type_str, skip_float32
            );
            Ok(())
        })
        .await
        .map_err(Into::into)
    }

    async fn search_quantized(
        &self,
        query_vector: &QuantizedVector,
        limit: usize,
        time_decay_hours: f32,
        filters: &SearchFilters,
    ) -> Result<Vec<SearchResult>, CoreError> {
        let qv = query_vector.clone();
        let filters = filters.clone();

        self.with_conn_read(move |conn| {
            let mut conditions = vec![
                "is_stale = 0".to_string(),
                "vector_int8 IS NOT NULL".to_string(),
            ];
            let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

            if let Some(ref after) = filters.after {
                conditions.push(format!("timestamp >= ?{}", param_values.len() + 1));
                param_values.push(Box::new(after.to_rfc3339()));
            }
            if let Some(ref before) = filters.before {
                conditions.push(format!("timestamp <= ?{}", param_values.len() + 1));
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
                    conditions.push(format!("content_type IN ({})", placeholders.join(", ")));
                    for ct in content_types {
                        param_values.push(Box::new(content_type_to_str(ct).to_string()));
                    }
                }
            }
            // regime_id filter: subquery over the existing
            // `activity_segments.regime_id` + `idx_segments_regime` index.
            // See spec §3.2 / C3a.
            if let Some(ref regime_id) = filters.regime_id {
                let idx = param_values.len() + 1;
                conditions.push(format!(
                    "segment_id IN (SELECT id FROM activity_segments WHERE regime_id = ?{idx})"
                ));
                param_values.push(Box::new(regime_id.clone()));
            }

            // Negative feedback: exclude dismissed segment IDs
            if !filters.excluded_segment_ids.is_empty() {
                let base_idx = param_values.len();
                let placeholders: Vec<String> = filters
                    .excluded_segment_ids
                    .iter()
                    .enumerate()
                    .map(|(i, _)| format!("?{}", base_idx + i + 1))
                    .collect();
                conditions.push(format!(
                    "segment_id NOT IN ({})",
                    placeholders.join(", ")
                ));
                for seg_id in &filters.excluded_segment_ids {
                    param_values.push(Box::new(seg_id.clone()));
                }
            }

            let where_clause = conditions.join(" AND ");
            let sql = format!(
                "SELECT segment_id, content_type, content_label, original_text, vector_int8, quant_scale, quant_offset, timestamp
                 FROM embedding_vectors
                 WHERE {where_clause}"
            );

            let mut stmt = conn.prepare(&sql).map_err(|e| {
                StorageError::Internal(format!("Failed to prepare quantized search: {e}"))
            })?;

            let params_ref: Vec<&dyn rusqlite::types::ToSql> =
                param_values.iter().map(|p| p.as_ref()).collect();

            // Fetch all qualifying rows and compute cosine similarities while
            // holding the SQLite mutex (the entire closure runs inside `with_conn`).
            // This is consistent with the non-quantized `search`/`search_filtered`
            // methods. The stmt is collected into a Vec first so the query cursor
            // is released before the brute-force scan.
            let rows: Vec<super::helpers::QuantizedVectorRow> = stmt
                .query_map(params_ref.as_slice(), map_quantized_row)
                .map_err(|e| StorageError::Internal(format!("Failed to query quantized vectors: {e}")))?
                .filter_map(|r| match r {
                    Ok(row) => Some(row),
                    Err(e) => { warn!("Skipping corrupted row in search_quantized: {e}"); None }
                })
                .collect();
            Ok(brute_force_search_quantized(rows, &qv, limit, time_decay_hours))
        })
        .await
        .map_err(Into::into)
    }

    async fn backfill_quantized(&self, batch_size: usize) -> Result<u64, CoreError> {
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, vector FROM embedding_vectors WHERE vector_int8 IS NULL LIMIT ?1",
                )
                .map_err(|e| {
                    StorageError::Internal(format!("Failed to prepare backfill query: {e}"))
                })?;

            let rows: Vec<(i64, Vec<u8>)> = stmt
                .query_map(params![batch_size as i64], |row| {
                    Ok((row.get(0)?, row.get::<_, Vec<u8>>(1)?))
                })
                .map_err(|e| {
                    StorageError::Internal(format!("Failed to query backfill rows: {e}"))
                })?
                .filter_map(|r| match r {
                    Ok(row) => Some(row),
                    Err(e) => { warn!("Skipping corrupted row in backfill_quantized: {e}"); None }
                })
                .collect();

            if rows.is_empty() {
                return Ok(0);
            }

            // Wrap batch updates in a transaction for performance.
            conn.execute_batch("BEGIN")
                .map_err(|e| StorageError::Internal(format!("Backfill BEGIN failed: {e}")))?;

            // F0/#5186: deliberately does NOT bump the HLC. This only re-encodes existing
            // rows into the INT8 columns (vector_int8/quant_scale/quant_offset), which the
            // sync extractor does NOT replicate (only vector/model_id/is_stale/... are
            // synced). Stamping here would re-propagate an unchanged synced payload for a
            // local-only storage optimization — pure waste. The row already carries the HLC
            // from its store()/store_quantized() insert.
            let mut update_stmt = conn
                .prepare(
                    "UPDATE embedding_vectors SET vector_int8 = ?1, quant_scale = ?2, quant_offset = ?3 WHERE id = ?4",
                )
                .map_err(|e| StorageError::Internal(format!("Failed to prepare backfill update: {e}")))?;

            let mut count: u64 = 0;
            for (id, blob) in &rows {
                let f32_vec = bytes_to_f32_vec(blob);
                let quantized =
                    maekon_core::quantization::ScalarQuantizer::quantize(&f32_vec).map_err(
                        |e| StorageError::Internal(format!("Backfill quantize failed for id={id}: {e}")),
                    )?;
                let int8_blob = i8_vec_to_bytes(&quantized.data);

                update_stmt
                    .execute(params![int8_blob, quantized.scale, quantized.offset, id])
                    .map_err(|e| {
                        StorageError::Internal(format!("Backfill update failed for id={id}: {e}"))
                    })?;

                count += 1;
            }

            conn.execute_batch("COMMIT")
                .map_err(|e| StorageError::Internal(format!("Backfill COMMIT failed: {e}")))?;

            debug!("Backfilled {count} vectors to INT8 quantized format");
            Ok(count)
        })
        .await
        .map_err(Into::into)
    }

    async fn count_unquantized(&self) -> Result<u64, CoreError> {
        self.with_conn_read(move |conn| {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM embedding_vectors WHERE vector_int8 IS NULL",
                    [],
                    |row| row.get(0),
                )
                .map_err(|e| {
                    StorageError::Internal(format!("Failed to count unquantized vectors: {e}"))
                })?;
            Ok(count as u64)
        })
        .await
        .map_err(Into::into)
    }

    async fn count_active_vectors(&self) -> Result<u64, CoreError> {
        self.with_conn_read(move |conn| {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM embedding_vectors WHERE is_stale = 0",
                    [],
                    |row| row.get(0),
                )
                .map_err(|e| {
                    StorageError::Internal(format!("Failed to count active vectors: {e}"))
                })?;
            Ok(count as u64)
        })
        .await
        .map_err(Into::into)
    }

    async fn get_metadata_by_ids(
        &self,
        ids: &[u64],
    ) -> Result<HashMap<u64, EmbeddingMetadata>, CoreError> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let ids = ids.to_vec();
        self.with_conn_read(move |conn| {
            // Build parameterized IN clause: WHERE id IN (?1, ?2, ...)
            let placeholders: Vec<String> =
                (1..=ids.len()).map(|i| format!("?{i}")).collect();
            let sql = format!(
                "SELECT id, segment_id, content_type, content_label, timestamp, original_text, model_id \
                 FROM embedding_vectors WHERE id IN ({})",
                placeholders.join(", ")
            );

            let mut stmt = conn.prepare(&sql).map_err(|e| {
                StorageError::Internal(format!("Failed to prepare metadata batch query: {e}"))
            })?;

            let params_boxed: Vec<Box<dyn rusqlite::types::ToSql>> =
                ids.iter().map(|id| Box::new(*id as i64) as Box<dyn rusqlite::types::ToSql>).collect();
            let params_ref: Vec<&dyn rusqlite::types::ToSql> =
                params_boxed.iter().map(|p| p.as_ref()).collect();

            let mut result = HashMap::new();
            let rows = stmt
                .query_map(params_ref.as_slice(), |row| {
                    let id: i64 = row.get(0)?;
                    let segment_id: String = row.get(1)?;
                    let content_type_str: String = row.get(2)?;
                    let content_label: Option<String> = row.get(3)?;
                    let ts_str: String = row.get(4)?;
                    let original_text: String = row.get(5)?;
                    let model_id: String = row.get(6)?;
                    Ok((id, segment_id, content_type_str, content_label, ts_str, original_text, model_id))
                })
                .map_err(|e| {
                    StorageError::Internal(format!("Failed to query metadata by ids: {e}"))
                })?;

            for row_result in rows {
                let (id, segment_id, content_type_str, content_label, ts_str, original_text, model_id) =
                    row_result.map_err(|e| {
                        StorageError::Internal(format!("Failed to read metadata row: {e}"))
                    })?;
                let timestamp = DateTime::parse_from_rfc3339(&ts_str)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now());
                let content_type = parse_content_type(&content_type_str);
                result.insert(
                    id as u64,
                    EmbeddingMetadata {
                        segment_id,
                        content_type,
                        content_label,
                        timestamp,
                        original_text,
                        model_id,
                    },
                );
            }
            Ok(result)
        })
        .await
        .map_err(Into::into)
    }

    async fn get_all_vectors_for_rebuild(&self) -> Result<Vec<(u64, Vec<f32>)>, CoreError> {
        self.with_conn_read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, vector FROM embedding_vectors \
                     WHERE is_stale = 0 AND LENGTH(vector) > 0",
                )
                .map_err(|e| {
                    StorageError::Internal(format!(
                        "Failed to prepare get_all_vectors_for_rebuild: {e}"
                    ))
                })?;

            let rows: Vec<(u64, Vec<f32>)> = stmt
                .query_map([], |row| {
                    let id: i64 = row.get(0)?;
                    let blob: Vec<u8> = row.get(1)?;
                    Ok((id as u64, bytes_to_f32_vec(&blob)))
                })
                .map_err(|e| {
                    StorageError::Internal(format!("Failed to query vectors for rebuild: {e}"))
                })?
                .filter_map(|r| match r {
                    Ok(row) => Some(row),
                    Err(e) => {
                        warn!("Skipping corrupted row in get_all_vectors_for_rebuild: {e}");
                        None
                    }
                })
                .collect();

            debug!("Fetched {} vectors for HNSW rebuild", rows.len());
            Ok(rows)
        })
        .await
        .map_err(Into::into)
    }

    async fn get_expired_ids(&self, max_days: u32) -> Result<Vec<u64>, CoreError> {
        self.with_conn_read(move |conn| {
            let cutoff = (Utc::now() - Duration::days(max_days as i64)).to_rfc3339();
            let mut stmt = conn
                .prepare("SELECT id FROM embedding_vectors WHERE timestamp < ?1")
                .map_err(|e| {
                    StorageError::Internal(format!("Failed to prepare get_expired_ids: {e}"))
                })?;
            let ids: Vec<u64> = stmt
                .query_map(params![cutoff], |row| {
                    let id: i64 = row.get(0)?;
                    Ok(id as u64)
                })
                .map_err(|e| {
                    StorageError::Internal(format!("Failed to query expired vector ids: {e}"))
                })?
                .filter_map(|r| match r {
                    Ok(row) => Some(row),
                    Err(e) => {
                        warn!("Skipping corrupted row in get_expired_ids: {e}");
                        None
                    }
                })
                .collect();
            Ok(ids)
        })
        .await
        .map_err(Into::into)
    }

    async fn last_insert_id(&self) -> Result<u64, CoreError> {
        self.with_conn_read(move |conn| {
            let id: i64 = conn
                .query_row("SELECT last_insert_rowid()", [], |row| row.get(0))
                .map_err(|e| {
                    StorageError::Internal(format!("Failed to get last_insert_rowid: {e}"))
                })?;
            Ok(id as u64)
        })
        .await
        .map_err(Into::into)
    }
}

// ── IVF routing private helpers ───────────────────────────────────────────────

impl SqliteVectorStore {
    /// Full brute-force search on f32 vectors (original `search` logic).
    ///
    /// Used as the fallback path when IVF is not available and as the
    /// unindexed-delta scan for the IVF-routed path.
    async fn brute_force_search_f32(
        &self,
        query_vector: &[f32],
        limit: usize,
        time_decay_hours: f32,
    ) -> Result<Vec<SearchResult>, CoreError> {
        let qv = query_vector.to_vec();
        self.with_conn_read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT segment_id, content_type, content_label, original_text, vector, timestamp
                     FROM embedding_vectors
                     WHERE is_stale = 0",
                )
                .map_err(|e| {
                    StorageError::Internal(format!("Failed to prepare search query: {e}"))
                })?;

            let rows: Vec<super::helpers::VectorRow> = stmt
                .query_map([], map_vector_row)
                .map_err(|e| {
                    StorageError::Internal(format!("Failed to query vectors: {e}"))
                })?
                .filter_map(|r| match r {
                    Ok(row) => Some(row),
                    Err(e) => {
                        warn!("Skipping corrupted row in brute_force_search_f32: {e}");
                        None
                    }
                })
                .collect();

            Ok(brute_force_search(rows, &qv, limit, time_decay_hours))
        })
        .await
        .map_err(Into::into)
    }

    /// Brute-force search restricted to vectors that have **no** IVF cluster
    /// assignment (i.e. inserted after the last `build_ivf_index` run).
    ///
    /// This is the unindexed-delta supplement used by the IVF routing path.
    /// The `NOT IN (SELECT vector_id FROM ivf_assignments)` sub-select is cheap
    /// because (a) the delta is expected to be small in steady state and
    /// (b) the scheduler rebuilds the index periodically so the delta window
    /// is bounded by the scheduler interval.
    async fn brute_force_unindexed_f32(
        &self,
        query_vector: &[f32],
        limit: usize,
        time_decay_hours: f32,
    ) -> Result<Vec<SearchResult>, CoreError> {
        let qv = query_vector.to_vec();
        self.with_conn_read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT segment_id, content_type, content_label, original_text, vector, timestamp
                     FROM embedding_vectors
                     WHERE is_stale = 0
                       AND id NOT IN (SELECT vector_id FROM ivf_assignments)",
                )
                .map_err(|e| {
                    StorageError::Internal(format!(
                        "Failed to prepare unindexed-delta search query: {e}"
                    ))
                })?;

            let rows: Vec<super::helpers::VectorRow> = stmt
                .query_map([], map_vector_row)
                .map_err(|e| {
                    StorageError::Internal(format!("Failed to query unindexed-delta vectors: {e}"))
                })?
                .filter_map(|r| match r {
                    Ok(row) => Some(row),
                    Err(e) => {
                        warn!("Skipping corrupted row in brute_force_unindexed_f32: {e}");
                        None
                    }
                })
                .collect();

            Ok(brute_force_search(rows, &qv, limit, time_decay_hours))
        })
        .await
        .map_err(Into::into)
    }

    /// Full brute-force filtered search (original `search_filtered` logic).
    async fn brute_force_search_filtered_f32(
        &self,
        query_vector: &[f32],
        limit: usize,
        time_decay_hours: f32,
        filters: &SearchFilters,
    ) -> Result<Vec<SearchResult>, CoreError> {
        let qv = query_vector.to_vec();
        let filters = filters.clone();

        self.with_conn_read(move |conn| {
            // Build dynamic WHERE clause (mirrors the original search_filtered logic).
            let mut conditions = vec!["is_stale = 0".to_string()];
            let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

            if let Some(ref after) = filters.after {
                conditions.push(format!("timestamp >= ?{}", param_values.len() + 1));
                param_values.push(Box::new(after.to_rfc3339()));
            }
            if let Some(ref before) = filters.before {
                conditions.push(format!("timestamp <= ?{}", param_values.len() + 1));
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
                    conditions.push(format!("content_type IN ({})", placeholders.join(", ")));
                    for ct in content_types {
                        param_values.push(Box::new(content_type_to_str(ct).to_string()));
                    }
                }
            }
            if let Some(ref regime_id) = filters.regime_id {
                let idx = param_values.len() + 1;
                conditions.push(format!(
                    "segment_id IN (SELECT id FROM activity_segments WHERE regime_id = ?{idx})"
                ));
                param_values.push(Box::new(regime_id.clone()));
            }
            if !filters.excluded_segment_ids.is_empty() {
                let base_idx = param_values.len();
                let placeholders: Vec<String> = filters
                    .excluded_segment_ids
                    .iter()
                    .enumerate()
                    .map(|(i, _)| format!("?{}", base_idx + i + 1))
                    .collect();
                conditions.push(format!("segment_id NOT IN ({})", placeholders.join(", ")));
                for seg_id in &filters.excluded_segment_ids {
                    param_values.push(Box::new(seg_id.clone()));
                }
            }

            let where_clause = conditions.join(" AND ");
            let sql = format!(
                "SELECT segment_id, content_type, content_label, original_text, vector, timestamp
                 FROM embedding_vectors
                 WHERE {where_clause}"
            );

            let mut stmt = conn.prepare(&sql).map_err(|e| {
                StorageError::Internal(format!("Failed to prepare filtered query: {e}"))
            })?;

            let params_ref: Vec<&dyn rusqlite::types::ToSql> =
                param_values.iter().map(|p| p.as_ref()).collect();

            let rows: Vec<super::helpers::VectorRow> = stmt
                .query_map(params_ref.as_slice(), map_vector_row)
                .map_err(|e| {
                    StorageError::Internal(format!("Failed to query filtered vectors: {e}"))
                })?
                .filter_map(|r| match r {
                    Ok(row) => Some(row),
                    Err(e) => {
                        warn!("Skipping corrupted row in brute_force_search_filtered_f32: {e}");
                        None
                    }
                })
                .collect();

            Ok(brute_force_search(rows, &qv, limit, time_decay_hours))
        })
        .await
        .map_err(Into::into)
    }

    /// Unindexed-delta brute-force filtered search supplement for the IVF path.
    async fn brute_force_unindexed_filtered_f32(
        &self,
        query_vector: &[f32],
        limit: usize,
        time_decay_hours: f32,
        filters: &SearchFilters,
    ) -> Result<Vec<SearchResult>, CoreError> {
        let qv = query_vector.to_vec();
        let filters = filters.clone();

        self.with_conn_read(move |conn| {
            let mut conditions = vec![
                "is_stale = 0".to_string(),
                "id NOT IN (SELECT vector_id FROM ivf_assignments)".to_string(),
            ];
            let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

            if let Some(ref after) = filters.after {
                conditions.push(format!("timestamp >= ?{}", param_values.len() + 1));
                param_values.push(Box::new(after.to_rfc3339()));
            }
            if let Some(ref before) = filters.before {
                conditions.push(format!("timestamp <= ?{}", param_values.len() + 1));
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
                    conditions.push(format!("content_type IN ({})", placeholders.join(", ")));
                    for ct in content_types {
                        param_values.push(Box::new(content_type_to_str(ct).to_string()));
                    }
                }
            }
            if let Some(ref regime_id) = filters.regime_id {
                let idx = param_values.len() + 1;
                conditions.push(format!(
                    "segment_id IN (SELECT id FROM activity_segments WHERE regime_id = ?{idx})"
                ));
                param_values.push(Box::new(regime_id.clone()));
            }
            if !filters.excluded_segment_ids.is_empty() {
                let base_idx = param_values.len();
                let placeholders: Vec<String> = filters
                    .excluded_segment_ids
                    .iter()
                    .enumerate()
                    .map(|(i, _)| format!("?{}", base_idx + i + 1))
                    .collect();
                conditions.push(format!("segment_id NOT IN ({})", placeholders.join(", ")));
                for seg_id in &filters.excluded_segment_ids {
                    param_values.push(Box::new(seg_id.clone()));
                }
            }

            let where_clause = conditions.join(" AND ");
            let sql = format!(
                "SELECT segment_id, content_type, content_label, original_text, vector, timestamp
                 FROM embedding_vectors
                 WHERE {where_clause}"
            );

            let mut stmt = conn.prepare(&sql).map_err(|e| {
                StorageError::Internal(format!(
                    "Failed to prepare unindexed-delta filtered query: {e}"
                ))
            })?;

            let params_ref: Vec<&dyn rusqlite::types::ToSql> =
                param_values.iter().map(|p| p.as_ref()).collect();

            let rows: Vec<super::helpers::VectorRow> = stmt
                .query_map(params_ref.as_slice(), map_vector_row)
                .map_err(|e| {
                    StorageError::Internal(format!(
                        "Failed to query unindexed-delta filtered vectors: {e}"
                    ))
                })?
                .filter_map(|r| match r {
                    Ok(row) => Some(row),
                    Err(e) => {
                        warn!("Skipping corrupted row in brute_force_unindexed_filtered_f32: {e}");
                        None
                    }
                })
                .collect();

            Ok(brute_force_search(rows, &qv, limit, time_decay_hours))
        })
        .await
        .map_err(Into::into)
    }

    /// 콜드-스타트 라우팅 미스 수정 (#5642).
    ///
    /// `corpus_dims_cache`가 `DIMS_CACHE_UNSET`(0)인 상태에서 첫 검색이 들어오면
    /// `app_meta` 테이블의 `CORPUS_DIMS_META_KEY` KV에서 영속화된 차원 값을 읽어
    /// 캐시를 채운다.  성공하면 다음 gate 평가에서 IVF 경로를 정상 사용할 수 있다.
    ///
    /// # 안전성
    ///
    /// - `with_conn_read` 는 독립적인 `spawn_blocking` 호출이므로 다른 클로저 내부에
    ///   중첩되지 않는다. (비재진입 mutex 데드락 방지)
    /// - KV 없거나 파싱 실패 시 캐시를 `DIMS_CACHE_UNSET`으로 유지 → brute-force 경로.
    /// - 이 메서드는 에러를 반환하지 않는다; 호출자는 결과를 무시한다.
    async fn try_hydrate_dims_cache(&self) {
        // `with_conn_read` 클로저 바깥에서 캐시 참조를 복제해 소유권을 전달한다.
        let cache_ref = Arc::clone(&self.corpus_dims_cache);
        let result = self
            .with_conn_read(move |conn| {
                let mut stmt = match conn.prepare("SELECT value FROM app_meta WHERE key = ?1") {
                    Ok(s) => s,
                    Err(e) => {
                        // prepare 실패 시 None 반환 → 캐시 유지
                        warn!("try_hydrate_dims_cache: prepare failed ({e})");
                        return Ok(None::<u32>);
                    }
                };

                let row: Option<String> = stmt
                    .query_row([super::CORPUS_DIMS_META_KEY], |r| r.get(0))
                    .optional()
                    .map_err(|e| {
                        crate::error::StorageError::Internal(format!(
                            "try_hydrate_dims_cache query error: {e}"
                        ))
                    })?;

                match row {
                    None => Ok(None),
                    Some(val) => match val.parse::<u32>() {
                        Ok(dims) if dims > 0 => Ok(Some(dims)),
                        Ok(_) => {
                            warn!("try_hydrate_dims_cache: persisted dims == 0 (ignored)");
                            Ok(None)
                        }
                        Err(e) => {
                            warn!("try_hydrate_dims_cache: parse failed for value '{val}' ({e})");
                            Ok(None)
                        }
                    },
                }
            })
            .await;

        match result {
            Ok(Some(dims)) => {
                cache_ref.store(dims, Ordering::Relaxed);
                debug!(
                    dims,
                    "IVF gate: lazy-hydrated corpus_dims_cache from app_meta"
                );
            }
            Ok(None) => {
                // KV 없거나 유효하지 않음 — 캐시를 DIMS_CACHE_UNSET으로 유지
            }
            Err(e) => {
                warn!(
                    "try_hydrate_dims_cache: with_conn_read error ({e}), keeping DIMS_CACHE_UNSET"
                );
            }
        }
    }
}
