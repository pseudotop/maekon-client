use crate::error::StorageError;
use chrono::{DateTime, Utc};

use super::super::SqliteStorage;

impl SqliteStorage {
    /// List closed segments whose time range falls within [from, to].
    /// Returns deserialized `SegmentSummary` structs from the `activity_segments` table.
    pub fn list_segments_between(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<oneshim_core::models::tiered_memory::SegmentSummary>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        // Check table existence (may not have run V9 migration yet)
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='activity_segments'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if !table_exists {
            return Ok(Vec::new());
        }

        let from_str = from.to_rfc3339();
        let to_str = to.to_rfc3339();

        let mut stmt = conn
            .prepare(
                "SELECT id, start_time, end_time, duration_secs, regime_id, trigger_reason, \
                 event_count, app_breakdown, category_breakdown, context_switch_count, \
                 dominant_category, avg_importance, patterns_json, content_activities_json, \
                 container_json, llm_summary \
                 FROM activity_segments \
                 WHERE start_time >= ?1 AND end_time <= ?2 \
                 ORDER BY start_time",
            )
            .map_err(|e| {
                StorageError::Internal(format!("Failed to prepare segments query: {e}"))
            })?;

        let segments: Vec<oneshim_core::models::tiered_memory::SegmentSummary> = stmt
            .query_map(rusqlite::params![from_str, to_str], |row| {
                let id: String = row.get(0)?;
                let start_str: String = row.get(1)?;
                let end_str: String = row.get(2)?;
                let dur: i64 = row.get(3)?;
                let regime: Option<String> = row.get(4)?;
                let reason_str: String = row.get(5)?;
                let events: i64 = row.get(6)?;
                let app_json: String = row.get(7)?;
                let cat_json: String = row.get(8)?;
                let switches: i64 = row.get(9)?;
                let dominant: String = row.get(10)?;
                let importance: f64 = row.get(11)?;
                let patterns_json: String = row.get(12)?;
                let content_json: String = row.get(13)?;
                let container_json: Option<String> = row.get(14)?;
                let llm_summary: Option<String> = row.get(15)?;
                Ok((
                    id,
                    start_str,
                    end_str,
                    dur,
                    regime,
                    reason_str,
                    events,
                    app_json,
                    cat_json,
                    switches,
                    dominant,
                    importance,
                    patterns_json,
                    content_json,
                    container_json,
                    llm_summary,
                ))
            })
            .map_err(|e| StorageError::Internal(format!("Failed to query segments: {e}")))?
            .filter_map(|r| r.ok())
            .filter_map(
                |(
                    id,
                    start_str,
                    end_str,
                    dur,
                    regime,
                    reason_str,
                    events,
                    app_json,
                    cat_json,
                    switches,
                    dominant,
                    importance,
                    patterns_json,
                    content_json,
                    container_json,
                    llm_summary,
                )| {
                    let start_time = chrono::DateTime::parse_from_rfc3339(&start_str)
                        .ok()?
                        .with_timezone(&chrono::Utc);
                    let end_time = chrono::DateTime::parse_from_rfc3339(&end_str)
                        .ok()?
                        .with_timezone(&chrono::Utc);
                    let trigger_reason: oneshim_core::models::tiered_memory::TriggerReason =
                        serde_json::from_str(&format!("\"{reason_str}\"")).unwrap_or(
                            oneshim_core::models::tiered_memory::TriggerReason::ScoreHigh,
                        );
                    let app_breakdown = serde_json::from_str(&app_json).unwrap_or_default();
                    let category_breakdown = serde_json::from_str(&cat_json).unwrap_or_default();
                    let patterns_detected =
                        serde_json::from_str(&patterns_json).unwrap_or_default();
                    let content_activities =
                        serde_json::from_str(&content_json).unwrap_or_default();
                    let container = container_json.and_then(|j| serde_json::from_str(&j).ok());

                    Some(oneshim_core::models::tiered_memory::SegmentSummary {
                        segment_id: id,
                        start_time,
                        end_time,
                        duration_secs: dur as u64,
                        regime_id: regime,
                        trigger_reason,
                        event_count: events as u32,
                        app_breakdown,
                        category_breakdown,
                        context_switch_count: switches as u32,
                        dominant_category: dominant,
                        avg_importance: importance as f32,
                        patterns_detected,
                        content_activities,
                        container,
                        llm_summary,
                    })
                },
            )
            .collect();

        Ok(segments)
    }
}
