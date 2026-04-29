use chrono::{DateTime, Utc};
use oneshim_core::models::gui_activity::GuiActivitySummary;
use oneshim_core::models::tiered_memory::{
    ContentActivity, ContentType, EngagementMetrics, WorkType,
};

/// Input for a single content tracker update.
pub struct ContentUpdateInput {
    pub content_label: String,
    pub content_type: ContentType,
    pub work_type: WorkType,
    pub engagement: EngagementMetrics,
    pub confidence: f32,
    pub timestamp: DateTime<Utc>,
    pub gui_summary: Option<GuiActivitySummary>,
}

/// Tracks the currently active content and accumulates duration.
///
/// When the content label changes, the previous content is finalized into a
/// `ContentActivity` record. Engagement metrics are maintained as running
/// averages over the active content's lifetime.
pub struct ContentTracker {
    current: Option<ActiveContent>,
    completed: Vec<ContentActivity>,
}

/// Internal state for the currently tracked content.
struct ActiveContent {
    content_label: String,
    content_type: ContentType,
    work_type: WorkType,
    engagement: EngagementMetrics,
    start_time: DateTime<Utc>,
    confidence: f32,
    update_count: u32,
    /// Latest GUI activity summary for this content (updated each tick).
    gui_summary: Option<GuiActivitySummary>,
}

impl ContentTracker {
    pub fn new() -> Self {
        Self {
            current: None,
            completed: Vec::new(),
        }
    }

    /// Update with new content detection. Returns finalized activity if content changed.
    ///
    /// When `gui_summary` is `Some`, the latest GUI activity summary is stored
    /// and propagated to the finalized `ContentActivity`.
    pub fn update(&mut self, input: ContentUpdateInput) -> Option<ContentActivity> {
        let changed = self
            .current
            .as_ref()
            .map(|c| c.content_label != input.content_label)
            .unwrap_or(true);

        if changed {
            // Finalize current content if any
            let finalized = self.finalize_current(input.timestamp);

            // Start tracking new content
            self.current = Some(ActiveContent {
                content_label: input.content_label,
                content_type: input.content_type,
                work_type: input.work_type,
                engagement: input.engagement,
                start_time: input.timestamp,
                confidence: input.confidence,
                update_count: 1,
                gui_summary: input.gui_summary,
            });

            if let Some(activity) = finalized {
                self.completed.push(activity.clone());
                return Some(activity);
            }
        } else if let Some(ref mut current) = self.current {
            // Same content — update engagement as running average
            current.update_count += 1;
            let n = current.update_count as f32;
            current.engagement.keystrokes_per_min = running_avg(
                current.engagement.keystrokes_per_min,
                input.engagement.keystrokes_per_min,
                n,
            );
            current.engagement.mouse_clicks_per_min = running_avg(
                current.engagement.mouse_clicks_per_min,
                input.engagement.mouse_clicks_per_min,
                n,
            );
            current.engagement.scroll_events_per_min = running_avg(
                current.engagement.scroll_events_per_min,
                input.engagement.scroll_events_per_min,
                n,
            );
            current.engagement.shortcut_ratio = running_avg(
                current.engagement.shortcut_ratio,
                input.engagement.shortcut_ratio,
                n,
            );
            current.engagement.idle_ratio = running_avg(
                current.engagement.idle_ratio,
                input.engagement.idle_ratio,
                n,
            );
            current.engagement.typing_burst_count += input.engagement.typing_burst_count;

            // Update work type and confidence to latest
            current.work_type = input.work_type;
            current.confidence = running_avg(current.confidence, input.confidence, n);

            // Update GUI summary to latest (replaces previous)
            if input.gui_summary.is_some() {
                current.gui_summary = input.gui_summary;
            }
        }

        None
    }

    /// Non-destructive snapshot of completed + current activities.
    /// Used by the scheduler to build `SegmentStats` for the LLM context
    /// without consuming the tracker state.
    pub fn peek(&self) -> Vec<ContentActivity> {
        let now = Utc::now();
        let mut result = self.completed.clone();
        if let Some(ref current) = self.current {
            let duration_secs = (now - current.start_time).num_seconds().max(0) as u64;
            result.push(ContentActivity {
                content_label: current.content_label.clone(),
                content_type: current.content_type,
                start_time: current.start_time,
                duration_secs,
                confidence: current.confidence,
                work_type: current.work_type,
                engagement: current.engagement.clone(),
                gui_summary: current.gui_summary.clone(),
            });
        }
        result
    }

