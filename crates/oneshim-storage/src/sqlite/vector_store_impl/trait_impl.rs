use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use oneshim_core::error::CoreError;
use oneshim_core::models::embedding::{EmbeddingMetadata, SearchFilters, SearchResult};
use oneshim_core::ports::vector_store::VectorStore;
use oneshim_core::quantization::QuantizedVector;
use rusqlite::params;
use tracing::{debug, warn};

use crate::error::StorageError;

use super::helpers::{
    brute_force_search, brute_force_search_quantized, bytes_to_f32_vec, content_type_to_str,
    f32_vec_to_bytes, i8_vec_to_bytes, map_quantized_row, map_vector_row, parse_content_type,
};
use super::SqliteVectorStore;

#[async_trait]
impl VectorStore for SqliteVectorStore {
    async fn store(&self, vector: Vec<f32>, metadata: EmbeddingMetadata) -> Result<(), CoreError> {
        if vector.is_empty() {
            return Err(CoreError::InvalidArguments {
                code: oneshim_core::error_codes::ValidationCode::InvalidArguments,
                message: "Cannot store empty f32 vector".to_string(),
            });
        }

        let blob = f32_vec_to_bytes(&vector);
        let content_type_str = content_type_to_str(&metadata.content_type).to_string();
        let timestamp_str = metadata.timestamp.to_rfc3339();

        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO embedding_vectors (segment_id, content_type, content_label, original_text, vector, model_id, timestamp)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    metadata.segment_id,
                    content_type_str,
                    metadata.content_label,
                    metadata.original_text,
                    blob,
                    metadata.model_id,
                    timestamp_str,
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
        let qv = query_vector.to_vec();

        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT segment_id, content_type, content_label, original_text, vector, timestamp
                     FROM embedding_vectors
                     WHERE is_stale = 0",
                )
                .map_err(|e| StorageError::Internal(format!("Failed to prepare search query: {e}")))?;

            let rows: Vec<super::helpers::VectorRow> = stmt
                .query_map([], map_vector_row)
                .map_err(|e| StorageError::Internal(format!("Failed to query vectors: {e}")))?
                .filter_map(|r| match r {
                    Ok(row) => Some(row),
                    Err(e) => { warn!("Skipping corrupted row in search: {e}"); None }
                })
                .collect();

            Ok(brute_force_search(rows, &qv, limit, time_decay_hours))
        })
        .await
        .map_err(Into::into)
    }

    async fn search_filtered(
        &self,
        query_vector: &[f32],
        limit: usize,
        time_decay_hours: f32,
        filters: &SearchFilters,
    ) -> Result<Vec<SearchResult>, CoreError> {
        let qv = query_vector.to_vec();
        let filters = filters.clone();

        self.with_conn(move |conn| {
            // Build dynamic WHERE clause
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
                        warn!("Skipping corrupted row in search_filtered: {e}");
                        None
                    }
                })
                .collect();

            Ok(brute_force_search(rows, &qv, limit, time_decay_hours))
        })
        .await
        .map_err(Into::into)
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
        self.with_conn(move |conn| {
            let updated = conn
                .execute(
                    "UPDATE embedding_vectors SET is_stale = 1 WHERE model_id = ?1",
                    params![model_id],
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
        self.with_conn(move |conn| {
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
        self.with_conn(move |conn| {
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
        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE embedding_vectors SET vector = ?1, model_id = ?2, is_stale = 0 WHERE id = ?3",
                params![blob, model_id, id],
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
                code: oneshim_core::error_codes::ValidationCode::InvalidArguments,
                message: "Cannot store empty INT8 vector".to_string(),
            });
        }

        // Validate f32/INT8 dimension consistency when f32 is being stored.
        if !skip_float32 && vector_f32.len() != vector_int8.data.len() {
            return Err(CoreError::InvalidArguments {
                code: oneshim_core::error_codes::ValidationCode::InvalidArguments,
                message: format!(
                    "Vector dimension mismatch: f32 has {}, INT8 has {}",
                    vector_f32.len(),
                    vector_int8.data.len()
                ),
            });
        }

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

        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO embedding_vectors (segment_id, content_type, content_label, original_text, vector, model_id, timestamp, vector_int8, quant_scale, quant_offset)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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

        self.with_conn(move |conn| {
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

            let mut update_stmt = conn
                .prepare(
                    "UPDATE embedding_vectors SET vector_int8 = ?1, quant_scale = ?2, quant_offset = ?3 WHERE id = ?4",
                )
                .map_err(|e| StorageError::Internal(format!("Failed to prepare backfill update: {e}")))?;

            let mut count: u64 = 0;
            for (id, blob) in &rows {
                let f32_vec = bytes_to_f32_vec(blob);
                let quantized =
                    oneshim_core::quantization::ScalarQuantizer::quantize(&f32_vec).map_err(
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
        self.with_conn(move |conn| {
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
        self.with_conn(move |conn| {
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
        self.with_conn(move |conn| {
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
        self.with_conn(move |conn| {
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
        self.with_conn(move |conn| {
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
        self.with_conn(move |conn| {
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
