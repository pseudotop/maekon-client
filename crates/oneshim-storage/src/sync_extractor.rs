//! ChangeExtractor implementation for SQLite.
//!
//! Queries activity_segments, regimes, regime_overrides, embedding_vectors,
//! suggestions, and trigger_params_snapshots for rows modified since a
//! given HLC watermark. Respects SyncConfig data minimization flags.

use async_trait::async_trait;
use rusqlite::Connection;
use std::sync::{Arc, Mutex};
use tracing::debug;

use crate::error::StorageError;
use oneshim_core::config::SyncConfig;
use oneshim_core::error::CoreError;
use oneshim_core::models::sync::{ChangeSet, ChangeSetKind};
use oneshim_core::ports::change_extractor::ChangeExtractor;
use oneshim_core::sync::Hlc;

/// SQLite-backed ChangeExtractor adapter.
pub struct SqliteSyncExtractor {
    conn: Arc<Mutex<Connection>>,
    device_id: String,
    device_name: String,
    sync_config: SyncConfig,
}

impl SqliteSyncExtractor {
    pub fn new(
        conn: Arc<Mutex<Connection>>,
        device_id: String,
        device_name: String,
        sync_config: SyncConfig,
    ) -> Self {
        Self {
            conn,
            device_id,
            device_name,
            sync_config,
        }
    }

    /// Backfill origin_device_id for pre-sync rows (empty string -> local device_id).
    /// Called once on first extraction. Idempotent.
    fn backfill_origin_device_id(conn: &Connection, device_id: &str) -> Result<u64, StorageError> {
        let tables = [
            "activity_segments",
            "regimes",
            "regime_overrides",
            "embedding_vectors",
            "suggestions",
            "trigger_params_snapshots",
        ];
        let mut total = 0u64;
        for table in &tables {
            let sql =
                format!("UPDATE {table} SET origin_device_id = ?1 WHERE origin_device_id = ''");
            let updated = conn
                .execute(&sql, rusqlite::params![device_id])
                .map_err(|e| {
                    StorageError::Internal(format!("backfill origin_device_id on {table}: {e}"))
                })?;
            total += updated as u64;
        }
        if total > 0 {
            debug!("backfilled origin_device_id on {total} rows");
        }
        Ok(total)
    }

    /// Query a single table for rows with HLC > watermark, returning JSON values.
    fn query_table_changes(
        conn: &Connection,
        table: &str,
        columns: &str,
        since: &Hlc,
    ) -> Result<Vec<serde_json::Value>, StorageError> {
        let sql = format!(
            "SELECT {columns} FROM {table} \
             WHERE (hlc_wall_ms > ?1) \
                OR (hlc_wall_ms = ?1 AND hlc_counter > ?2) \
                OR (hlc_wall_ms = ?1 AND hlc_counter = ?2 AND origin_device_id > ?3) \
             ORDER BY hlc_wall_ms, hlc_counter"
        );
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| StorageError::Internal(format!("prepare query for {table}: {e}")))?;

