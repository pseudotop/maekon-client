//! Dashboard service — digest generation, statistics, and caching.

use chrono::{NaiveDate, Utc};
use std::collections::HashMap;
use tracing::warn;

use maekon_api_contracts::dashboard::{RawContentActivity, RawContentActivityBrief};
use maekon_core::error::CoreError;
use maekon_core::models::daily_digest::{
    self, ContentBrief, DailyDigest, DailyStatistics, DayComparison, TimelineEntry,
};
use maekon_core::models::storage_records::SegmentSummaryRecord;
use maekon_core::models::tiered_memory::{resolve_regime_label, Regime, WorkType};

use crate::AppState;

/// Generate or retrieve a cached daily digest for the given date.
///
/// Iter-96: return `CoreError` instead of `String` so the typed
/// `err.code()` survives through to the handler's
/// `From<CoreError> for ApiError` conversion — the stringified form
/// lost wire codes at the service boundary, collapsing every storage
/// failure into `ApiError::Internal`.
pub async fn get_or_generate_digest(
    state: &AppState,
    date_str: &str,
    date: NaiveDate,
) -> Result<DailyDigest, CoreError> {
    // #6276: the CURRENT day's digest is a moving target (segments keep
    // accumulating), so it must NOT be served from OR written to the cache —
    // otherwise the first view of "today" freezes the digest at its partial-day
    // state for the rest of the day. Treat the date as "today" if it matches the
    // current date in EITHER UTC or Local: handlers derive date_str from
    // Utc::now() while the scheduler rolls digests over on Local, so the union
    // conservatively avoids a near-midnight off-by-one that would cache a still-
    // accumulating day. Past days remain cacheable (finalized).
    let is_today = date == Utc::now().date_naive() || date == chrono::Local::now().date_naive();

    // 1. Check cache (skip for today — always regenerate fresh).
    if !is_today {
        if let Some(cached) = state.core.storage.get_daily_digest(date_str).await? {
            return Ok(cached);
        }
    }

    // 2. Generate from segments
    let segment_records = state.core.storage.get_segments_for_date(date_str).await?;

    let digest = build_daily_digest(&segment_records, date, state).await;

    // 3. Cache the result (skip for today — do not persist a partial-day digest).
    if !is_today {
        if let Err(e) = state.core.storage.save_daily_digest(&digest).await {
            warn!("Failed to cache daily digest: {e}");
        }
    }

    Ok(digest)
}

