//! B3-7 test-only `FailingStorage` — a `WebStorage` wrapper around
//! `SqliteStorage` that selectively injects failures for specified
//! operations.  Delegates all other methods to the inner storage.
//!
//! This file is included via `#[path]` from the integration test file, which
//! is itself gated on `#[cfg(feature = "grpc-dashboard")]`. No inner `#![cfg]`
//! attribute is needed — the caller's gate is sufficient.
//!
//! Currently injectable faults:
//! - `start_idle_period` — returns `CoreError::Storage` when `fail_start_idle`
//!   is set (simulates DB write failure without killing the whole server).

// The delegation methods all use `.map_err(Into::into)` for consistency even
// when the error type is already `CoreError`. This is intentional boilerplate.
#![allow(clippy::useless_conversion)]

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use maekon_core::error::CoreError;
use maekon_core::models::activity::{IdlePeriod, ProcessSnapshot, SessionStats};
use maekon_core::models::annotation::FrameAnnotation;
use maekon_core::models::daily_digest::DailyDigest;
use maekon_core::models::dashboard_streaming::{
    DashboardEventRecord, DashboardEventSignal, MetricBucketRecord,
};
use maekon_core::models::event::Event;
use maekon_core::models::storage_records::{
    DeletedRangeCounts, EventExportRecord, FocusInterruptionRecord, FocusWorkSessionRecord,
    FrameExportRecord, FrameRecord, FrameTagLinkRecord, GuiInteractionRecord, HourlyMetricsRecord,
    LocalSuggestionRecord, MetricExportRecord, NewGuiInteraction, SearchEventRow, SearchFrameRow,
    SegmentSummaryRecord, StorageStatsSummaryRecord, SuggestionRecord, TagRecord,
};
use maekon_core::models::suggestion::Suggestion;
use maekon_core::models::system::SystemMetrics;
use maekon_core::models::tiered_memory::SegmentSummary;
use maekon_core::models::work_session::FocusMetrics;
use maekon_core::ports::annotation_storage::AnnotationStorage;
use maekon_core::ports::storage::{MetricsStorage, StorageService};
use maekon_core::ports::web_storage::{
    ActivityStatsStorage, BackupStorage, CoachingQueryStorage, DashboardStreamingStorage,
    DigestStorage, EventQueryStorage, FocusQueryStorage, FrameQueryStorage, GuiInteractionStorage,
    HabitStorage, SegmentQueryStorage, StorageMaintenanceStorage, SuggestionQueryStorage,
    TagStorage,
};
use maekon_core::types::TimeWindow;
use maekon_storage::sqlite::SqliteStorage;

/// Wraps `SqliteStorage` and injects configurable faults on specific methods.
/// All other methods delegate to the inner `SqliteStorage`.
pub struct FailingStorage {
    inner: Arc<SqliteStorage>,
    pub(crate) fail_start_idle: bool,
}

impl FailingStorage {
    pub fn new(inner: Arc<SqliteStorage>) -> Self {
        Self {
            inner,
            fail_start_idle: false,
        }
    }

    pub fn with_fail_start_idle(mut self) -> Self {
        self.fail_start_idle = true;
        self
    }
}

// ── StorageService ────────────────────────────────────────────────────────────

#[async_trait]
impl StorageService for FailingStorage {
    async fn save_event(&self, event: &Event) -> Result<(), CoreError> {
        self.inner.save_event(event).await
    }

