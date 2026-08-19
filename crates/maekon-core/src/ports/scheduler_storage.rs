//! Scheduler-facing storage port — the sync SQLite surface the background
//! scheduler loops call through `Arc<dyn SchedulerStorage>`.
//!
//! Relocated from `src-tauri` (#7731, ctd-W2 B4): previously `SchedulerStorage`
//! was defined directly in the binary crate's `scheduler/config.rs`, alongside
//! a ~160-line mechanical 1:1 forwarding `impl SchedulerStorage for
//! SqliteStorage` that just called the already-existing inherent
//! `SqliteStorage` methods. Every new storage need therefore required 3
//! coordinated edits (this trait + that forwarding impl, both in the binary,
//! plus the real inherent method in `maekon-storage`). Moving the trait here
//! (a port, per Hexagonal Architecture) and implementing it directly on
//! `SqliteStorage` in `maekon-storage` (see
//! `maekon_storage::sqlite::scheduler_storage_impl`) collapses that to a
//! single crate: the trait method and its inherent SQL body now live next to
//! each other.
//!
//! Extends [`MetricsStorage`] (also a port) rather than re-declaring its
//! methods. Deliberately synchronous even though sibling storage ports
//! (`DigestStorage`, `GuiInteractionStorage` under [`crate::ports::web_storage`])
//! converged to `#[async_trait]` under ADR-026 — the scheduler loop call sites
//! that hold this trait object still dispatch synchronously (offloading to
//! `spawn_blocking` at the call site, not inside the trait), so `SqliteStorage`
//! keeps a sync inherent twin for the handful of methods shared with those
//! async ports. See the per-method ADR-026 notes on the `impl` in
//! `maekon-storage` for the specific twins.

use chrono::{DateTime, Utc};

use crate::error::CoreError;
use crate::models::context::WindowBounds;
use crate::models::daily_digest::DailyDigest;
use crate::models::frame::FrameMetadata;
use crate::models::storage_records::{EgressLedgerRecord, NewGuiInteraction, SegmentSummaryRecord};
use crate::models::tiered_memory::SegmentSummary;
use crate::models::weekly_digest::WeeklyDigest;
use crate::ports::storage::MetricsStorage;

/// Local SQLite surface consumed by the scheduler's background loops.
///
/// # Errors
/// All methods return `CoreError::Storage` (wire: `storage.failed`) on
/// SQLite failures (lock contention, constraint violation, disk I/O).
pub trait SchedulerStorage: MetricsStorage + Send + Sync {
    fn save_frame_metadata_with_bounds(
        &self,
        metadata: &FrameMetadata,
        file_path: Option<&str>,
        ocr_text: Option<&str>,
        bounds: Option<&WindowBounds>,
    ) -> Result<i64, CoreError>;

    /// Check whether server-sourced suggestions exist within the given lookback
    /// window (in seconds). Used by the analysis loop to suppress local LLM
    /// analysis when the server is actively providing suggestions.
    fn has_recent_server_suggestions(&self, lookback_secs: u64) -> Result<bool, CoreError>;

    /// List recent weekly digests, newest first.
    fn list_weekly_digests(&self, limit: usize) -> Result<Vec<WeeklyDigest>, CoreError>;

    /// List recent daily digests, newest first.
    fn list_daily_digests(&self, limit: usize) -> Result<Vec<DailyDigest>, CoreError>;

    /// Save a weekly digest. Upserts by week_start.
    fn save_weekly_digest(&self, digest: &WeeklyDigest) -> Result<(), CoreError>;