async fn build_daily_digest(
    records: &[SegmentSummaryRecord],
    date: NaiveDate,
    state: &AppState,
) -> DailyDigest {
    // #7678 D2: resolve human regime labels (name > auto_label) instead of
    // leaking the opaque positional `regime_id` ("regime-N") into the
    // timeline — mirrors the #7480 coaching-path fix. Best-effort: an absent
    // `regime_storage` binding or a load failure falls back to per-segment
    // `dominant_category` (same as an unresolved id) rather than failing the
    // whole digest.
    let regimes: Vec<Regime> = match state.core.regime_storage {
        Some(ref rs) => rs.load_all().await.unwrap_or_default(),
        None => Vec::new(),
    };

    let timeline: Vec<TimelineEntry> = records
        .iter()
        .map(|r| {
            let regime_label = r
                .regime_id
                .as_deref()
                .and_then(|id| resolve_regime_label(&regimes, id))
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

    let statistics = compute_statistics(records, &regimes, state, &date).await;

    DailyDigest {
        date,
        insight: None,
        timeline,
        statistics,
        generated_at: Utc::now(),
    }
}

async fn compute_statistics(
    records: &[SegmentSummaryRecord],
    regimes: &[Regime],
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

    // #7683 (sibling of the #7678 D2 timeline fix): key the distribution map
    // by the resolved human regime label (name > auto_label), not the opaque
    // positional `regime_id` ("regime-N") — that id was leaked straight into
    // the `StatisticsPanel` chart labels. Falls back to `dominant_category`
    // when the id is absent or unresolved, mirroring the timeline's fallback.
    let total_secs: u64 = records.iter().map(|r| r.duration_secs).sum();
    let regime_distribution = if total_secs > 0 {
        let mut dur_by_regime: HashMap<String, u64> = HashMap::new();
        for r in records {
            let label = r
                .regime_id
                .as_deref()
                .and_then(|id| resolve_regime_label(regimes, id))
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
        .await
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

    // #7678 D2: `build_daily_digest` must render the HUMAN regime label, not
    // the opaque positional `regime_id` ("regime-N") that production actually
    // assigns — mirrors the #7480 coaching-path fix + the maekon-analysis
    // digest test of the same name.
    mod regime_label_resolution {
        use super::*;
        use async_trait::async_trait;
        use maekon_core::error::CoreError;
        use maekon_core::models::tiered_memory::{
            Regime, RegimeFeatures, RegimeStatus, TriggerParams,
        };
        use maekon_core::ports::regime_storage::RegimeStoragePort;
        use std::sync::Arc;

        /// Manual mock (project convention: no mockall) returning a fixed regime set.
        struct FixedRegimeStorage(Vec<Regime>);

        #[async_trait]
        impl RegimeStoragePort for FixedRegimeStorage {
            async fn load_all(&self) -> Result<Vec<Regime>, CoreError> {
                Ok(self.0.clone())
            }
            async fn save_all(&self, _regimes: &[Regime]) -> Result<(), CoreError> {
                Ok(())
            }
        }

        fn make_regime(id: &str, name: Option<&str>, auto_label: &str) -> Regime {
            Regime {
                regime_id: id.to_string(),
                name: name.map(String::from),
                auto_label: auto_label.to_string(),
                centroid: RegimeFeatures::default(),
                optimal_params: TriggerParams::default(),
                sample_count: 1,
                first_seen: Utc::now(),
                last_seen: Utc::now(),
                status: RegimeStatus::Active,
            }
        }

        fn make_record(regime_id: Option<&str>, dominant_category: &str) -> SegmentSummaryRecord {
            SegmentSummaryRecord {
                segment_id: "seg-1".to_string(),
                start_time: Utc::now().to_rfc3339(),
                end_time: Utc::now().to_rfc3339(),
                duration_secs: 1800,
                dominant_category: dominant_category.to_string(),
                regime_id: regime_id.map(String::from),
                app_breakdown: "{}".to_string(),
                content_activities_json: "[]".to_string(),
                context_switch_count: 0,
                llm_summary: None,
            }
        }

        // #7738 D-4: funnel through the canonical test-state helper.
        fn test_state_with_regimes(regimes: Vec<Regime>) -> AppState {
            let mut state = crate::test_local_auth::test_app_state_with_event_capacity(8);
            state.core.regime_storage = Some(Arc::new(FixedRegimeStorage(regimes)));
            state
        }

        #[tokio::test]
        async fn resolves_human_name_not_opaque_regime_id() {
            let state = test_state_with_regimes(vec![make_regime(
                "regime-0",
                Some("Deep Focus"),
                "auto-deep-focus",
            )]);
            let records = vec![make_record(Some("regime-0"), "Development")];

            let digest = build_daily_digest(&records, Utc::now().date_naive(), &state).await;

            assert_eq!(digest.timeline[0].regime_label, "Deep Focus");
            assert_ne!(digest.timeline[0].regime_label, "regime-0");
        }

        #[tokio::test]
        async fn falls_back_to_dominant_category_when_regime_storage_absent() {
            // #7738 D-4: funnel through the canonical test-state helper.
            let state = crate::test_local_auth::test_app_state_with_event_capacity(8); // regime_storage: None
            let records = vec![make_record(Some("regime-0"), "Development")];

            let digest = build_daily_digest(&records, Utc::now().date_naive(), &state).await;

            assert_eq!(digest.timeline[0].regime_label, "Development");
            assert_ne!(digest.timeline[0].regime_label, "regime-0");
        }

        #[tokio::test]
        async fn falls_back_to_dominant_category_when_regime_unresolved() {
            // regime_id present but absent from the loaded regime set (e.g.
            // archived/evicted) — must not leak the opaque id downstream.
            let state = test_state_with_regimes(vec![]);
            let records = vec![make_record(Some("regime-unknown"), "Communication")];

            let digest = build_daily_digest(&records, Utc::now().date_naive(), &state).await;

            assert_eq!(digest.timeline[0].regime_label, "Communication");
            assert_ne!(digest.timeline[0].regime_label, "regime-unknown");
        }

        // #7683 (sibling of #7678 D2): `statistics.regime_distribution` — the
        // map that feeds the `StatisticsPanel` chart labels — must be keyed
        // by the resolved HUMAN regime label, not the opaque positional
        // `regime_id` production actually assigns. Before this fix,
        // `compute_statistics` built the key directly from
        // `r.regime_id.unwrap_or(dominant_category)`, so this test would
        // have asserted `contains_key("deep-focus-1")` (the raw id) and
        // failed the `contains_key("Deep Focus")` / `!contains_key(id)`
        // assertions below.
        #[tokio::test]
        async fn regime_distribution_keyed_by_human_label_not_opaque_regime_id() {
            let state = test_state_with_regimes(vec![make_regime(
                "deep-focus-1",
                Some("Deep Focus"),
                "auto-deep-focus",
            )]);
            let records = vec![make_record(Some("deep-focus-1"), "Development")];

            let digest = build_daily_digest(&records, Utc::now().date_naive(), &state).await;

            assert!(digest
                .statistics
                .regime_distribution
                .contains_key("Deep Focus"));
            assert!(!digest
                .statistics
                .regime_distribution
                .contains_key("deep-focus-1"));
        }

        // Companion case: an id absent from the loaded regime set must fall
        // back to `dominant_category` in the distribution map too (mirrors
        // the timeline fallback), never leaking the opaque id.
        #[tokio::test]
        async fn regime_distribution_falls_back_to_dominant_category_when_regime_unresolved() {
            let state = test_state_with_regimes(vec![]);
            let records = vec![make_record(Some("regime-unknown"), "Communication")];

            let digest = build_daily_digest(&records, Utc::now().date_naive(), &state).await;

            assert!(digest
                .statistics
                .regime_distribution
                .contains_key("Communication"));
            assert!(!digest
                .statistics
                .regime_distribution
                .contains_key("regime-unknown"));
        }
    }
}
