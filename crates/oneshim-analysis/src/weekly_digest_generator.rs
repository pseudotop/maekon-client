use std::collections::HashMap;

use chrono::{DateTime, Utc};

use oneshim_core::models::tiered_memory::SegmentSummary;
use oneshim_core::models::tiered_memory::WorkType;
use oneshim_core::models::weekly_digest::{ContentRanking, WeekComparison, WeeklyDigest};

/// Accumulator mapping content_label → (total_secs, work_type_key → (WorkType, duration)).
type ContentAccumulator = HashMap<String, (u64, HashMap<String, (WorkType, u64)>)>;

/// Pure algorithm component that aggregates `SegmentSummary` data into a `WeeklyDigest`.
///
/// Takes a slice of segments for the target week plus an optional previous digest
/// for week-over-week comparison. No I/O — all data is passed in.
pub struct WeeklyDigestGenerator;

impl WeeklyDigestGenerator {
    /// Generate a weekly digest from closed segments within the given week range.
    pub fn generate(
        segments: &[SegmentSummary],
        week_start: DateTime<Utc>,
        week_end: DateTime<Utc>,
        prev_digest: Option<&WeeklyDigest>,
    ) -> WeeklyDigest {
        let total_tracked_hours: f32 = segments
            .iter()
            .map(|s| s.duration_secs as f32 / 3600.0)
            .sum();

        // Regime breakdown: sum duration (hours) per regime_id
        let regime_breakdown = Self::compute_regime_breakdown(segments);

        // Category breakdown: sum from each segment's category_breakdown (secs -> hours)
        let category_breakdown = Self::compute_category_breakdown(segments);

        // Top content: flatten content_activities, group by label, sum duration, rank
        let top_content = Self::compute_top_content(segments);

        // Deep work hours: segments with dominant_category == "Development"
        let deep_work_hours: f32 = segments
            .iter()
            .filter(|s| s.dominant_category == "Development")
            .map(|s| s.duration_secs as f32 / 3600.0)
            .sum();

        // Communication hours: segments with dominant_category == "Communication"
        let communication_hours: f32 = segments
            .iter()
            .filter(|s| s.dominant_category == "Communication")
            .map(|s| s.duration_secs as f32 / 3600.0)
            .sum();

        // Context switches: sum across all segments
        let context_switches_total: u32 = segments.iter().map(|s| s.context_switch_count).sum();

        // Longest deep work segment (in minutes)
        let longest_deep_work_segment_mins = segments
            .iter()
            .filter(|s| s.dominant_category == "Development")
            .map(|s| (s.duration_secs / 60) as u32)
            .max()
            .unwrap_or(0);

        // Comparison with previous week
        let comparison = prev_digest.map(|prev| WeekComparison {
            deep_work_delta_hours: deep_work_hours - prev.deep_work_hours,
            communication_delta_hours: communication_hours - prev.communication_hours,
            context_switch_delta: context_switches_total as i32
                - prev.context_switches_total as i32,
            trend_summary: format_trend_summary(
                deep_work_hours,
                prev.deep_work_hours,
                communication_hours,
                prev.communication_hours,
            ),
        });

        WeeklyDigest {
            week_start,
            week_end,
            total_tracked_hours,
            regime_breakdown,
            category_breakdown,
            top_content,
            deep_work_hours,
            communication_hours,
            context_switches_total,
            longest_deep_work_segment_mins,
            comparison,
            llm_narrative: None, // Set by scheduler if LLM enabled
        }
    }

    /// Sum duration (hours) per regime label.
    fn compute_regime_breakdown(segments: &[SegmentSummary]) -> HashMap<String, f32> {
        let mut breakdown: HashMap<String, f32> = HashMap::new();
        for seg in segments {
            let label = seg.regime_id.as_deref().unwrap_or("Unknown").to_string();
            *breakdown.entry(label).or_default() += seg.duration_secs as f32 / 3600.0;
        }
        breakdown
    }

    /// Sum category durations (hours) from each segment's category_breakdown.
    fn compute_category_breakdown(segments: &[SegmentSummary]) -> HashMap<String, f32> {
        let mut breakdown: HashMap<String, f32> = HashMap::new();
        for seg in segments {
            for (cat, secs) in &seg.category_breakdown {
                *breakdown.entry(cat.clone()).or_insert(0.0) += *secs as f32 / 3600.0;
            }
        }
        breakdown
    }