    async fn get_events(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<Event>, CoreError> {
        self.inner.get_events(from, to, limit).await
    }

    async fn get_pending_events(&self, limit: usize) -> Result<Vec<Event>, CoreError> {
        self.inner.get_pending_events(limit).await
    }

    async fn mark_as_sent(&self, event_ids: &[String]) -> Result<(), CoreError> {
        self.inner.mark_as_sent(event_ids).await
    }

    async fn mark_unsent_as_sent_before(&self, before: DateTime<Utc>) -> Result<usize, CoreError> {
        self.inner.mark_unsent_as_sent_before(before).await
    }

    async fn enforce_retention(&self) -> Result<usize, CoreError> {
        self.inner.enforce_retention().await
    }

    async fn save_suggestion(&self, suggestion: &Suggestion) -> Result<(), CoreError> {
        self.inner.save_suggestion(suggestion).await
    }

    async fn save_activity_segment(&self, summary: &SegmentSummary) -> Result<(), CoreError> {
        self.inner.save_activity_segment(summary).await
    }

    async fn update_segment_llm_summary(
        &self,
        segment_id: &str,
        llm_summary: &str,
    ) -> Result<(), CoreError> {
        self.inner
            .update_segment_llm_summary(segment_id, llm_summary)
            .await
    }
}

// ── MetricsStorage ────────────────────────────────────────────────────────────

#[async_trait]
impl MetricsStorage for FailingStorage {
    async fn save_metrics(&self, metrics: &SystemMetrics) -> Result<(), CoreError> {
        self.inner.save_metrics(metrics).await
    }

