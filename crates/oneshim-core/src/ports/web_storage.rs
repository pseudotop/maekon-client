//! Synchronous storage port for the local web dashboard (events, frames, metrics, tags, exports).
//!
//! The [`WebStorage`] supertrait composes focused sub-traits, each covering a
//! single storage concern.  Consumers that only need a subset of capabilities
//! can accept the narrow sub-trait instead of the full `WebStorage`.
//!
//! # Errors (applies to all sub-traits)
//! `CoreError::Storage` (wire: `storage.failed`) for every SQLite operation
//! (iter-47 mass fix pattern: prepare/query/execute/transaction/FTS5 match).
//! Consistent conventions across sub-traits:
//! - Get-style methods returning `Option<T>` use `Ok(None)` for not-found
//!   (tag_id, frame_id, date, suggestion_id).
//! - List-style methods use `Ok(Vec::new())` / `Ok(HashMap::new())` for
//!   empty results rather than an Err variant.
//! - Mutators returning `Result<bool, _>` (e.g., `update_tag`, `delete_tag`,
//!   `dismiss_unified_suggestion`) use `Ok(false)` to signal rowcount=0.
//! - `mark_suggestion_*` variants (rowcount=0 on unknown id) are Ok(()) —
//!   no distinct NotFound error is surfaced.
//! - Default-implementation sub-traits (`DigestStorage`, `GuiInteractionStorage`,
//!   `HabitStorage`, `SegmentQueryStorage`, `CoachingQueryStorage`) provide
//!   no-op/`Ok(vec![])` defaults for stores without those tables; only
//!   override implementations can surface Storage errors.

use chrono::{DateTime, Utc};

use crate::error::CoreError;
use crate::models::activity::SessionStats;
use crate::types::TimeWindow;
// (additional imports retained below)
use crate::models::daily_digest::DailyDigest;
use crate::models::storage_records::{
    DeletedRangeCounts, EventExportRecord, FocusInterruptionRecord, FocusWorkSessionRecord,
    FrameExportRecord, FrameRecord, FrameTagLinkRecord, GuiInteractionRecord, HourlyMetricsRecord,
    LocalSuggestionRecord, MetricExportRecord, NewGuiInteraction, SearchEventRow, SearchFrameRow,
    SegmentDetailRecord, SegmentSummaryRecord, StorageStatsSummaryRecord, SuggestionRecord,
    TagRecord,
};
use crate::models::work_session::FocusMetrics;
use crate::ports::annotation_storage::AnnotationStorage;
use crate::ports::storage::{MetricsStorage, StorageService};

// ---------------------------------------------------------------------------
// Sub-trait: TagStorage
// ---------------------------------------------------------------------------

/// CRUD operations for user-defined tags and frame-tag associations.
pub trait TagStorage: Send + Sync {
    fn get_all_tags(&self) -> Result<Vec<TagRecord>, CoreError>;
    fn get_tag(&self, tag_id: i64) -> Result<Option<TagRecord>, CoreError>;
    fn get_tag_ids_for_frames(
        &self,
        frame_ids: &[i64],
    ) -> Result<std::collections::HashMap<i64, Vec<i64>>, CoreError>;
    fn create_tag(&self, name: &str, color: &str) -> Result<TagRecord, CoreError>;
    fn update_tag(&self, tag_id: i64, name: &str, color: &str) -> Result<bool, CoreError>;
    fn delete_tag(&self, tag_id: i64) -> Result<bool, CoreError>;
    fn get_tags_for_frame(&self, frame_id: i64) -> Result<Vec<TagRecord>, CoreError>;
    fn add_tag_to_frame(&self, frame_id: i64, tag_id: i64) -> Result<(), CoreError>;
    fn remove_tag_from_frame(&self, frame_id: i64, tag_id: i64) -> Result<bool, CoreError>;
}

// ---------------------------------------------------------------------------
// Sub-trait: FrameQueryStorage
// ---------------------------------------------------------------------------

