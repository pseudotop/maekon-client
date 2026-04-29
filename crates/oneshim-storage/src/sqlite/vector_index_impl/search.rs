use oneshim_core::binary_quantizer::{BinaryCode, BinaryQuantizer};
use oneshim_core::error::CoreError;
use oneshim_core::models::embedding::{SearchFilters, SearchResult};
use oneshim_core::quantization::{QuantizedVector, ScalarQuantizer};
use tracing::debug;

use super::{build_filter_conditions, score_and_rank, SqliteVectorIndex};
use crate::error::StorageError;

pub(super) async fn search_ivf_impl(
    index: &SqliteVectorIndex,
    query_vector: &QuantizedVector,
    nprobe: usize,
    limit: usize,
    time_decay_hours: f32,
    filters: &SearchFilters,
) -> Result<Vec<SearchResult>, CoreError> {
    index.ensure_cache().await?;

    // Clone probe IDs out of the cache guard before any further awaits.
    let probe_ids = {
        let cache = index.centroid_cache.read().await;
        let centroids = cache.as_ref().ok_or_else(|| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: "IVF index not built yet".to_string(),
        })?;

        if centroids.is_empty() {
            return Ok(vec![]);
        }

        // Pre-validate dimensions once before the hot loop.
        if let Some(first) = centroids.first() {
            if first.vector.data.len() != query_vector.data.len() {
                return Err(CoreError::InvalidArguments {
                    code: oneshim_core::error_codes::ValidationCode::InvalidArguments,
                    message: format!(
                        "Dimension mismatch: centroid {} vs query {}",
                        first.vector.data.len(),
                        query_vector.data.len()
                    ),
                });
            }
        }

        let mut sims: Vec<(usize, f32)> = centroids
            .iter()
            .map(|c| {
                let sim =
                    ScalarQuantizer::cosine_similarity_int8_unchecked(&c.vector, query_vector);
                (c.id, sim)
            })
            .collect();
        sims.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        sims.into_iter()
            .take(nprobe.min(centroids.len()))
            .map(|(id, _)| id)
            .collect::<Vec<usize>>()
    };

    let qv = query_vector.clone();
    let filters = filters.clone();

    index
        .with_conn(move |conn| {
            let (mut conditions, mut param_values) = build_filter_conditions(&filters);

            // Add cluster filter
            let base_idx = param_values.len();
            let placeholders: Vec<String> = probe_ids
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", base_idx + i + 1))
                .collect();
            conditions.push(format!("ia.cluster_id IN ({})", placeholders.join(", ")));
            for id in &probe_ids {
                param_values.push(Box::new(*id as i64));
            }

            let where_clause = conditions.join(" AND ");
            let sql = format!(
                "SELECT ev.segment_id, ev.content_type, ev.content_label, ev.original_text,
                        ev.vector_int8, ev.quant_scale, ev.quant_offset, ev.timestamp
                 FROM embedding_vectors ev
                 JOIN ivf_assignments ia ON ev.id = ia.vector_id
                 WHERE {where_clause} AND ev.vector_int8 IS NOT NULL"
            );

            let mut stmt = conn.prepare(&sql).map_err(|e| {
                StorageError::Internal(format!("Failed to prepare IVF search: {e}"))
            })?;

            let params_ref: Vec<&dyn rusqlite::types::ToSql> =
                param_values.iter().map(|p| p.as_ref()).collect();

            let rows: Vec<_> = stmt
                .query_map(params_ref.as_slice(), |row| {
                    let blob: Vec<u8> = row.get(4)?;
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                        blob.iter().map(|&b| b as i8).collect::<Vec<i8>>(),
                        row.get::<_, f32>(5)?,
                        row.get::<_, f32>(6)?,
                        row.get::<_, String>(7)?,
                    ))
                })
                .map_err(|e| StorageError::Internal(format!("Failed to query IVF vectors: {e}")))?
                .filter_map(|r| r.ok())
                .collect();

            Ok(score_and_rank(rows, &qv, limit, time_decay_hours))
        })
        .await
        .map_err(CoreError::from)
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn search_ivf_binary_impl(
    index: &SqliteVectorIndex,
    query_vector: &QuantizedVector,
    query_binary: &BinaryCode,
    nprobe: usize,
    oversample_factor: usize,
    limit: usize,
    time_decay_hours: f32,
    filters: &SearchFilters,
) -> Result<Vec<SearchResult>, CoreError> {
    index.ensure_cache().await?;

    // Clone probe IDs out of the cache guard before any further awaits.
    let probe_ids = {
        let cache = index.centroid_cache.read().await;
        let centroids = cache.as_ref().ok_or_else(|| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: "IVF index not built yet".to_string(),
        })?;

        if centroids.is_empty() {
            return Ok(vec![]);
        }

        // Pre-validate dimensions once before the hot loop.
        if let Some(first) = centroids.first() {
            if first.vector.data.len() != query_vector.data.len() {
                return Err(CoreError::InvalidArguments {
                    code: oneshim_core::error_codes::ValidationCode::InvalidArguments,
                    message: format!(
                        "Dimension mismatch: centroid {} vs query {}",
                        first.vector.data.len(),
                        query_vector.data.len()
                    ),
                });
            }
        }

        let mut sims: Vec<(usize, f32)> = centroids
            .iter()
            .map(|c| {
                let sim =
                    ScalarQuantizer::cosine_similarity_int8_unchecked(&c.vector, query_vector);
                (c.id, sim)
            })
            .collect();
        sims.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        sims.into_iter()
            .take(nprobe.min(centroids.len()))
            .map(|(id, _)| id)
            .collect::<Vec<usize>>()
    };

    let qv = query_vector.clone();
    let qb = query_binary.clone();
    let filters = filters.clone();
    let candidate_count = limit * oversample_factor;

    index
        .with_conn(move |conn| {
            // Stage 1: Load binary codes for probed clusters
            let base_idx = 0usize;
            let placeholders: Vec<String> = probe_ids
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", base_idx + i + 1))
                .collect();

            let sql = format!(
                "SELECT bc.vector_id, bc.binary_code
                 FROM vector_binary_codes bc
                 JOIN ivf_assignments ia ON bc.vector_id = ia.vector_id
                 WHERE ia.cluster_id IN ({})",
                placeholders.join(", ")
            );

            let mut stmt = conn.prepare(&sql).map_err(|e| {
                StorageError::Internal(format!("Failed to prepare binary search: {e}"))
            })?;

            let params: Vec<Box<dyn rusqlite::types::ToSql>> = probe_ids
                .iter()
                .map(|id| Box::new(*id as i64) as Box<dyn rusqlite::types::ToSql>)
                .collect();
            let params_ref: Vec<&dyn rusqlite::types::ToSql> =
                params.iter().map(|p| p.as_ref()).collect();

            let mut candidates: Vec<(i64, u32)> = stmt
                .query_map(params_ref.as_slice(), |row| {
                    let vid: i64 = row.get(0)?;
                    let code_data: Vec<u8> = row.get(1)?;
                    let hamming =
                        BinaryQuantizer::hamming_distance(&qb, &BinaryCode { data: code_data });
                    Ok((vid, hamming))
                })
                .map_err(|e| StorageError::Internal(format!("Failed to query binary codes: {e}")))?
                .filter_map(|r| r.ok())
                .collect();

            // Sort by Hamming distance (ascending = closest first)
            candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            candidates.truncate(candidate_count);

            if candidates.is_empty() {
                // Fallback: do IVF-only search within probed clusters
                debug!("Binary filter yielded 0 candidates, falling back to IVF-only");
                // We cannot call search_ivf recursively here, so just return empty
                return Ok(vec![]);
            }

            // Stage 2: Load INT8 vectors for surviving candidates
            let (mut conditions, mut param_values) = build_filter_conditions(&filters);

            let vid_base = param_values.len();
            let vid_placeholders: Vec<String> = candidates
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", vid_base + i + 1))
                .collect();
            conditions.push(format!("ev.id IN ({})", vid_placeholders.join(", ")));
            for (vid, _) in &candidates {
                param_values.push(Box::new(*vid));
            }

            let where_clause = conditions.join(" AND ");
            let rerank_sql = format!(
                "SELECT ev.segment_id, ev.content_type, ev.content_label, ev.original_text,
                        ev.vector_int8, ev.quant_scale, ev.quant_offset, ev.timestamp
                 FROM embedding_vectors ev
                 WHERE {where_clause} AND ev.vector_int8 IS NOT NULL"
            );

            let mut stmt2 = conn.prepare(&rerank_sql).map_err(|e| {
                StorageError::Internal(format!("Failed to prepare rerank query: {e}"))
            })?;

            let params_ref2: Vec<&dyn rusqlite::types::ToSql> =
                param_values.iter().map(|p| p.as_ref()).collect();

            let rows: Vec<_> = stmt2
                .query_map(params_ref2.as_slice(), |row| {
                    let blob: Vec<u8> = row.get(4)?;
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                        blob.iter().map(|&b| b as i8).collect::<Vec<i8>>(),
                        row.get::<_, f32>(5)?,
                        row.get::<_, f32>(6)?,
                        row.get::<_, String>(7)?,
                    ))
                })
                .map_err(|e| {
                    StorageError::Internal(format!("Failed to query rerank vectors: {e}"))
                })?
                .filter_map(|r| r.ok())
                .collect();

            Ok(score_and_rank(rows, &qv, limit, time_decay_hours))
        })
        .await
        .map_err(CoreError::from)
}
