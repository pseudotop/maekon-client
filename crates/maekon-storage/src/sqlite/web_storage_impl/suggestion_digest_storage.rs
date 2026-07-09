//! `SuggestionQueryStorage`, `DigestStorage` implementations.
//!
//! Converted to `#[async_trait]` async per ADR-026 PR-6 (async storage
//! convergence). Each trait method delegates to a `*_async` inherent helper on
//! `SqliteStorage` that routes the SQLite work through the
//! `with_conn`/`with_conn_read` funnel (`spawn_blocking`), so the
//! single-connection `parking_lot` guard is acquired on a blocking-pool thread
//! and never held across an `.await` (#4928 erase barrier preserved). Reads use
//! `with_conn_read` (never skipped); writes use the `with_conn` write funnel
//! (skipped when `deletion_flag`/`erasing` is set). The lone in-scope
//! `block_in_place` bridge noted by ADR-026 is removed: the digest write methods
//! now await the funnel directly instead of running synchronously inside a
//! `block_in_place` context.
//!
//! Each digest method keeps a sync inherent twin (`*` without the `_async`
//! suffix) sharing one `*_inner(&Connection, ...)` SQL body, because the
//! src-tauri `SchedulerStorage` trait and the concrete `Arc<SqliteStorage>`
//! command call sites still resolve to the synchronous path.

use async_trait::async_trait;

use maekon_core::error::CoreError;
use maekon_core::models::daily_digest::DailyDigest;
use maekon_core::models::storage_records::{SegmentSummaryRecord, SuggestionRecord};
use maekon_core::models::weekly_digest::WeeklyDigest;
use maekon_core::ports::web_storage::{DigestStorage, SuggestionQueryStorage};

use crate::error::StorageError;
use crate::sqlite::SqliteStorage;

/// Convert a local calendar date (`YYYY-MM-DD`, interpreted in `tz`) into the
/// UTC rfc3339 half-open window `[from, to)` covering that whole local day
/// (#5664).
///
/// The producer writes `start_time` as UTC `to_rfc3339()` (`...+00:00`), while
/// callers (dashboard, daily digest) ask for a LOCAL calendar date. Naive
/// `{date}T00:00:00`/`T23:59:59` string bounds therefore dropped segments near
/// local midnight in any non-UTC timezone, and the closed `T23:59:59` upper
/// bound additionally dropped sub-second rows even in UTC. This helper fixes
/// both: the bounds are real UTC instants and the window is half-open.
///
/// DST handling: a local midnight that does not exist (spring-forward gap) or
/// is ambiguous (fall-back overlap) resolves to the earliest valid instant,
/// probing forward hour-by-hour for gap days (some zones skip midnight).
/// Returns `None` for unparseable dates — callers preserve the pre-#5664
/// behaviour of returning no rows for garbage input.
///
/// Generic over `TimeZone` so tests can inject a `FixedOffset` instead of
/// depending on the machine-local zone.
pub(crate) fn local_date_utc_window<Tz: chrono::TimeZone>(
    date: &str,
    tz: &Tz,
) -> Option<(String, String)> {
    let day = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    let next = day.succ_opt()?;

    fn day_start_utc<Tz: chrono::TimeZone>(
        day: chrono::NaiveDate,
        tz: &Tz,
    ) -> Option<chrono::DateTime<chrono::Utc>> {
        for hour in 0..=3u32 {
            let naive = day.and_hms_opt(hour, 0, 0)?;
            let resolved = tz
                .from_local_datetime(&naive)
                .earliest()
                .or_else(|| tz.from_local_datetime(&naive).latest());
            if let Some(t) = resolved {
                return Some(t.with_timezone(&chrono::Utc));
            }
        }
        None
    }

    let from = day_start_utc(day, tz)?;
    let to = day_start_utc(next, tz)?;
    Some((from.to_rfc3339(), to.to_rfc3339()))
}

// ---------------------------------------------------------------------------
// SuggestionQueryStorage
// ---------------------------------------------------------------------------

#[async_trait]
impl SuggestionQueryStorage for SqliteStorage {
    async fn list_suggestions(&self, limit: usize) -> Result<Vec<SuggestionRecord>, CoreError> {
        SqliteStorage::list_suggestions_async(self, limit)
            .await
            .map_err(Into::into)
    }

