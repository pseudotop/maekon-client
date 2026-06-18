//! `BackupStorage`, `SegmentQueryStorage`, `GuiInteractionStorage` implementations.
//!
//! Converted to `#[async_trait]` async per ADR-026 PR-7 (async storage
//! convergence). Each trait method routes the SQLite work through the
//! `with_conn`/`with_conn_read` funnel (`spawn_blocking`), so the
//! single-connection `parking_lot` guard is acquired on a blocking-pool thread
//! and never held across an `.await` (#4928 erase barrier preserved). Reads use
//! `with_conn_read` (never skipped); writes use the `with_conn` write funnel
//! (skipped when `deletion_flag`/`erasing` is set).
//!
//! `BackupStorage` delegates to `*_async` inherent helpers (defined in
//! `maintenance/backup.rs`, `maintenance/export.rs`, `metrics/mod.rs`) that each
//! keep a synchronous inherent twin sharing one `*_inner(&Connection, ...)` SQL
//! body — the sync twins back the storage `#[cfg(test)]` suites + the GDPR erase
//! spawn_blocking path. `GuiInteractionStorage`/`SegmentQueryStorage` keep a sync
//! inherent twin too, because the src-tauri `SchedulerStorage` trait + the
//! capture-loop `spawn_blocking` call sites still resolve to the synchronous path.

use async_trait::async_trait;

use maekon_core::error::CoreError;
use maekon_core::models::storage_records::{
    EventExportRecord, FrameExportRecord, FrameTagLinkRecord, GuiInteractionRecord,
    HourlyMetricsRecord, MetricExportRecord, NewGuiInteraction, SegmentDetailRecord, TagRecord,
};
use maekon_core::ports::web_storage::{BackupStorage, GuiInteractionStorage, SegmentQueryStorage};

use super::scrub_basic_pii;
use crate::error::StorageError;
use crate::sqlite::SqliteStorage;

// ---------------------------------------------------------------------------
// BackupStorage
// ---------------------------------------------------------------------------

#[async_trait]
impl BackupStorage for SqliteStorage {
    async fn list_backup_tags(&self) -> Result<Vec<TagRecord>, CoreError> {
        SqliteStorage::list_backup_tags_async(self)
            .await
            .map_err(Into::into)
    }

    async fn list_backup_frame_tags(&self) -> Result<Vec<FrameTagLinkRecord>, CoreError> {
        SqliteStorage::list_backup_frame_tags_async(self)
            .await
            .map_err(Into::into)
    }

