//! Dashboard service — digest generation, statistics, and caching.

use chrono::{NaiveDate, Utc};
use std::collections::HashMap;
use tracing::warn;

use oneshim_api_contracts::dashboard::{RawContentActivity, RawContentActivityBrief};
use oneshim_core::error::CoreError;
use oneshim_core::models::daily_digest::{
    self, ContentBrief, DailyDigest, DailyStatistics, DayComparison, TimelineEntry,
};
use oneshim_core::models::storage_records::SegmentSummaryRecord;
use oneshim_core::models::tiered_memory::WorkType;

use crate::AppState;

/// Generate or retrieve a cached daily digest for the given date.
///
/// Iter-96: return `CoreError` instead of `String` so the typed
/// `err.code()` survives through to the handler's
/// `From<CoreError> for ApiError` conversion — the stringified form
/// lost wire codes at the service boundary, collapsing every storage
/// failure into `ApiError::Internal`.
pub fn get_or_generate_digest(
    state: &AppState,
    date_str: &str,
    date: NaiveDate,
) -> Result<DailyDigest, CoreError> {
    // 1. Check cache
    if let Some(cached) = state.core.storage.get_daily_digest(date_str)? {
        return Ok(cached);
    }

    // 2. Generate from segments
    let segment_records = state.core.storage.get_segments_for_date(date_str)?;

    let digest = build_daily_digest(&segment_records, date, state);

    // 3. Cache the result
    if let Err(e) = state.core.storage.save_daily_digest(&digest) {
        warn!("Failed to cache daily digest: {e}");
    }

    Ok(digest)
}

fn build_daily_digest(
    records: &[SegmentSummaryRecord],
    date: NaiveDate,
    state: &AppState,
) -> DailyDigest {
    let timeline: Vec<TimelineEntry> = records
        .iter()
        .map(|r| {
            let regime_label = r
                .regime_id
                .clone()
                .unwrap_or_else(|| r.dominant_category.clone());
            let regime_color = daily_digest::regime_color(&regime_label).to_string();
            let content_summary = parse_content_briefs(&r.content_activities_json);
            let dominant_app = parse_dominant_app(&r.app_breakdown);

            TimelineEntry {
                segment_id: r.segment_id.clone(),
                start_time: r.start_time.parse().unwrap_or_else(|_| Utc::now()),
                end_time: r.end_time.parse().unwrap_or_else(|_| Utc::now()),
                duration_mins: (r.duration_secs / 60) as u32,
                regime_label,
                regime_color,
                dominant_app,
                content_summary,
                annotation: None,
            }
        })
        .collect();

    let statistics = compute_statistics(records, state, &date);

    DailyDigest {
        date,
        insight: None,
        timeline,
        statistics,
        generated_at: Utc::now(),
    }
}

fn compute_statistics(
    records: &[SegmentSummaryRecord],
    state: &AppState,
    date: &NaiveDate,
) -> DailyStatistics {
    let deep_work_hours: f32 = records
        .iter()
        .filter(|r| daily_digest::is_deep_work(r.regime_id.as_deref(), &r.dominant_category))
        .map(|r| r.duration_secs as f32 / 3600.0)
        .sum();

    let communication_hours: f32 = records
        .iter()
        .filter(|r| daily_digest::is_communication(r.regime_id.as_deref(), &r.dominant_category))
        .map(|r| r.duration_secs as f32 / 3600.0)
        .sum();

    let meeting_hours: f32 = records
        .iter()
        .filter(|r| daily_digest::is_meeting(&r.dominant_category))
        .map(|r| r.duration_secs as f32 / 3600.0)
        .sum();

    let context_switches: u32 = records.iter().map(|r| r.context_switch_count).sum();

    let (longest_focus_mins, longest_focus_content) = records
        .iter()
        .filter(|r| daily_digest::is_deep_work(r.regime_id.as_deref(), &r.dominant_category))
        .map(|r| {
            let mins = (r.duration_secs / 60) as u32;
            let content = parse_top_content(&r.content_activities_json);
            (mins, content)
        })
        .max_by_key(|(mins, _)| *mins)
        .unwrap_or((0, String::new()));

    let total_secs: u64 = records.iter().map(|r| r.duration_secs).sum();
    let regime_distribution = if total_secs > 0 {
        let mut dur_by_regime: HashMap<String, u64> = HashMap::new();
        for r in records {
            let label = r
                .regime_id
                .clone()
                .unwrap_or_else(|| r.dominant_category.clone());
            *dur_by_regime.entry(label).or_default() += r.duration_secs;
        }
        dur_by_regime
            .into_iter()
            .map(|(label, secs)| {
                let pct = (secs as f64 / total_secs as f64 * 100.0).round() as u32;
                (label, pct)
            })
            .collect()
    } else {
        HashMap::new()
    };

    let prev_date = date
        .pred_opt()
        .unwrap_or(*date)
        .format("%Y-%m-%d")
        .to_string();
    let comparison = state
        .core
        .storage
        .get_daily_digest(&prev_date)
        .ok()
        .flatten()
        .map(|prev| DayComparison {
            deep_work_delta: deep_work_hours - prev.statistics.deep_work_hours,
            communication_delta: communication_hours - prev.statistics.communication_hours,
            context_switch_delta: context_switches as i32 - prev.statistics.context_switches as i32,
        });

    DailyStatistics {
        deep_work_hours,
        communication_hours,
        meeting_hours,
        context_switches,
        longest_focus_mins,
        longest_focus_content,
        regime_distribution,
        comparison,
    }
}

