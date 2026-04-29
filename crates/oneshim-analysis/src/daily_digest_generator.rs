use std::collections::HashMap;

use chrono::{NaiveDate, Utc};

use oneshim_core::models::daily_digest::{
    self, ContentBrief, DailyDigest, DailyStatistics, DayComparison, TimelineEntry,
};
use oneshim_core::models::tiered_memory::SegmentSummary;

/// Pure algorithm component that aggregates `SegmentSummary` data into a `DailyDigest`.
///
/// Takes a slice of segments for the target day plus an optional previous digest
/// for day-over-day comparison. No I/O — all data is passed in.
pub struct DailyDigestGenerator;

impl DailyDigestGenerator {
    /// Generate a daily digest from closed segments within the given day.
    pub fn generate(
        segments: &[SegmentSummary],
        date: NaiveDate,
        prev_digest: Option<&DailyDigest>,
    ) -> DailyDigest {
        let timeline = Self::build_timeline(segments);
        let statistics = Self::compute_statistics(segments, prev_digest);

        DailyDigest {
            date,
            insight: None, // Filled by DailyInsightGenerator in scheduler aggregation loop
            timeline,
            statistics,
            generated_at: Utc::now(),
        }
    }

    /// Build timeline entries from segments, sorted by start_time.
    fn build_timeline(segments: &[SegmentSummary]) -> Vec<TimelineEntry> {
        let mut entries: Vec<TimelineEntry> = segments
            .iter()
            .map(|seg| {
                let regime_label = seg
                    .regime_id
                    .as_deref()
                    .unwrap_or(&seg.dominant_category)
                    .to_string();
                let regime_color = daily_digest::regime_color(&regime_label).to_string();
                let dominant_app = Self::find_dominant_app(&seg.app_breakdown);
                let content_summary = Self::build_content_summary(&seg.content_activities);

                TimelineEntry {
                    segment_id: seg.segment_id.clone(),
                    start_time: seg.start_time,
                    end_time: seg.end_time,
                    duration_mins: (seg.duration_secs / 60) as u32,
                    regime_label,
                    regime_color,
                    dominant_app,
                    content_summary,
                    annotation: None, // Filled by DailyInsightGenerator
                }
            })
            .collect();

        entries.sort_by_key(|e| e.start_time);
        entries
    }

    /// Find the app with the highest duration from the app breakdown map.
    fn find_dominant_app(app_breakdown: &HashMap<String, u64>) -> String {
        app_breakdown
            .iter()
            .max_by_key(|(_, &dur)| dur)
            .map(|(app, _)| app.clone())
            .unwrap_or_default()
    }

    /// Build top-3 content summaries from content activities sorted by duration.
    fn build_content_summary(
        activities: &[oneshim_core::models::tiered_memory::ContentActivity],
    ) -> Vec<ContentBrief> {
        let mut sorted: Vec<_> = activities.iter().collect();
        sorted.sort_by_key(|a| std::cmp::Reverse(a.duration_secs));

        sorted
            .into_iter()
            .take(3)
            .map(|ca| ContentBrief {
                content: ca.content_label.clone(),
                work_type: ca.work_type,
                mins: (ca.duration_secs / 60) as u32,
            })
            .collect()
    }

