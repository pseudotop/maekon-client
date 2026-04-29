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
use oneshim_core::error::CoreError;
use oneshim_core::models::activity::{IdlePeriod, ProcessSnapshot, SessionStats};
use oneshim_core::models::annotation::FrameAnnotation;
use oneshim_core::models::daily_digest::DailyDigest;
use oneshim_core::models::dashboard_streaming::{
    DashboardEventRecord, DashboardEventSignal, MetricBucketRecord,
};
use oneshim_core::models::event::Event;
use oneshim_core::models::storage_records::{
    DeletedRangeCounts, EventExportRecord, FocusInterruptionRecord, FocusWorkSessionRecord,
    FrameExportRecord, FrameRecord, FrameTagLinkRecord, GuiInteractionRecord, HourlyMetricsRecord,
    LocalSuggestionRecord, MetricExportRecord, NewGuiInteraction, SearchEventRow, SearchFrameRow,
    SegmentSummaryRecord, StorageStatsSummaryRecord, SuggestionRecord, TagRecord,
};
use oneshim_core::models::suggestion::Suggestion;
use oneshim_core::models::system::SystemMetrics;
use oneshim_core::models::work_session::FocusMetrics;
use oneshim_core::ports::annotation_storage::AnnotationStorage;
use oneshim_core::ports::storage::{MetricsStorage, StorageService};
use oneshim_core::ports::web_storage::{
    ActivityStatsStorage, BackupStorage, CoachingQueryStorage, DashboardStreamingStorage,
    DigestStorage, EventQueryStorage, FocusQueryStorage, FrameQueryStorage, GuiInteractionStorage,
    HabitStorage, SegmentQueryStorage, StorageMaintenanceStorage, SuggestionQueryStorage,
    TagStorage,
};
use oneshim_core::types::TimeWindow;
use oneshim_storage::sqlite::SqliteStorage;

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
                code: oneshim_core::error_codes::StorageCode::Failed,
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

impl TagStorage for FailingStorage {
    fn get_all_tags(&self) -> Result<Vec<TagRecord>, CoreError> {
        self.inner.get_all_tags().map_err(Into::into)
    }

    fn get_tag(&self, tag_id: i64) -> Result<Option<TagRecord>, CoreError> {
        self.inner.get_tag(tag_id).map_err(Into::into)
    }

    fn get_tag_ids_for_frames(
        &self,
        frame_ids: &[i64],
    ) -> Result<std::collections::HashMap<i64, Vec<i64>>, CoreError> {
        self.inner
            .get_tag_ids_for_frames(frame_ids)
            .map_err(Into::into)
    }

    fn create_tag(&self, name: &str, color: &str) -> Result<TagRecord, CoreError> {
        self.inner.create_tag(name, color).map_err(Into::into)
    }

    fn update_tag(&self, tag_id: i64, name: &str, color: &str) -> Result<bool, CoreError> {
        self.inner
            .update_tag(tag_id, name, color)
            .map_err(Into::into)
    }

    fn delete_tag(&self, tag_id: i64) -> Result<bool, CoreError> {
        self.inner.delete_tag(tag_id).map_err(Into::into)
    }

    fn get_tags_for_frame(&self, frame_id: i64) -> Result<Vec<TagRecord>, CoreError> {
        self.inner.get_tags_for_frame(frame_id).map_err(Into::into)
    }

    fn add_tag_to_frame(&self, frame_id: i64, tag_id: i64) -> Result<(), CoreError> {
        self.inner
            .add_tag_to_frame(frame_id, tag_id)
            .map_err(Into::into)
    }

    fn remove_tag_from_frame(&self, frame_id: i64, tag_id: i64) -> Result<bool, CoreError> {
        self.inner
            .remove_tag_from_frame(frame_id, tag_id)
            .map_err(Into::into)
    }
}

// ── FrameQueryStorage ────────────────────────────────────────────────────────

impl FrameQueryStorage for FailingStorage {
    fn count_frames_in_range(&self, window: &TimeWindow) -> Result<u64, CoreError> {
        self.inner.count_frames_in_range(window).map_err(Into::into)
    }

