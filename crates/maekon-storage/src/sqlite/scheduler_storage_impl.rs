//! `SchedulerStorage` implementation for `SqliteStorage`.
//!
//! Relocated from `src-tauri/src/scheduler/config.rs` (#7731, ctd-W2 B4,
//! 2026-07-03): the port trait now lives in
//! `maekon_core::ports::scheduler_storage`, and this file replaces the binary
//! crate's ~160-line mechanical 1:1 forwarding shim (`impl SchedulerStorage
//! for SqliteStorage` in the binary, calling back into these same inherent
//! methods). Adding a new `SchedulerStorage` method now touches this file
//! only, instead of 3 coordinated edits split across the binary and this
//! crate.
//!
//! Every method below is a thin forward to an already-existing inherent
//! `SqliteStorage` method defined elsewhere in this crate (`frames.rs`,
//! `edge_intelligence/`, `web_storage_impl/`, `maintenance/`, `mod.rs`,
//! `habit_storage.rs`) — this file's only job is the trait-object seam plus
//! the `StorageError -> CoreError` conversion.

use chrono::{DateTime, Utc};

use maekon_core::error::CoreError;
use maekon_core::models::context::WindowBounds;
use maekon_core::models::daily_digest::DailyDigest;
use maekon_core::models::frame::FrameMetadata;
use maekon_core::models::storage_records::{
    EgressLedgerRecord, NewGuiInteraction, SegmentSummaryRecord,
};
use maekon_core::models::tiered_memory::SegmentSummary;
use maekon_core::models::weekly_digest::WeeklyDigest;
use maekon_core::ports::scheduler_storage::SchedulerStorage;

use crate::sqlite::SqliteStorage;

impl SchedulerStorage for SqliteStorage {
    fn save_frame_metadata_with_bounds(
        &self,
        metadata: &FrameMetadata,
        file_path: Option<&str>,
        ocr_text: Option<&str>,
        bounds: Option<&WindowBounds>,
    ) -> Result<i64, CoreError> {
        SqliteStorage::save_frame_metadata_with_bounds(self, metadata, file_path, ocr_text, bounds)
            .map_err(Into::into)
    }

    fn has_recent_server_suggestions(&self, lookback_secs: u64) -> Result<bool, CoreError> {
        SqliteStorage::has_recent_server_suggestions(self, lookback_secs).map_err(Into::into)
    }

    fn list_weekly_digests(&self, limit: usize) -> Result<Vec<WeeklyDigest>, CoreError> {
        // ADR-026 PR-6: `DigestStorage` is now async; this sync scheduler trait
        // resolves to the inherent synchronous `SqliteStorage` twin instead.
        SqliteStorage::list_weekly_digests(self, limit)
    }

    fn list_daily_digests(&self, limit: usize) -> Result<Vec<DailyDigest>, CoreError> {
        SqliteStorage::list_daily_digests(self, limit)
    }

    fn save_weekly_digest(&self, digest: &WeeklyDigest) -> Result<(), CoreError> {
        SqliteStorage::save_weekly_digest(self, digest)
    }