    /// List closed segments whose time range falls within [from, to].
    fn list_segments_between(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<SegmentSummary>, CoreError>;

    /// Delete activity segments older than `max_days`. Returns the count of deleted rows.
    fn enforce_segment_retention(&self, max_days: u32) -> Result<usize, CoreError>;

    /// Delete weekly digests older than `max_weeks`. Returns the count of deleted rows.
    fn enforce_digest_retention(&self, max_weeks: u32) -> Result<usize, CoreError>;

    /// Get a cached daily digest by date (YYYY-MM-DD).
    fn get_daily_digest(&self, date: &str) -> Result<Option<DailyDigest>, CoreError>;

    /// Save a daily digest. Upserts by date.
    fn save_daily_digest(&self, digest: &DailyDigest) -> Result<(), CoreError>;

    /// Return whether downstream processing already completed for a digest period.
    fn has_digest_processing_marker(&self, kind: &str, period_key: &str)
        -> Result<bool, CoreError>;

    /// Mark downstream processing complete for a digest period.
    fn save_digest_processing_marker(
        &self,
        kind: &str,
        period_key: &str,
        completed_at: DateTime<Utc>,
    ) -> Result<(), CoreError>;

    /// Get activity segment summary records for a given date (YYYY-MM-DD).
    fn get_segments_for_date(&self, date: &str) -> Result<Vec<SegmentSummaryRecord>, CoreError>;

    /// Save a GUI interaction event (delegates to WebStorage V13 table).
    fn save_gui_interaction(&self, input: &NewGuiInteraction<'_>) -> Result<(), CoreError>;

    /// Enforce retention for all auxiliary tables (work_sessions, interruptions,
    /// gui_interactions, suggestions, local_suggestions, focus_metrics,
    /// daily_digests, regime_overrides). Returns total rows deleted.
    fn enforce_all_retention(&self) -> Result<u64, CoreError>;

    /// Enforce the compliance-window age cap on the security audit trails
    /// (`audit_log` + `session_audit_log`, #8056 P3). Both are excluded from
    /// `enforce_all_retention` and RETAINED across GDPR erasure, so without this
    /// they grow unbounded. `audit_log` is pruned CHAIN-SAFELY (oldest contiguous
    /// prefix only, recording the retained chain's new root anchor) so ADR-072
    /// tamper-evidence is preserved. Returns total rows pruned. (Trait method so
    /// the scheduler can call it through the `dyn SchedulerStorage` seam.)
    fn enforce_audit_retention(&self) -> Result<u64, CoreError>;

    /// GC the GDPR Art.17 erasure tombstone outbox (#5174 S5/R4): hard-delete
    /// `sync_tombstones` older than `max(data_retention_days, 90)` days. Returns
    /// rows deleted. (Trait method so the scheduler can call it through the
    /// `dyn SchedulerStorage` seam — the inherent `SqliteStorage` impl does the work.)
    fn gc_sync_tombstones(&self, data_retention_days: u32) -> Result<usize, CoreError>;

    /// Persist a habit-streak day row (#5669). Called by the coaching loop when
    /// goal minutes flush, giving the HabitTracker widget a local producer —
    /// the `habit_streaks` table previously had no production writer, so the
    /// widget rendered "No data" forever on standalone. Default no-op keeps
    /// non-SQLite test doubles compiling without tracking habits.
    fn upsert_habit_streak(
        &self,
        _regime_label: &str,
        _date: &str,
        _minutes_logged: u32,
        _target_minutes: u32,
        _met: bool,
    ) -> Result<(), CoreError> {
        Ok(())
    }

    // --- SQLite maintenance methods ---

    /// Execute a passive WAL checkpoint. PASSIVE mode is non-blocking and
    /// safe to call while concurrent reads are in progress.
    fn wal_checkpoint_passive(&self) -> Result<(), CoreError>;

    /// Run VACUUM if the freelist occupies more than `threshold_percent` of
    /// the total page count. Returns `true` if VACUUM was actually executed.
    fn maybe_vacuum(&self, threshold_percent: u64) -> Result<bool, CoreError>;

    /// Incrementally merge FTS5 b-tree segments. Call periodically (every
    /// 5-10 minutes) to keep write-amplification low.
    fn fts_merge(&self, pages: u32) -> Result<(), CoreError>;

    /// Run a full FTS5 optimize pass (merges all segments into one). Expensive
    /// but dramatically speeds up subsequent queries. Call once daily.
    fn fts_optimize(&self) -> Result<(), CoreError>;

    /// Run `ANALYZE` to refresh SQLite query planner statistics. Call after
    /// bulk operations (IVF index builds, large batch inserts).
    // #7719: unlike its siblings (`fts_merge`/`maybe_vacuum`/`fts_optimize`,
    // all called from `scheduler/loops/system.rs`'s maintenance loop), no
    // caller invokes `run_analyze` — a genuine gap in the maintenance
    // schedule, not an intentional exclusion. Kept as the documented
    // interface for whenever it's added to that loop.
    #[allow(dead_code)]
    fn run_analyze(&self) -> Result<(), CoreError>;

    /// Record a single egress event in the audit ledger (`egress_ledger`) (V36, #4803/E20).
    ///
    /// Retains events that left the device (`uploaded`) or were blocked by
    /// policy (`blocked`) as regulatory-compliance evidence. Called from the
    /// synchronous loops (events/monitor); the `record_id` UNIQUE constraint
    /// deduplicates re-runs.
    fn record_egress(&self, record: &EgressLedgerRecord) -> Result<(), CoreError>;
}