        let rows = stmt
            .query_map(
                rusqlite::params![since.wall_ms, since.counter, &since.device_id],
                |row| {
                    let json_str: String = row.get(0)?;
                    Ok(json_str)
                },
            )
            .map_err(|e| StorageError::Internal(format!("query {table}: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            let json_str =
                row.map_err(|e| StorageError::Internal(format!("row read {table}: {e}")))?;
            let value: serde_json::Value = serde_json::from_str(&json_str)
                .map_err(|e| StorageError::Internal(format!("json parse {table}: {e}")))?;
            results.push(value);
        }
        Ok(results)
    }

    /// Find the maximum HLC across all syncable tables.
    fn compute_max_hlc(conn: &Connection, device_id: &str) -> Result<Hlc, StorageError> {
        let tables = [
            "activity_segments",
            "regimes",
            "regime_overrides",
            "embedding_vectors",
            "suggestions",
            "trigger_params_snapshots",
        ];

        let mut max = Hlc::default();
        for table in &tables {
            let sql = format!(
                "SELECT COALESCE(MAX(hlc_wall_ms), 0), \
                        COALESCE(MAX(hlc_counter), 0) \
                 FROM {table} WHERE hlc_wall_ms = (\
                   SELECT COALESCE(MAX(hlc_wall_ms), 0) FROM {table}\
                 )"
            );
            let (wall_ms, counter): (u64, u32) = conn
                .query_row(&sql, [], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(|e| StorageError::Internal(format!("max HLC query on {table}: {e}")))?;

            let candidate = Hlc {
                wall_ms,
                counter,
                device_id: device_id.to_string(),
            };
            if candidate > max {
                max = candidate;
            }
        }
        Ok(max)
    }
}

#[async_trait]
impl ChangeExtractor for SqliteSyncExtractor {
    async fn get_changes_since(&self, since: &Hlc) -> Result<ChangeSet, CoreError> {
        let conn = self.conn.clone();
        let since = since.clone();
        let device_id = self.device_id.clone();
        let device_name = self.device_name.clone();
        let include_content = self.sync_config.include_content_activities;
        let include_embed_text = self.sync_config.include_embedding_text;

        tokio::task::spawn_blocking(move || {
            let guard = conn
                .lock()
                .map_err(|e| StorageError::Internal(format!("SQLite lock poisoned: {e}")))?;

            // Backfill on first extraction
            Self::backfill_origin_device_id(&guard, &device_id)?;

            // --- Build per-table JSON extraction queries ---
            // Each query uses json_object() to produce a self-contained JSON row.

            // activity_segments (append-only)
            let seg_cols = if include_content {
                "json_object('id',id,'start_time',start_time,'end_time',end_time,\
                 'duration_secs',duration_secs,'regime_id',regime_id,\
                 'dominant_category',dominant_category,'app_breakdown',app_breakdown,\
                 'llm_summary',llm_summary,'content_activities_json',content_activities_json,\
                 'hlc_wall_ms',hlc_wall_ms,'hlc_counter',hlc_counter,\
                 'origin_device_id',origin_device_id)"
            } else {
                "json_object('id',id,'start_time',start_time,'end_time',end_time,\
                 'duration_secs',duration_secs,'regime_id',regime_id,\
                 'dominant_category',dominant_category,'app_breakdown',app_breakdown,\
                 'llm_summary',llm_summary,\
                 'hlc_wall_ms',hlc_wall_ms,'hlc_counter',hlc_counter,\
                 'origin_device_id',origin_device_id)"
            };
            let segments =
                Self::query_table_changes(&guard, "activity_segments", seg_cols, &since)?;

            // regimes (LWW, includes tombstone columns)
            let regimes = Self::query_table_changes(
                &guard,
                "regimes",
                "json_object('id',id,'label',label,'detected_at',detected_at,\
                 'last_seen_at',last_seen_at,'occurrence_count',occurrence_count,\
                 'avg_density',avg_density,'avg_importance',avg_importance,\
                 'dominant_category',dominant_category,'params_snapshot_id',params_snapshot_id,\
                 'is_active',is_active,'is_deleted',is_deleted,'deleted_at',deleted_at,\
                 'hlc_wall_ms',hlc_wall_ms,'hlc_counter',hlc_counter,\
                 'origin_device_id',origin_device_id)",
                &since,
            )?;

            // regime_overrides (append-only)
            let overrides = Self::query_table_changes(
                &guard,
                "regime_overrides",
                "json_object('override_id',override_id,'segment_id',segment_id,\
                 'original_regime_id',original_regime_id,'action_type',action_type,\
                 'action_data',action_data,'created_at',created_at,\
                 'hlc_wall_ms',hlc_wall_ms,'hlc_counter',hlc_counter,\
                 'origin_device_id',origin_device_id)",
                &since,
            )?;

            // embedding_vectors (LWW, includes tombstone; respects include_embed_text)
            let embed_cols = if include_embed_text {
                "json_object('id',id,'segment_id',segment_id,'content_type',content_type,\
                 'content_label',content_label,'original_text',original_text,\
                 'vector',hex(vector),'model_id',model_id,'timestamp',timestamp,\
                 'is_stale',is_stale,'is_deleted',is_deleted,'deleted_at',deleted_at,\
                 'hlc_wall_ms',hlc_wall_ms,'hlc_counter',hlc_counter,\
                 'origin_device_id',origin_device_id)"
            } else {
                "json_object('id',id,'segment_id',segment_id,'content_type',content_type,\
                 'content_label',content_label,\
                 'vector',hex(vector),'model_id',model_id,'timestamp',timestamp,\
                 'is_stale',is_stale,'is_deleted',is_deleted,'deleted_at',deleted_at,\
                 'hlc_wall_ms',hlc_wall_ms,'hlc_counter',hlc_counter,\
                 'origin_device_id',origin_device_id)"
            };
            let embeddings =
                Self::query_table_changes(&guard, "embedding_vectors", embed_cols, &since)?;

            // suggestions (LWW, monotonic status merge)
            let suggestions = Self::query_table_changes(
                &guard,
                "suggestions",
                "json_object('suggestion_id',suggestion_id,'suggestion_type',suggestion_type,\
                 'source',source,'content',content,'priority',priority,\
                 'confidence_score',confidence_score,'relevance_score',relevance_score,\
                 'is_actionable',is_actionable,'reasoning',reasoning,\
                 'shown_at',shown_at,'dismissed_at',dismissed_at,'acted_at',acted_at,\
                 'created_at',created_at,'expires_at',expires_at,\
                 'is_deleted',is_deleted,'deleted_at',deleted_at,\
                 'hlc_wall_ms',hlc_wall_ms,'hlc_counter',hlc_counter,\
                 'origin_device_id',origin_device_id)",
                &since,
            )?;

            // trigger_params_snapshots (append-only)
            let param_snapshots = Self::query_table_changes(
                &guard,
                "trigger_params_snapshots",
                "json_object('id',id,'created_at',created_at,'preset',preset,\
                 'params_json',params_json,\
                 'hlc_wall_ms',hlc_wall_ms,'hlc_counter',hlc_counter,\
                 'origin_device_id',origin_device_id)",
                &since,
            )?;

            // Compute new watermark from max HLC across all extracted rows
            let watermark = Self::compute_max_hlc(&guard, &device_id)?;

            Ok::<ChangeSet, StorageError>(ChangeSet {
                kind: ChangeSetKind::Data,
                origin_device_id: device_id,
                origin_device_name: device_name,
                watermark,
                segments,
                regimes,
                overrides,
                embeddings,
                suggestions,
                param_snapshots,
                preferences: Vec::new(), // deferred to Phase 3b
            })
        })
        .await
        .map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("spawn_blocking join error: {e}"),
        })?
        .map_err(CoreError::from)
    }