    fn list_segments_between(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<SegmentSummary>, CoreError> {
        SqliteStorage::list_segments_between(self, from, to).map_err(Into::into)
    }

    fn enforce_segment_retention(&self, max_days: u32) -> Result<usize, CoreError> {
        SqliteStorage::enforce_segment_retention(self, max_days).map_err(Into::into)
    }

    fn enforce_digest_retention(&self, max_weeks: u32) -> Result<usize, CoreError> {
        SqliteStorage::enforce_digest_retention(self, max_weeks).map_err(Into::into)
    }

    fn get_daily_digest(&self, date: &str) -> Result<Option<DailyDigest>, CoreError> {
        SqliteStorage::get_daily_digest(self, date)
    }

    fn save_daily_digest(&self, digest: &DailyDigest) -> Result<(), CoreError> {
        SqliteStorage::save_daily_digest(self, digest)
    }

    fn has_digest_processing_marker(
        &self,
        kind: &str,
        period_key: &str,
    ) -> Result<bool, CoreError> {
        SqliteStorage::has_digest_processing_marker(self, kind, period_key)
    }

    fn save_digest_processing_marker(
        &self,
        kind: &str,
        period_key: &str,
        completed_at: DateTime<Utc>,
    ) -> Result<(), CoreError> {
        SqliteStorage::save_digest_processing_marker(self, kind, period_key, completed_at)
    }

    fn get_segments_for_date(&self, date: &str) -> Result<Vec<SegmentSummaryRecord>, CoreError> {
        SqliteStorage::get_segments_for_date(self, date)
    }

    fn save_gui_interaction(&self, input: &NewGuiInteraction<'_>) -> Result<(), CoreError> {
        // ADR-026 PR-7: `GuiInteractionStorage` is now async; this sync scheduler
        // trait resolves to the inherent synchronous `SqliteStorage` twin instead.
        SqliteStorage::save_gui_interaction(self, input).map_err(Into::into)
    }

    fn enforce_all_retention(&self) -> Result<u64, CoreError> {
        SqliteStorage::enforce_all_retention(self).map_err(Into::into)
    }

    fn enforce_audit_retention(&self) -> Result<u64, CoreError> {
        SqliteStorage::enforce_audit_retention(self).map_err(Into::into)
    }

    fn gc_sync_tombstones(&self, data_retention_days: u32) -> Result<usize, CoreError> {
        SqliteStorage::gc_sync_tombstones(self, data_retention_days).map_err(Into::into)
    }

    fn wal_checkpoint_passive(&self) -> Result<(), CoreError> {
        SqliteStorage::wal_checkpoint_passive(self).map_err(Into::into)
    }

    fn maybe_vacuum(&self, threshold_percent: u64) -> Result<bool, CoreError> {
        SqliteStorage::maybe_vacuum(self, threshold_percent).map_err(Into::into)
    }

    fn fts_merge(&self, pages: u32) -> Result<(), CoreError> {
        SqliteStorage::fts_merge(self, pages).map_err(Into::into)
    }

    fn fts_optimize(&self) -> Result<(), CoreError> {
        SqliteStorage::fts_optimize(self).map_err(Into::into)
    }

    fn run_analyze(&self) -> Result<(), CoreError> {
        SqliteStorage::run_analyze(self).map_err(Into::into)
    }

    fn record_egress(&self, record: &EgressLedgerRecord) -> Result<(), CoreError> {
        SqliteStorage::record_egress(self, record).map_err(Into::into)
    }

    fn upsert_habit_streak(
        &self,
        regime_label: &str,
        date: &str,
        minutes_logged: u32,
        target_minutes: u32,
        met: bool,
    ) -> Result<(), CoreError> {
        // Forwards to the pre-existing inherent sync twin (habit_storage.rs);
        // callers offload via spawn_blocking (write_lock must not run inline
        // on the async monitor loop).
        SqliteStorage::upsert_habit_streak(
            self,
            regime_label,
            date,
            minutes_logged,
            target_minutes,
            met,
        )
        .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #5669: the SchedulerStorage seam must reach the real habit_streaks
    /// table — the widget reader (query_habit_streaks) must see the row the
    /// coaching loop writes through the trait object.
    #[test]
    fn upsert_habit_streak_round_trips_through_scheduler_storage_seam() {
        let storage = SqliteStorage::open_in_memory(30).expect("open_in_memory failed");
        // Same local-date key the coaching loop writes (must stay inside the
        // reader's `date >= date('now', '-N days')` window on any run date).
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let seam: &dyn SchedulerStorage = &storage;
        seam.upsert_habit_streak("Deep Work", &today, 75, 60, true)
            .expect("trait-object upsert must reach the inherent impl");

        let rows = storage
            .query_habit_streaks(7)
            .expect("query_habit_streaks failed");
        assert_eq!(rows.len(), 1, "the seam write must land in habit_streaks");
        assert_eq!(rows[0].regime_label, "Deep Work");
        assert_eq!(rows[0].date, today);
        assert_eq!(rows[0].minutes_logged, 75);
        assert_eq!(rows[0].target_minutes, 60);
        assert!(rows[0].met);
    }
}