    /// Flatten content activities across segments, group by label, rank by total time.
    fn compute_top_content(segments: &[SegmentSummary]) -> Vec<ContentRanking> {
        // Accumulate (total_secs, dominant work_type with max duration) per content_label
        // Use String key for work_type since WorkType doesn't implement Hash.
        let mut content_map: ContentAccumulator = HashMap::new();

        for seg in segments {
            for ca in &seg.content_activities {
                let entry = content_map
                    .entry(ca.content_label.clone())
                    .or_insert_with(|| (0, HashMap::new()));
                entry.0 += ca.duration_secs;
                let wt_key = format!("{:?}", ca.work_type);
                let wt_entry = entry.1.entry(wt_key).or_insert((ca.work_type, 0));
                wt_entry.1 += ca.duration_secs;
            }
        }

        // Convert to rankings sorted by total duration descending
        let mut rankings: Vec<ContentRanking> = content_map
            .into_iter()
            .map(|(label, (total_secs, work_types))| {
                let dominant_work_type = work_types
                    .into_values()
                    .max_by_key(|(_, dur)| *dur)
                    .map(|(wt, _)| wt)
                    .unwrap_or_default();

                ContentRanking {
                    content_label: label,
                    total_mins: (total_secs / 60) as u32,
                    dominant_work_type,
                }
            })
            .collect();

        rankings.sort_by_key(|r| std::cmp::Reverse(r.total_mins));

        // Return top 10
        rankings.truncate(10);
        rankings
    }
}