    async fn local_watermark(&self) -> Result<Hlc, CoreError> {
        let conn = self.conn.clone();
        let device_id = self.device_id.clone();

        tokio::task::spawn_blocking(move || {
            let guard = conn
                .lock()
                .map_err(|e| StorageError::Internal(format!("SQLite lock poisoned: {e}")))?;
            Self::compute_max_hlc(&guard, &device_id)
        })
        .await
        .map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("spawn_blocking join error: {e}"),
        })?
        .map_err(CoreError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::SqliteStorage;

    fn setup() -> (SqliteStorage, String) {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let (device_id, _) = storage.ensure_device_identity("Test Device").unwrap();
        (storage, device_id)
    }

    #[tokio::test]
    async fn empty_db_returns_empty_changeset() {
        let (storage, device_id) = setup();
        let extractor = SqliteSyncExtractor::new(
            storage.connection_arc(),
            device_id,
            "Test".to_string(),
            SyncConfig::default(),
        );
        let cs = extractor.get_changes_since(&Hlc::default()).await.unwrap();
        assert!(cs.is_empty());
        assert_eq!(cs.kind, ChangeSetKind::Data);
    }

    #[tokio::test]
    async fn local_watermark_returns_default_on_empty_db() {
        let (storage, device_id) = setup();
        let extractor = SqliteSyncExtractor::new(
            storage.connection_arc(),
            device_id,
            "Test".to_string(),
            SyncConfig::default(),
        );
        let wm = extractor.local_watermark().await.unwrap();
        assert_eq!(wm.wall_ms, 0);
        assert_eq!(wm.counter, 0);
    }

    #[tokio::test]
    async fn backfill_sets_origin_device_id() {
        let (storage, device_id) = setup();
        // Insert a segment with empty origin_device_id (simulating pre-V14 data)
        {
            let conn = storage.connection_arc();
            let guard = conn.lock().unwrap();
            guard
                .execute(
                    "INSERT INTO activity_segments \
                 (id, start_time, end_time, duration_secs, trigger_reason, \
                  dominant_category, hlc_wall_ms, hlc_counter, origin_device_id) \
                 VALUES ('seg-1', '2026-01-01T00:00:00', '2026-01-01T01:00:00', \
                         3600, 'timer', 'Development', 100, 1, '')",
                    [],
                )
                .unwrap();
        }

        let extractor = SqliteSyncExtractor::new(
            storage.connection_arc(),
            device_id.clone(),
            "Test".to_string(),
            SyncConfig::default(),
        );
        let cs = extractor.get_changes_since(&Hlc::default()).await.unwrap();
        assert_eq!(cs.segments.len(), 1);

        // Verify backfill happened
        let conn = storage.connection_arc();
        let guard = conn.lock().unwrap();
        let origin: String = guard
            .query_row(
                "SELECT origin_device_id FROM activity_segments WHERE id = 'seg-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(origin, device_id);
    }

    #[tokio::test]
    async fn watermark_filters_old_rows() {
        let (storage, device_id) = setup();
        {
            let conn = storage.connection_arc();
            let guard = conn.lock().unwrap();
            // Row with HLC (100, 1)
            guard
                .execute(
                    "INSERT INTO activity_segments \
                 (id, start_time, end_time, duration_secs, trigger_reason, \
                  dominant_category, hlc_wall_ms, hlc_counter, origin_device_id) \
                 VALUES ('seg-old', '2026-01-01T00:00:00', '2026-01-01T01:00:00', \
                         3600, 'timer', 'Development', 100, 1, ?1)",
                    rusqlite::params![device_id],
                )
                .unwrap();
            // Row with HLC (200, 0)
            guard
                .execute(
                    "INSERT INTO activity_segments \
                 (id, start_time, end_time, duration_secs, trigger_reason, \
                  dominant_category, hlc_wall_ms, hlc_counter, origin_device_id) \
                 VALUES ('seg-new', '2026-01-02T00:00:00', '2026-01-02T01:00:00', \
                         3600, 'timer', 'Communication', 200, 0, ?1)",
                    rusqlite::params![device_id],
                )
                .unwrap();
        }

        let extractor = SqliteSyncExtractor::new(
            storage.connection_arc(),
            device_id,
            "Test".to_string(),
            SyncConfig::default(),
        );

        // Watermark at (150, 0) should only return seg-new
        let since = Hlc {
            wall_ms: 150,
            counter: 0,
            device_id: "".to_string(),
        };
        let cs = extractor.get_changes_since(&since).await.unwrap();
        assert_eq!(cs.segments.len(), 1);
        assert_eq!(cs.segments[0]["id"], "seg-new");
    }
}
