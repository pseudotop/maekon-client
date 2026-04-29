use chrono::{DateTime, Utc};
use oneshim_core::error::CoreError;
use oneshim_core::models::activity::SessionStats;
use oneshim_core::models::daily_digest::DailyDigest;
use oneshim_core::models::storage_records::{
    DeletedRangeCounts, EventExportRecord, FocusInterruptionRecord, FocusWorkSessionRecord,
    FrameExportRecord, FrameRecord, FrameTagLinkRecord, GuiInteractionRecord, HourlyMetricsRecord,
    LocalSuggestionRecord, MetricExportRecord, NewGuiInteraction, SearchEventRow, SearchFrameRow,
    SegmentDetailRecord, SegmentSummaryRecord, StorageStatsSummaryRecord, SuggestionRecord,
    TagRecord,
};
use oneshim_core::models::work_session::FocusMetrics;
use oneshim_core::ports::web_storage::{
    ActivityStatsStorage, BackupStorage, CoachingQueryStorage, DigestStorage, EventQueryStorage,
    FocusQueryStorage, FrameQueryStorage, GuiInteractionStorage, SegmentQueryStorage,
    StorageMaintenanceStorage, SuggestionQueryStorage, TagStorage,
};
use oneshim_core::types::TimeWindow;

use super::SqliteStorage;

/// Defense-in-depth PII scrub for text stored in gui_interactions.
/// Replaces strings containing '@' (likely emails) with "[FILTERED]".
/// This is a lightweight fallback — primary filtering is the caller's responsibility.
fn scrub_basic_pii(text: &str) -> String {
    // If text contains an '@' sign, it likely contains an email address
    if text.contains('@') {
        return "[FILTERED]".to_string();
    }
    text.to_string()
}

impl SqliteStorage {
    /// Parse a daily digest row from its constituent JSON columns.
    fn parse_daily_digest_row(
        date_str: &str,
        insight_json: Option<&str>,
        timeline_json: &str,
        statistics_json: &str,
        generated_at_str: &str,
    ) -> Result<DailyDigest, CoreError> {
        let date = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d").map_err(|e| {
            CoreError::Storage {
                code: oneshim_core::error_codes::StorageCode::Failed,
                message: format!("Invalid date in daily_digests: {e}"),
            }
        })?;
        let insight = insight_json
            .map(serde_json::from_str)
            .transpose()
            .map_err(|e| CoreError::Storage {
                code: oneshim_core::error_codes::StorageCode::Failed,
                message: format!("Failed to deserialize insight: {e}"),
            })?;
        let timeline = serde_json::from_str(timeline_json).map_err(|e| CoreError::Storage {
            code: oneshim_core::error_codes::StorageCode::Failed,
            message: format!("Failed to deserialize timeline: {e}"),
        })?;
        let statistics = serde_json::from_str(statistics_json).map_err(|e| CoreError::Storage {
            code: oneshim_core::error_codes::StorageCode::Failed,
            message: format!("Failed to deserialize statistics: {e}"),
        })?;
        let generated_at = chrono::DateTime::parse_from_rfc3339(generated_at_str)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());

        Ok(DailyDigest {
            date,
            insight,
            timeline,
            statistics,
            generated_at,
        })
    }
}

// ---------------------------------------------------------------------------
// EventQueryStorage
// ---------------------------------------------------------------------------

impl EventQueryStorage for SqliteStorage {
    fn count_events_in_range(&self, window: &TimeWindow) -> Result<u64, CoreError> {
        SqliteStorage::count_events_in_range(self, window).map_err(Into::into)
    }

    fn count_search_events(&self, pattern: &str) -> Result<u64, CoreError> {
        SqliteStorage::count_search_events(self, pattern).map_err(Into::into)
    }