    async fn dismiss_unified_suggestion(&self, suggestion_id: &str) -> Result<bool, CoreError> {
        SqliteStorage::dismiss_unified_suggestion_async(self, suggestion_id)
            .await
            .map_err(Into::into)
    }

    async fn mark_unified_suggestion_shown(&self, suggestion_id: &str) -> Result<bool, CoreError> {
        SqliteStorage::mark_unified_suggestion_shown_async(self, suggestion_id)
            .await
            .map_err(Into::into)
    }

    async fn mark_unified_suggestion_acted(&self, suggestion_id: &str) -> Result<bool, CoreError> {
        SqliteStorage::mark_unified_suggestion_acted_async(self, suggestion_id)
            .await
            .map_err(Into::into)
    }

    async fn has_recent_server_suggestions(&self, lookback_secs: u64) -> Result<bool, CoreError> {
        SqliteStorage::has_recent_server_suggestions_async(self, lookback_secs)
            .await
            .map_err(Into::into)
    }
}

// ---------------------------------------------------------------------------
// DigestStorage
// ---------------------------------------------------------------------------

#[async_trait]
impl DigestStorage for SqliteStorage {
    async fn list_weekly_digests(&self, limit: usize) -> Result<Vec<WeeklyDigest>, CoreError> {
        SqliteStorage::list_weekly_digests_async(self, limit)
            .await
            .map_err(Into::into)
    }

    async fn get_current_week_digest(&self) -> Result<Option<WeeklyDigest>, CoreError> {
        SqliteStorage::get_current_week_digest_async(self)
            .await
            .map_err(Into::into)
    }

    async fn save_weekly_digest(&self, digest: &WeeklyDigest) -> Result<(), CoreError> {
        // Move owned data into the Send + 'static closure (no borrowed &str).
        let digest = digest.clone();
        SqliteStorage::save_weekly_digest_async(self, digest)
            .await
            .map_err(Into::into)
    }

    async fn save_daily_digest(&self, digest: &DailyDigest) -> Result<(), CoreError> {
        let digest = digest.clone();
        SqliteStorage::save_daily_digest_async(self, digest)
            .await
            .map_err(Into::into)
    }

    async fn get_daily_digest(&self, date: &str) -> Result<Option<DailyDigest>, CoreError> {
        SqliteStorage::get_daily_digest_async(self, date.to_owned())
            .await
            .map_err(Into::into)
    }

    async fn list_daily_digests(&self, limit: usize) -> Result<Vec<DailyDigest>, CoreError> {
        SqliteStorage::list_daily_digests_async(self, limit)
            .await
            .map_err(Into::into)
    }

    async fn get_segments_for_date(
        &self,
        date: &str,
    ) -> Result<Vec<SegmentSummaryRecord>, CoreError> {
        SqliteStorage::get_segments_for_date_async(self, date.to_owned())
            .await
            .map_err(Into::into)
    }
}

// ---------------------------------------------------------------------------
// Inherent digest storage methods (sync twin + async funnel helper + shared
// `*_inner` SQL body). The sync twins keep the src-tauri `SchedulerStorage`
// trait and concrete `Arc<SqliteStorage>` command call sites synchronous; the
// async helpers back the `DigestStorage` trait impl above.
// ---------------------------------------------------------------------------

impl SqliteStorage {
    // ---- weekly digests ---------------------------------------------------

    /// List recent weekly digests, newest first.
    pub fn list_weekly_digests(&self, limit: usize) -> Result<Vec<WeeklyDigest>, CoreError> {
        // Read — read_lock (independent of deletion_flag).
        let read = self.conn.read_lock();
        Self::list_weekly_digests_inner(read.conn(), limit).map_err(Into::into)
    }

    /// Async `list_weekly_digests` over the read funnel (ADR-026 PR-6).
    pub(crate) async fn list_weekly_digests_async(
        &self,
        limit: usize,
    ) -> Result<Vec<WeeklyDigest>, StorageError> {
        self.with_conn_read(move |conn| Self::list_weekly_digests_inner(conn, limit))
            .await
    }