/// Format a human-readable trend summary comparing current vs previous week.
fn format_trend_summary(dw: f32, prev_dw: f32, comm: f32, prev_comm: f32) -> String {
    let dw_pct = if prev_dw > 0.0 {
        ((dw - prev_dw) / prev_dw * 100.0) as i32
    } else {
        0
    };
    let comm_pct = if prev_comm > 0.0 {
        ((comm - prev_comm) / prev_comm * 100.0) as i32
    } else {
        0
    };

    let dw_trend = if dw_pct >= 0 {
        format!("Deep work up {dw_pct}%")
    } else {
        format!("Deep work down {}%", dw_pct.abs())
    };
    let comm_trend = if comm_pct >= 0 {
        format!("communication up {comm_pct}%")
    } else {
        format!("communication down {}%", comm_pct.abs())
    };

    format!("{dw_trend}, {comm_trend}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use oneshim_core::models::tiered_memory::{
        ContentActivity, ContentType, EngagementMetrics, TriggerReason,
    };

    // ── Helpers ────────────────────────────────────────────────────

    fn make_segment(
        id: &str,
        duration_secs: u64,
        dominant_category: &str,
        regime_id: Option<&str>,
        context_switches: u32,
        categories: HashMap<String, u64>,
        content_activities: Vec<ContentActivity>,
    ) -> SegmentSummary {
        let now = Utc::now();
        SegmentSummary {
            segment_id: id.to_string(),
            start_time: now - Duration::seconds(duration_secs as i64),
            end_time: now,
            duration_secs,
            regime_id: regime_id.map(String::from),
            trigger_reason: TriggerReason::RegimeChange,
            event_count: 10,
            app_breakdown: HashMap::new(),
            category_breakdown: categories,
            context_switch_count: context_switches,
            dominant_category: dominant_category.to_string(),
            avg_importance: 0.5,
            patterns_detected: vec![],
            content_activities,
            container: None,
            llm_summary: None,
        }
    }

    fn make_content_activity(
        label: &str,
        duration_secs: u64,
        work_type: WorkType,
    ) -> ContentActivity {
        ContentActivity {
            content_label: label.to_string(),
            content_type: ContentType::File,
            start_time: Utc::now() - Duration::seconds(duration_secs as i64),
            duration_secs,
            confidence: 0.9,
            work_type,
            engagement: EngagementMetrics::default(),
            gui_summary: None,
        }
    }

    fn week_range() -> (DateTime<Utc>, DateTime<Utc>) {
        let end = Utc::now();
        let start = end - Duration::days(7);
        (start, end)
    }

    // ── Tests ──────────────────────────────────────────────────────

    #[test]
    fn generate_from_test_segments() {
        let (start, end) = week_range();

        let categories1 = HashMap::from([
            ("Development".to_string(), 2700u64),  // 45 min
            ("Communication".to_string(), 300u64), // 5 min
        ]);
        let categories2 = HashMap::from([
            ("Communication".to_string(), 3600u64), // 60 min
        ]);

        let segments = vec![
            make_segment(
                "seg-1",
                3000,
                "Development",
                Some("deep_work"),
                3,
                categories1,
                vec![
                    make_content_activity("main.rs", 1800, WorkType::ActiveCoding),
                    make_content_activity("auth.rs", 900, WorkType::ActiveCoding),
                ],
            ),
            make_segment(
                "seg-2",
                3600,
                "Communication",
                Some("meetings"),
                1,
                categories2,
                vec![make_content_activity(
                    "Slack: #general",
                    3600,
                    WorkType::ActiveMeeting,
                )],
            ),
        ];

        let digest = WeeklyDigestGenerator::generate(&segments, start, end, None);

        // Total hours: (3000 + 3600) / 3600 ≈ 1.833
        assert!((digest.total_tracked_hours - 1.8333).abs() < 0.1);

        // Deep work hours from segments with dominant_category == Development
        assert!((digest.deep_work_hours - 3000.0 / 3600.0).abs() < 0.01);

        // Communication hours
        assert!((digest.communication_hours - 1.0).abs() < 0.01);

        // Context switches total
        assert_eq!(digest.context_switches_total, 4);

        // Longest deep work segment: 3000 / 60 = 50 min
        assert_eq!(digest.longest_deep_work_segment_mins, 50);

        // Regime breakdown should have deep_work and meetings
        assert!(digest.regime_breakdown.contains_key("deep_work"));
        assert!(digest.regime_breakdown.contains_key("meetings"));

        // Category breakdown
        assert!(digest.category_breakdown.contains_key("Development"));
        assert!(digest.category_breakdown.contains_key("Communication"));

        // Top content: main.rs should be first (1800s > 3600s for Slack actually)
        assert!(!digest.top_content.is_empty());

        // No comparison without previous digest
        assert!(digest.comparison.is_none());

        // No LLM narrative by default
        assert!(digest.llm_narrative.is_none());
    }

    #[test]
    fn comparison_delta_calculation() {
        let (start, end) = week_range();

        let prev_digest = WeeklyDigest {
            week_start: start - Duration::days(7),
            week_end: start,
            total_tracked_hours: 30.0,
            regime_breakdown: HashMap::new(),
            category_breakdown: HashMap::new(),
            top_content: vec![],
            deep_work_hours: 20.0,
            communication_hours: 10.0,
            context_switches_total: 50,
            longest_deep_work_segment_mins: 90,
            comparison: None,
            llm_narrative: None,
        };

        // Current week: 25h deep work, 8h communication, 40 switches
        let categories = HashMap::from([("Development".to_string(), 90000u64)]); // 25h
        let segments = vec![make_segment(
            "seg-1",
            90000,
            "Development",
            None,
            40,
            categories,
            vec![],
        )];

        let digest = WeeklyDigestGenerator::generate(&segments, start, end, Some(&prev_digest));

        let comp = digest.comparison.expect("Should have comparison");

        // Deep work delta: 25.0 - 20.0 = 5.0
        assert!((comp.deep_work_delta_hours - 5.0).abs() < 0.01);

        // Communication delta: 0.0 - 10.0 = -10.0
        assert!((comp.communication_delta_hours - (-10.0)).abs() < 0.01);

        // Context switch delta: 40 - 50 = -10
        assert_eq!(comp.context_switch_delta, -10);

        // Trend summary should contain percentages
        assert!(comp.trend_summary.contains("Deep work up"));
        assert!(comp.trend_summary.contains("communication down"));
    }

    #[test]
    fn empty_week_returns_zeros() {
        let (start, end) = week_range();

        let digest = WeeklyDigestGenerator::generate(&[], start, end, None);

        assert!((digest.total_tracked_hours - 0.0).abs() < f32::EPSILON);
        assert!((digest.deep_work_hours - 0.0).abs() < f32::EPSILON);
        assert!((digest.communication_hours - 0.0).abs() < f32::EPSILON);
        assert_eq!(digest.context_switches_total, 0);
        assert_eq!(digest.longest_deep_work_segment_mins, 0);
        assert!(digest.regime_breakdown.is_empty());
        assert!(digest.category_breakdown.is_empty());
        assert!(digest.top_content.is_empty());
        assert!(digest.comparison.is_none());
    }

    #[test]
    fn no_previous_digest_no_comparison() {
        let (start, end) = week_range();
        let categories = HashMap::from([("Development".to_string(), 3600u64)]);
        let segments = vec![make_segment(
            "seg-1",
            3600,
            "Development",
            None,
            5,
            categories,
            vec![],
        )];

        let digest = WeeklyDigestGenerator::generate(&segments, start, end, None);

        assert!(digest.comparison.is_none());
    }

    #[test]
    fn top_content_ranked_by_duration() {
        let (start, end) = week_range();
        let categories = HashMap::from([("Development".to_string(), 7200u64)]);

        let segments = vec![make_segment(
            "seg-1",
            7200,
            "Development",
            None,
            2,
            categories,
            vec![
                make_content_activity("small.rs", 300, WorkType::ActiveCoding),
                make_content_activity("big.rs", 6000, WorkType::ActiveCoding),
                make_content_activity("medium.rs", 900, WorkType::CodeReview),
            ],
        )];

        let digest = WeeklyDigestGenerator::generate(&segments, start, end, None);

        assert_eq!(digest.top_content.len(), 3);
        assert_eq!(digest.top_content[0].content_label, "big.rs");
        assert_eq!(digest.top_content[1].content_label, "medium.rs");
        assert_eq!(digest.top_content[2].content_label, "small.rs");
    }

    #[test]
    fn format_trend_summary_positive_and_negative() {
        let summary = format_trend_summary(25.0, 20.0, 8.0, 10.0);
        assert!(summary.contains("Deep work up 25%"));
        assert!(summary.contains("communication down 20%"));
    }

    #[test]
    fn format_trend_summary_zero_previous() {
        let summary = format_trend_summary(10.0, 0.0, 5.0, 0.0);
        assert!(summary.contains("Deep work up 0%"));
        assert!(summary.contains("communication up 0%"));
    }
}