    async fn get_metrics(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<SystemMetrics>, CoreError> {
        self.inner.get_metrics(from, to, limit).await
    }

    async fn aggregate_hourly_metrics(&self, hour: DateTime<Utc>) -> Result<(), CoreError> {
        self.inner.aggregate_hourly_metrics(hour).await
    }

    async fn cleanup_old_metrics(&self, before: DateTime<Utc>) -> Result<usize, CoreError> {
        self.inner.cleanup_old_metrics(before).await
    }

    async fn cleanup_old_hourly_metrics(&self, before: DateTime<Utc>) -> Result<usize, CoreError> {
        self.inner.cleanup_old_hourly_metrics(before).await
    }

    async fn save_process_snapshot(&self, snapshot: &ProcessSnapshot) -> Result<(), CoreError> {
        self.inner.save_process_snapshot(snapshot).await
    }

    async fn get_process_snapshots(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<ProcessSnapshot>, CoreError> {
        self.inner.get_process_snapshots(from, to, limit).await
    }

    async fn cleanup_old_process_snapshots(
        &self,
        before: DateTime<Utc>,
    ) -> Result<usize, CoreError> {
        self.inner.cleanup_old_process_snapshots(before).await
    }

    /// Injected fault: returns Storage error when `fail_start_idle` is set.
    async fn start_idle_period(&self, start_time: DateTime<Utc>) -> Result<i64, CoreError> {
        if self.fail_start_idle {
            return Err(CoreError::Storage {
                message: "injected: start_idle_period forced failure".to_string(),
                code: maekon_core::error_codes::StorageCode::Failed,
            });
        }
        self.inner.start_idle_period(start_time).await
    }

    async fn end_idle_period(&self, id: i64, end_time: DateTime<Utc>) -> Result<(), CoreError> {
        self.inner.end_idle_period(id, end_time).await
    }

    async fn get_ongoing_idle_period(&self) -> Result<Option<(i64, IdlePeriod)>, CoreError> {
        self.inner.get_ongoing_idle_period().await
    }

    async fn get_idle_periods(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<IdlePeriod>, CoreError> {
        self.inner.get_idle_periods(from, to).await
    }

    async fn cleanup_old_idle_periods(&self, before: DateTime<Utc>) -> Result<usize, CoreError> {
        self.inner.cleanup_old_idle_periods(before).await
    }

    async fn upsert_session(&self, stats: &SessionStats) -> Result<(), CoreError> {
        self.inner.upsert_session(stats).await
    }

    async fn get_session(&self, session_id: &str) -> Result<Option<SessionStats>, CoreError> {
        self.inner.get_session(session_id).await
    }

    async fn end_session(
        &self,
        session_id: &str,
        ended_at: DateTime<Utc>,
    ) -> Result<(), CoreError> {
        self.inner.end_session(session_id, ended_at).await
    }

    async fn increment_session_counters(
        &self,
        session_id: &str,
        events: u64,
        frames: u64,
        idle_secs: u64,
    ) -> Result<(), CoreError> {
        self.inner
            .increment_session_counters(session_id, events, frames, idle_secs)
            .await
    }
}

// ── TagStorage ────────────────────────────────────────────────────────────────

#[async_trait]
impl TagStorage for FailingStorage {
    async fn get_all_tags(&self) -> Result<Vec<TagRecord>, CoreError> {
        TagStorage::get_all_tags(&*self.inner)
            .await
            .map_err(Into::into)
    }

    async fn get_tag(&self, tag_id: i64) -> Result<Option<TagRecord>, CoreError> {
        TagStorage::get_tag(&*self.inner, tag_id)
            .await
            .map_err(Into::into)
    }

    async fn get_tag_ids_for_frames(
        &self,
        frame_ids: &[i64],
    ) -> Result<std::collections::HashMap<i64, Vec<i64>>, CoreError> {
        TagStorage::get_tag_ids_for_frames(&*self.inner, frame_ids)
            .await
            .map_err(Into::into)
    }

    async fn create_tag(&self, name: &str, color: &str) -> Result<TagRecord, CoreError> {
        TagStorage::create_tag(&*self.inner, name, color)
            .await
            .map_err(Into::into)
    }

    async fn update_tag(&self, tag_id: i64, name: &str, color: &str) -> Result<bool, CoreError> {
        TagStorage::update_tag(&*self.inner, tag_id, name, color)
            .await
            .map_err(Into::into)
    }

    async fn delete_tag(&self, tag_id: i64) -> Result<bool, CoreError> {
        TagStorage::delete_tag(&*self.inner, tag_id)
            .await
            .map_err(Into::into)
    }

    async fn get_tags_for_frame(&self, frame_id: i64) -> Result<Vec<TagRecord>, CoreError> {
        TagStorage::get_tags_for_frame(&*self.inner, frame_id)
            .await
            .map_err(Into::into)
    }

    async fn add_tag_to_frame(&self, frame_id: i64, tag_id: i64) -> Result<(), CoreError> {
        TagStorage::add_tag_to_frame(&*self.inner, frame_id, tag_id)
            .await
            .map_err(Into::into)
    }

    async fn remove_tag_from_frame(&self, frame_id: i64, tag_id: i64) -> Result<bool, CoreError> {
        TagStorage::remove_tag_from_frame(&*self.inner, frame_id, tag_id)
            .await
            .map_err(Into::into)
    }
}

// ── FrameQueryStorage ────────────────────────────────────────────────────────

#[async_trait]
impl FrameQueryStorage for FailingStorage {
    async fn count_frames_in_range(&self, window: &TimeWindow) -> Result<u64, CoreError> {
        FrameQueryStorage::count_frames_in_range(&*self.inner, window)
            .await
            .map_err(Into::into)
    }

    async fn get_frames(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<FrameRecord>, CoreError> {
        FrameQueryStorage::get_frames(&*self.inner, from, to, limit)
            .await
            .map_err(Into::into)
    }

    async fn get_frame_file_path(&self, frame_id: i64) -> Result<Option<String>, CoreError> {
        FrameQueryStorage::get_frame_file_path(&*self.inner, frame_id)
            .await
            .map_err(Into::into)
    }

    async fn list_all_frame_file_paths(&self) -> Result<Vec<String>, CoreError> {
        FrameQueryStorage::list_all_frame_file_paths(&*self.inner)
            .await
            .map_err(Into::into)
    }

    async fn list_frame_file_paths_in_range(
        &self,
        window: &TimeWindow,
    ) -> Result<Vec<String>, CoreError> {
        FrameQueryStorage::list_frame_file_paths_in_range(&*self.inner, window)
            .await
            .map_err(Into::into)
    }

    async fn count_search_frames(
        &self,
        count_sql: &str,
        pattern: Option<&str>,
    ) -> Result<u64, CoreError> {
        FrameQueryStorage::count_search_frames(&*self.inner, count_sql, pattern)
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
        FrameQueryStorage::search_frames_with_sql(&*self.inner, select_sql, pattern, limit, offset)
            .await
            .map_err(Into::into)
    }
}

// ── EventQueryStorage ────────────────────────────────────────────────────────

#[async_trait]
impl EventQueryStorage for FailingStorage {
    async fn count_events_in_range(&self, window: &TimeWindow) -> Result<u64, CoreError> {
        EventQueryStorage::count_events_in_range(&*self.inner, window)
            .await
            .map_err(Into::into)
    }

    async fn count_search_events(&self, pattern: &str) -> Result<u64, CoreError> {
        EventQueryStorage::count_search_events(&*self.inner, pattern)
            .await
            .map_err(Into::into)
    }

    async fn search_events(
        &self,
        pattern: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<SearchEventRow>, CoreError> {
        EventQueryStorage::search_events(&*self.inner, pattern, limit, offset)
            .await
            .map_err(Into::into)
    }
}

// ── StorageMaintenanceStorage ────────────────────────────────────────────────

#[async_trait]
impl StorageMaintenanceStorage for FailingStorage {
    async fn get_storage_stats_summary(&self) -> Result<StorageStatsSummaryRecord, CoreError> {
        StorageMaintenanceStorage::get_storage_stats_summary(&*self.inner)
            .await
            .map_err(Into::into)
    }

    async fn delete_data_in_range(
        &self,
        window: &TimeWindow,
        delete_events: bool,
        delete_frames: bool,
        delete_metrics: bool,
        delete_processes: bool,
        delete_idle: bool,
    ) -> Result<DeletedRangeCounts, CoreError> {
        StorageMaintenanceStorage::delete_data_in_range(
            &*self.inner,
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
        StorageMaintenanceStorage::delete_all_data(&*self.inner)
            .await
            .map_err(Into::into)
    }
}

// ── ActivityStatsStorage ─────────────────────────────────────────────────────

#[async_trait]
impl ActivityStatsStorage for FailingStorage {
    async fn get_app_durations_by_date(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<(String, i64)>, CoreError> {
        ActivityStatsStorage::get_app_durations_by_date(&*self.inner, from, to)
            .await
            .map_err(Into::into)
    }

    async fn get_daily_active_secs(
        &self,
        window: &TimeWindow,
    ) -> Result<Vec<(String, i64)>, CoreError> {
        ActivityStatsStorage::get_daily_active_secs(&*self.inner, window)
            .await
            .map_err(Into::into)
    }

    async fn list_session_stats(&self, limit: usize) -> Result<Vec<SessionStats>, CoreError> {
        ActivityStatsStorage::list_session_stats(&*self.inner, limit)
            .await
            .map_err(Into::into)
    }
}

// ── FocusQueryStorage ────────────────────────────────────────────────────────

#[async_trait]
impl FocusQueryStorage for FailingStorage {
    async fn get_or_create_focus_metrics(&self, date: &str) -> Result<FocusMetrics, CoreError> {
        FocusQueryStorage::get_or_create_focus_metrics(&*self.inner, date)
            .await
            .map_err(Into::into)
    }

    async fn get_recent_focus_metrics(
        &self,
        days: usize,
    ) -> Result<Vec<(String, FocusMetrics)>, CoreError> {
        FocusQueryStorage::get_recent_focus_metrics(&*self.inner, days)
            .await
            .map_err(Into::into)
    }

    async fn list_work_sessions(
        &self,
        from: &str,
        to: &str,
        limit: usize,
    ) -> Result<Vec<FocusWorkSessionRecord>, CoreError> {
        FocusQueryStorage::list_work_sessions(&*self.inner, from, to, limit)
            .await
            .map_err(Into::into)
    }

    async fn list_interruptions(
        &self,
        from: &str,
        to: &str,
        limit: usize,
    ) -> Result<Vec<FocusInterruptionRecord>, CoreError> {
        FocusQueryStorage::list_interruptions(&*self.inner, from, to, limit)
            .await
            .map_err(Into::into)
    }

    async fn list_recent_local_suggestions(
        &self,
        cutoff: &str,
        limit: usize,
    ) -> Result<Vec<LocalSuggestionRecord>, CoreError> {
        FocusQueryStorage::list_recent_local_suggestions(&*self.inner, cutoff, limit)
            .await
            .map_err(Into::into)
    }

    async fn mark_suggestion_shown(&self, suggestion_id: i64) -> Result<(), CoreError> {
        FocusQueryStorage::mark_suggestion_shown(&*self.inner, suggestion_id)
            .await
            .map_err(Into::into)
    }

    async fn mark_suggestion_dismissed(&self, suggestion_id: i64) -> Result<(), CoreError> {
        FocusQueryStorage::mark_suggestion_dismissed(&*self.inner, suggestion_id)
            .await
            .map_err(Into::into)
    }

    async fn mark_suggestion_acted(&self, suggestion_id: i64) -> Result<(), CoreError> {
        FocusQueryStorage::mark_suggestion_acted(&*self.inner, suggestion_id)
            .await
            .map_err(Into::into)
    }
}

// ── SuggestionQueryStorage ───────────────────────────────────────────────────

// ADR-026 PR-6: async sub-trait. Delegation is fully-qualified so the async
// trait method (not the synchronous inherent twin on `SqliteStorage`) is
// selected on the concrete `Arc<SqliteStorage>` inner.
#[async_trait]
impl SuggestionQueryStorage for FailingStorage {
    async fn list_suggestions(&self, limit: usize) -> Result<Vec<SuggestionRecord>, CoreError> {
        SuggestionQueryStorage::list_suggestions(&*self.inner, limit)
            .await
            .map_err(Into::into)
    }

    async fn dismiss_unified_suggestion(&self, suggestion_id: &str) -> Result<bool, CoreError> {
        SuggestionQueryStorage::dismiss_unified_suggestion(&*self.inner, suggestion_id)
            .await
            .map_err(Into::into)
    }

    async fn mark_unified_suggestion_shown(&self, suggestion_id: &str) -> Result<bool, CoreError> {
        SuggestionQueryStorage::mark_unified_suggestion_shown(&*self.inner, suggestion_id)
            .await
            .map_err(Into::into)
    }

    async fn mark_unified_suggestion_acted(&self, suggestion_id: &str) -> Result<bool, CoreError> {
        SuggestionQueryStorage::mark_unified_suggestion_acted(&*self.inner, suggestion_id)
            .await
            .map_err(Into::into)
    }

    async fn has_recent_server_suggestions(&self, lookback_secs: u64) -> Result<bool, CoreError> {
        SuggestionQueryStorage::has_recent_server_suggestions(&*self.inner, lookback_secs)
            .await
            .map_err(Into::into)
    }
}

// ── DigestStorage ────────────────────────────────────────────────────────────

#[async_trait]
impl DigestStorage for FailingStorage {
    async fn save_daily_digest(&self, digest: &DailyDigest) -> Result<(), CoreError> {
        DigestStorage::save_daily_digest(&*self.inner, digest)
            .await
            .map_err(Into::into)
    }

    async fn get_daily_digest(&self, date: &str) -> Result<Option<DailyDigest>, CoreError> {
        DigestStorage::get_daily_digest(&*self.inner, date)
            .await
            .map_err(Into::into)
    }

    async fn list_daily_digests(&self, limit: usize) -> Result<Vec<DailyDigest>, CoreError> {
        DigestStorage::list_daily_digests(&*self.inner, limit)
            .await
            .map_err(Into::into)
    }

    async fn get_segments_for_date(
        &self,
        date: &str,
    ) -> Result<Vec<SegmentSummaryRecord>, CoreError> {
        DigestStorage::get_segments_for_date(&*self.inner, date)
            .await
            .map_err(Into::into)
    }

    async fn list_weekly_digests(
        &self,
        limit: usize,
    ) -> Result<Vec<maekon_core::models::weekly_digest::WeeklyDigest>, CoreError> {
        DigestStorage::list_weekly_digests(&*self.inner, limit)
            .await
            .map_err(Into::into)
    }

    async fn get_current_week_digest(
        &self,
    ) -> Result<Option<maekon_core::models::weekly_digest::WeeklyDigest>, CoreError> {
        DigestStorage::get_current_week_digest(&*self.inner)
            .await
            .map_err(Into::into)
    }

    async fn save_weekly_digest(
        &self,
        digest: &maekon_core::models::weekly_digest::WeeklyDigest,
    ) -> Result<(), CoreError> {
        DigestStorage::save_weekly_digest(&*self.inner, digest)
            .await
            .map_err(Into::into)
    }
}

// ── BackupStorage ────────────────────────────────────────────────────────────

// ADR-026 PR-7: async sub-trait. Delegation is fully-qualified so the async
// trait method (not the synchronous inherent twin on `SqliteStorage`) is
// selected on the concrete `Arc<SqliteStorage>` inner.
#[async_trait]
impl BackupStorage for FailingStorage {
    async fn list_backup_tags(&self) -> Result<Vec<TagRecord>, CoreError> {
        BackupStorage::list_backup_tags(&*self.inner).await
    }

    async fn list_backup_frame_tags(&self) -> Result<Vec<FrameTagLinkRecord>, CoreError> {
        BackupStorage::list_backup_frame_tags(&*self.inner).await
    }

    async fn list_event_exports(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<EventExportRecord>, CoreError> {
        BackupStorage::list_event_exports(&*self.inner, from, to).await
    }

    async fn list_metric_exports(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<MetricExportRecord>, CoreError> {
        BackupStorage::list_metric_exports(&*self.inner, from, to).await
    }

    async fn list_frame_exports(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<FrameExportRecord>, CoreError> {
        BackupStorage::list_frame_exports(&*self.inner, from, to).await
    }

    async fn list_hourly_metrics_since(
        &self,
        from: &str,
    ) -> Result<Vec<HourlyMetricsRecord>, CoreError> {
        BackupStorage::list_hourly_metrics_since(&*self.inner, from).await
    }

    async fn upsert_backup_tag(
        &self,
        id: i64,
        name: &str,
        color: &str,
        created_at: &str,
    ) -> Result<(), CoreError> {
        BackupStorage::upsert_backup_tag(&*self.inner, id, name, color, created_at).await
    }

    async fn upsert_backup_frame_tag(
        &self,
        frame_id: i64,
        tag_id: i64,
        created_at: &str,
    ) -> Result<(), CoreError> {
        BackupStorage::upsert_backup_frame_tag(&*self.inner, frame_id, tag_id, created_at).await
    }

    async fn upsert_backup_event(
        &self,
        event_id: &str,
        event_type: &str,
        timestamp: &str,
        app_name: Option<&str>,
        window_title: Option<&str>,
    ) -> Result<(), CoreError> {
        BackupStorage::upsert_backup_event(
            &*self.inner,
            event_id,
            event_type,
            timestamp,
            app_name,
            window_title,
        )
        .await
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
        BackupStorage::upsert_backup_frame(
            &*self.inner,
            id,
            timestamp,
            trigger_type,
            app_name,
            window_title,
            importance,
            width,
            height,
            ocr_text,
        )
        .await
    }
}

// ── GuiInteractionStorage ────────────────────────────────────────────────────

#[async_trait]
impl GuiInteractionStorage for FailingStorage {
    async fn save_gui_interaction(&self, input: &NewGuiInteraction<'_>) -> Result<(), CoreError> {
        GuiInteractionStorage::save_gui_interaction(&*self.inner, input).await
    }

    async fn list_gui_interactions_for_segment(
        &self,
        segment_id: &str,
    ) -> Result<Vec<GuiInteractionRecord>, CoreError> {
        GuiInteractionStorage::list_gui_interactions_for_segment(&*self.inner, segment_id).await
    }

    async fn query_gui_interaction_density(
        &self,
        start: &str,
        end: &str,
    ) -> Result<Vec<(String, u32)>, CoreError> {
        GuiInteractionStorage::query_gui_interaction_density(&*self.inner, start, end).await
    }
}

// ── SegmentQueryStorage ──────────────────────────────────────────────────────

#[async_trait]
impl SegmentQueryStorage for FailingStorage {
    async fn get_segment_details(
        &self,
        segment_ids: &[String],
    ) -> Result<
        std::collections::HashMap<
            String,
            maekon_core::models::storage_records::SegmentDetailRecord,
        >,
        CoreError,
    > {
        SegmentQueryStorage::get_segment_details(&*self.inner, segment_ids).await
    }
}

// ── CoachingQueryStorage ─────────────────────────────────────────────────────

#[async_trait]
impl CoachingQueryStorage for FailingStorage {
    async fn query_coaching_events(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<maekon_core::models::coaching::CoachingEventRow>, CoreError> {
        // Fully-qualified to select the async trait method (SqliteStorage also has
        // a synchronous inherent twin of the same name).
        CoachingQueryStorage::query_coaching_events(&*self.inner, limit, offset).await
    }

    async fn query_coaching_events_since(
        &self,
        since_date: &str,
    ) -> Result<Vec<maekon_core::models::coaching::CoachingEventRow>, CoreError> {
        CoachingQueryStorage::query_coaching_events_since(&*self.inner, since_date).await
    }
}

// ── HabitStorage ─────────────────────────────────────────────────────────────

#[async_trait]
impl HabitStorage for FailingStorage {
    async fn upsert_habit_streak(
        &self,
        regime_label: &str,
        date: &str,
        minutes_logged: u32,
        target_minutes: u32,
        met: bool,
    ) -> Result<(), CoreError> {
        // Fully-qualified to select the async trait method (SqliteStorage also has
        // a synchronous inherent twin of the same name).
        HabitStorage::upsert_habit_streak(
            &*self.inner,
            regime_label,
            date,
            minutes_logged,
            target_minutes,
            met,
        )
        .await
    }

    async fn query_habit_streaks(
        &self,
        days: u32,
    ) -> Result<Vec<maekon_core::models::coaching::HabitStreakRow>, CoreError> {
        HabitStorage::query_habit_streaks(&*self.inner, days).await
    }
}

// ── AnnotationStorage ────────────────────────────────────────────────────────

#[async_trait]
impl AnnotationStorage for FailingStorage {
    async fn list_annotations(&self, frame_id: i64) -> Result<Vec<FrameAnnotation>, CoreError> {
        self.inner
            .list_annotations(frame_id)
            .await
            .map_err(Into::into)
    }

    async fn save_annotation(&self, annotation: &FrameAnnotation) -> Result<(), CoreError> {
        self.inner
            .save_annotation(annotation)
            .await
            .map_err(Into::into)
    }

    async fn delete_annotation(&self, annotation_id: &str) -> Result<(), CoreError> {
        self.inner
            .delete_annotation(annotation_id)
            .await
            .map_err(Into::into)
    }
}

// ── DashboardStreamingStorage ────────────────────────────────────────────────

#[async_trait]
impl DashboardStreamingStorage for FailingStorage {
    async fn aggregate_metrics_window(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<MetricBucketRecord, CoreError> {
        // `SqliteStorage` has no sync inherent twin for this method — the async
        // `DashboardStreamingStorage` trait method (ADR-026 PR-9) resolves
        // unambiguously on the inner `Arc<SqliteStorage>`.
        self.inner.aggregate_metrics_window(from, to).await
    }

    async fn fetch_dashboard_event_source(
        &self,
        signal: &DashboardEventSignal,
    ) -> Result<DashboardEventRecord, CoreError> {
        self.inner.fetch_dashboard_event_source(signal).await
    }
}

// ── WebStorage blanket impl fires automatically via the above. ────────────────
// (WebStorage is implemented for any T that satisfies all 17 sub-traits +
//  Send + Sync; FailingStorage satisfies all of them.)