    fn list_weekly_digests_inner(
        conn: &rusqlite::Connection,
        limit: usize,
    ) -> Result<Vec<WeeklyDigest>, StorageError> {
        let mut stmt = conn
            .prepare(
                "SELECT stats_json, comparison_json, llm_narrative FROM weekly_digests ORDER BY week_start DESC LIMIT ?1",
            )
            .map_err(|e| StorageError::Internal(format!("Failed to prepare weekly_digests query: {e}")))?;
        let digests: Vec<WeeklyDigest> = stmt
            .query_map(rusqlite::params![limit as i64], |row| {
                let stats_json: String = row.get(0)?;
                let comparison_json: Option<String> = row.get(1)?;
                let llm_narrative: Option<String> = row.get(2)?;
                Ok((stats_json, comparison_json, llm_narrative))
            })
            .map_err(|e| StorageError::Internal(format!("Failed to query weekly_digests: {e}")))?
            .filter_map(|r| r.ok())
            .filter_map(|(stats_json, comparison_json, llm_narrative)| {
                let mut digest: WeeklyDigest = serde_json::from_str(&stats_json).ok()?;
                if let Some(ref cj) = comparison_json {
                    digest.comparison = serde_json::from_str(cj).ok();
                }
                digest.llm_narrative = llm_narrative;
                Some(digest)
            })
            .collect();
        Ok(digests)
    }

    /// Get the digest for the current week (if it exists).
    pub fn get_current_week_digest(&self) -> Result<Option<WeeklyDigest>, CoreError> {
        // The most recent digest is the current week if it overlaps with now.
        Ok(self.list_weekly_digests(1)?.into_iter().next())
    }

    /// Async `get_current_week_digest` over the read funnel (ADR-026 PR-6).
    pub(crate) async fn get_current_week_digest_async(
        &self,
    ) -> Result<Option<WeeklyDigest>, StorageError> {
        Ok(self.list_weekly_digests_async(1).await?.into_iter().next())
    }

    /// Save a weekly digest. Upserts by week_start.
    pub fn save_weekly_digest(&self, digest: &WeeklyDigest) -> Result<(), CoreError> {
        let params = Self::weekly_digest_params(digest)?;
        // Write — write_lock (skipped when deletion_flag is set; weekly_digests ∈ ALL_TABLES).
        self.conn
            .write_lock()
            .run((), |conn| Self::save_weekly_digest_inner(conn, &params))
            .map_err(Into::into)
    }

    /// Async `save_weekly_digest` over the write funnel (ADR-026 PR-6).
    pub(crate) async fn save_weekly_digest_async(
        &self,
        digest: WeeklyDigest,
    ) -> Result<(), StorageError> {
        let params = Self::weekly_digest_params(&digest)?;
        self.with_conn(move |conn| Self::save_weekly_digest_inner(conn, &params))
            .await
    }

    fn weekly_digest_params(digest: &WeeklyDigest) -> Result<WeeklyDigestParams, StorageError> {
        let stats_json = serde_json::to_string(digest)
            .map_err(|e| StorageError::Internal(format!("Failed to serialize digest: {e}")))?;
        let comparison_json = digest
            .comparison
            .as_ref()
            .map(|c| serde_json::to_string(c).unwrap_or_default());
        Ok(WeeklyDigestParams {
            week_start: digest.week_start.to_rfc3339(),
            week_end: digest.week_end.to_rfc3339(),
            stats_json,
            comparison_json,
            llm_narrative: digest.llm_narrative.clone(),
        })
    }

