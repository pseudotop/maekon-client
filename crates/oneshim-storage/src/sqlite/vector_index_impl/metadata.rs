use oneshim_core::binary_quantizer::{BinaryCode, QuantileThresholds};
use oneshim_core::ports::vector_index::IndexMeta;
use oneshim_core::quantization::{QuantizedVector, ScalarQuantizer};
use rusqlite::params;

use super::SqliteVectorIndex;
use crate::error::StorageError;

pub(super) async fn assign_to_cluster_impl(
    index: &SqliteVectorIndex,
    vector_id: i64,
    vector: &QuantizedVector,
) -> Result<(), StorageError> {
    index.ensure_cache().await?;

    let cluster_id = {
        let cache = index.centroid_cache.read().await;
        let centroids = cache
            .as_ref()
            .ok_or_else(|| StorageError::Internal("IVF index not built yet".to_string()))?;

        // Pre-validate dimensions once before the hot loop.
        if let Some(first) = centroids.first() {
            if first.vector.data.len() != vector.data.len() {
                return Err(StorageError::Config(format!(
                    "Dimension mismatch: centroid {} vs query {}",
                    first.vector.data.len(),
                    vector.data.len()
                )));
            }
        }

        centroids
            .iter()
            .map(|c| {
                let sim = ScalarQuantizer::cosine_similarity_int8_unchecked(&c.vector, vector);
                (c.id, sim)
            })
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(id, _)| id)
            .unwrap_or(0)
    };

    index
        .with_conn(move |conn| {
            conn.execute(
                "INSERT OR REPLACE INTO ivf_assignments (vector_id, cluster_id) VALUES (?1, ?2)",
                params![vector_id, cluster_id as i64],
            )
            .map_err(|e| {
                StorageError::Internal(format!("Failed to assign vector to cluster: {e}"))
            })?;
            Ok(())
        })
        .await
}

pub(super) async fn store_binary_code_impl(
    index: &SqliteVectorIndex,
    vector_id: i64,
    code: &BinaryCode,
) -> Result<(), StorageError> {
    let code_data = code.data.clone();
    index
        .with_conn(move |conn| {
            conn.execute(
                "INSERT OR REPLACE INTO vector_binary_codes (vector_id, binary_code) VALUES (?1, ?2)",
                params![vector_id, code_data],
            )
            .map_err(|e| {
                StorageError::Internal(format!("Failed to store binary code: {e}"))
            })?;
            Ok(())
        })
        .await
}

pub(super) async fn get_index_meta_impl(
    index: &SqliteVectorIndex,
) -> Result<IndexMeta, StorageError> {
    index
        .with_conn(move |conn| {
            let get_meta = |key: &str| -> Option<String> {
                conn.query_row(
                    "SELECT value FROM vector_index_meta WHERE key = ?1",
                    params![key],
                    |row| row.get(0),
                )
                .ok()
            };

            let total: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM embedding_vectors WHERE is_stale = 0",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(0);

            let unindexed: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM embedding_vectors
                     WHERE is_stale = 0
                       AND id NOT IN (SELECT vector_id FROM ivf_assignments)",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(total);

            let ivf_vec_count: u64 = get_meta("ivf_vector_count")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);

            Ok(IndexMeta {
                ivf_built_at: get_meta("ivf_built_at"),
                ivf_vector_count: ivf_vec_count,
                binary_built_at: get_meta("binary_built_at"),
                total_vector_count: total as u64,
                unindexed_count: unindexed as u64,
            })
        })
        .await
}

pub(super) async fn count_unindexed_impl(index: &SqliteVectorIndex) -> Result<u64, StorageError> {
    index
        .with_conn(move |conn| {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM embedding_vectors
                     WHERE is_stale = 0
                       AND id NOT IN (SELECT vector_id FROM ivf_assignments)",
                    [],
                    |row| row.get(0),
                )
                .map_err(|e| {
                    StorageError::Internal(format!("Failed to count unindexed vectors: {e}"))
                })?;
            Ok(count as u64)
        })
        .await
}

pub(super) async fn load_quantile_thresholds_impl(
    index: &SqliteVectorIndex,
) -> Result<Option<QuantileThresholds>, StorageError> {
    index
        .with_conn(move |conn| {
            let json_opt: Option<String> = conn
                .query_row(
                    "SELECT value FROM vector_index_meta WHERE key = 'binary_quantile_thresholds'",
                    [],
                    |row| row.get(0),
                )
                .ok();

            match json_opt {
                Some(json) => {
                    let thresholds: QuantileThresholds =
                        serde_json::from_str(&json).map_err(|e| {
                            StorageError::Internal(format!(
                                "Failed to deserialize quantile thresholds: {e}"
                            ))
                        })?;
                    Ok(Some(thresholds))
                }
                None => Ok(None),
            }
        })
        .await
}
