//! `TagStorage`, `ActivityStatsStorage`, `FocusQueryStorage` implementations.
//!
//! Converted to `#[async_trait]` async per ADR-026 PR-5 (async storage
//! convergence). Each method delegates to a `*_async` inherent helper on
//! `SqliteStorage` that routes the SQLite work through the
//! `with_conn`/`with_conn_read` funnel (`spawn_blocking`), so the
//! single-connection `parking_lot` guard is acquired on a blocking-pool thread
//! and never held across an `.await` (#4928 erase barrier preserved). Reads use
//! `with_conn_read` (never skipped); writes use the `with_conn` write funnel
//! (skipped when `deletion_flag`/`erasing` is set).

use async_trait::async_trait;

use maekon_core::error::CoreError;
use maekon_core::models::activity::SessionStats;
use maekon_core::models::storage_records::{
    FocusInterruptionRecord, FocusWorkSessionRecord, LocalSuggestionRecord, TagRecord,
};
use maekon_core::models::work_session::FocusMetrics;
use maekon_core::ports::web_storage::{ActivityStatsStorage, FocusQueryStorage, TagStorage};
use maekon_core::types::TimeWindow;

use crate::sqlite::SqliteStorage;

// ---------------------------------------------------------------------------
// TagStorage
// ---------------------------------------------------------------------------

#[async_trait]
impl TagStorage for SqliteStorage {
    async fn get_all_tags(&self) -> Result<Vec<TagRecord>, CoreError> {
        SqliteStorage::get_all_tags_async(self)
            .await
            .map_err(Into::into)
    }

    async fn get_tag(&self, tag_id: i64) -> Result<Option<TagRecord>, CoreError> {
        SqliteStorage::get_tag_async(self, tag_id)
            .await
            .map_err(Into::into)
    }

    async fn get_tag_ids_for_frames(
        &self,
        frame_ids: &[i64],
    ) -> Result<std::collections::HashMap<i64, Vec<i64>>, CoreError> {
        SqliteStorage::get_tag_ids_for_frames_async(self, frame_ids)
            .await
            .map_err(Into::into)
    }

    async fn create_tag(&self, name: &str, color: &str) -> Result<TagRecord, CoreError> {
        SqliteStorage::create_tag_async(self, name, color)
            .await
            .map_err(Into::into)
    }

    async fn update_tag(&self, tag_id: i64, name: &str, color: &str) -> Result<bool, CoreError> {
        SqliteStorage::update_tag_async(self, tag_id, name, color)
            .await
            .map_err(Into::into)
    }

    async fn delete_tag(&self, tag_id: i64) -> Result<bool, CoreError> {
        SqliteStorage::delete_tag_async(self, tag_id)
            .await
            .map_err(Into::into)
    }

    async fn get_tags_for_frame(&self, frame_id: i64) -> Result<Vec<TagRecord>, CoreError> {
        SqliteStorage::get_tags_for_frame_async(self, frame_id)
            .await
            .map_err(Into::into)
    }

    async fn add_tag_to_frame(&self, frame_id: i64, tag_id: i64) -> Result<(), CoreError> {
        SqliteStorage::add_tag_to_frame_async(self, frame_id, tag_id)
            .await
            .map_err(Into::into)
    }

    async fn remove_tag_from_frame(&self, frame_id: i64, tag_id: i64) -> Result<bool, CoreError> {
        SqliteStorage::remove_tag_from_frame_async(self, frame_id, tag_id)
            .await
            .map_err(Into::into)
    }
}

// ---------------------------------------------------------------------------
// ActivityStatsStorage
// ---------------------------------------------------------------------------

#[async_trait]
impl ActivityStatsStorage for SqliteStorage {
    async fn get_app_durations_by_date(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<(String, i64)>, CoreError> {
        SqliteStorage::get_app_durations_by_date_async(self, from, to)
            .await
            .map_err(Into::into)
    }

    async fn get_daily_active_secs(
        &self,
        window: &TimeWindow,
    ) -> Result<Vec<(String, i64)>, CoreError> {
        SqliteStorage::get_daily_active_secs_async(self, window)
            .await
            .map_err(Into::into)
    }

    async fn list_session_stats(&self, limit: usize) -> Result<Vec<SessionStats>, CoreError> {
        SqliteStorage::list_session_stats_async(self, limit)
            .await
            .map_err(Into::into)
    }
}

// ---------------------------------------------------------------------------
// FocusQueryStorage
// ---------------------------------------------------------------------------

#[async_trait]
impl FocusQueryStorage for SqliteStorage {
    async fn get_or_create_focus_metrics(&self, date: &str) -> Result<FocusMetrics, CoreError> {
        SqliteStorage::get_or_create_focus_metrics_async(self, date)
            .await
            .map_err(Into::into)
    }

    async fn get_recent_focus_metrics(
        &self,
        days: usize,
    ) -> Result<Vec<(String, FocusMetrics)>, CoreError> {
        SqliteStorage::get_recent_focus_metrics_async(self, days)
            .await
            .map_err(Into::into)
    }

    async fn list_work_sessions(
        &self,
        from: &str,
        to: &str,
        limit: usize,
    ) -> Result<Vec<FocusWorkSessionRecord>, CoreError> {
        SqliteStorage::list_work_sessions_async(self, from, to, limit)
            .await
            .map_err(Into::into)
    }

    async fn list_interruptions(
        &self,
        from: &str,
        to: &str,
        limit: usize,
    ) -> Result<Vec<FocusInterruptionRecord>, CoreError> {
        SqliteStorage::list_interruptions_async(self, from, to, limit)
            .await
            .map_err(Into::into)
    }

    async fn list_recent_local_suggestions(
        &self,
        cutoff: &str,
        limit: usize,
    ) -> Result<Vec<LocalSuggestionRecord>, CoreError> {
        SqliteStorage::list_recent_local_suggestions_async(self, cutoff, limit)
            .await
            .map_err(Into::into)
    }

    async fn mark_suggestion_shown(&self, suggestion_id: i64) -> Result<(), CoreError> {
        SqliteStorage::mark_suggestion_shown_async(self, suggestion_id)
            .await
            .map_err(Into::into)
    }

    async fn mark_suggestion_dismissed(&self, suggestion_id: i64) -> Result<(), CoreError> {
        SqliteStorage::mark_suggestion_dismissed_async(self, suggestion_id)
            .await
            .map_err(Into::into)
    }

    async fn mark_suggestion_acted(&self, suggestion_id: i64) -> Result<(), CoreError> {
        SqliteStorage::mark_suggestion_acted_async(self, suggestion_id)
            .await
            .map_err(Into::into)
    }
}