    /// Compute aggregate statistics for the day.
    fn compute_statistics(
        segments: &[SegmentSummary],
        prev_digest: Option<&DailyDigest>,
    ) -> DailyStatistics {
        let deep_work_hours: f32 = segments
            .iter()
            .filter(|s| daily_digest::is_deep_work(s.regime_id.as_deref(), &s.dominant_category))
            .map(|s| s.duration_secs as f32 / 3600.0)
            .sum();

        let communication_hours: f32 = segments
            .iter()
            .filter(|s| {
                daily_digest::is_communication(s.regime_id.as_deref(), &s.dominant_category)
            })
            .map(|s| s.duration_secs as f32 / 3600.0)
            .sum();

        let meeting_hours: f32 = segments
            .iter()
            .filter(|s| daily_digest::is_meeting(&s.dominant_category))
            .map(|s| s.duration_secs as f32 / 3600.0)
            .sum();

        let context_switches: u32 = segments.iter().map(|s| s.context_switch_count).sum();

        // Longest focus block (deep work segments only)
        let (longest_focus_mins, longest_focus_content) = segments
            .iter()
            .filter(|s| daily_digest::is_deep_work(s.regime_id.as_deref(), &s.dominant_category))
            .map(|s| {
                let mins = (s.duration_secs / 60) as u32;
                let content = s
                    .content_activities
                    .iter()
                    .max_by_key(|ca| ca.duration_secs)
                    .map(|ca| ca.content_label.clone())
                    .unwrap_or_default();
                (mins, content)
            })
            .max_by_key(|(mins, _)| *mins)
            .unwrap_or((0, String::new()));

        // Regime distribution as percentage
        let regime_distribution = Self::compute_regime_distribution(segments);

        // Comparison with previous day
        let comparison = prev_digest.map(|prev| DayComparison {
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

    /// Compute regime distribution as percentage (0-100) of total duration.
    fn compute_regime_distribution(segments: &[SegmentSummary]) -> HashMap<String, u32> {
        let total_secs: u64 = segments.iter().map(|s| s.duration_secs).sum();
        if total_secs == 0 {
            return HashMap::new();
        }

        let mut duration_by_regime: HashMap<String, u64> = HashMap::new();
        for seg in segments {
            let label = seg
                .regime_id
                .as_deref()
                .unwrap_or(&seg.dominant_category)
                .to_string();
            *duration_by_regime.entry(label).or_default() += seg.duration_secs;
        }

        duration_by_regime
            .into_iter()
            .map(|(label, secs)| {
                let pct = (secs as f64 / total_secs as f64 * 100.0).round() as u32;
                (label, pct)
            })
            .collect()
    }
}

// Classification helpers (regime_color, is_deep_work, is_communication, is_meeting)
// are now shared from oneshim_core::models::daily_digest.

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use oneshim_core::models::tiered_memory::{
        ContentActivity, ContentType, EngagementMetrics, TriggerReason, WorkType,
    };

    fn make_segment(
        id: &str,
        duration_secs: u64,
        dominant_category: &str,
        regime_id: Option<&str>,
        context_switches: u32,
        apps: HashMap<String, u64>,
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
            app_breakdown: apps,
            category_breakdown: HashMap::new(),
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

    #[test]
    fn generate_from_test_segments() {
        let date = Utc::now().date_naive();

        let segments = vec![
            make_segment(
                "seg-1",
                2700, // 45 min
                "Development",
                Some("Deep Focus"),
                3,
                HashMap::from([("VSCode".to_string(), 2700u64)]),
                vec![
                    make_content_activity("auth.rs", 1800, WorkType::ActiveCoding),
                    make_content_activity("tests.rs", 900, WorkType::ActiveCoding),
                ],
            ),
            make_segment(
                "seg-2",
                900, // 15 min
                "Communication",
                Some("Communication"),
                1,
                HashMap::from([("Slack".to_string(), 900u64)]),
                vec![make_content_activity(
                    "#engineering",
                    900,
                    WorkType::ActiveMeeting,
                )],
            ),
        ];

        let digest = DailyDigestGenerator::generate(&segments, date, None);

        assert_eq!(digest.date, date);
        assert!(digest.insight.is_none());
        assert_eq!(digest.timeline.len(), 2);

        // Deep work: 2700s = 0.75h
        assert!((digest.statistics.deep_work_hours - 0.75).abs() < 0.01);

        // Communication: 900s = 0.25h
        assert!((digest.statistics.communication_hours - 0.25).abs() < 0.01);

        // Context switches total
        assert_eq!(digest.statistics.context_switches, 4);

        // Longest focus: 2700/60 = 45 min
        assert_eq!(digest.statistics.longest_focus_mins, 45);
        assert_eq!(digest.statistics.longest_focus_content, "auth.rs");

        // Regime distribution
        assert!(digest
            .statistics
            .regime_distribution
            .contains_key("Deep Focus"));
        assert!(digest
            .statistics
            .regime_distribution
            .contains_key("Communication"));

        // No comparison without previous digest
        assert!(digest.statistics.comparison.is_none());
    }

    #[test]
    fn empty_day_returns_zeroed_stats() {
        let date = Utc::now().date_naive();
        let digest = DailyDigestGenerator::generate(&[], date, None);

        assert_eq!(digest.timeline.len(), 0);
        assert!((digest.statistics.deep_work_hours - 0.0).abs() < f32::EPSILON);
        assert!((digest.statistics.communication_hours - 0.0).abs() < f32::EPSILON);
        assert!((digest.statistics.meeting_hours - 0.0).abs() < f32::EPSILON);
        assert_eq!(digest.statistics.context_switches, 0);
        assert_eq!(digest.statistics.longest_focus_mins, 0);
        assert!(digest.statistics.longest_focus_content.is_empty());
        assert!(digest.statistics.regime_distribution.is_empty());
        assert!(digest.statistics.comparison.is_none());
    }

    #[test]
    fn comparison_delta_with_previous_digest() {
        let date = Utc::now().date_naive();

        let prev = DailyDigest {
            date: date - chrono::Duration::days(1),
            insight: None,
            timeline: vec![],
            statistics: DailyStatistics {
                deep_work_hours: 3.0,
                communication_hours: 1.5,
                meeting_hours: 0.5,
                context_switches: 10,
                longest_focus_mins: 60,
                longest_focus_content: "old.rs".to_string(),
                regime_distribution: HashMap::new(),
                comparison: None,
            },
            generated_at: Utc::now(),
        };

        // Current day: 4h deep work, 1h communication, 5 switches
        let segments = vec![
            make_segment(
                "seg-1",
                14400, // 4h
                "Development",
                Some("Deep Focus"),
                3,
                HashMap::from([("VSCode".to_string(), 14400u64)]),
                vec![make_content_activity(
                    "main.rs",
                    14400,
                    WorkType::ActiveCoding,
                )],
            ),
            make_segment(
                "seg-2",
                3600, // 1h
                "Communication",
                Some("Communication"),
                2,
                HashMap::from([("Slack".to_string(), 3600u64)]),
                vec![],
            ),
        ];

        let digest = DailyDigestGenerator::generate(&segments, date, Some(&prev));

        let comp = digest
            .statistics
            .comparison
            .expect("Should have comparison");
        // deep work delta: 4.0 - 3.0 = 1.0
        assert!((comp.deep_work_delta - 1.0).abs() < 0.01);
        // comm delta: 1.0 - 1.5 = -0.5
        assert!((comp.communication_delta - (-0.5)).abs() < 0.01);
        // context switch delta: 5 - 10 = -5
        assert_eq!(comp.context_switch_delta, -5);
    }

    #[test]
    fn timeline_sorted_by_start_time() {
        let now = Utc::now();
        let date = now.date_naive();

        let earlier = SegmentSummary {
            segment_id: "seg-early".to_string(),
            start_time: now - Duration::hours(4),
            end_time: now - Duration::hours(3),
            duration_secs: 3600,
            regime_id: Some("Development".to_string()),
            trigger_reason: TriggerReason::RegimeChange,
            event_count: 5,
            app_breakdown: HashMap::new(),
            category_breakdown: HashMap::new(),
            context_switch_count: 1,
            dominant_category: "Development".to_string(),
            avg_importance: 0.5,
            patterns_detected: vec![],
            content_activities: vec![],
            container: None,
            llm_summary: None,
        };

        let later = SegmentSummary {
            segment_id: "seg-late".to_string(),
            start_time: now - Duration::hours(2),
            end_time: now - Duration::hours(1),
            duration_secs: 3600,
            regime_id: Some("Communication".to_string()),
            trigger_reason: TriggerReason::RegimeChange,
            event_count: 5,
            app_breakdown: HashMap::new(),
            category_breakdown: HashMap::new(),
            context_switch_count: 0,
            dominant_category: "Communication".to_string(),
            avg_importance: 0.5,
            patterns_detected: vec![],
            content_activities: vec![],
            container: None,
            llm_summary: None,
        };

        // Pass in reverse order
        let digest = DailyDigestGenerator::generate(&[later, earlier], date, None);

        assert_eq!(digest.timeline[0].segment_id, "seg-early");
        assert_eq!(digest.timeline[1].segment_id, "seg-late");
    }

    #[test]
    fn regime_color_mapping() {
        // Delegated to oneshim_core::models::daily_digest::regime_color
        assert_eq!(daily_digest::regime_color("Deep Focus"), "#3B82F6");
        assert_eq!(daily_digest::regime_color("Development"), "#3B82F6");
        assert_eq!(daily_digest::regime_color("Communication"), "#F59E0B");
        assert_eq!(daily_digest::regime_color("Research"), "#10B981");
        assert_eq!(daily_digest::regime_color("Meeting"), "#8B5CF6");
        assert_eq!(daily_digest::regime_color("Idle"), "#E5E7EB");
        assert_eq!(daily_digest::regime_color("Unknown"), "#6B7280");
    }

    #[test]
    fn content_summary_top_3() {
        let date = Utc::now().date_naive();

        let segments = vec![make_segment(
            "seg-1",
            7200,
            "Development",
            Some("Deep Focus"),
            0,
            HashMap::from([("VSCode".to_string(), 7200u64)]),
            vec![
                make_content_activity("a.rs", 100, WorkType::ActiveCoding),
                make_content_activity("b.rs", 200, WorkType::ActiveCoding),
                make_content_activity("c.rs", 300, WorkType::ActiveCoding),
                make_content_activity("d.rs", 400, WorkType::ActiveCoding),
            ],
        )];

        let digest = DailyDigestGenerator::generate(&segments, date, None);

        // Should have top 3 by duration
        assert_eq!(digest.timeline[0].content_summary.len(), 3);
        assert_eq!(digest.timeline[0].content_summary[0].content, "d.rs");
        assert_eq!(digest.timeline[0].content_summary[1].content, "c.rs");
        assert_eq!(digest.timeline[0].content_summary[2].content, "b.rs");
    }

    #[test]
    fn dominant_app_from_breakdown() {
        let date = Utc::now().date_naive();

        let segments = vec![make_segment(
            "seg-1",
            3600,
            "Development",
            Some("Deep Focus"),
            0,
            HashMap::from([
                ("VSCode".to_string(), 2400u64),
                ("Terminal".to_string(), 1200u64),
            ]),
            vec![],
        )];

        let digest = DailyDigestGenerator::generate(&segments, date, None);
        assert_eq!(digest.timeline[0].dominant_app, "VSCode");
    }
}