    /// Drain all activities (called when segment closes).
    /// Finalizes the current content and returns all completed activities.
    pub fn drain_all(&mut self, end_time: DateTime<Utc>) -> Vec<ContentActivity> {
        if let Some(activity) = self.finalize_current(end_time) {
            self.completed.push(activity);
        }
        self.current = None;
        std::mem::take(&mut self.completed)
    }

    /// Finalize the current active content into a ContentActivity.
    fn finalize_current(&mut self, end_time: DateTime<Utc>) -> Option<ContentActivity> {
        let current = self.current.take()?;
        let duration_secs = (end_time - current.start_time).num_seconds().max(0) as u64;

        Some(ContentActivity {
            content_label: current.content_label,
            content_type: current.content_type,
            start_time: current.start_time,
            duration_secs,
            confidence: current.confidence,
            work_type: current.work_type,
            engagement: current.engagement,
            gui_summary: current.gui_summary,
        })
    }
}

impl Default for ContentTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Incremental running average: old_avg + (new_val - old_avg) / n
fn running_avg(old_avg: f32, new_val: f32, n: f32) -> f32 {
    old_avg + (new_val - old_avg) / n
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn make_engagement(kpm: f32, clicks: f32, scroll: f32) -> EngagementMetrics {
        EngagementMetrics {
            keystrokes_per_min: kpm,
            mouse_clicks_per_min: clicks,
            scroll_events_per_min: scroll,
            shortcut_ratio: 0.0,
            typing_burst_count: 0,
            idle_ratio: 0.0,
        }
    }

    fn input(
        label: &str,
        ct: ContentType,
        wt: WorkType,
        eng: EngagementMetrics,
        conf: f32,
        ts: DateTime<Utc>,
        gui: Option<oneshim_core::models::gui_activity::GuiActivitySummary>,
    ) -> ContentUpdateInput {
        ContentUpdateInput {
            content_label: label.to_string(),
            content_type: ct,
            work_type: wt,
            engagement: eng,
            confidence: conf,
            timestamp: ts,
            gui_summary: gui,
        }
    }

    #[test]
    fn first_update_starts_tracking() {
        let mut tracker = ContentTracker::new();
        let now = Utc::now();
        let result = tracker.update(input(
            "main.rs",
            ContentType::File,
            WorkType::ActiveCoding,
            make_engagement(40.0, 5.0, 2.0),
            0.95,
            now,
            None,
        ));
        assert!(result.is_none());
        assert!(tracker.current.is_some());
    }

    #[test]
    fn content_switch_produces_activity() {
        let mut tracker = ContentTracker::new();
        let t0 = Utc::now();
        let t1 = t0 + Duration::seconds(120);

        tracker.update(input(
            "main.rs",
            ContentType::File,
            WorkType::ActiveCoding,
            make_engagement(40.0, 5.0, 2.0),
            0.95,
            t0,
            None,
        ));

        let activity = tracker.update(input(
            "index.ts",
            ContentType::File,
            WorkType::ActiveCoding,
            make_engagement(35.0, 3.0, 1.0),
            0.90,
            t1,
            None,
        ));

        let activity = activity.unwrap();
        assert_eq!(activity.content_label, "main.rs");
        assert_eq!(activity.duration_secs, 120);
        assert_eq!(activity.content_type, ContentType::File);
    }

    #[test]
    fn same_content_updates_engagement() {
        let mut tracker = ContentTracker::new();
        let t0 = Utc::now();
        let t1 = t0 + Duration::seconds(60);

        tracker.update(input(
            "main.rs",
            ContentType::File,
            WorkType::ActiveCoding,
            make_engagement(40.0, 5.0, 2.0),
            0.90,
            t0,
            None,
        ));

        let result = tracker.update(input(
            "main.rs",
            ContentType::File,
            WorkType::ActiveCoding,
            make_engagement(50.0, 7.0, 4.0),
            0.95,
            t1,
            None,
        ));

        assert!(result.is_none());
        let current = tracker.current.as_ref().unwrap();
        assert!((current.engagement.keystrokes_per_min - 45.0).abs() < 0.1);
    }

    #[test]
    fn drain_all_returns_everything() {
        let mut tracker = ContentTracker::new();
        let t0 = Utc::now();
        let t1 = t0 + Duration::seconds(60);
        let t2 = t1 + Duration::seconds(120);
        let t3 = t2 + Duration::seconds(30);

        tracker.update(input(
            "main.rs",
            ContentType::File,
            WorkType::ActiveCoding,
            make_engagement(40.0, 5.0, 2.0),
            0.95,
            t0,
            None,
        ));
        tracker.update(input(
            "index.ts",
            ContentType::File,
            WorkType::ActiveCoding,
            make_engagement(35.0, 3.0, 1.0),
            0.90,
            t1,
            None,
        ));
        tracker.update(input(
            "#general",
            ContentType::Channel,
            WorkType::ActiveMeeting,
            make_engagement(20.0, 2.0, 0.0),
            0.85,
            t2,
            None,
        ));

        let activities = tracker.drain_all(t3);
        assert_eq!(activities.len(), 3);
        assert_eq!(activities[0].content_label, "main.rs");
        assert_eq!(activities[1].content_label, "index.ts");
        assert_eq!(activities[2].content_label, "#general");
        assert_eq!(activities[2].duration_secs, 30);
    }

    #[test]
    fn drain_empty_tracker() {
        let mut tracker = ContentTracker::new();
        let activities = tracker.drain_all(Utc::now());
        assert!(activities.is_empty());
    }

    #[test]
    fn duration_accumulates_correctly() {
        let mut tracker = ContentTracker::new();
        let t0 = Utc::now();
        let t1 = t0 + Duration::seconds(300);

        tracker.update(input(
            "report.docx",
            ContentType::File,
            WorkType::Writing,
            make_engagement(25.0, 2.0, 1.0),
            0.90,
            t0,
            None,
        ));

        let activities = tracker.drain_all(t1);
        assert_eq!(activities.len(), 1);
        assert_eq!(activities[0].duration_secs, 300);
    }

    #[test]
    fn zero_duration_content() {
        let mut tracker = ContentTracker::new();
        let t0 = Utc::now();

        tracker.update(input(
            "main.rs",
            ContentType::File,
            WorkType::ActiveCoding,
            make_engagement(40.0, 5.0, 2.0),
            0.95,
            t0,
            None,
        ));

        let activity = tracker.update(input(
            "lib.rs",
            ContentType::File,
            WorkType::ActiveCoding,
            make_engagement(30.0, 4.0, 1.0),
            0.90,
            t0,
            None,
        ));

        let activity = activity.unwrap();
        assert_eq!(activity.duration_secs, 0);
    }

    #[test]
    fn gui_summary_propagated_to_finalized_activity() {
        use oneshim_core::models::gui_activity::GuiActivitySummary;

        let mut tracker = ContentTracker::new();
        let t0 = Utc::now();
        let t1 = t0 + Duration::seconds(60);

        let gui_summary = GuiActivitySummary {
            app_name: "VS Code".to_string(),
            window_title: "main.rs".to_string(),
            content_label: "main.rs".to_string(),
            start_time: t0,
            end_time: t0,
            duration_secs: 60,
            button_clicks: 5,
            text_entries: 2,
            tab_switches: 0,
            menu_accesses: 0,
            tree_navigations: 0,
            scroll_events: 0,
            save_count: 1,
            test_run_count: 0,
            search_count: 0,
            build_count: 0,
            undo_redo_count: 0,
            copy_paste_count: 0,
            top_elements: vec![],
            unmatched_click_count: 0,
            summary_line: "5 clicks, 1 saves".to_string(),
        };

        tracker.update(input(
            "main.rs",
            ContentType::File,
            WorkType::ActiveCoding,
            make_engagement(40.0, 5.0, 2.0),
            0.95,
            t0,
            Some(gui_summary.clone()),
        ));

        let activity = tracker.update(input(
            "lib.rs",
            ContentType::File,
            WorkType::ActiveCoding,
            make_engagement(30.0, 4.0, 1.0),
            0.90,
            t1,
            None,
        ));

        let activity = activity.unwrap();
        assert!(activity.gui_summary.is_some());
        let gs = activity.gui_summary.unwrap();
        assert_eq!(gs.save_count, 1);
        assert_eq!(gs.button_clicks, 5);
    }
}
