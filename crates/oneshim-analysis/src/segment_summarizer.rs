use std::collections::HashMap;

use chrono::{DateTime, Utc};
use oneshim_core::models::event::Event;
use oneshim_core::models::tiered_memory::{
    ContainerInfo, ContentActivity, SegmentSummary, TriggerReason,
};
use oneshim_core::models::work_session::AppCategory;

use crate::pattern_miner::PatternMiner;

/// Convert AppCategory to a human-readable string without relying on Debug format.
fn category_to_str(cat: &AppCategory) -> &'static str {
    match cat {
        AppCategory::Communication => "Communication",
        AppCategory::Development => "Development",
        AppCategory::Documentation => "Documentation",
        AppCategory::Browser => "Browser",
        AppCategory::Design => "Design",
        AppCategory::Media => "Media",
        AppCategory::System => "System",
        AppCategory::Other => "Other",
    }
}

/// Produce a `SegmentSummary` from raw events and content activities.
///
/// Computes per-app and per-category breakdowns, context-switch count,
/// average importance, dominant category, and detected patterns.
pub struct SegmentSummarizer {
    pattern_miner: PatternMiner,
}

impl SegmentSummarizer {
    pub fn new() -> Self {
        Self {
            pattern_miner: PatternMiner,
        }
    }

    /// Summarize a closed segment.
    #[allow(clippy::too_many_arguments)]
    pub fn summarize(
        &self,
        segment_id: String,
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
        events: &[Event],
        content_activities: Vec<ContentActivity>,
        container: Option<ContainerInfo>,
        trigger_reason: TriggerReason,
        regime_id: Option<String>,
    ) -> SegmentSummary {
        let duration_secs = (end_time - start_time).num_seconds().max(0) as u64;
        let event_count = events.len() as u32;

        let (app_breakdown, category_breakdown) = self.compute_breakdowns(events);
        let context_switch_count = self.count_context_switches(events);
        let dominant_category = self.find_dominant_category(&category_breakdown);
        let avg_importance = self.compute_avg_importance(events);
        let patterns_detected = self.pattern_miner.detect(events);

        SegmentSummary {
            segment_id,
            start_time,
            end_time,
            duration_secs,
            regime_id,
            trigger_reason,
            event_count,
            app_breakdown,
            category_breakdown,
            context_switch_count,
            dominant_category,
            avg_importance,
            patterns_detected,
            content_activities,
            container,
            llm_summary: None,
        }
    }

    /// Compute per-app and per-category time breakdowns.
    ///
    /// For Context events, time is estimated as the gap between consecutive
    /// events. For other event types, each counts as 1 second.
    fn compute_breakdowns(&self, events: &[Event]) -> (HashMap<String, u64>, HashMap<String, u64>) {
        let mut app_breakdown: HashMap<String, u64> = HashMap::new();
        let mut category_breakdown: HashMap<String, u64> = HashMap::new();

        // Extract context events with timestamps for duration estimation
        let ctx_events: Vec<(&str, &str, DateTime<Utc>)> = events
            .iter()
            .filter_map(|e| match e {
                Event::Context(ctx) => Some((ctx.app_name.as_str(), "", ctx.timestamp)),
                Event::User(u) => Some((u.app_name.as_str(), "", u.timestamp)),
                _ => None,
            })
            .collect();

        if ctx_events.is_empty() {
            return (app_breakdown, category_breakdown);
        }

        for i in 0..ctx_events.len() {
            let (app_name, _, ts) = ctx_events[i];
            let duration = if i + 1 < ctx_events.len() {
                let next_ts = ctx_events[i + 1].2;
                (next_ts - ts).num_seconds().max(0) as u64
            } else {
                // Last event: assign 1 second minimum
                1
            };

            *app_breakdown.entry(app_name.to_string()).or_insert(0) += duration;
            let category = AppCategory::from_app_name(app_name);
            *category_breakdown
                .entry(category_to_str(&category).to_string())
                .or_insert(0) += duration;
        }

        (app_breakdown, category_breakdown)
    }

    /// Count context switches (consecutive events with different app names).
    fn count_context_switches(&self, events: &[Event]) -> u32 {
        let app_names: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                Event::Context(ctx) => Some(ctx.app_name.as_str()),
                Event::User(u) => Some(u.app_name.as_str()),
                _ => None,
            })
            .collect();

        if app_names.len() < 2 {
            return 0;
        }

        app_names
            .windows(2)
            .filter(|pair| pair[0] != pair[1])
            .count() as u32
    }

    /// Find the category with the most accumulated time.
    fn find_dominant_category(&self, category_breakdown: &HashMap<String, u64>) -> String {
        category_breakdown
            .iter()
            .max_by_key(|(_, &v)| v)
            .map(|(k, _)| k.clone())
            .unwrap_or_else(|| "Unknown".to_string())
    }

    /// Compute average importance from context events' input_activity_level.
    fn compute_avg_importance(&self, events: &[Event]) -> f32 {
        let mut sum = 0.0_f32;
        let mut count = 0u32;

        for event in events {
            if let Event::Context(ctx) = event {
                sum += ctx.input_activity_level;
                count += 1;
            }
        }

        if count > 0 {
            sum / count as f32
        } else {
            0.0
        }
    }
}