    async fn list_event_exports(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<EventExportRecord>, CoreError> {
        SqliteStorage::list_event_exports_async(self, from, to)
            .await
            .map_err(Into::into)
    }

    async fn list_metric_exports(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<MetricExportRecord>, CoreError> {
        SqliteStorage::list_metric_exports_async(self, from, to)
            .await
            .map_err(Into::into)
    }

    async fn list_frame_exports(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<FrameExportRecord>, CoreError> {
        SqliteStorage::list_frame_exports_async(self, from, to)
            .await
            .map_err(Into::into)
    }

    async fn list_hourly_metrics_since(
        &self,
        from: &str,
    ) -> Result<Vec<HourlyMetricsRecord>, CoreError> {
        SqliteStorage::list_hourly_metrics_since_async(self, from)
            .await
            .map_err(Into::into)
    }

    async fn upsert_backup_tag(
        &self,
        id: i64,
        name: &str,
        color: &str,
        created_at: &str,
    ) -> Result<(), CoreError> {
        // Move owned data into the Send + 'static closure (no borrowed &str).
        SqliteStorage::upsert_backup_tag_async(
            self,
            id,
            name.to_owned(),
            color.to_owned(),
            created_at.to_owned(),
        )
        .await
        .map_err(Into::into)
    }

    async fn upsert_backup_frame_tag(
        &self,
        frame_id: i64,
        tag_id: i64,
        created_at: &str,
    ) -> Result<(), CoreError> {
        SqliteStorage::upsert_backup_frame_tag_async(self, frame_id, tag_id, created_at.to_owned())
            .await
            .map_err(Into::into)
    }

    async fn upsert_backup_event(
        &self,
        event_id: &str,
        event_type: &str,
        timestamp: &str,
        app_name: Option<&str>,
        window_title: Option<&str>,
    ) -> Result<(), CoreError> {
        SqliteStorage::upsert_backup_event_async(
            self,
            event_id.to_owned(),
            event_type.to_owned(),
            timestamp.to_owned(),
            app_name.map(str::to_owned),
            window_title.map(str::to_owned),
        )
        .await
        .map_err(Into::into)
    }

    async fn upsert_backup_frame(
        &self,
        id: i64,
        timestamp: &str,
        trigger_type: &str,
        app_name: &str,
        window_title: &str,
        importance: f32,
        width: i32,
        height: i32,
        ocr_text: Option<&str>,
    ) -> Result<(), CoreError> {
        SqliteStorage::upsert_backup_frame_async(
            self,
            id,
            timestamp.to_owned(),
            trigger_type.to_owned(),
            app_name.to_owned(),
            window_title.to_owned(),
            importance,
            width,
            height,
            ocr_text.map(str::to_owned),
        )
        .await
        .map_err(Into::into)
    }
}

// ---------------------------------------------------------------------------
// SegmentQueryStorage
// ---------------------------------------------------------------------------

#[async_trait]
impl SegmentQueryStorage for SqliteStorage {
    async fn get_segment_details(
        &self,
        segment_ids: &[String],
    ) -> Result<std::collections::HashMap<String, SegmentDetailRecord>, CoreError> {
        SqliteStorage::get_segment_details_async(self, segment_ids)
            .await
            .map_err(Into::into)
    }
}

// ---------------------------------------------------------------------------
// GuiInteractionStorage
// ---------------------------------------------------------------------------

#[async_trait]
impl GuiInteractionStorage for SqliteStorage {
    async fn save_gui_interaction(&self, input: &NewGuiInteraction<'_>) -> Result<(), CoreError> {
        // Move owned data into the Send + 'static closure (no borrowed lifetime).
        let input = owned_gui_interaction(input);
        SqliteStorage::save_gui_interaction_async(self, input)
            .await
            .map_err(Into::into)
    }

    async fn list_gui_interactions_for_segment(
        &self,
        segment_id: &str,
    ) -> Result<Vec<GuiInteractionRecord>, CoreError> {
        SqliteStorage::list_gui_interactions_for_segment_async(self, segment_id)
            .await
            .map_err(Into::into)
    }

    async fn query_gui_interaction_density(
        &self,
        start: &str,
        end: &str,
    ) -> Result<Vec<(String, u32)>, CoreError> {
        SqliteStorage::query_gui_interaction_density_async(self, start, end)
            .await
            .map_err(Into::into)
    }
}

// ---------------------------------------------------------------------------
// Inherent storage methods (sync twin + async funnel helper + shared `*_inner`
// SQL body) for SegmentQuery + GuiInteraction. The sync twins keep the src-tauri
// `SchedulerStorage` trait + the capture-loop `spawn_blocking` call sites
// synchronous; the async helpers back the trait impls above.
// ---------------------------------------------------------------------------

/// Owned, `Send + 'static` parameter bundle for a GUI-interaction write, built
/// before entering the `spawn_blocking` closure (no borrowed lifetime fields).
pub(crate) struct OwnedGuiInteraction {
    event_id: String,
    segment_id: Option<String>,
    timestamp: String,
    element_text: Option<String>,
    element_type: Option<String>,
    interaction_type: String,
    bbox_json: Option<String>,
    app_name: String,
    type_confidence: f32,
}

impl SqliteStorage {
    // ---- segment details --------------------------------------------------

    /// Get activity segment details for the given IDs (sync twin).
    pub fn get_segment_details(
        &self,
        segment_ids: &[String],
    ) -> Result<std::collections::HashMap<String, SegmentDetailRecord>, StorageError> {
        if segment_ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        // Read — read_lock (independent of deletion_flag).
        let read = self.conn.read_lock();
        Self::get_segment_details_inner(read.conn(), segment_ids)
    }

    /// Async `get_segment_details` over the read funnel (ADR-026 PR-7).
    pub(crate) async fn get_segment_details_async(
        &self,
        segment_ids: &[String],
    ) -> Result<std::collections::HashMap<String, SegmentDetailRecord>, StorageError> {
        if segment_ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        // owned move into the Send + 'static closure (no borrowed slice).
        let segment_ids = segment_ids.to_vec();
        self.with_conn_read(move |conn| Self::get_segment_details_inner(conn, &segment_ids))
            .await
    }

    fn get_segment_details_inner(
        conn: &rusqlite::Connection,
        segment_ids: &[String],
    ) -> Result<std::collections::HashMap<String, SegmentDetailRecord>, StorageError> {
        // Check if the activity_segments table exists.
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='activity_segments'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if !table_exists {
            return Ok(std::collections::HashMap::new());
        }

        let mut map = std::collections::HashMap::new();
        for id in segment_ids {
            let result = conn.query_row(
                "SELECT id, start_time, end_time, duration_secs, llm_summary, dominant_category, regime_id
                 FROM activity_segments WHERE id = ?1",
                rusqlite::params![id],
                |row| {
                    Ok(SegmentDetailRecord {
                        segment_id: row.get(0)?,
                        start_time: row.get(1)?,
                        end_time: row.get(2)?,
                        duration_secs: row.get::<_, i64>(3)? as u64,
                        llm_summary: row.get(4)?,
                        dominant_category: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
                        regime_label: row.get(6)?,
                    })
                },
            );
            if let Ok(record) = result {
                map.insert(id.clone(), record);
            }
        }
        Ok(map)
    }

    // ---- gui interaction writes -------------------------------------------

    /// Save a GUI interaction event (sync twin).
    pub fn save_gui_interaction(&self, input: &NewGuiInteraction<'_>) -> Result<(), StorageError> {
        let params = owned_gui_interaction(input);
        // Write — write_lock (skipped when deletion_flag is set; gui_interactions in ALL_TABLES).
        self.conn
            .write_lock()
            .run((), |conn| Self::save_gui_interaction_inner(conn, &params))
    }

    /// Async `save_gui_interaction` over the write funnel (ADR-026 PR-7).
    pub(crate) async fn save_gui_interaction_async(
        &self,
        params: OwnedGuiInteraction,
    ) -> Result<(), StorageError> {
        self.with_conn(move |conn| Self::save_gui_interaction_inner(conn, &params))
            .await
    }

    fn save_gui_interaction_inner(
        conn: &rusqlite::Connection,
        params: &OwnedGuiInteraction,
    ) -> Result<(), StorageError> {
        // Defense-in-depth: basic PII scrub on element_text at storage boundary.
        // Primary filtering is the caller's responsibility (see port doc comment).
        let scrubbed_text = params.element_text.as_deref().map(scrub_basic_pii);
        let scrubbed_ref = scrubbed_text.as_deref();

        conn.execute(
            "INSERT INTO gui_interactions (event_id, segment_id, timestamp, element_text, element_type, interaction_type, bbox_json, app_name, type_confidence)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                params.event_id,
                params.segment_id,
                params.timestamp,
                scrubbed_ref,
                params.element_type,
                params.interaction_type,
                params.bbox_json,
                params.app_name,
                params.type_confidence,
            ],
        )
        .map_err(|e| StorageError::Internal(format!("Failed to save GUI interaction: {e}")))?;
        Ok(())
    }

    // ---- gui interaction reads --------------------------------------------

    /// List GUI interaction events for a given segment (sync twin).
    pub fn list_gui_interactions_for_segment(
        &self,
        segment_id: &str,
    ) -> Result<Vec<GuiInteractionRecord>, StorageError> {
        // Read — read_lock (independent of deletion_flag).
        let read = self.conn.read_lock();
        Self::list_gui_interactions_for_segment_inner(read.conn(), segment_id)
    }

    /// Async `list_gui_interactions_for_segment` over the read funnel (ADR-026 PR-7).
    pub(crate) async fn list_gui_interactions_for_segment_async(
        &self,
        segment_id: &str,
    ) -> Result<Vec<GuiInteractionRecord>, StorageError> {
        // owned move into the Send + 'static closure (no borrowed &str).
        let segment_id = segment_id.to_owned();
        self.with_conn_read(move |conn| {
            Self::list_gui_interactions_for_segment_inner(conn, &segment_id)
        })
        .await
    }

    fn list_gui_interactions_for_segment_inner(
        conn: &rusqlite::Connection,
        segment_id: &str,
    ) -> Result<Vec<GuiInteractionRecord>, StorageError> {
        let mut stmt = conn
            .prepare(
                "SELECT id, event_id, segment_id, timestamp, element_text, element_type,
                        interaction_type, bbox_json, app_name, created_at, type_confidence
                 FROM gui_interactions
                 WHERE segment_id = ?1
                 ORDER BY timestamp ASC",
            )
            .map_err(|e| {
                StorageError::Internal(format!("Failed to prepare GUI interaction query: {e}"))
            })?;

        let records: Vec<GuiInteractionRecord> = stmt
            .query_map(rusqlite::params![segment_id], |row| {
                Ok(GuiInteractionRecord {
                    id: row.get(0)?,
                    event_id: row.get(1)?,
                    segment_id: row.get(2)?,
                    timestamp: row.get(3)?,
                    element_text: row.get(4)?,
                    element_type: row.get(5)?,
                    interaction_type: row.get(6)?,
                    bbox_json: row.get(7)?,
                    app_name: row.get(8)?,
                    created_at: row.get(9)?,
                    type_confidence: row.get::<_, Option<f64>>(10)?.unwrap_or(1.0) as f32,
                })
            })
            .map_err(|e| StorageError::Internal(format!("Failed to query GUI interactions: {e}")))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(records)
    }

    /// Count GUI interactions per hour within a date range (sync twin).
    pub fn query_gui_interaction_density(
        &self,
        start: &str,
        end: &str,
    ) -> Result<Vec<(String, u32)>, StorageError> {
        // Read — read_lock (independent of deletion_flag).
        let read = self.conn.read_lock();
        Self::query_gui_interaction_density_inner(read.conn(), start, end)
    }

    /// Async `query_gui_interaction_density` over the read funnel (ADR-026 PR-7).
    pub(crate) async fn query_gui_interaction_density_async(
        &self,
        start: &str,
        end: &str,
    ) -> Result<Vec<(String, u32)>, StorageError> {
        // owned move into the Send + 'static closure (no borrowed &str).
        let start = start.to_owned();
        let end = end.to_owned();
        self.with_conn_read(move |conn| {
            Self::query_gui_interaction_density_inner(conn, &start, &end)
        })
        .await
    }

    fn query_gui_interaction_density_inner(
        conn: &rusqlite::Connection,
        start: &str,
        end: &str,
    ) -> Result<Vec<(String, u32)>, StorageError> {
        let mut stmt = conn
            .prepare(
                "SELECT strftime('%Y-%m-%dT%H:00:00Z', timestamp) AS hour, COUNT(*) AS cnt
                 FROM gui_interactions
                 WHERE timestamp >= ?1 AND timestamp < ?2
                 GROUP BY hour
                 ORDER BY hour",
            )
            .map_err(|e| {
                StorageError::Internal(format!(
                    "Failed to prepare GUI interaction density query: {e}"
                ))
            })?;

        let rows: Vec<(String, u32)> = stmt
            .query_map(rusqlite::params![start, end], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
            })
            .map_err(|e| {
                StorageError::Internal(format!("GUI interaction density query failed: {e}"))
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(rows)
    }
}

/// Build an owned, `Send + 'static` copy of the borrowed interaction params so
/// the write can cross the `spawn_blocking` funnel boundary (ADR-026 PR-7).
fn owned_gui_interaction(input: &NewGuiInteraction<'_>) -> OwnedGuiInteraction {
    OwnedGuiInteraction {
        event_id: input.event_id.to_owned(),
        segment_id: input.segment_id.map(str::to_owned),
        timestamp: input.timestamp.to_owned(),
        element_text: input.element_text.map(str::to_owned),
        element_type: input.element_type.map(str::to_owned),
        interaction_type: input.interaction_type.to_owned(),
        bbox_json: input.bbox_json.map(str::to_owned),
        app_name: input.app_name.to_owned(),
        type_confidence: input.type_confidence,
    }
}
