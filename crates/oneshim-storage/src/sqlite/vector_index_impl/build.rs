use async_trait::async_trait;
use oneshim_core::binary_quantizer::{BinaryCode, BinaryQuantizer};
use oneshim_core::error::CoreError;
use oneshim_core::ivf_index::{IvfBuildConfig, IvfIndex};
use oneshim_core::models::embedding::{SearchFilters, SearchResult};
use oneshim_core::ports::vector_index::VectorIndex;
use oneshim_core::quantization::{QuantizedVector, ScalarQuantizer};
use rusqlite::params;
use tracing::{debug, info};

use super::{metadata, search, SqliteVectorIndex};
use crate::error::StorageError;

// These methods are part of the VectorIndex trait impl but defined here
// for organizational clarity. The #[async_trait] impl block is in this file.
// search methods are in search.rs, metadata methods are in metadata.rs.

#[async_trait]
#[allow(clippy::too_many_arguments)]
impl VectorIndex for SqliteVectorIndex {
    async fn build_ivf_index(
        &self,
        n_clusters: usize,
        n_iterations: usize,
    ) -> Result<usize, CoreError> {
        // Load all non-stale INT8 vectors
        let vectors = self
            .with_conn(move |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, vector_int8, quant_scale, quant_offset
                     FROM embedding_vectors
                     WHERE is_stale = 0 AND vector_int8 IS NOT NULL",
                    )
                    .map_err(|e| {
                        StorageError::Internal(format!("Failed to prepare vector load: {e}"))
                    })?;

                let rows: Vec<(i64, QuantizedVector)> = stmt
                    .query_map([], |row| {
                        let id: i64 = row.get(0)?;
                        let blob: Vec<u8> = row.get(1)?;
                        let scale: f32 = row.get(2)?;
                        let offset: f32 = row.get(3)?;
                        Ok((
                            id,
                            QuantizedVector {
                                data: blob.iter().map(|&b| b as i8).collect(),
                                scale,
                                offset,
                            },
                        ))
                    })
                    .map_err(|e| StorageError::Internal(format!("Failed to load vectors: {e}")))?
                    .filter_map(|r| r.ok())
                    .collect();

                Ok(rows)
            })
            .await?;

        if vectors.is_empty() {
            // Iter-106: storage query returned no vectors — this is a
            // NotFound condition (the resource required to build the index
            // doesn't exist yet), not an internal runtime failure. Wire code
            // `not_found.resource_missing` so telemetry can distinguish
            // "user hasn't accumulated enough data" from "build pipeline
            // crashed".
            return Err(CoreError::NotFound {
                code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
                resource_type: "embedding_vectors".to_string(),
                id: "ivf_index_build".to_string(),
            });
        }

        let actual_clusters = n_clusters.min(vectors.len());

        // Build IVF index in memory (outside of SQLite lock)
        let config = IvfBuildConfig {
            n_clusters: actual_clusters,
            n_iterations,
            seed: 42,
        };
        let index = IvfIndex::build(&vectors, &config)?;

        // Persist to SQLite
        let centroids_data: Vec<(usize, Vec<u8>, f32, f32, usize)> = index
            .centroids()
            .iter()
            .map(|c| {
                let blob: Vec<u8> = c.vector.data.iter().map(|&b| b as u8).collect();
                (c.id, blob, c.vector.scale, c.vector.offset, c.member_count)
            })
            .collect();

        let assignments: Vec<(i64, usize)> = index
            .assignments()
            .iter()
            .map(|(&vid, &cid)| (vid, cid))
            .collect();

        let n_clusters_result = centroids_data.len();
        let n_vectors = assignments.len();

        self.with_conn(move |conn| {
            // Clear old data
            conn.execute("DELETE FROM ivf_assignments", [])
                .map_err(|e| StorageError::Internal(format!("Failed to clear assignments: {e}")))?;
            conn.execute("DELETE FROM ivf_centroids", [])
                .map_err(|e| StorageError::Internal(format!("Failed to clear centroids: {e}")))?;

            // Insert centroids
            {
                let tx = conn.unchecked_transaction().map_err(|e| {
                    StorageError::Internal(format!("Failed to begin transaction: {e}"))
                })?;

                let mut stmt = tx
                    .prepare(
                        "INSERT INTO ivf_centroids (id, centroid_int8, centroid_scale, centroid_offset, vector_count)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                    )
                    .map_err(|e| {
                        StorageError::Internal(format!("Failed to prepare centroid insert: {e}"))
                    })?;

                for (id, blob, scale, offset, count) in &centroids_data {
                    stmt.execute(params![*id as i64, blob, scale, offset, *count as i64])
                        .map_err(|e| {
                            StorageError::Internal(format!(
                                "Failed to insert centroid {id}: {e}"
                            ))
                        })?;
                }
                drop(stmt);
                tx.commit().map_err(|e| {
                    StorageError::Internal(format!("Failed to commit centroids: {e}"))
                })?;
            }

            // Insert assignments in chunks of 1000
            for chunk in assignments.chunks(1000) {
                let tx = conn.unchecked_transaction().map_err(|e| {
                    StorageError::Internal(format!("Failed to begin assignment tx: {e}"))
                })?;

                let mut stmt = tx
                    .prepare(
                        "INSERT OR REPLACE INTO ivf_assignments (vector_id, cluster_id)
                         VALUES (?1, ?2)",
                    )
                    .map_err(|e| {
                        StorageError::Internal(format!(
                            "Failed to prepare assignment insert: {e}"
                        ))
                    })?;

                for (vid, cid) in chunk {
                    stmt.execute(params![vid, *cid as i64])
                        .map_err(|e| {
                            StorageError::Internal(format!(
                                "Failed to insert assignment for vector {vid}: {e}"
                            ))
                        })?;
                }
                drop(stmt);
                tx.commit().map_err(|e| {
                    StorageError::Internal(format!("Failed to commit assignments: {e}"))
                })?;
            }

            // Update meta
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "INSERT OR REPLACE INTO vector_index_meta (key, value, updated_at) VALUES ('ivf_built_at', ?1, ?1)",
                params![now],
            ).map_err(|e| StorageError::Internal(format!("Failed to update index meta: {e}")))?;

            conn.execute(
                "INSERT OR REPLACE INTO vector_index_meta (key, value, updated_at) VALUES ('ivf_vector_count', ?1, ?2)",
                params![n_vectors.to_string(), now],
            ).map_err(|e| StorageError::Internal(format!("Failed to update vector count meta: {e}")))?;

            // WAL checkpoint
            if let Err(e) = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)") {
                debug!("execute_batch failed: {e}");
            }

            // Refresh query planner statistics after bulk index writes
            if let Err(e) = conn.execute_batch("ANALYZE") {
                debug!("execute_batch failed: {e}");
            }

            info!(
                "IVF index built: {} clusters, {} vectors",
                n_clusters_result, n_vectors
            );

            Ok(n_clusters_result)
        })
        .await?;

        self.invalidate_cache().await;
        Ok(n_clusters_result)
    }

    async fn build_binary_codes(&self) -> Result<u64, CoreError> {
        // Load all non-stale vectors and dequantize to f32
        let vectors_data = self
            .with_conn(move |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, vector_int8, quant_scale, quant_offset
                     FROM embedding_vectors
                     WHERE is_stale = 0 AND vector_int8 IS NOT NULL",
                    )
                    .map_err(|e| {
                        StorageError::Internal(format!("Failed to prepare vector load: {e}"))
                    })?;

                let rows: Vec<(i64, Vec<f32>)> = stmt
                    .query_map([], |row| {
                        let id: i64 = row.get(0)?;
                        let blob: Vec<u8> = row.get(1)?;
                        let scale: f32 = row.get(2)?;
                        let offset: f32 = row.get(3)?;
                        let qv = QuantizedVector {
                            data: blob.iter().map(|&b| b as i8).collect(),
                            scale,
                            offset,
                        };
                        Ok((id, ScalarQuantizer::dequantize(&qv)))
                    })
                    .map_err(|e| StorageError::Internal(format!("Failed to load vectors: {e}")))?
                    .filter_map(|r| r.ok())
                    .collect();

                Ok(rows)
            })
            .await?;

        if vectors_data.is_empty() {
            return Ok(0);
        }

        let dims = vectors_data[0].1.len();
        let f32_vecs: Vec<Vec<f32>> = vectors_data.iter().map(|(_, v)| v.clone()).collect();

        // Compute thresholds
        let thresholds = BinaryQuantizer::compute_thresholds(&f32_vecs, dims)?;

        // Encode each vector
        let codes: Vec<(i64, Vec<u8>)> = vectors_data
            .iter()
            .map(|(id, v)| {
                let code =
                    BinaryQuantizer::encode(v, &thresholds).unwrap_or(BinaryCode { data: vec![] });
                (*id, code.data)
            })
            .collect();

        let count = codes.len() as u64;

        // Store thresholds as JSON
        let thresholds_json =
            serde_json::to_string(&thresholds).map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("Failed to serialize thresholds: {e}"),
            })?;

        // Persist to SQLite in chunks
        self.with_conn(move |conn| {
            for chunk in codes.chunks(1000) {
                let tx = conn.unchecked_transaction().map_err(|e| {
                    StorageError::Internal(format!("Failed to begin binary code tx: {e}"))
                })?;

                let mut stmt = tx
                    .prepare(
                        "INSERT OR REPLACE INTO vector_binary_codes (vector_id, binary_code)
                         VALUES (?1, ?2)",
                    )
                    .map_err(|e| {
                        StorageError::Internal(format!(
                            "Failed to prepare binary code insert: {e}"
                        ))
                    })?;

                for (vid, code_data) in chunk {
                    stmt.execute(params![vid, code_data]).map_err(|e| {
                        StorageError::Internal(format!(
                            "Failed to insert binary code for vector {vid}: {e}"
                        ))
                    })?;
                }
                drop(stmt);
                tx.commit().map_err(|e| {
                    StorageError::Internal(format!("Failed to commit binary codes: {e}"))
                })?;
            }

            // Store thresholds and build time
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "INSERT OR REPLACE INTO vector_index_meta (key, value, updated_at) VALUES ('binary_quantile_thresholds', ?1, ?2)",
                params![thresholds_json, now],
            ).map_err(|e| StorageError::Internal(format!("Failed to store thresholds: {e}")))?;

            conn.execute(
                "INSERT OR REPLACE INTO vector_index_meta (key, value, updated_at) VALUES ('binary_built_at', ?1, ?1)",
                params![now],
            ).map_err(|e| StorageError::Internal(format!("Failed to update binary build time: {e}")))?;

            if let Err(e) = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)") {
                debug!("execute_batch failed: {e}");
            }

            // Refresh query planner statistics after bulk binary code writes
            if let Err(e) = conn.execute_batch("ANALYZE") {
                debug!("execute_batch failed: {e}");
            }

            info!("Binary codes built for {} vectors", count);
            Ok(count)
        })
        .await
        .map_err(CoreError::from)
    }

    // Remaining trait methods are in search.rs and metadata.rs
    // They are forwarded here via the partial impl pattern.
    // NOTE: Rust requires all trait methods in a single impl block,
    // so we include search and metadata methods inline below.

    async fn search_ivf(
        &self,
        query_vector: &QuantizedVector,
        nprobe: usize,
        limit: usize,
        time_decay_hours: f32,
        filters: &SearchFilters,
    ) -> Result<Vec<SearchResult>, CoreError> {
        search::search_ivf_impl(self, query_vector, nprobe, limit, time_decay_hours, filters).await
    }

    async fn search_ivf_binary(
        &self,
        query_vector: &QuantizedVector,
        query_binary: &BinaryCode,
        nprobe: usize,
        oversample_factor: usize,
        limit: usize,
        time_decay_hours: f32,
        filters: &SearchFilters,
    ) -> Result<Vec<SearchResult>, CoreError> {
        search::search_ivf_binary_impl(
            self,
            query_vector,
            query_binary,
            nprobe,
            oversample_factor,
            limit,
            time_decay_hours,
            filters,
        )
        .await
    }

    async fn assign_to_cluster(
        &self,
        vector_id: i64,
        vector: &QuantizedVector,
    ) -> Result<(), CoreError> {
        Ok(metadata::assign_to_cluster_impl(self, vector_id, vector).await?)
    }

    async fn store_binary_code(&self, vector_id: i64, code: &BinaryCode) -> Result<(), CoreError> {
        Ok(metadata::store_binary_code_impl(self, vector_id, code).await?)
    }

    async fn get_index_meta(
        &self,
    ) -> Result<oneshim_core::ports::vector_index::IndexMeta, CoreError> {
        Ok(metadata::get_index_meta_impl(self).await?)
    }

    async fn count_unindexed(&self) -> Result<u64, CoreError> {
        Ok(metadata::count_unindexed_impl(self).await?)
    }

    async fn load_quantile_thresholds(
        &self,
    ) -> Result<Option<oneshim_core::binary_quantizer::QuantileThresholds>, CoreError> {
        Ok(metadata::load_quantile_thresholds_impl(self).await?)
    }
}