fn parse_content_briefs(json_str: &str) -> Vec<ContentBrief> {
    let activities: Vec<RawContentActivity> = serde_json::from_str(json_str).unwrap_or_default();
    let mut sorted = activities;
    sorted.sort_by(|a, b| {
        b.duration_secs
            .unwrap_or(0)
            .cmp(&a.duration_secs.unwrap_or(0))
    });

    sorted
        .into_iter()
        .take(3)
        .map(|a| ContentBrief {
            content: a.content_label.unwrap_or_default(),
            work_type: parse_work_type(a.work_type.as_deref()),
            mins: (a.duration_secs.unwrap_or(0) / 60) as u32,
        })
        .collect()
}

pub(crate) fn parse_work_type(s: Option<&str>) -> WorkType {
    match s {
        Some("ACTIVE_CODING") => WorkType::ActiveCoding,
        Some("CODE_REVIEW") => WorkType::CodeReview,
        Some("ACTIVE_MEETING") => WorkType::ActiveMeeting,
        Some("PASSIVE_MEETING") => WorkType::PassiveMeeting,
        Some("BROWSING") => WorkType::Browsing,
        _ => WorkType::Unknown,
    }
}

fn parse_dominant_app(json_str: &str) -> String {
    let breakdown: HashMap<String, u64> = serde_json::from_str(json_str).unwrap_or_default();
    breakdown
        .into_iter()
        .max_by_key(|(_, dur)| *dur)
        .map(|(app, _)| app)
        .unwrap_or_default()
}

fn parse_top_content(json_str: &str) -> String {
    let activities: Vec<RawContentActivityBrief> =
        serde_json::from_str(json_str).unwrap_or_default();
    activities
        .into_iter()
        .max_by_key(|a| a.duration_secs.unwrap_or(0))
        .and_then(|a| a.content_label)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_work_type_variants() {
        assert_eq!(
            parse_work_type(Some("ACTIVE_CODING")),
            WorkType::ActiveCoding
        );
        assert_eq!(parse_work_type(Some("CODE_REVIEW")), WorkType::CodeReview);
        assert_eq!(parse_work_type(None), WorkType::Unknown);
    }

    #[test]
    fn parse_dominant_app_from_json() {
        let json = r#"{"VSCode": 2400, "Terminal": 1200}"#;
        assert_eq!(parse_dominant_app(json), "VSCode");
    }

    #[test]
    fn parse_dominant_app_empty() {
        assert!(parse_dominant_app("{}").is_empty());
        assert!(parse_dominant_app("").is_empty());
    }

    #[test]
    fn parse_content_briefs_top3() {
        let json = r#"[
            {"content_label": "a.rs", "duration_secs": 100, "work_type": "ACTIVE_CODING"},
            {"content_label": "b.rs", "duration_secs": 200, "work_type": "ACTIVE_CODING"},
            {"content_label": "c.rs", "duration_secs": 300, "work_type": "CODE_REVIEW"},
            {"content_label": "d.rs", "duration_secs": 400, "work_type": "ACTIVE_CODING"}
        ]"#;
        let briefs = parse_content_briefs(json);
        assert_eq!(briefs.len(), 3);
        assert_eq!(briefs[0].content, "d.rs");
        assert_eq!(briefs[1].content, "c.rs");
        assert_eq!(briefs[2].content, "b.rs");
    }

    #[test]
    fn parse_content_briefs_empty_json() {
        let briefs = parse_content_briefs("");
        assert!(briefs.is_empty());
    }
}