/// Read-only frame queries, counts, and full-text search.
pub trait FrameQueryStorage: Send + Sync {
    fn count_frames_in_range(&self, window: &TimeWindow) -> Result<u64, CoreError>;
    fn get_frames(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<FrameRecord>, CoreError>;
    fn get_frame_file_path(&self, frame_id: i64) -> Result<Option<String>, CoreError>;
    fn list_frame_file_paths_in_range(&self, window: &TimeWindow)
        -> Result<Vec<String>, CoreError>;

    fn count_search_frames(&self, count_sql: &str, pattern: Option<&str>)
        -> Result<u64, CoreError>;
    fn search_frames_with_sql(
        &self,
        select_sql: &str,
        pattern: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<SearchFrameRow>, CoreError>;
}

// ---------------------------------------------------------------------------
// Sub-trait: EventQueryStorage
// ---------------------------------------------------------------------------

/// Read-only event queries, counts, and full-text search.
pub trait EventQueryStorage: Send + Sync {
    fn count_events_in_range(&self, window: &TimeWindow) -> Result<u64, CoreError>;
    fn count_search_events(&self, pattern: &str) -> Result<u64, CoreError>;
    fn search_events(
        &self,
        pattern: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<SearchEventRow>, CoreError>;
}

// ---------------------------------------------------------------------------
// Sub-trait: StorageMaintenanceStorage
// ---------------------------------------------------------------------------

/// Storage statistics, range deletion, and full data wipe.
pub trait StorageMaintenanceStorage: Send + Sync {
    fn get_storage_stats_summary(&self) -> Result<StorageStatsSummaryRecord, CoreError>;

    #[allow(clippy::too_many_arguments)]
    fn delete_data_in_range(
        &self,
        window: &TimeWindow,
        delete_events: bool,
        delete_frames: bool,
        delete_metrics: bool,
        delete_processes: bool,
        delete_idle: bool,
    ) -> Result<DeletedRangeCounts, CoreError>;

    fn delete_all_data(&self) -> Result<(), CoreError>;
}

// ---------------------------------------------------------------------------
// Sub-trait: ActivityStatsStorage
// ---------------------------------------------------------------------------

/// Aggregated activity statistics (app durations, active time, session stats).
pub trait ActivityStatsStorage: Send + Sync {
    fn get_app_durations_by_date(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<(String, i64)>, CoreError>;
    fn get_daily_active_secs(&self, window: &TimeWindow) -> Result<Vec<(String, i64)>, CoreError>;
    fn list_session_stats(&self, limit: usize) -> Result<Vec<SessionStats>, CoreError>;
}

// ---------------------------------------------------------------------------
// Sub-trait: FocusQueryStorage
// ---------------------------------------------------------------------------

/// Focus metrics, work sessions, interruptions, and local suggestions.
pub trait FocusQueryStorage: Send + Sync {
    fn get_or_create_focus_metrics(&self, date: &str) -> Result<FocusMetrics, CoreError>;
    fn get_recent_focus_metrics(
        &self,
        days: usize,
    ) -> Result<Vec<(String, FocusMetrics)>, CoreError>;
    fn list_work_sessions(
        &self,
        from: &str,
        to: &str,
        limit: usize,
    ) -> Result<Vec<FocusWorkSessionRecord>, CoreError>;
    fn list_interruptions(
        &self,
        from: &str,
        to: &str,
        limit: usize,
    ) -> Result<Vec<FocusInterruptionRecord>, CoreError>;
    fn list_recent_local_suggestions(
        &self,
        cutoff: &str,
        limit: usize,
    ) -> Result<Vec<LocalSuggestionRecord>, CoreError>;
    fn mark_suggestion_shown(&self, suggestion_id: i64) -> Result<(), CoreError>;
    fn mark_suggestion_dismissed(&self, suggestion_id: i64) -> Result<(), CoreError>;
    fn mark_suggestion_acted(&self, suggestion_id: i64) -> Result<(), CoreError>;
}

// ---------------------------------------------------------------------------
// Sub-trait: SuggestionQueryStorage
// ---------------------------------------------------------------------------

/// Unified suggestion queries (V8 `suggestions` table).
pub trait SuggestionQueryStorage: Send + Sync {
    /// List unified suggestions from the V8 `suggestions` table, newest first.
    /// Only returns non-dismissed suggestions.
    fn list_suggestions(&self, limit: usize) -> Result<Vec<SuggestionRecord>, CoreError>;

    /// Dismiss a unified suggestion by its string `suggestion_id`.
    fn dismiss_unified_suggestion(&self, suggestion_id: &str) -> Result<bool, CoreError>;

    /// Check whether server-sourced (LLM_SERVER) suggestions exist within the
    /// given lookback window (in seconds). Used for server coexistence gating.
    fn has_recent_server_suggestions(&self, lookback_secs: u64) -> Result<bool, CoreError>;
}

// ---------------------------------------------------------------------------
// Sub-trait: DigestStorage
// ---------------------------------------------------------------------------

/// Daily and weekly digest persistence and retrieval.
pub trait DigestStorage: Send + Sync {
    /// Save a daily digest. Upserts by date.
    fn save_daily_digest(&self, _digest: &DailyDigest) -> Result<(), CoreError> {
        Ok(()) // No-op default — storage adapters override
    }

    /// Get the daily digest for a specific date (YYYY-MM-DD).
    fn get_daily_digest(&self, _date: &str) -> Result<Option<DailyDigest>, CoreError> {
        Ok(None)
    }

    /// List recent daily digests, newest first.
    fn list_daily_digests(&self, _limit: usize) -> Result<Vec<DailyDigest>, CoreError> {
        Ok(vec![])
    }

    /// Get activity segment summaries for a given date (YYYY-MM-DD).
    /// Used as input for daily digest generation.
    fn get_segments_for_date(&self, _date: &str) -> Result<Vec<SegmentSummaryRecord>, CoreError> {
        Ok(vec![])
    }

    /// List recent weekly digests, newest first.
    fn list_weekly_digests(
        &self,
        limit: usize,
    ) -> Result<Vec<crate::models::weekly_digest::WeeklyDigest>, CoreError>;

    /// Get the digest for the current week (if exists).
    fn get_current_week_digest(
        &self,
    ) -> Result<Option<crate::models::weekly_digest::WeeklyDigest>, CoreError>;

    /// Save a weekly digest. Upserts by week_start.
    fn save_weekly_digest(
        &self,
        digest: &crate::models::weekly_digest::WeeklyDigest,
    ) -> Result<(), CoreError>;
}

// ---------------------------------------------------------------------------
// Sub-trait: BackupStorage
// ---------------------------------------------------------------------------

/// Backup export/import operations (tags, frame-tags, events, frames, metrics).
pub trait BackupStorage: Send + Sync {
    fn list_backup_tags(&self) -> Result<Vec<TagRecord>, CoreError>;
    fn list_backup_frame_tags(&self) -> Result<Vec<FrameTagLinkRecord>, CoreError>;
    fn list_event_exports(&self, from: &str, to: &str)
        -> Result<Vec<EventExportRecord>, CoreError>;
    fn list_metric_exports(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<MetricExportRecord>, CoreError>;
    fn list_frame_exports(&self, from: &str, to: &str)
        -> Result<Vec<FrameExportRecord>, CoreError>;
    fn list_hourly_metrics_since(&self, from: &str) -> Result<Vec<HourlyMetricsRecord>, CoreError>;
    fn upsert_backup_tag(
        &self,
        id: i64,
        name: &str,
        color: &str,
        created_at: &str,
    ) -> Result<(), CoreError>;
    fn upsert_backup_frame_tag(
        &self,
        frame_id: i64,
        tag_id: i64,
        created_at: &str,
    ) -> Result<(), CoreError>;
    fn upsert_backup_event(
        &self,
        event_id: &str,
        event_type: &str,
        timestamp: &str,
        app_name: Option<&str>,
        window_title: Option<&str>,
    ) -> Result<(), CoreError>;
    #[allow(clippy::too_many_arguments)]
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
    ) -> Result<(), CoreError>;
}

// ---------------------------------------------------------------------------
// Sub-trait: GuiInteractionStorage
// ---------------------------------------------------------------------------

/// GUI interaction event persistence and queries.
pub trait GuiInteractionStorage: Send + Sync {
    /// Save a GUI interaction event to the gui_interactions table (V13).
    ///
    /// **Privacy contract**: Callers MUST apply PII filtering to `element_text`
    /// before calling this method. The storage adapter applies a basic email/phone
    /// scrub as defense-in-depth, but upstream filtering via `sanitize_title_with_level()`
    /// is the primary safeguard.
    fn save_gui_interaction(&self, _input: &NewGuiInteraction<'_>) -> Result<(), CoreError> {
        Ok(()) // No-op default — storage adapters override
    }

    /// List GUI interaction events for a given segment.
    fn list_gui_interactions_for_segment(
        &self,
        _segment_id: &str,
    ) -> Result<Vec<GuiInteractionRecord>, CoreError> {
        Ok(vec![])
    }

    /// Count GUI interactions per hour within a date range.
    /// Returns `(hour_string, count)` pairs sorted chronologically.
    fn query_gui_interaction_density(
        &self,
        _start: &str,
        _end: &str,
    ) -> Result<Vec<(String, u32)>, CoreError> {
        Ok(vec![])
    }
}

// ---------------------------------------------------------------------------
// Sub-trait: SegmentQueryStorage
// ---------------------------------------------------------------------------

/// Segment detail retrieval (enriches vector search results).
pub trait SegmentQueryStorage: Send + Sync {
    /// Retrieve segment details for the given segment IDs.
    /// Used to enrich vector search results with segment metadata.
    /// Default implementation returns an empty map (for stores without segment support).
    fn get_segment_details(
        &self,
        _segment_ids: &[String],
    ) -> Result<std::collections::HashMap<String, SegmentDetailRecord>, CoreError> {
        Ok(std::collections::HashMap::new())
    }
}

// ---------------------------------------------------------------------------
// Sub-trait: CoachingQueryStorage
// ---------------------------------------------------------------------------

/// Coaching event queries.
pub trait CoachingQueryStorage: Send + Sync {
    /// Query coaching events, newest first, with pagination.
    /// Default returns empty — storage adapters that support coaching tables override.
    fn query_coaching_events(
        &self,
        _limit: u32,
        _offset: u32,
    ) -> Result<Vec<crate::models::coaching::CoachingEventRow>, CoreError> {
        Ok(vec![])
    }

    /// Query coaching events shown on or after `since_date` (YYYY-MM-DD format).
    fn query_coaching_events_since(
        &self,
        _since_date: &str,
    ) -> Result<Vec<crate::models::coaching::CoachingEventRow>, CoreError> {
        Ok(vec![])
    }
}

// ---------------------------------------------------------------------------
// Sub-trait: HabitStorage
// ---------------------------------------------------------------------------

/// Habit streak persistence and queries for daily regime tracking.
pub trait HabitStorage: Send + Sync {
    /// Upsert a daily habit record for a regime.
    fn upsert_habit_streak(
        &self,
        _regime_label: &str,
        _date: &str,
        _minutes_logged: u32,
        _target_minutes: u32,
        _met: bool,
    ) -> Result<(), CoreError> {
        Ok(()) // No-op default — storage adapters override
    }

    /// Query habit streak rows for all regimes within the last `days` days.
    fn query_habit_streaks(
        &self,
        _days: u32,
    ) -> Result<Vec<crate::models::coaching::HabitStreakRow>, CoreError> {
        Ok(vec![])
    }
}

// ---------------------------------------------------------------------------
// Sub-trait: DashboardStreamingStorage
// ---------------------------------------------------------------------------

/// v2b dashboard streaming reads. Frame lookups hit the DB; Idle and
/// AiRuntimeStatus have no DB persistence so `fetch_dashboard_event_source`
/// is a Frame-only entry point — Idle / AiRuntimeStatus are served from
/// the RealtimeEvent payload carried on event_tx (see design §4 data flow).
pub trait DashboardStreamingStorage: Send + Sync {
    /// Aggregate a single MetricBucket from raw `system_metrics` rows in
    /// the half-open `[from, to)` window. Returns a zero-initialised
    /// bucket when the window is empty. Averages cpu_usage / memory_used
    /// and (future) sums keystroke / mouse-click counters.
    ///
    /// # Errors
    /// Returns `CoreError::Storage` on SQL / IO failure;
    /// `CoreError::Internal` on mutex-lock poisoning.
    fn aggregate_metrics_window(
        &self,
        from: chrono::DateTime<chrono::Utc>,
        to: chrono::DateTime<chrono::Utc>,
    ) -> Result<crate::models::dashboard_streaming::MetricBucketRecord, CoreError>;

    /// Fetch a canonical frames-table row for the event signal. Only
    /// DashboardEventSignal::Frame(id) is a real DB lookup; calling with
    /// any other variant is a bug (the v2b SubscribeEvents handler
    /// converts Idle / AiRuntimeStatus directly from the event payload).
    ///
    /// # Errors
    /// - `CoreError::NotFound` when the frame id is missing (defensive —
    ///   see design §5 event↔DB race).
    /// - `CoreError::Storage` on SQL / IO failure.
    /// - `CoreError::Internal` when called with a non-Frame signal.
    fn fetch_dashboard_event_source(
        &self,
        signal: &crate::models::dashboard_streaming::DashboardEventSignal,
    ) -> Result<crate::models::dashboard_streaming::DashboardEventRecord, CoreError>;
}

// ---------------------------------------------------------------------------
// Composed supertrait
// ---------------------------------------------------------------------------

/// Composed storage port for the local web dashboard.
///
/// Inherits from all focused sub-traits plus the base [`StorageService`] and
/// [`MetricsStorage`] ports.  Consumers that only need a narrow slice of
/// functionality can accept a sub-trait reference instead.
pub trait WebStorage:
    StorageService
    + MetricsStorage
    + TagStorage
    + FrameQueryStorage
    + EventQueryStorage
    + StorageMaintenanceStorage
    + ActivityStatsStorage
    + FocusQueryStorage
    + SuggestionQueryStorage
    + DigestStorage
    + BackupStorage
    + GuiInteractionStorage
    + SegmentQueryStorage
    + CoachingQueryStorage
    + HabitStorage
    + AnnotationStorage
    + DashboardStreamingStorage
    + Send
    + Sync
{
}

/// Blanket implementation: any type that satisfies all sub-traits automatically
/// implements `WebStorage`.
impl<T> WebStorage for T where
    T: StorageService
        + MetricsStorage
        + TagStorage
        + FrameQueryStorage
        + EventQueryStorage
        + StorageMaintenanceStorage
        + ActivityStatsStorage
        + FocusQueryStorage
        + SuggestionQueryStorage
        + DigestStorage
        + BackupStorage
        + GuiInteractionStorage
        + SegmentQueryStorage
        + CoachingQueryStorage
        + HabitStorage
        + AnnotationStorage
        + DashboardStreamingStorage
        + Send
        + Sync
{
}