    fn save_weekly_digest_inner(
        conn: &rusqlite::Connection,
        params: &WeeklyDigestParams,
    ) -> Result<(), StorageError> {
        conn.execute(
            "INSERT OR REPLACE INTO weekly_digests (week_start, week_end, stats_json, comparison_json, llm_narrative)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                params.week_start,
                params.week_end,
                params.stats_json,
                params.comparison_json,
                params.llm_narrative
            ],
        )
        .map_err(|e| StorageError::Internal(format!("Failed to save weekly digest: {e}")))?;
        Ok(())
    }

    // ---- daily digests ----------------------------------------------------

    /// Save a daily digest. Upserts by date.
    pub fn save_daily_digest(&self, digest: &DailyDigest) -> Result<(), CoreError> {
        let params = Self::daily_digest_params(digest)?;
        // Write — write_lock (skipped when deletion_flag is set; daily_digests ∈ ALL_TABLES).
        self.conn
            .write_lock()
            .run((), |conn| Self::save_daily_digest_inner(conn, &params))
            .map_err(Into::into)
    }

    /// Async `save_daily_digest` over the write funnel (ADR-026 PR-6).
    pub(crate) async fn save_daily_digest_async(
        &self,
        digest: DailyDigest,
    ) -> Result<(), StorageError> {
        let params = Self::daily_digest_params(&digest)?;
        self.with_conn(move |conn| Self::save_daily_digest_inner(conn, &params))
            .await
    }

    fn daily_digest_params(digest: &DailyDigest) -> Result<DailyDigestParams, StorageError> {
        let insight_json = digest
            .insight
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| StorageError::Internal(format!("Failed to serialize insight: {e}")))?;
        let timeline_json = serde_json::to_string(&digest.timeline)
            .map_err(|e| StorageError::Internal(format!("Failed to serialize timeline: {e}")))?;
        let statistics_json = serde_json::to_string(&digest.statistics)
            .map_err(|e| StorageError::Internal(format!("Failed to serialize statistics: {e}")))?;
        Ok(DailyDigestParams {
            date: digest.date.to_string(), // YYYY-MM-DD
            insight_json,
            timeline_json,
            statistics_json,
            generated_at: digest.generated_at.to_rfc3339(),
        })
    }

    fn save_daily_digest_inner(
        conn: &rusqlite::Connection,
        params: &DailyDigestParams,
    ) -> Result<(), StorageError> {
        conn.execute(
            "INSERT OR REPLACE INTO daily_digests (date, insight_json, timeline_json, statistics_json, generated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                params.date,
                params.insight_json,
                params.timeline_json,
                params.statistics_json,
                params.generated_at
            ],
        )
        .map_err(|e| StorageError::Internal(format!("Failed to save daily digest: {e}")))?;
        Ok(())
    }

    /// Get the daily digest for a specific date (YYYY-MM-DD).
    pub fn get_daily_digest(&self, date: &str) -> Result<Option<DailyDigest>, CoreError> {
        // Read — read_lock (independent of deletion_flag).
        let read = self.conn.read_lock();
        Self::get_daily_digest_inner(read.conn(), date).map_err(Into::into)
    }

    /// Async `get_daily_digest` over the read funnel (ADR-026 PR-6).
    pub(crate) async fn get_daily_digest_async(
        &self,
        date: String,
    ) -> Result<Option<DailyDigest>, StorageError> {
        self.with_conn_read(move |conn| Self::get_daily_digest_inner(conn, &date))
            .await
    }

    fn get_daily_digest_inner(
        conn: &rusqlite::Connection,
        date: &str,
    ) -> Result<Option<DailyDigest>, StorageError> {
        let result = conn.query_row(
            "SELECT date, insight_json, timeline_json, statistics_json, generated_at
             FROM daily_digests WHERE date = ?1",
            rusqlite::params![date],
            |row| {
                let date_str: String = row.get(0)?;
                let insight_json: Option<String> = row.get(1)?;
                let timeline_json: String = row.get(2)?;
                let statistics_json: String = row.get(3)?;
                let generated_at_str: String = row.get(4)?;
                Ok((
                    date_str,
                    insight_json,
                    timeline_json,
                    statistics_json,
                    generated_at_str,
                ))
            },
        );

        match result {
            Ok((date_str, insight_json, timeline_json, statistics_json, generated_at_str)) => {
                // `parse_daily_digest_row` returns `CoreError`; `?` wraps it via
                // `StorageError::Core` so the typed wire code survives the
                // round-trip back to `CoreError` at the trait boundary.
                let digest = SqliteStorage::parse_daily_digest_row(
                    &date_str,
                    insight_json.as_deref(),
                    &timeline_json,
                    &statistics_json,
                    &generated_at_str,
                )?;
                Ok(Some(digest))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(StorageError::Internal(format!(
                "Failed to get daily digest: {e}"
            ))),
        }
    }

    /// List recent daily digests, newest first.
    pub fn list_daily_digests(&self, limit: usize) -> Result<Vec<DailyDigest>, CoreError> {
        // Read — read_lock (independent of deletion_flag).
        let read = self.conn.read_lock();
        Self::list_daily_digests_inner(read.conn(), limit).map_err(Into::into)
    }

    /// Async `list_daily_digests` over the read funnel (ADR-026 PR-6).
    pub(crate) async fn list_daily_digests_async(
        &self,
        limit: usize,
    ) -> Result<Vec<DailyDigest>, StorageError> {
        self.with_conn_read(move |conn| Self::list_daily_digests_inner(conn, limit))
            .await
    }

    fn list_daily_digests_inner(
        conn: &rusqlite::Connection,
        limit: usize,
    ) -> Result<Vec<DailyDigest>, StorageError> {
        let mut stmt = conn
            .prepare(
                "SELECT date, insight_json, timeline_json, statistics_json, generated_at
                 FROM daily_digests ORDER BY date DESC LIMIT ?1",
            )
            .map_err(|e| {
                StorageError::Internal(format!("Failed to prepare daily_digests query: {e}"))
            })?;

        let digests: Vec<DailyDigest> = stmt
            .query_map(rusqlite::params![limit as i64], |row| {
                let date_str: String = row.get(0)?;
                let insight_json: Option<String> = row.get(1)?;
                let timeline_json: String = row.get(2)?;
                let statistics_json: String = row.get(3)?;
                let generated_at_str: String = row.get(4)?;
                Ok((
                    date_str,
                    insight_json,
                    timeline_json,
                    statistics_json,
                    generated_at_str,
                ))
            })
            .map_err(|e| StorageError::Internal(format!("Failed to query daily_digests: {e}")))?
            .filter_map(|r| r.ok())
            .filter_map(
                |(date_str, insight_json, timeline_json, statistics_json, generated_at_str)| {
                    SqliteStorage::parse_daily_digest_row(
                        &date_str,
                        insight_json.as_deref(),
                        &timeline_json,
                        &statistics_json,
                        &generated_at_str,
                    )
                    .ok()
                },
            )
            .collect();

        Ok(digests)
    }

    /// Return whether downstream digest processing has completed for a period.
    pub fn has_digest_processing_marker(
        &self,
        kind: &str,
        period_key: &str,
    ) -> Result<bool, CoreError> {
        let read = self.conn.read_lock();
        read.conn()
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM digest_processing_markers
                    WHERE kind = ?1 AND period_key = ?2
                 )",
                rusqlite::params![kind, period_key],
                |row| row.get::<_, i64>(0),
            )
            .map(|value| value != 0)
            .map_err(|e| {
                StorageError::Internal(format!("Failed to read digest processing marker: {e}"))
            })
            .map_err(Into::into)
    }

    /// Mark downstream digest processing complete for a period.
    pub fn save_digest_processing_marker(
        &self,
        kind: &str,
        period_key: &str,
        completed_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), CoreError> {
        self.conn
            .write_lock()
            .run((), |conn| {
                conn.execute(
                    "INSERT OR REPLACE INTO digest_processing_markers
                     (kind, period_key, completed_at)
                     VALUES (?1, ?2, ?3)",
                    rusqlite::params![kind, period_key, completed_at.to_rfc3339()],
                )
                .map_err(|e| {
                    StorageError::Internal(format!("Failed to save digest processing marker: {e}"))
                })?;
                Ok::<(), StorageError>(())
            })
            .map_err(Into::into)
    }

    // ---- segment summaries ------------------------------------------------

    /// Get activity segment summaries for a given date (YYYY-MM-DD).
    pub fn get_segments_for_date(
        &self,
        date: &str,
    ) -> Result<Vec<SegmentSummaryRecord>, CoreError> {
        // Read — read_lock (independent of deletion_flag).
        let read = self.conn.read_lock();
        Self::get_segments_for_date_inner(read.conn(), date).map_err(Into::into)
    }

    /// Async `get_segments_for_date` over the read funnel (ADR-026 PR-6).
    pub(crate) async fn get_segments_for_date_async(
        &self,
        date: String,
    ) -> Result<Vec<SegmentSummaryRecord>, StorageError> {
        self.with_conn_read(move |conn| Self::get_segments_for_date_inner(conn, &date))
            .await
    }

    fn get_segments_for_date_inner(
        conn: &rusqlite::Connection,
        date: &str,
    ) -> Result<Vec<SegmentSummaryRecord>, StorageError> {
        // #5664: the caller's `date` is a LOCAL calendar date; rows store UTC
        // rfc3339. Convert to a UTC half-open window in the machine-local zone.
        let Some((from, to)) = local_date_utc_window(date, &chrono::Local) else {
            // Unparseable date — preserve the pre-#5664 "garbage in, no rows
            // out" behaviour.
            return Ok(vec![]);
        };
        Self::get_segments_in_utc_window_inner(conn, &from, &to)
    }

    /// Query segments whose `start_time` falls in the UTC half-open window
    /// `[from, to)` (#5664). `datetime()` normalises stored values across the
    /// `+00:00`/`Z` suffix forms and sub-second precision, so lexical-form
    /// differences cannot drop boundary rows.
    pub(crate) fn get_segments_in_utc_window_inner(
        conn: &rusqlite::Connection,
        from: &str,
        to: &str,
    ) -> Result<Vec<SegmentSummaryRecord>, StorageError> {
        // Check if the activity_segments table exists.
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='activity_segments'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if !table_exists {
            return Ok(vec![]);
        }

        let mut stmt = conn
            .prepare(
                "SELECT id, start_time, end_time, duration_secs, dominant_category,
                        regime_id, app_breakdown, content_activities_json,
                        context_switch_count, llm_summary
                 FROM activity_segments
                 WHERE datetime(start_time) >= datetime(?1)
                   AND datetime(start_time) < datetime(?2)
                 ORDER BY start_time ASC",
            )
            .map_err(|e| {
                StorageError::Internal(format!("Failed to prepare segments query: {e}"))
            })?;

        let records: Vec<SegmentSummaryRecord> = stmt
            .query_map(rusqlite::params![from, to], |row| {
                Ok(SegmentSummaryRecord {
                    segment_id: row.get(0)?,
                    start_time: row.get(1)?,
                    end_time: row.get(2)?,
                    duration_secs: row.get::<_, i64>(3)? as u64,
                    dominant_category: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    regime_id: row.get(5)?,
                    app_breakdown: row
                        .get::<_, Option<String>>(6)?
                        .unwrap_or_else(|| "{}".to_string()),
                    content_activities_json: row
                        .get::<_, Option<String>>(7)?
                        .unwrap_or_else(|| "[]".to_string()),
                    context_switch_count: row.get::<_, Option<i64>>(8)?.unwrap_or(0) as u32,
                    llm_summary: row.get(9)?,
                })
            })
            .map_err(|e| StorageError::Internal(format!("Failed to query segments: {e}")))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(records)
    }
}

