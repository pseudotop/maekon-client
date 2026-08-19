use std::collections::HashMap;

use chrono::{NaiveDate, Utc};

use maekon_core::models::daily_digest::{
    self, ContentBrief, DailyDigest, DailyStatistics, DayComparison, TimelineEntry,
};
use maekon_core::models::tiered_memory::{resolve_regime_label, Regime, SegmentSummary};

/// Pure algorithm component that aggregates `SegmentSummary` data into a `DailyDigest`.
///
/// Takes a slice of segments for the target day plus an optional previous digest
/// for day-over-day comparison. No I/O — all data is passed in.
pub struct DailyDigestGenerator;

impl DailyDigestGenerator {
    /// Generate a daily digest from closed segments within the given day.
    ///
    /// `regimes` resolves each segment's opaque `regime_id` ("regime-N") to its
    /// human-readable label (mirrors the #7480 coaching-path fix, #7678 D2) — pass
    /// an empty slice when no regime list is available; the timeline then falls
    /// back to `dominant_category` per segment rather than leaking the opaque id.
    pub fn generate(
        segments: &[SegmentSummary],
        date: NaiveDate,
        prev_digest: Option<&DailyDigest>,
        regimes: &[Regime],
    ) -> DailyDigest {
        let timeline = Self::build_timeline(segments, regimes);
        let statistics = Self::compute_statistics(segments, prev_digest, regimes);

        DailyDigest {
            date,
            insight: None, // Filled by DailyInsightGenerator in scheduler aggregation loop
            timeline,
            statistics,
            generated_at: Utc::now(),
        }
    }