    fn search_events(
        &self,
        pattern: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<SearchEventRow>, CoreError> {
        SqliteStorage::search_events(self, pattern, limit, offset).map_err(Into::into)
    }
}

// ---------------------------------------------------------------------------
// FrameQueryStorage
// ---------------------------------------------------------------------------

impl FrameQueryStorage for SqliteStorage {
    fn count_frames_in_range(&self, window: &TimeWindow) -> Result<u64, CoreError> {
        SqliteStorage::count_frames_in_range(self, window).map_err(Into::into)
    }

    fn get_frames(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<FrameRecord>, CoreError> {
        SqliteStorage::get_frames(self, from, to, limit).map_err(Into::into)
    }

    fn get_frame_file_path(&self, frame_id: i64) -> Result<Option<String>, CoreError> {
        SqliteStorage::get_frame_file_path(self, frame_id).map_err(Into::into)
    }

    fn list_frame_file_paths_in_range(
        &self,
        window: &TimeWindow,
    ) -> Result<Vec<String>, CoreError> {
        SqliteStorage::list_frame_file_paths_in_range(self, window).map_err(Into::into)
    }

    fn count_search_frames(
        &self,
        count_sql: &str,
        pattern: Option<&str>,
    ) -> Result<u64, CoreError> {
        SqliteStorage::count_search_frames(self, count_sql, pattern).map_err(Into::into)
    }

    fn search_frames_with_sql(
        &self,
        select_sql: &str,
        pattern: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<SearchFrameRow>, CoreError> {
        SqliteStorage::search_frames_with_sql(self, select_sql, pattern, limit, offset)
            .map_err(Into::into)
    }
}

// ---------------------------------------------------------------------------
// StorageMaintenanceStorage
// ---------------------------------------------------------------------------

impl StorageMaintenanceStorage for SqliteStorage {
    fn get_storage_stats_summary(&self) -> Result<StorageStatsSummaryRecord, CoreError> {
        SqliteStorage::get_storage_stats_summary(self).map_err(Into::into)
    }

    #[allow(clippy::too_many_arguments)]
    fn delete_data_in_range(
        &self,
        window: &TimeWindow,
        delete_events: bool,
        delete_frames: bool,
        delete_metrics: bool,
        delete_processes: bool,
        delete_idle: bool,
    ) -> Result<DeletedRangeCounts, CoreError> {
        SqliteStorage::delete_data_in_range(
            self,
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
        SqliteStorage::delete_all_data(self).map_err(Into::into)
    }
}

// ---------------------------------------------------------------------------
// TagStorage
// ---------------------------------------------------------------------------

impl TagStorage for SqliteStorage {
    fn get_all_tags(&self) -> Result<Vec<TagRecord>, CoreError> {
        SqliteStorage::get_all_tags(self).map_err(Into::into)
    }

    fn get_tag(&self, tag_id: i64) -> Result<Option<TagRecord>, CoreError> {
        SqliteStorage::get_tag(self, tag_id).map_err(Into::into)
    }

    fn get_tag_ids_for_frames(
        &self,
        frame_ids: &[i64],
    ) -> Result<std::collections::HashMap<i64, Vec<i64>>, CoreError> {
        SqliteStorage::get_tag_ids_for_frames(self, frame_ids).map_err(Into::into)
    }

    fn create_tag(&self, name: &str, color: &str) -> Result<TagRecord, CoreError> {
        SqliteStorage::create_tag(self, name, color).map_err(Into::into)
    }

    fn update_tag(&self, tag_id: i64, name: &str, color: &str) -> Result<bool, CoreError> {
        SqliteStorage::update_tag(self, tag_id, name, color).map_err(Into::into)
    }

    fn delete_tag(&self, tag_id: i64) -> Result<bool, CoreError> {
        SqliteStorage::delete_tag(self, tag_id).map_err(Into::into)
    }

    fn get_tags_for_frame(&self, frame_id: i64) -> Result<Vec<TagRecord>, CoreError> {
        SqliteStorage::get_tags_for_frame(self, frame_id).map_err(Into::into)
    }

    fn add_tag_to_frame(&self, frame_id: i64, tag_id: i64) -> Result<(), CoreError> {
        SqliteStorage::add_tag_to_frame(self, frame_id, tag_id).map_err(Into::into)
    }

    fn remove_tag_from_frame(&self, frame_id: i64, tag_id: i64) -> Result<bool, CoreError> {
        SqliteStorage::remove_tag_from_frame(self, frame_id, tag_id).map_err(Into::into)
    }
}

// ---------------------------------------------------------------------------
// ActivityStatsStorage
// ---------------------------------------------------------------------------

impl ActivityStatsStorage for SqliteStorage {
    fn get_app_durations_by_date(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<(String, i64)>, CoreError> {
        SqliteStorage::get_app_durations_by_date(self, from, to).map_err(Into::into)
    }

    fn get_daily_active_secs(&self, window: &TimeWindow) -> Result<Vec<(String, i64)>, CoreError> {
        SqliteStorage::get_daily_active_secs(self, window).map_err(Into::into)
    }

    fn list_session_stats(&self, limit: usize) -> Result<Vec<SessionStats>, CoreError> {
        SqliteStorage::list_session_stats(self, limit).map_err(Into::into)
    }
}

// ---------------------------------------------------------------------------
// FocusQueryStorage
// ---------------------------------------------------------------------------

impl FocusQueryStorage for SqliteStorage {
    fn get_or_create_focus_metrics(&self, date: &str) -> Result<FocusMetrics, CoreError> {
        SqliteStorage::get_or_create_focus_metrics(self, date).map_err(Into::into)
    }

    fn get_recent_focus_metrics(
        &self,
        days: usize,
    ) -> Result<Vec<(String, FocusMetrics)>, CoreError> {
        SqliteStorage::get_recent_focus_metrics(self, days).map_err(Into::into)
    }

    fn list_work_sessions(
        &self,
        from: &str,
        to: &str,
        limit: usize,
    ) -> Result<Vec<FocusWorkSessionRecord>, CoreError> {
        SqliteStorage::list_work_sessions(self, from, to, limit).map_err(Into::into)
    }

    fn list_interruptions(
        &self,
        from: &str,
        to: &str,
        limit: usize,
    ) -> Result<Vec<FocusInterruptionRecord>, CoreError> {
        SqliteStorage::list_interruptions(self, from, to, limit).map_err(Into::into)
    }

    fn list_recent_local_suggestions(
        &self,
        cutoff: &str,
        limit: usize,
    ) -> Result<Vec<LocalSuggestionRecord>, CoreError> {
        SqliteStorage::list_recent_local_suggestions(self, cutoff, limit).map_err(Into::into)
    }

    fn mark_suggestion_shown(&self, suggestion_id: i64) -> Result<(), CoreError> {
        SqliteStorage::mark_suggestion_shown(self, suggestion_id).map_err(Into::into)
    }

    fn mark_suggestion_dismissed(&self, suggestion_id: i64) -> Result<(), CoreError> {
        SqliteStorage::mark_suggestion_dismissed(self, suggestion_id).map_err(Into::into)
    }

    fn mark_suggestion_acted(&self, suggestion_id: i64) -> Result<(), CoreError> {
        SqliteStorage::mark_suggestion_acted(self, suggestion_id).map_err(Into::into)
    }
}

// ---------------------------------------------------------------------------
// SuggestionQueryStorage
// ---------------------------------------------------------------------------

impl SuggestionQueryStorage for SqliteStorage {
    fn list_suggestions(&self, limit: usize) -> Result<Vec<SuggestionRecord>, CoreError> {
        SqliteStorage::list_suggestions(self, limit).map_err(Into::into)
    }

    fn dismiss_unified_suggestion(&self, suggestion_id: &str) -> Result<bool, CoreError> {
        SqliteStorage::dismiss_unified_suggestion(self, suggestion_id).map_err(Into::into)
    }

    fn has_recent_server_suggestions(&self, lookback_secs: u64) -> Result<bool, CoreError> {
        SqliteStorage::has_recent_server_suggestions(self, lookback_secs).map_err(Into::into)
    }
}

// ---------------------------------------------------------------------------
// DigestStorage
// ---------------------------------------------------------------------------

impl DigestStorage for SqliteStorage {
    fn list_weekly_digests(
        &self,
        limit: usize,
    ) -> Result<Vec<oneshim_core::models::weekly_digest::WeeklyDigest>, CoreError> {
        let guard = self.conn.lock().map_err(|e| CoreError::Storage {
            code: oneshim_core::error_codes::StorageCode::Failed,
            message: format!("SQLite lock poisoned: {e}"),
        })?;
        let mut stmt = guard
            .prepare(
                "SELECT stats_json, comparison_json, llm_narrative FROM weekly_digests ORDER BY week_start DESC LIMIT ?1",
            )
            .map_err(|e| CoreError::Storage { code: oneshim_core::error_codes::StorageCode::Failed, message: format!("Failed to prepare weekly_digests query: {e}") })?;
        let digests: Vec<oneshim_core::models::weekly_digest::WeeklyDigest> = stmt
            .query_map(rusqlite::params![limit as i64], |row| {
                let stats_json: String = row.get(0)?;
                let comparison_json: Option<String> = row.get(1)?;
                let llm_narrative: Option<String> = row.get(2)?;
                Ok((stats_json, comparison_json, llm_narrative))
            })
            .map_err(|e| CoreError::Storage {
                code: oneshim_core::error_codes::StorageCode::Failed,
                message: format!("Failed to query weekly_digests: {e}"),
            })?
            .filter_map(|r| r.ok())
            .filter_map(|(stats_json, comparison_json, llm_narrative)| {
                let mut digest: oneshim_core::models::weekly_digest::WeeklyDigest =
                    serde_json::from_str(&stats_json).ok()?;
                if let Some(ref cj) = comparison_json {
                    digest.comparison = serde_json::from_str(cj).ok();
                }
                digest.llm_narrative = llm_narrative;
                Some(digest)
            })
            .collect();
        Ok(digests)
    }

    fn get_current_week_digest(
        &self,
    ) -> Result<Option<oneshim_core::models::weekly_digest::WeeklyDigest>, CoreError> {
        let digests = self.list_weekly_digests(1)?;
        // The most recent digest is the current week if it overlaps with now
        Ok(digests.into_iter().next())
    }

    fn save_weekly_digest(
        &self,
        digest: &oneshim_core::models::weekly_digest::WeeklyDigest,
    ) -> Result<(), CoreError> {
        let guard = self.conn.lock().map_err(|e| CoreError::Storage {
            code: oneshim_core::error_codes::StorageCode::Failed,
            message: format!("SQLite lock poisoned: {e}"),
        })?;
        let stats_json = serde_json::to_string(digest).map_err(|e| CoreError::Storage {
            code: oneshim_core::error_codes::StorageCode::Failed,
            message: format!("Failed to serialize digest: {e}"),
        })?;
        let comparison_json = digest
            .comparison
            .as_ref()
            .map(|c| serde_json::to_string(c).unwrap_or_default());
        let week_start = digest.week_start.to_rfc3339();
        let week_end = digest.week_end.to_rfc3339();

        guard
            .execute(
                "INSERT OR REPLACE INTO weekly_digests (week_start, week_end, stats_json, comparison_json, llm_narrative)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![week_start, week_end, stats_json, comparison_json, digest.llm_narrative],
            )
            .map_err(|e| CoreError::Storage { code: oneshim_core::error_codes::StorageCode::Failed, message: format!("Failed to save weekly digest: {e}") })?;
        Ok(())
    }

    fn save_daily_digest(&self, digest: &DailyDigest) -> Result<(), CoreError> {
        let conn = self.conn.lock().map_err(|e| CoreError::Storage {
            code: oneshim_core::error_codes::StorageCode::Failed,
            message: format!("SQLite lock poisoned: {e}"),
        })?;

        let date_str = digest.date.to_string(); // YYYY-MM-DD
        let insight_json = digest
            .insight
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| CoreError::Storage {
                code: oneshim_core::error_codes::StorageCode::Failed,
                message: format!("Failed to serialize insight: {e}"),
            })?;
        let timeline_json =
            serde_json::to_string(&digest.timeline).map_err(|e| CoreError::Storage {
                code: oneshim_core::error_codes::StorageCode::Failed,
                message: format!("Failed to serialize timeline: {e}"),
            })?;
        let statistics_json =
            serde_json::to_string(&digest.statistics).map_err(|e| CoreError::Storage {
                code: oneshim_core::error_codes::StorageCode::Failed,
                message: format!("Failed to serialize statistics: {e}"),
            })?;
        let generated_at = digest.generated_at.to_rfc3339();

        conn.execute(
            "INSERT OR REPLACE INTO daily_digests (date, insight_json, timeline_json, statistics_json, generated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![date_str, insight_json, timeline_json, statistics_json, generated_at],
        )
        .map_err(|e| CoreError::Storage { code: oneshim_core::error_codes::StorageCode::Failed, message: format!("Failed to save daily digest: {e}") })?;

        Ok(())
    }

    fn get_daily_digest(&self, date: &str) -> Result<Option<DailyDigest>, CoreError> {
        let conn = self.conn.lock().map_err(|e| CoreError::Storage {
            code: oneshim_core::error_codes::StorageCode::Failed,
            message: format!("SQLite lock poisoned: {e}"),
        })?;

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
                let digest = Self::parse_daily_digest_row(
                    &date_str,
                    insight_json.as_deref(),
                    &timeline_json,
                    &statistics_json,
                    &generated_at_str,
                )?;
                Ok(Some(digest))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(CoreError::Storage {
                code: oneshim_core::error_codes::StorageCode::Failed,
                message: format!("Failed to get daily digest: {e}"),
            }),
        }
    }

    fn list_daily_digests(&self, limit: usize) -> Result<Vec<DailyDigest>, CoreError> {
        let conn = self.conn.lock().map_err(|e| CoreError::Storage {
            code: oneshim_core::error_codes::StorageCode::Failed,
            message: format!("SQLite lock poisoned: {e}"),
        })?;

        let mut stmt = conn
            .prepare(
                "SELECT date, insight_json, timeline_json, statistics_json, generated_at
                 FROM daily_digests ORDER BY date DESC LIMIT ?1",
            )
            .map_err(|e| CoreError::Storage {
                code: oneshim_core::error_codes::StorageCode::Failed,
                message: format!("Failed to prepare daily_digests query: {e}"),
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
            .map_err(|e| CoreError::Storage {
                code: oneshim_core::error_codes::StorageCode::Failed,
                message: format!("Failed to query daily_digests: {e}"),
            })?
            .filter_map(|r| r.ok())
            .filter_map(
                |(date_str, insight_json, timeline_json, statistics_json, generated_at_str)| {
                    Self::parse_daily_digest_row(
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

    fn get_segments_for_date(&self, date: &str) -> Result<Vec<SegmentSummaryRecord>, CoreError> {
        let conn = self.conn.lock().map_err(|e| CoreError::Storage {
            code: oneshim_core::error_codes::StorageCode::Failed,
            message: format!("SQLite lock poisoned: {e}"),
        })?;

        // Check if the activity_segments table exists
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

        // date is YYYY-MM-DD; select segments whose start_time falls on that day
        let from = format!("{date}T00:00:00");
        let to = format!("{date}T23:59:59");

        let mut stmt = conn
            .prepare(
                "SELECT id, start_time, end_time, duration_secs, dominant_category,
                        regime_id, app_breakdown, content_activities_json,
                        context_switch_count, llm_summary
                 FROM activity_segments
                 WHERE start_time >= ?1 AND start_time <= ?2
                 ORDER BY start_time ASC",
            )
            .map_err(|e| CoreError::Storage {
                code: oneshim_core::error_codes::StorageCode::Failed,
                message: format!("Failed to prepare segments query: {e}"),
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
            .map_err(|e| CoreError::Storage {
                code: oneshim_core::error_codes::StorageCode::Failed,
                message: format!("Failed to query segments: {e}"),
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(records)
    }
}

// ---------------------------------------------------------------------------
// BackupStorage
// ---------------------------------------------------------------------------

impl BackupStorage for SqliteStorage {
    fn list_backup_tags(&self) -> Result<Vec<TagRecord>, CoreError> {
        SqliteStorage::list_backup_tags(self).map_err(Into::into)
    }

    fn list_backup_frame_tags(&self) -> Result<Vec<FrameTagLinkRecord>, CoreError> {
        SqliteStorage::list_backup_frame_tags(self).map_err(Into::into)
    }

    fn list_event_exports(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<EventExportRecord>, CoreError> {
        SqliteStorage::list_event_exports(self, from, to).map_err(Into::into)
    }

    fn list_metric_exports(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<MetricExportRecord>, CoreError> {
        SqliteStorage::list_metric_exports(self, from, to).map_err(Into::into)
    }

    fn list_frame_exports(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<FrameExportRecord>, CoreError> {
        SqliteStorage::list_frame_exports(self, from, to).map_err(Into::into)
    }

    fn list_hourly_metrics_since(&self, from: &str) -> Result<Vec<HourlyMetricsRecord>, CoreError> {
        SqliteStorage::list_hourly_metrics_since(self, from).map_err(Into::into)
    }

    fn upsert_backup_tag(
        &self,
        id: i64,
        name: &str,
        color: &str,
        created_at: &str,
    ) -> Result<(), CoreError> {
        SqliteStorage::upsert_backup_tag(self, id, name, color, created_at).map_err(Into::into)
    }

    fn upsert_backup_frame_tag(
        &self,
        frame_id: i64,
        tag_id: i64,
        created_at: &str,
    ) -> Result<(), CoreError> {
        SqliteStorage::upsert_backup_frame_tag(self, frame_id, tag_id, created_at)
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
        SqliteStorage::upsert_backup_event(
            self,
            event_id,
            event_type,
            timestamp,
            app_name,
            window_title,
        )
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
        SqliteStorage::upsert_backup_frame(
            self,
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

// ---------------------------------------------------------------------------
// SegmentQueryStorage
// ---------------------------------------------------------------------------

impl SegmentQueryStorage for SqliteStorage {
    fn get_segment_details(
        &self,
        segment_ids: &[String],
    ) -> Result<std::collections::HashMap<String, SegmentDetailRecord>, CoreError> {
        if segment_ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        let conn = self.conn.lock().map_err(|e| CoreError::Storage {
            code: oneshim_core::error_codes::StorageCode::Failed,
            message: format!("SQLite lock poisoned: {e}"),
        })?;

        // Check if the activity_segments table exists
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
}

// ---------------------------------------------------------------------------
// GuiInteractionStorage
// ---------------------------------------------------------------------------

impl GuiInteractionStorage for SqliteStorage {
    fn save_gui_interaction(&self, input: &NewGuiInteraction<'_>) -> Result<(), CoreError> {
        // Defense-in-depth: basic PII scrub on element_text at storage boundary.
        // Primary filtering is the caller's responsibility (see port doc comment).
        let scrubbed_text = input.element_text.map(scrub_basic_pii);
        let scrubbed_ref = scrubbed_text.as_deref();

        let conn = self.conn.lock().map_err(|e| CoreError::Storage {
            code: oneshim_core::error_codes::StorageCode::Failed,
            message: format!("SQLite lock poisoned: {e}"),
        })?;
        conn.execute(
            "INSERT INTO gui_interactions (event_id, segment_id, timestamp, element_text, element_type, interaction_type, bbox_json, app_name, type_confidence)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                input.event_id,
                input.segment_id,
                input.timestamp,
                scrubbed_ref,
                input.element_type,
                input.interaction_type,
                input.bbox_json,
                input.app_name,
                input.type_confidence,
            ],
        )
        .map_err(|e| CoreError::Storage { code: oneshim_core::error_codes::StorageCode::Failed, message: format!("Failed to save GUI interaction: {e}") })?;
        Ok(())
    }

    fn list_gui_interactions_for_segment(
        &self,
        segment_id: &str,
    ) -> Result<Vec<GuiInteractionRecord>, CoreError> {
        let conn = self.conn.lock().map_err(|e| CoreError::Storage {
            code: oneshim_core::error_codes::StorageCode::Failed,
            message: format!("SQLite lock poisoned: {e}"),
        })?;
        let mut stmt = conn
            .prepare(
                "SELECT id, event_id, segment_id, timestamp, element_text, element_type,
                        interaction_type, bbox_json, app_name, created_at, type_confidence
                 FROM gui_interactions
                 WHERE segment_id = ?1
                 ORDER BY timestamp ASC",
            )
            .map_err(|e| CoreError::Storage {
                code: oneshim_core::error_codes::StorageCode::Failed,
                message: format!("Failed to prepare GUI interaction query: {e}"),
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
            .map_err(|e| CoreError::Storage {
                code: oneshim_core::error_codes::StorageCode::Failed,
                message: format!("Failed to query GUI interactions: {e}"),
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(records)
    }

    fn query_gui_interaction_density(
        &self,
        start: &str,
        end: &str,
    ) -> Result<Vec<(String, u32)>, CoreError> {
        let conn = self.conn.lock().map_err(|e| CoreError::Storage {
            code: oneshim_core::error_codes::StorageCode::Failed,
            message: format!("SQLite lock poisoned: {e}"),
        })?;
        let mut stmt = conn
            .prepare(
                "SELECT strftime('%Y-%m-%dT%H:00:00Z', timestamp) AS hour, COUNT(*) AS cnt
                 FROM gui_interactions
                 WHERE timestamp >= ?1 AND timestamp < ?2
                 GROUP BY hour
                 ORDER BY hour",
            )
            .map_err(|e| CoreError::Storage {
                code: oneshim_core::error_codes::StorageCode::Failed,
                message: format!("Failed to prepare GUI interaction density query: {e}"),
            })?;

        let rows: Vec<(String, u32)> = stmt
            .query_map(rusqlite::params![start, end], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
            })
            .map_err(|e| CoreError::Storage {
                code: oneshim_core::error_codes::StorageCode::Failed,
                message: format!("GUI interaction density query failed: {e}"),
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(rows)
    }
}

// ---------------------------------------------------------------------------
// CoachingQueryStorage
// ---------------------------------------------------------------------------

impl CoachingQueryStorage for SqliteStorage {
    fn query_coaching_events(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<oneshim_core::models::coaching::CoachingEventRow>, CoreError> {
        // Delegate to the coaching_storage impl on SqliteStorage
        self.query_coaching_events(limit, offset)
            .map_err(Into::into)
    }

    fn query_coaching_events_since(
        &self,
        since_date: &str,
    ) -> Result<Vec<oneshim_core::models::coaching::CoachingEventRow>, CoreError> {
        self.query_coaching_events_since(since_date)
            .map_err(Into::into)
    }
}

// ---------------------------------------------------------------------------
// HabitStorage
// ---------------------------------------------------------------------------

impl oneshim_core::ports::web_storage::HabitStorage for SqliteStorage {
    fn upsert_habit_streak(
        &self,
        regime_label: &str,
        date: &str,
        minutes_logged: u32,
        target_minutes: u32,
        met: bool,
    ) -> Result<(), CoreError> {
        self.upsert_habit_streak(regime_label, date, minutes_logged, target_minutes, met)
            .map_err(Into::into)
    }

    fn query_habit_streaks(
        &self,
        days: u32,
    ) -> Result<Vec<oneshim_core::models::coaching::HabitStreakRow>, CoreError> {
        self.query_habit_streaks(days).map_err(Into::into)
    }
}

/// Parse the centroid (center x, center y) from a bbox JSON string.
///
/// Accepts:
/// - `{"x":100,"y":200,"w":50,"h":30}` → centroid (125, 215)
/// - `{"x":100,"y":200}` → (100, 200)
/// - `[100, 200, 150, 230]` (x, y, x2, y2) → centroid (125, 215)
#[cfg(test)]
fn parse_bbox_centroid(json: &str) -> Option<(u32, u32)> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    if let Some(obj) = v.as_object() {
        let x = obj.get("x").and_then(|v| v.as_f64())? as u32;
        let y = obj.get("y").and_then(|v| v.as_f64())? as u32;
        let w = obj.get("w").and_then(|v| v.as_f64()).unwrap_or(0.0) as u32;
        let h = obj.get("h").and_then(|v| v.as_f64()).unwrap_or(0.0) as u32;
        Some((x + w / 2, y + h / 2))
    } else if let Some(arr) = v.as_array() {
        if arr.len() >= 4 {
            let x1 = arr[0].as_f64()? as u32;
            let y1 = arr[1].as_f64()? as u32;
            let x2 = arr[2].as_f64()? as u32;
            let y2 = arr[3].as_f64()? as u32;
            Some(((x1 + x2) / 2, (y1 + y2) / 2))
        } else if arr.len() >= 2 {
            let x = arr[0].as_f64()? as u32;
            let y = arr[1].as_f64()? as u32;
            Some((x, y))
        } else {
            None
        }
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::parse_bbox_centroid;

    #[test]
    fn object_with_xywh_returns_centroid() {
        // x=100, y=200, w=50, h=30 → centroid = (125, 215)
        let json = r#"{"x":100,"y":200,"w":50,"h":30}"#;
        assert_eq!(parse_bbox_centroid(json), Some((125, 215)));
    }

    #[test]
    fn object_with_xy_only_returns_point() {
        let json = r#"{"x":300,"y":400}"#;
        assert_eq!(parse_bbox_centroid(json), Some((300, 400)));
    }

    #[test]
    fn array_four_elements_returns_centroid() {
        // [x1, y1, x2, y2] = [100, 200, 150, 230] → centroid = (125, 215)
        let json = "[100, 200, 150, 230]";
        assert_eq!(parse_bbox_centroid(json), Some((125, 215)));
    }

    #[test]
    fn array_two_elements_returns_point() {
        let json = "[50, 75]";
        assert_eq!(parse_bbox_centroid(json), Some((50, 75)));
    }

    #[test]
    fn short_array_returns_none() {
        let json = "[42]";
        assert_eq!(parse_bbox_centroid(json), None);

        let json_empty = "[]";
        assert_eq!(parse_bbox_centroid(json_empty), None);
    }

    #[test]
    fn malformed_json_returns_none() {
        assert_eq!(parse_bbox_centroid("not json"), None);
        assert_eq!(parse_bbox_centroid("{broken"), None);
        assert_eq!(parse_bbox_centroid(""), None);
    }

    #[test]
    fn negative_coordinates_saturate_to_zero() {
        // f64 → u32 cast: negative values saturate to 0 in Rust.
        // x=-10 → 0u32, y=-20 → 0u32; w and h default to 0 when absent.
        let json = r#"{"x":-10,"y":-20}"#;
        let result = parse_bbox_centroid(json);
        assert_eq!(result, Some((0, 0)));

        // Negative coordinates in array form
        let arr_json = "[-5, -15, -1, -3]";
        let (cx, cy) = parse_bbox_centroid(arr_json).unwrap();
        assert_eq!(cx, 0);
        assert_eq!(cy, 0);
    }
}
