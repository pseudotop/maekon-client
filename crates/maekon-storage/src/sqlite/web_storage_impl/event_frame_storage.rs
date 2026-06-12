//! `EventQueryStorage`, `FrameQueryStorage`, `StorageMaintenanceStorage` implementations.
//!
//! Converted to `#[async_trait]` async per ADR-026 PR-4 (async storage
//! convergence). Each method delegates to a `*_async` inherent helper on
//! `SqliteStorage` that routes the SQLite work through the
//! `with_conn`/`with_conn_read` funnel (`spawn_blocking`), so the
//! single-connection `parking_lot` guard is acquired on a blocking-pool thread
//! and never held across an `.await` (#4928 erase barrier preserved). The
//! reads use `with_conn_read` (never skipped); `delete_data_in_range` uses the
//! `with_conn` write funnel (skips when `deletion_flag` is set, which is
//! harmless since a full erase already wipes every table); `delete_all_data`
//! is the erase body itself and therefore offloads its synchronous
//! `lock_for_erase` transaction directly onto `spawn_blocking` (it must NOT go
//! through `write_lock`, which would skip the wipe — see
//! `delete_all_data_async`).

use async_trait::async_trait;

use maekon_core::error::CoreError;
use maekon_core::models::storage_records::{
    DeletedRangeCounts, FrameRecord, SearchEventRow, SearchFrameRow, StorageStatsSummaryRecord,
};
use maekon_core::ports::web_storage::{
    EventQueryStorage, FrameQueryStorage, StorageMaintenanceStorage,
};
use maekon_core::types::TimeWindow;

use chrono::{DateTime, Utc};

use crate::sqlite::SqliteStorage;

// ---------------------------------------------------------------------------
// EventQueryStorage
// ---------------------------------------------------------------------------

#[async_trait]
impl EventQueryStorage for SqliteStorage {
    async fn count_events_in_range(&self, window: &TimeWindow) -> Result<u64, CoreError> {
        SqliteStorage::count_events_in_range_async(self, window)
            .await
            .map_err(Into::into)
    }

    async fn count_search_events(&self, pattern: &str) -> Result<u64, CoreError> {
        SqliteStorage::count_search_events_async(self, pattern)
            .await
            .map_err(Into::into)
    }

    async fn search_events(
        &self,
        pattern: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<SearchEventRow>, CoreError> {
        SqliteStorage::search_events_async(self, pattern, limit, offset)
            .await
            .map_err(Into::into)
    }
}

// ---------------------------------------------------------------------------
// FrameQueryStorage
// ---------------------------------------------------------------------------

#[async_trait]
impl FrameQueryStorage for SqliteStorage {
    async fn count_frames_in_range(&self, window: &TimeWindow) -> Result<u64, CoreError> {
        SqliteStorage::count_frames_in_range_async(self, window)
            .await
            .map_err(Into::into)
    }

    async fn get_frames(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<FrameRecord>, CoreError> {
        SqliteStorage::get_frames_async(self, from, to, limit)
            .await
            .map_err(Into::into)
    }

    async fn get_frame_file_path(&self, frame_id: i64) -> Result<Option<String>, CoreError> {
        SqliteStorage::get_frame_file_path_async(self, frame_id)
            .await
            .map_err(Into::into)
    }

    async fn list_all_frame_file_paths(&self) -> Result<Vec<String>, CoreError> {
        SqliteStorage::list_all_frame_file_paths_async(self)
            .await
            .map_err(Into::into)
    }

    async fn list_frame_file_paths_in_range(
        &self,
        window: &TimeWindow,
    ) -> Result<Vec<String>, CoreError> {
        SqliteStorage::list_frame_file_paths_in_range_async(self, window)
            .await
            .map_err(Into::into)
    }

    async fn count_search_frames(
        &self,
        count_sql: &str,
        pattern: Option<&str>,
    ) -> Result<u64, CoreError> {
        SqliteStorage::count_search_frames_async(self, count_sql, pattern)
            .await
            .map_err(Into::into)
    }

    async fn search_frames_with_sql(
        &self,
        select_sql: &str,
        pattern: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<SearchFrameRow>, CoreError> {
        SqliteStorage::search_frames_with_sql_async(self, select_sql, pattern, limit, offset)
            .await
            .map_err(Into::into)
    }
}

// ---------------------------------------------------------------------------
// StorageMaintenanceStorage
// ---------------------------------------------------------------------------

#[async_trait]
impl StorageMaintenanceStorage for SqliteStorage {
    async fn get_storage_stats_summary(&self) -> Result<StorageStatsSummaryRecord, CoreError> {
        SqliteStorage::get_storage_stats_summary_async(self)
            .await
            .map_err(Into::into)
    }

    #[allow(clippy::too_many_arguments)]
    async fn delete_data_in_range(
        &self,
        window: &TimeWindow,
        delete_events: bool,
        delete_frames: bool,
        delete_metrics: bool,
        delete_processes: bool,
        delete_idle: bool,
    ) -> Result<DeletedRangeCounts, CoreError> {
        SqliteStorage::delete_data_in_range_async(
            self,
            window,
            delete_events,
            delete_frames,
            delete_metrics,
            delete_processes,
            delete_idle,
        )
        .await
        .map_err(Into::into)
    }

    async fn delete_all_data(&self) -> Result<(), CoreError> {
        SqliteStorage::delete_all_data_async(self)
            .await
            .map_err(Into::into)
    }
}