    /// Build timeline entries from segments, sorted by start_time.
    fn build_timeline(segments: &[SegmentSummary], regimes: &[Regime]) -> Vec<TimelineEntry> {
        let mut entries: Vec<TimelineEntry> = segments
            .iter()
            .map(|seg| {
                // #7678 D2: resolve the human regime label (name > auto_label)
                // instead of leaking the opaque `regime_id` ("regime-N") into the
                // UI timeline; fall back to `dominant_category` when unresolved
                // (mirrors the prior no-regime_id fallback, #7480).
                let regime_label = seg
                    .regime_id
                    .as_deref()
                    .and_then(|id| resolve_regime_label(regimes, id))
                    .unwrap_or_else(|| seg.dominant_category.clone());
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
        activities: &[maekon_core::models::tiered_memory::ContentActivity],
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
        regimes: &[Regime],
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
        let regime_distribution = Self::compute_regime_distribution(segments, regimes);

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
    ///
    /// #7683 (sibling of the #7678 D2 timeline fix): the map is keyed by the
    /// resolved human regime label (name > auto_label), not the opaque
    /// positional `regime_id` ("regime-N") — the id was previously leaked
    /// straight into the `StatisticsPanel` chart labels. Falls back to
    /// `dominant_category` when the id is absent or unresolved, mirroring the
    /// timeline's fallback. When two ids resolve to the same label, their
    /// durations are summed under that shared key.
    fn compute_regime_distribution(
        segments: &[SegmentSummary],
        regimes: &[Regime],
    ) -> HashMap<String, u32> {
        let total_secs: u64 = segments.iter().map(|s| s.duration_secs).sum();
        if total_secs == 0 {
            return HashMap::new();
        }

        let mut duration_by_regime: HashMap<String, u64> = HashMap::new();
        for seg in segments {
            let label = seg
                .regime_id
                .as_deref()
                .and_then(|id| resolve_regime_label(regimes, id))
                .unwrap_or_else(|| seg.dominant_category.clone());
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
// are now shared from maekon_core::models::daily_digest.

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use maekon_core::models::tiered_memory::{
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

        // #7683: the fixture's `regime_id` values ("Deep Focus"/"Communication")
        // are themselves human labels, not the opaque positional ids production
        // assigns (see the `resolve_regime_label_*` tests below for the realistic
        // shape). Supply a matching `regimes` list so `regime_distribution`
        // resolves to those same labels instead of falling back to
        // `dominant_category` for an unresolved id.
        let regimes = vec![
            make_regime("Deep Focus", Some("Deep Focus"), "auto-deep-focus"),
            make_regime("Communication", Some("Communication"), "auto-communication"),
        ];
        let digest = DailyDigestGenerator::generate(&segments, date, None, &regimes);

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
        let digest = DailyDigestGenerator::generate(&[], date, None, &[]);

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

        let digest = DailyDigestGenerator::generate(&segments, date, Some(&prev), &[]);

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
        let digest = DailyDigestGenerator::generate(&[later, earlier], date, None, &[]);

        assert_eq!(digest.timeline[0].segment_id, "seg-early");
        assert_eq!(digest.timeline[1].segment_id, "seg-late");
    }

    #[test]
    fn regime_color_mapping() {
        // Delegated to maekon_core::models::daily_digest::regime_color
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

        let digest = DailyDigestGenerator::generate(&segments, date, None, &[]);

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

        let digest = DailyDigestGenerator::generate(&segments, date, None, &[]);
        assert_eq!(digest.timeline[0].dominant_app, "VSCode");
    }

    fn make_regime(id: &str, name: Option<&str>, auto_label: &str) -> Regime {
        use maekon_core::models::tiered_memory::{RegimeFeatures, RegimeStatus, TriggerParams};
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

    // #7678 D2: the digest timeline must render the HUMAN regime label, not the
    // opaque positional `regime_id` ("regime-N"/"cluster-N") that production
    // actually assigns (see `daily_digest.rs`'s doc example). Prior tests here
    // used a human string directly AS the `regime_id` fixture — a test-double that
    // masked the opaque-id-as-label bug (it never modeled the real "regime-N" shape).
    #[test]
    fn regime_label_resolves_human_name_not_opaque_regime_id() {
        let date = Utc::now().date_naive();

        let segments = vec![make_segment(
            "seg-1",
            1800,
            "Development",
            Some("regime-0"), // opaque positional id, as production assigns
            0,
            HashMap::new(),
            vec![],
        )];
        let regimes = vec![make_regime(
            "regime-0",
            Some("Deep Focus"),
            "auto-deep-focus",
        )];

        let digest = DailyDigestGenerator::generate(&segments, date, None, &regimes);

        assert_eq!(digest.timeline[0].regime_label, "Deep Focus");
        assert_ne!(digest.timeline[0].regime_label, "regime-0");
    }

    #[test]
    fn regime_label_falls_back_to_auto_label_when_name_unset() {
        let date = Utc::now().date_naive();

        let segments = vec![make_segment(
            "seg-1",
            1800,
            "Development",
            Some("regime-0"),
            0,
            HashMap::new(),
            vec![],
        )];
        let regimes = vec![make_regime("regime-0", None, "auto-deep-focus")];

        let digest = DailyDigestGenerator::generate(&segments, date, None, &regimes);

        assert_eq!(digest.timeline[0].regime_label, "auto-deep-focus");
    }

    #[test]
    fn regime_label_falls_back_to_dominant_category_when_regime_unresolved() {
        let date = Utc::now().date_naive();

        // regime_id present but absent from the regimes list (e.g. archived /
        // evicted) — must NOT leak the opaque id into the timeline.
        let segments = vec![make_segment(
            "seg-1",
            1800,
            "Development",
            Some("regime-unknown"),
            0,
            HashMap::new(),
            vec![],
        )];

        let digest = DailyDigestGenerator::generate(&segments, date, None, &[]);

        assert_eq!(digest.timeline[0].regime_label, "Development");
        assert_ne!(digest.timeline[0].regime_label, "regime-unknown");
    }

    // #7683 (sibling of #7678 D2): `regime_distribution` — the stats map that
    // feeds the `StatisticsPanel` chart labels — must be keyed by the
    // resolved HUMAN regime label, not the opaque positional `regime_id`
    // production actually assigns. Before this fix, `compute_regime_distribution`
    // built its key directly from `seg.regime_id.unwrap_or(dominant_category)`,
    // so this test would have asserted `contains_key("deep-focus-1")` (the raw
    // id) and failed the `contains_key("Deep Focus")` / `!contains_key(id)`
    // assertions below.
    #[test]
    fn regime_distribution_keyed_by_human_label_not_opaque_regime_id() {
        let date = Utc::now().date_naive();

        let segments = vec![make_segment(
            "seg-1",
            1800,
            "Development",
            Some("deep-focus-1"), // opaque positional id, as production assigns
            0,
            HashMap::new(),
            vec![],
        )];
        let regimes = vec![make_regime(
            "deep-focus-1",
            Some("Deep Focus"),
            "auto-deep-focus",
        )];

        let digest = DailyDigestGenerator::generate(&segments, date, None, &regimes);

        assert!(digest
            .statistics
            .regime_distribution
            .contains_key("Deep Focus"));
        assert!(!digest
            .statistics
            .regime_distribution
            .contains_key("deep-focus-1"));
    }

    // Companion case: an id absent from the regimes list must fall back to
    // `dominant_category` in the distribution map too (mirrors the timeline
    // fallback), never leaking the opaque id.
    #[test]
    fn regime_distribution_falls_back_to_dominant_category_when_regime_unresolved() {
        let date = Utc::now().date_naive();

        let segments = vec![make_segment(
            "seg-1",
            1800,
            "Communication",
            Some("regime-unknown"),
            0,
            HashMap::new(),
            vec![],
        )];

        let digest = DailyDigestGenerator::generate(&segments, date, None, &[]);

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