/// Owned, `Send + 'static` parameter bundle for a weekly-digest write, built
/// before entering the `spawn_blocking` closure (no borrowed `&str`).
struct WeeklyDigestParams {
    week_start: String,
    week_end: String,
    stats_json: String,
    comparison_json: Option<String>,
    llm_narrative: Option<String>,
}

/// Owned, `Send + 'static` parameter bundle for a daily-digest write.
struct DailyDigestParams {
    date: String,
    insight_json: Option<String>,
    timeline_json: String,
    statistics_json: String,
    generated_at: String,
}

#[cfg(test)]
mod date_window_tests {
    use super::*;
    use chrono::FixedOffset;

    fn kst() -> FixedOffset {
        FixedOffset::east_opt(9 * 3600).unwrap()
    }

    #[test]
    fn window_utc_is_identity_day() {
        let (from, to) = local_date_utc_window("2026-06-11", &chrono::Utc).unwrap();
        assert_eq!(from, "2026-06-11T00:00:00+00:00");
        assert_eq!(to, "2026-06-12T00:00:00+00:00");
    }

    #[test]
    fn window_kst_shifts_into_previous_utc_day() {
        // Local 2026-06-11 in KST(+9) = [2026-06-10T15:00Z, 2026-06-11T15:00Z).
        let (from, to) = local_date_utc_window("2026-06-11", &kst()).unwrap();
        assert_eq!(from, "2026-06-10T15:00:00+00:00");
        assert_eq!(to, "2026-06-11T15:00:00+00:00");
    }

