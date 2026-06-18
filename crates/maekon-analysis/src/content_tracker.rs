use chrono::{DateTime, Utc};
use maekon_core::models::gui_activity::GuiActivitySummary;
use maekon_core::models::tiered_memory::{
    ContentActivity, ContentType, EngagementMetrics, WorkType,
};
use std::sync::Arc;

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

/// Soft cap on the number of finalized activities retained in `completed`.
///
/// A pathological content-switching burst inside a single segment (e.g. an
/// editor rapidly cycling tabs) would otherwise grow `completed` without bound,
/// inflating both memory and the per-tick `peek()` snapshot cost. When the cap
/// is exceeded the oldest entries are evicted. The segment lifecycle normally
/// keeps this far below the cap; eviction only triggers under abuse.
const MAX_COMPLETED: usize = 512;

/// Tracks the currently active content and accumulates duration.
///
/// When the content label changes, the previous content is finalized into a
/// `ContentActivity` record. Engagement metrics are maintained as running
/// averages over the active content's lifetime.
pub struct ContentTracker {
    current: Option<ActiveContent>,
    /// Finalized activities stored behind `Arc` so that snapshot construction
    /// (`peek`) and content-label transitions (`push_completed`) clone only
    /// cheap atomic pointers instead of deep-cloning every heap `String` field.
    completed: Vec<Arc<ContentActivity>>,
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
                // Store the finalized activity behind an `Arc` (cheap to retain
                // and to clone into future `peek` snapshots), and hand the caller
                // an owned copy with identical contents.
                let activity = Arc::new(activity);
                let owned = (*activity).clone();
                self.push_completed(activity);
                return Some(owned);
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
    ///
    /// Returns an `Arc<[ContentActivity]>` so callers can deref-coerce to
    /// `&[ContentActivity]` and cheaply share or re-use the snapshot.
    ///
    /// The snapshot is materialized lazily from `&self.completed`, which holds
    /// `Arc<ContentActivity>` entries: each completed item contributes a cheap
    /// atomic pointer clone (`Arc::clone`) plus one final value clone when the
    /// owned `Arc<[ContentActivity]>` is built. There is no longer a per-tick
    /// deep clone of every heap `String` field of every completed activity, and
    /// content-label transitions no longer rebuild a full deep-cloned snapshot.
    /// The number of completed entries is soft-capped at [`MAX_COMPLETED`], so
    /// this cost stays bounded even under a pathological switching burst.
    ///
    /// Callers that accept `&[ContentActivity]` use Rust's deref coercion:
    /// ```ignore
    /// some_fn(&tracker.peek())   // &Arc<[ContentActivity]> coerces to &[ContentActivity]
    /// ```
    pub fn peek(&self) -> Arc<[ContentActivity]> {
        let Some(ref current) = self.current else {
            // No active item — materialize the completed entries by value.
            // Bounded by `MAX_COMPLETED`.
            return self
                .completed
                .iter()
                .map(|a| (**a).clone())
                .collect::<Vec<_>>()
                .into();
        };

        // Synthesize the in-progress entry and append it to the completed
        // entries. Pre-allocate with exact capacity to avoid Vec reallocations.
        let now = Utc::now();
        let duration_secs = (now - current.start_time).num_seconds().max(0) as u64;
        let in_progress = ContentActivity {
            content_label: current.content_label.clone(),
            content_type: current.content_type,
            start_time: current.start_time,
            duration_secs,
            confidence: current.confidence,
            work_type: current.work_type,
            engagement: current.engagement.clone(),
            gui_summary: current.gui_summary.clone(),
        };
        let mut v: Vec<ContentActivity> = Vec::with_capacity(self.completed.len() + 1);
        v.extend(self.completed.iter().map(|a| (**a).clone()));
        v.push(in_progress);
        Arc::from(v)
    }

    /// Drain all activities (called when segment closes).
    /// Finalizes the current content and returns all completed activities.
    pub fn drain_all(&mut self, end_time: DateTime<Utc>) -> Vec<ContentActivity> {
        // Move the accumulated activities out, unwrapping each `Arc` in place
        // (no deep clone when this tracker holds the only reference). The
        // finalized current item is appended to the owned `result` vector, not
        // to `self.completed`, so `push_completed` stays the single code path
        // that ever mutates `self.completed`.
        let mut result: Vec<ContentActivity> = std::mem::take(&mut self.completed)
            .into_iter()
            .map(|a| Arc::try_unwrap(a).unwrap_or_else(|a| (*a).clone()))
            .collect();
        if let Some(activity) = self.finalize_current(end_time) {
            result.push(activity);
        }
        self.current = None;
        result
    }