impl Default for SegmentSummarizer {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a slice of ContentActivity records into ContentSummaryEntry values
/// for the ContextAssembler.
pub fn to_content_summary_entries(
    activities: &[ContentActivity],
) -> Vec<crate::assembler::ContentSummaryEntry> {
    activities
        .iter()
        .map(|ca| {
            let gui_patterns = ca
                .gui_summary
                .as_ref()
                .map(|gs| {
                    crate::pattern_miner::detect_gui_patterns(gs, ca.work_type)
                        .into_iter()
                        .map(|p| format!("{p:?}"))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            crate::assembler::ContentSummaryEntry {
                content: ca.content_label.clone(),
                content_type: format!("{:?}", ca.content_type),
                work_type: format!("{:?}", ca.work_type),
                mins: (ca.duration_secs / 60).max(1) as u32,
                gui_summary_line: ca.gui_summary.as_ref().map(|gs| gs.summary_line.clone()),
                gui_patterns,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use oneshim_core::models::event::ContextEvent;
    use oneshim_core::models::tiered_memory::TriggerReason;

    fn make_ctx(app: &str, ts: DateTime<Utc>, importance: f32) -> Event {
        Event::Context(ContextEvent {
            app_name: app.to_string(),
            window_title: format!("{app} Window"),
            prev_app_name: None,
            timestamp: ts,
            input_activity_level: importance,
        })
    }

    #[test]
    fn correct_app_breakdown() {
        let summarizer = SegmentSummarizer::new();
        let t0 = Utc::now();
        let events = vec![
            make_ctx("VSCode", t0, 0.8),
            make_ctx("VSCode", t0 + Duration::seconds(60), 0.7),
            make_ctx("Slack", t0 + Duration::seconds(120), 0.5),
            make_ctx("Slack", t0 + Duration::seconds(180), 0.4),
        ];

        let summary = summarizer.summarize(
            "seg-1".to_string(),
            t0,
            t0 + Duration::seconds(200),
            &events,
            vec![],
            None,
            TriggerReason::ScoreHigh,
            None,
        );

        // VSCode: 60s (t0→t0+60) + 60s (t0+60→t0+120) = 120s
        assert_eq!(summary.app_breakdown.get("VSCode"), Some(&120));
        // Slack: 60s (t0+120→t0+180) + 1s (last event)
        assert_eq!(summary.app_breakdown.get("Slack"), Some(&61));
    }

    #[test]
    fn dominant_category_detected() {
        let summarizer = SegmentSummarizer::new();
        let t0 = Utc::now();
        let events = vec![
            make_ctx("VSCode", t0, 0.8),
            make_ctx("VSCode", t0 + Duration::seconds(100), 0.7),
            make_ctx("Slack", t0 + Duration::seconds(150), 0.5),
        ];

        let summary = summarizer.summarize(
            "seg-2".to_string(),
            t0,
            t0 + Duration::seconds(160),
            &events,
            vec![],
            None,
            TriggerReason::ScoreHigh,
            None,
        );

        assert_eq!(summary.dominant_category, "Development");
    }

    #[test]
    fn context_switch_count() {
        let summarizer = SegmentSummarizer::new();
        let t0 = Utc::now();
        let events = vec![
            make_ctx("VSCode", t0, 0.8),
            make_ctx("Slack", t0 + Duration::seconds(30), 0.5),
            make_ctx("VSCode", t0 + Duration::seconds(60), 0.7),
            make_ctx("Chrome", t0 + Duration::seconds(90), 0.6),
        ];

        let summary = summarizer.summarize(
            "seg-3".to_string(),
            t0,
            t0 + Duration::seconds(120),
            &events,
            vec![],
            None,
            TriggerReason::ScoreHigh,
            None,
        );

        // VSCode→Slack→VSCode→Chrome = 3 switches
        assert_eq!(summary.context_switch_count, 3);
    }

    #[test]
    fn avg_importance_computed() {
        let summarizer = SegmentSummarizer::new();
        let t0 = Utc::now();
        let events = vec![
            make_ctx("VSCode", t0, 0.8),
            make_ctx("VSCode", t0 + Duration::seconds(30), 0.6),
        ];

        let summary = summarizer.summarize(
            "seg-4".to_string(),
            t0,
            t0 + Duration::seconds(60),
            &events,
            vec![],
            None,
            TriggerReason::ScoreHigh,
            None,
        );

        assert!((summary.avg_importance - 0.7).abs() < 0.01);
    }

    #[test]
    fn empty_segment() {
        let summarizer = SegmentSummarizer::new();
        let t0 = Utc::now();

        let summary = summarizer.summarize(
            "seg-empty".to_string(),
            t0,
            t0 + Duration::seconds(30),
            &[],
            vec![],
            None,
            TriggerReason::IdleStart,
            None,
        );

        assert_eq!(summary.event_count, 0);
        assert!(summary.app_breakdown.is_empty());
        assert_eq!(summary.context_switch_count, 0);
        assert!((summary.avg_importance - 0.0).abs() < f32::EPSILON);
        assert_eq!(summary.dominant_category, "Unknown");
    }

    #[test]
    fn duration_computed_correctly() {
        let summarizer = SegmentSummarizer::new();
        let t0 = Utc::now();
        let t1 = t0 + Duration::seconds(300);

        let summary = summarizer.summarize(
            "seg-dur".to_string(),
            t0,
            t1,
            &[],
            vec![],
            None,
            TriggerReason::ForcedMaxDuration,
            Some("regime-1".to_string()),
        );

        assert_eq!(summary.duration_secs, 300);
        assert_eq!(summary.regime_id, Some("regime-1".to_string()));
        assert_eq!(summary.trigger_reason, TriggerReason::ForcedMaxDuration);
    }

    #[test]
    fn content_activities_preserved() {
        use oneshim_core::models::tiered_memory::{ContentType, EngagementMetrics, WorkType};

        let summarizer = SegmentSummarizer::new();
        let t0 = Utc::now();
        let activities = vec![ContentActivity {
            content_label: "main.rs".to_string(),
            content_type: ContentType::File,
            start_time: t0,
            duration_secs: 120,
            confidence: 0.95,
            work_type: WorkType::ActiveCoding,
            engagement: EngagementMetrics::default(),
            gui_summary: None,
        }];

        let summary = summarizer.summarize(
            "seg-ca".to_string(),
            t0,
            t0 + Duration::seconds(120),
            &[],
            activities,
            None,
            TriggerReason::ScoreHigh,
            None,
        );

        assert_eq!(summary.content_activities.len(), 1);
        assert_eq!(summary.content_activities[0].content_label, "main.rs");
    }

    #[test]
    fn no_context_switches_for_same_app() {
        let summarizer = SegmentSummarizer::new();
        let t0 = Utc::now();
        let events = vec![
            make_ctx("VSCode", t0, 0.8),
            make_ctx("VSCode", t0 + Duration::seconds(30), 0.7),
            make_ctx("VSCode", t0 + Duration::seconds(60), 0.9),
        ];

        let summary = summarizer.summarize(
            "seg-ns".to_string(),
            t0,
            t0 + Duration::seconds(90),
            &events,
            vec![],
            None,
            TriggerReason::ScoreHigh,
            None,
        );

        assert_eq!(summary.context_switch_count, 0);
    }

    #[test]
    fn gui_patterns_populated_from_summary() {
        use oneshim_core::models::gui_activity::GuiActivitySummary;
        use oneshim_core::models::tiered_memory::{ContentType, EngagementMetrics, WorkType};

        let t0 = Utc::now();
        let gui = GuiActivitySummary {
            app_name: "VS Code".to_string(),
            window_title: "main.rs".to_string(),
            content_label: "main.rs".to_string(),
            start_time: t0,
            end_time: t0 + Duration::seconds(300),
            duration_secs: 300,
            button_clicks: 5,
            text_entries: 10,
            tab_switches: 0,
            menu_accesses: 0,
            tree_navigations: 0,
            scroll_events: 0,
            save_count: 2,
            test_run_count: 1,
            search_count: 0,
            build_count: 0,
            undo_redo_count: 0,
            copy_paste_count: 0,
            top_elements: vec![],
            unmatched_click_count: 0,
            summary_line: "5 clicks, 2 saves, 1 test".to_string(),
        };

        let activities = vec![ContentActivity {
            content_label: "main.rs".to_string(),
            content_type: ContentType::File,
            start_time: t0,
            duration_secs: 300,
            confidence: 0.9,
            work_type: WorkType::ActiveCoding,
            engagement: EngagementMetrics::default(),
            gui_summary: Some(gui),
        }];

        let entries = to_content_summary_entries(&activities);
        assert_eq!(entries.len(), 1);
        assert!(
            entries[0]
                .gui_patterns
                .contains(&"TestDrivenDevelopment".to_string()),
            "TDD pattern should be detected (test_run=1 + save=2 + ActiveCoding): {:?}",
            entries[0].gui_patterns
        );
    }

    #[test]
    fn gui_patterns_empty_when_no_gui_summary() {
        use oneshim_core::models::tiered_memory::{ContentType, EngagementMetrics, WorkType};
        let t0 = Utc::now();
        let activities = vec![ContentActivity {
            content_label: "readme.md".to_string(),
            content_type: ContentType::File,
            start_time: t0,
            duration_secs: 60,
            confidence: 0.8,
            work_type: WorkType::Reading,
            engagement: EngagementMetrics::default(),
            gui_summary: None,
        }];

        let entries = to_content_summary_entries(&activities);
        assert!(entries[0].gui_patterns.is_empty());
    }
}