    #[test]
    fn window_negative_offset_shifts_into_next_utc_day() {
        let tz = FixedOffset::west_opt(10 * 3600).unwrap(); // UTC-10 (Hawaii)
        let (from, to) = local_date_utc_window("2026-06-11", &tz).unwrap();
        assert_eq!(from, "2026-06-11T10:00:00+00:00");
        assert_eq!(to, "2026-06-12T10:00:00+00:00");
    }

    #[test]
    fn window_rejects_garbage_date() {
        assert!(local_date_utc_window("not-a-date", &chrono::Utc).is_none());
        assert!(local_date_utc_window("2026-13-45", &chrono::Utc).is_none());
    }

    /// #5664 regression core: a segment at local 00:30 KST (= previous-day
    /// 15:30 UTC) must be returned for the LOCAL date and not its neighbours,
    /// and the half-open + datetime() comparison must keep sub-second and
    /// `Z`-suffix boundary rows.
    #[test]
    fn kst_midnight_and_boundary_rows_are_returned() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        {
            let conn = storage.conn.test_lock();
            // 00:30 KST on local 2026-06-11 → 2026-06-10T15:30Z.
            conn.execute(
                "INSERT INTO activity_segments (id, start_time, end_time, duration_secs, trigger_reason, dominant_category, event_count, avg_importance)
                 VALUES ('seg-midnight', '2026-06-10T15:30:00+00:00', '2026-06-10T16:00:00+00:00', 1800, 'SCORE_HIGH', 'Development', 10, 0.5)",
                [],
            )
            .unwrap();
            // 23:59:59.5 KST on local 2026-06-11 → 2026-06-11T14:59:59.500Z,
            // stored with the `Z` suffix form. The old closed `T23:59:59`
            // lexical bound dropped sub-second rows like this one.
            conn.execute(
                "INSERT INTO activity_segments (id, start_time, end_time, duration_secs, trigger_reason, dominant_category, event_count, avg_importance)
                 VALUES ('seg-subsec', '2026-06-11T14:59:59.500Z', '2026-06-11T15:10:00Z', 600, 'SCORE_HIGH', 'Development', 10, 0.5)",
                [],
            )
            .unwrap();
            // First instant of the NEXT local day (00:00:00 KST 2026-06-12 =
            // 2026-06-11T15:00:00Z) — must be excluded (half-open upper bound).
            conn.execute(
                "INSERT INTO activity_segments (id, start_time, end_time, duration_secs, trigger_reason, dominant_category, event_count, avg_importance)
                 VALUES ('seg-nextday', '2026-06-11T15:00:00+00:00', '2026-06-11T15:30:00+00:00', 1800, 'SCORE_HIGH', 'Development', 10, 0.5)",
                [],
            )
            .unwrap();
        }

        let (from, to) = local_date_utc_window("2026-06-11", &kst()).unwrap();
        let conn = storage.conn.test_lock();
        let rows = SqliteStorage::get_segments_in_utc_window_inner(&conn, &from, &to).unwrap();
        let ids: Vec<&str> = rows.iter().map(|r| r.segment_id.as_str()).collect();
        assert_eq!(ids, vec!["seg-midnight", "seg-subsec"]);

        // The midnight row belongs to local 2026-06-11, NOT local 2026-06-10.
        let (from_prev, to_prev) = local_date_utc_window("2026-06-10", &kst()).unwrap();
        let prev =
            SqliteStorage::get_segments_in_utc_window_inner(&conn, &from_prev, &to_prev).unwrap();
        assert!(prev.iter().all(|r| r.segment_id != "seg-midnight"));

        // And the next-day row belongs to local 2026-06-12.
        let (from_next, to_next) = local_date_utc_window("2026-06-12", &kst()).unwrap();
        let next =
            SqliteStorage::get_segments_in_utc_window_inner(&conn, &from_next, &to_next).unwrap();
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].segment_id, "seg-nextday");
    }
}