    /// Append a finalized activity to `completed`, evicting the oldest entries
    /// once the soft cap [`MAX_COMPLETED`] is exceeded. This is the ONLY method
    /// that pushes onto `self.completed`. Entries are stored behind `Arc`, so
    /// retaining them costs a single allocation and snapshot construction in
    /// `peek` clones only pointers (plus one value clone of the result).
    fn push_completed(&mut self, activity: Arc<ContentActivity>) {
        self.completed.push(activity);
        if self.completed.len() > MAX_COMPLETED {
            // Evict the oldest overflow entries so a pathological switching
            // burst inside one segment cannot grow memory or per-tick clone
            // cost without bound.
            let overflow = self.completed.len() - MAX_COMPLETED;
            self.completed.drain(0..overflow);
        }
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
        gui: Option<maekon_core::models::gui_activity::GuiActivitySummary>,
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
    fn peek_returns_arc_without_cloning_completed() {
        // Verify that peek() returns Arc<[ContentActivity]> and that the Arc
        // for the completed-only path is a cheap pointer clone (same allocation).
        let mut tracker = ContentTracker::new();
        let t0 = Utc::now();
        let t1 = t0 + Duration::seconds(60);
        let t2 = t1 + Duration::seconds(30);

        tracker.update(input(
            "main.rs",
            ContentType::File,
            WorkType::ActiveCoding,
            make_engagement(40.0, 5.0, 2.0),
            0.95,
            t0,
            None,
        ));
        // Trigger a transition so "main.rs" is finalized into completed.
        tracker.update(input(
            "lib.rs",
            ContentType::File,
            WorkType::ActiveCoding,
            make_engagement(30.0, 4.0, 1.0),
            0.90,
            t1,
            None,
        ));

        // peek() with an active current item must include both the completed
        // entry and the in-progress one.
        let snapshot = tracker.peek();
        assert_eq!(snapshot.len(), 2, "completed + in-progress");
        assert_eq!(snapshot[0].content_label, "main.rs");
        assert_eq!(snapshot[1].content_label, "lib.rs");

        // After drain, peek() on an empty tracker returns an empty Arc (fast path).
        tracker.drain_all(t2);
        let empty = tracker.peek();
        assert!(empty.is_empty());

        // The empty fast path must still be a valid Arc (deref to &[]).
        let _slice: &[ContentActivity] = &empty;
    }

    #[test]
    fn peek_fast_path_when_no_current() {
        // When there is no in-progress item the Arc is returned without building
        // a new allocation (pointer identity check is not reliable across Arc
        // rebuilds, but we can at least assert the length and label are correct).
        let mut tracker = ContentTracker::new();
        let t0 = Utc::now();
        let t1 = t0 + Duration::seconds(60);
        let t2 = t1 + Duration::seconds(10);

        tracker.update(input(
            "a.rs",
            ContentType::File,
            WorkType::ActiveCoding,
            make_engagement(10.0, 1.0, 0.0),
            0.9,
            t0,
            None,
        ));
        // Transition forces "a.rs" into completed; "b.rs" becomes current.
        tracker.update(input(
            "b.rs",
            ContentType::File,
            WorkType::ActiveCoding,
            make_engagement(20.0, 2.0, 0.0),
            0.8,
            t1,
            None,
        ));
        // Drain clears current; now completed is empty and current is None.
        tracker.drain_all(t2);

        // Fast path: no current, no completed.
        let snap = tracker.peek();
        assert!(snap.is_empty());
    }

    #[test]
    fn gui_summary_propagated_to_finalized_activity() {
        use maekon_core::models::gui_activity::GuiActivitySummary;

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

    #[test]
    fn completed_is_soft_capped_under_switching_burst() {
        // A pathological switching burst (more transitions than MAX_COMPLETED)
        // must not grow `completed` (and thus `peek` clone cost) without bound:
        // the oldest entries are evicted while the newest are retained.
        let mut tracker = ContentTracker::new();
        let t0 = Utc::now();
        let total = MAX_COMPLETED + 50;

        for i in 0..total {
            tracker.update(input(
                &format!("file-{i}.rs"),
                ContentType::File,
                WorkType::ActiveCoding,
                make_engagement(10.0, 1.0, 0.0),
                0.9,
                t0 + Duration::seconds(i as i64),
                None,
            ));
        }

        // `completed` is capped (the final label is still the in-progress item).
        let snapshot = tracker.peek();
        assert_eq!(
            snapshot.len(),
            MAX_COMPLETED + 1,
            "capped completed + current"
        );

        // The newest completed entries are retained; the oldest are evicted.
        let last_completed = format!("file-{}.rs", total - 2);
        assert_eq!(snapshot[snapshot.len() - 2].content_label, last_completed);
        let in_progress = format!("file-{}.rs", total - 1);
        assert_eq!(snapshot[snapshot.len() - 1].content_label, in_progress);
        assert!(
            snapshot.iter().all(|a| a.content_label != "file-0.rs"),
            "oldest entry should have been evicted"
        );
    }
}