    fn get_frames(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<FrameRecord>, CoreError> {
        self.inner.get_frames(from, to, limit).map_err(Into::into)
    }

    fn get_frame_file_path(&self, frame_id: i64) -> Result<Option<String>, CoreError> {
        self.inner.get_frame_file_path(frame_id).map_err(Into::into)
    }

    fn list_frame_file_paths_in_range(
        &self,
        window: &TimeWindow,
    ) -> Result<Vec<String>, CoreError> {
        self.inner
            .list_frame_file_paths_in_range(window)
            .map_err(Into::into)
    }

    fn count_search_frames(
        &self,
        count_sql: &str,
        pattern: Option<&str>,
    ) -> Result<u64, CoreError> {
        self.inner
            .count_search_frames(count_sql, pattern)
            .map_err(Into::into)
    }

    fn search_frames_with_sql(
        &self,
        select_sql: &str,
        pattern: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<SearchFrameRow>, CoreError> {
        self.inner
            .search_frames_with_sql(select_sql, pattern, limit, offset)
            .map_err(Into::into)
    }
}

// ── EventQueryStorage ────────────────────────────────────────────────────────

impl EventQueryStorage for FailingStorage {
    fn count_events_in_range(&self, window: &TimeWindow) -> Result<u64, CoreError> {
        self.inner.count_events_in_range(window).map_err(Into::into)
    }

    fn count_search_events(&self, pattern: &str) -> Result<u64, CoreError> {
        self.inner.count_search_events(pattern).map_err(Into::into)
    }

    fn search_events(
        &self,
        pattern: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<SearchEventRow>, CoreError> {
        self.inner
            .search_events(pattern, limit, offset)
            .map_err(Into::into)
    }
}

// ── StorageMaintenanceStorage ────────────────────────────────────────────────

impl StorageMaintenanceStorage for FailingStorage {
    fn get_storage_stats_summary(&self) -> Result<StorageStatsSummaryRecord, CoreError> {
        self.inner.get_storage_stats_summary().map_err(Into::into)
    }

    fn delete_data_in_range(
        &self,
        window: &TimeWindow,
        delete_events: bool,
        delete_frames: bool,
        delete_metrics: bool,
        delete_processes: bool,
        delete_idle: bool,
    ) -> Result<DeletedRangeCounts, CoreError> {
        self.inner
            .delete_data_in_range(
                window,
                delete_events,
                delete_frames,
                delete_metrics,
                delete_processes,
                delete_idle,
            )
            .map_err(Into::into)
    }

    fn delete_all_data(&self) -> Result<(), CoreError> {
        self.inner.delete_all_data().map_err(Into::into)
    }
}

// ── ActivityStatsStorage ─────────────────────────────────────────────────────

impl ActivityStatsStorage for FailingStorage {
    fn get_app_durations_by_date(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<(String, i64)>, CoreError> {
        self.inner
            .get_app_durations_by_date(from, to)
            .map_err(Into::into)
    }

    fn get_daily_active_secs(&self, window: &TimeWindow) -> Result<Vec<(String, i64)>, CoreError> {
        self.inner.get_daily_active_secs(window).map_err(Into::into)
    }

    fn list_session_stats(&self, limit: usize) -> Result<Vec<SessionStats>, CoreError> {
        self.inner.list_session_stats(limit).map_err(Into::into)
    }
}

// ── FocusQueryStorage ────────────────────────────────────────────────────────

impl FocusQueryStorage for FailingStorage {
    fn get_or_create_focus_metrics(&self, date: &str) -> Result<FocusMetrics, CoreError> {
        self.inner
            .get_or_create_focus_metrics(date)
            .map_err(Into::into)
    }

    fn get_recent_focus_metrics(
        &self,
        days: usize,
    ) -> Result<Vec<(String, FocusMetrics)>, CoreError> {
        self.inner
            .get_recent_focus_metrics(days)
            .map_err(Into::into)
    }

    fn list_work_sessions(
        &self,
        from: &str,
        to: &str,
        limit: usize,
    ) -> Result<Vec<FocusWorkSessionRecord>, CoreError> {
        self.inner
            .list_work_sessions(from, to, limit)
            .map_err(Into::into)
    }

    fn list_interruptions(
        &self,
        from: &str,
        to: &str,
        limit: usize,
    ) -> Result<Vec<FocusInterruptionRecord>, CoreError> {
        self.inner
            .list_interruptions(from, to, limit)
            .map_err(Into::into)
    }

    fn list_recent_local_suggestions(
        &self,
        cutoff: &str,
        limit: usize,
    ) -> Result<Vec<LocalSuggestionRecord>, CoreError> {
        self.inner
            .list_recent_local_suggestions(cutoff, limit)
            .map_err(Into::into)
    }

    fn mark_suggestion_shown(&self, suggestion_id: i64) -> Result<(), CoreError> {
        self.inner
            .mark_suggestion_shown(suggestion_id)
            .map_err(Into::into)
    }

    fn mark_suggestion_dismissed(&self, suggestion_id: i64) -> Result<(), CoreError> {
        self.inner
            .mark_suggestion_dismissed(suggestion_id)
            .map_err(Into::into)
    }

    fn mark_suggestion_acted(&self, suggestion_id: i64) -> Result<(), CoreError> {
        self.inner
            .mark_suggestion_acted(suggestion_id)
            .map_err(Into::into)
    }
}

// ── SuggestionQueryStorage ───────────────────────────────────────────────────

impl SuggestionQueryStorage for FailingStorage {
    fn list_suggestions(&self, limit: usize) -> Result<Vec<SuggestionRecord>, CoreError> {
        self.inner.list_suggestions(limit).map_err(Into::into)
    }

    fn dismiss_unified_suggestion(&self, suggestion_id: &str) -> Result<bool, CoreError> {
        self.inner
            .dismiss_unified_suggestion(suggestion_id)
            .map_err(Into::into)
    }

    fn has_recent_server_suggestions(&self, lookback_secs: u64) -> Result<bool, CoreError> {
        self.inner
            .has_recent_server_suggestions(lookback_secs)
            .map_err(Into::into)
    }
}

// ── DigestStorage ────────────────────────────────────────────────────────────

impl DigestStorage for FailingStorage {
    fn save_daily_digest(&self, digest: &DailyDigest) -> Result<(), CoreError> {
        self.inner.save_daily_digest(digest).map_err(Into::into)
    }

    fn get_daily_digest(&self, date: &str) -> Result<Option<DailyDigest>, CoreError> {
        self.inner.get_daily_digest(date).map_err(Into::into)
    }

    fn list_daily_digests(&self, limit: usize) -> Result<Vec<DailyDigest>, CoreError> {
        self.inner.list_daily_digests(limit).map_err(Into::into)
    }

    fn get_segments_for_date(&self, date: &str) -> Result<Vec<SegmentSummaryRecord>, CoreError> {
        self.inner.get_segments_for_date(date).map_err(Into::into)
    }

    fn list_weekly_digests(
        &self,
        limit: usize,
    ) -> Result<Vec<oneshim_core::models::weekly_digest::WeeklyDigest>, CoreError> {
        self.inner.list_weekly_digests(limit).map_err(Into::into)
    }

    fn get_current_week_digest(
        &self,
    ) -> Result<Option<oneshim_core::models::weekly_digest::WeeklyDigest>, CoreError> {
        self.inner.get_current_week_digest().map_err(Into::into)
    }

    fn save_weekly_digest(
        &self,
        digest: &oneshim_core::models::weekly_digest::WeeklyDigest,
    ) -> Result<(), CoreError> {
        self.inner.save_weekly_digest(digest).map_err(Into::into)
    }
}

// ── BackupStorage ────────────────────────────────────────────────────────────

impl BackupStorage for FailingStorage {
    fn list_backup_tags(&self) -> Result<Vec<TagRecord>, CoreError> {
        self.inner.list_backup_tags().map_err(Into::into)
    }

    fn list_backup_frame_tags(&self) -> Result<Vec<FrameTagLinkRecord>, CoreError> {
        self.inner.list_backup_frame_tags().map_err(Into::into)
    }

    fn list_event_exports(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<EventExportRecord>, CoreError> {
        self.inner.list_event_exports(from, to).map_err(Into::into)
    }

    fn list_metric_exports(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<MetricExportRecord>, CoreError> {
        self.inner.list_metric_exports(from, to).map_err(Into::into)
    }

    fn list_frame_exports(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<FrameExportRecord>, CoreError> {
        self.inner.list_frame_exports(from, to).map_err(Into::into)
    }

    fn list_hourly_metrics_since(&self, from: &str) -> Result<Vec<HourlyMetricsRecord>, CoreError> {
        self.inner
            .list_hourly_metrics_since(from)
            .map_err(Into::into)
    }

    fn upsert_backup_tag(
        &self,
        id: i64,
        name: &str,
        color: &str,
        created_at: &str,
    ) -> Result<(), CoreError> {
        self.inner
            .upsert_backup_tag(id, name, color, created_at)
            .map_err(Into::into)
    }

    fn upsert_backup_frame_tag(
        &self,
        frame_id: i64,
        tag_id: i64,
        created_at: &str,
    ) -> Result<(), CoreError> {
        self.inner
            .upsert_backup_frame_tag(frame_id, tag_id, created_at)
            .map_err(Into::into)
    }

    fn upsert_backup_event(
        &self,
        event_id: &str,
        event_type: &str,
        timestamp: &str,
        app_name: Option<&str>,
        window_title: Option<&str>,
    ) -> Result<(), CoreError> {
        self.inner
            .upsert_backup_event(event_id, event_type, timestamp, app_name, window_title)
            .map_err(Into::into)
    }

    fn upsert_backup_frame(
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
        self.inner
            .upsert_backup_frame(
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
            .map_err(Into::into)
    }
}

// ── GuiInteractionStorage ────────────────────────────────────────────────────

impl GuiInteractionStorage for FailingStorage {
    fn save_gui_interaction(&self, input: &NewGuiInteraction<'_>) -> Result<(), CoreError> {
        self.inner.save_gui_interaction(input).map_err(Into::into)
    }

    fn list_gui_interactions_for_segment(
        &self,
        segment_id: &str,
    ) -> Result<Vec<GuiInteractionRecord>, CoreError> {
        self.inner
            .list_gui_interactions_for_segment(segment_id)
            .map_err(Into::into)
    }

    fn query_gui_interaction_density(
        &self,
        start: &str,
        end: &str,
    ) -> Result<Vec<(String, u32)>, CoreError> {
        self.inner
            .query_gui_interaction_density(start, end)
            .map_err(Into::into)
    }
}

// ── SegmentQueryStorage ──────────────────────────────────────────────────────

impl SegmentQueryStorage for FailingStorage {
    fn get_segment_details(
        &self,
        segment_ids: &[String],
    ) -> Result<
        std::collections::HashMap<
            String,
            oneshim_core::models::storage_records::SegmentDetailRecord,
        >,
        CoreError,
    > {
        self.inner
            .get_segment_details(segment_ids)
            .map_err(Into::into)
    }
}

// ── CoachingQueryStorage ─────────────────────────────────────────────────────

impl CoachingQueryStorage for FailingStorage {
    fn query_coaching_events(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<oneshim_core::models::coaching::CoachingEventRow>, CoreError> {
        self.inner
            .query_coaching_events(limit, offset)
            .map_err(Into::into)
    }

    fn query_coaching_events_since(
        &self,
        since_date: &str,
    ) -> Result<Vec<oneshim_core::models::coaching::CoachingEventRow>, CoreError> {
        self.inner
            .query_coaching_events_since(since_date)
            .map_err(Into::into)
    }
}

// ── HabitStorage ─────────────────────────────────────────────────────────────

impl HabitStorage for FailingStorage {
    fn upsert_habit_streak(
        &self,
        regime_label: &str,
        date: &str,
        minutes_logged: u32,
        target_minutes: u32,
        met: bool,
    ) -> Result<(), CoreError> {
        self.inner
            .upsert_habit_streak(regime_label, date, minutes_logged, target_minutes, met)
            .map_err(Into::into)
    }

    fn query_habit_streaks(
        &self,
        days: u32,
    ) -> Result<Vec<oneshim_core::models::coaching::HabitStreakRow>, CoreError> {
        self.inner.query_habit_streaks(days).map_err(Into::into)
    }
}

// ── AnnotationStorage ────────────────────────────────────────────────────────

impl AnnotationStorage for FailingStorage {
    fn list_annotations(&self, frame_id: i64) -> Result<Vec<FrameAnnotation>, CoreError> {
        self.inner.list_annotations(frame_id).map_err(Into::into)
    }

    fn save_annotation(&self, annotation: &FrameAnnotation) -> Result<(), CoreError> {
        self.inner.save_annotation(annotation).map_err(Into::into)
    }

    fn delete_annotation(&self, annotation_id: &str) -> Result<(), CoreError> {
        self.inner
            .delete_annotation(annotation_id)
            .map_err(Into::into)
    }
}

// ── DashboardStreamingStorage ────────────────────────────────────────────────

impl DashboardStreamingStorage for FailingStorage {
    fn aggregate_metrics_window(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<MetricBucketRecord, CoreError> {
        self.inner
            .aggregate_metrics_window(from, to)
            .map_err(Into::into)
    }

    fn fetch_dashboard_event_source(
        &self,
        signal: &DashboardEventSignal,
    ) -> Result<DashboardEventRecord, CoreError> {
        self.inner
            .fetch_dashboard_event_source(signal)
            .map_err(Into::into)
    }
}

// ── WebStorage blanket impl fires automatically via the above. ────────────────
// (WebStorage is implemented for any T that satisfies all 17 sub-traits +
//  Send + Sync; FailingStorage satisfies all of them.)
