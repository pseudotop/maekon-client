use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::gui_interaction::GuiElementType;

/// Aggregated summary of GUI interactions within a time window.
///
/// Produced by `GuiActivityAggregator` from a stream of `GuiInteractionEvent`s.
/// Designed to be attached to `ContentActivity` for enriching LLM context
/// (e.g., "edited auth.rs: 15 min coding, 3 saves, 2 test runs").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuiActivitySummary {
    /// Application name (e.g., "Visual Studio Code").
    pub app_name: String,
    /// Window title at the time of the activity.
    pub window_title: String,
    /// Content label (e.g., file name or URL).
    pub content_label: String,
    /// Start of the aggregation window.
    pub start_time: DateTime<Utc>,
    /// End of the aggregation window.
    pub end_time: DateTime<Utc>,
    /// Duration of the aggregation window in seconds.
    pub duration_secs: u64,

    // --- Interaction counts ---
    /// Total button/link clicks.
    pub button_clicks: u32,
    /// Total text entry actions.
    pub text_entries: u32,
    /// Total tab switch events.
    pub tab_switches: u32,
    /// Total menu access events.
    pub menu_accesses: u32,
    /// Total tree navigation events.
    pub tree_navigations: u32,
    /// Total scroll events.
    pub scroll_events: u32,

    // --- Semantic action counts ---
    /// Number of save actions detected (Cmd/Ctrl+S or button click).
    pub save_count: u32,
    /// Number of test run actions detected.
    pub test_run_count: u32,
    /// Number of search actions detected (Cmd/Ctrl+F or search button).
    pub search_count: u32,
    /// Number of build actions detected.
    pub build_count: u32,
    /// Number of undo/redo actions detected.
    pub undo_redo_count: u32,
    /// Number of copy/paste actions detected.
    pub copy_paste_count: u32,

    // --- Top elements ---
    /// Most frequently interacted elements: (text, type, count), sorted descending.
    pub top_elements: Vec<(String, GuiElementType, u32)>,

    // --- Coverage metrics ---
    /// Clicks that did not match any GUI element from the detector.
    /// Justifies Phase 3 improvements to detection accuracy.
    pub unmatched_click_count: u32,

    // --- Human-readable ---
    /// One-line summary, e.g., "15 clicks, 3 saves, 2 test runs".
    pub summary_line: String,
}

impl GuiActivitySummary {
    /// Generate a human-readable summary line from the counts.
    ///
    /// Format: "N clicks, M keystrokes[, K saves][, J test runs][, ...semantic actions]"
    pub fn generate_summary_line(&self) -> String {
        let mut parts = Vec::new();

        let total_clicks = self.button_clicks + self.tab_switches + self.menu_accesses;
        if total_clicks > 0 {
            parts.push(format!("{total_clicks} clicks"));
        }
        if self.text_entries > 0 {
            parts.push(format!("{} text entries", self.text_entries));
        }
        if self.scroll_events > 0 {
            parts.push(format!("{} scrolls", self.scroll_events));
        }

        // Semantic actions
        if self.save_count > 0 {
            parts.push(format!("{} saves", self.save_count));
        }
        if self.test_run_count > 0 {
            parts.push(format!("{} test runs", self.test_run_count));
        }
        if self.search_count > 0 {
            parts.push(format!("{} searches", self.search_count));
        }
        if self.build_count > 0 {
            parts.push(format!("{} builds", self.build_count));
        }
        if self.undo_redo_count > 0 {
            parts.push(format!("{} undo/redo", self.undo_redo_count));
        }
        if self.copy_paste_count > 0 {
            parts.push(format!("{} copy/paste", self.copy_paste_count));
        }

        if parts.is_empty() {
            "no interactions".to_string()
        } else {
            parts.join(", ")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_summary() -> GuiActivitySummary {
        GuiActivitySummary {
            app_name: "VS Code".to_string(),
            window_title: "main.rs — VS Code".to_string(),
            content_label: "main.rs".to_string(),
            start_time: Utc::now(),
            end_time: Utc::now(),
            duration_secs: 300,
            button_clicks: 10,
            text_entries: 5,
            tab_switches: 2,
            menu_accesses: 1,
            tree_navigations: 0,
            scroll_events: 8,
            save_count: 3,
            test_run_count: 2,
            search_count: 1,
            build_count: 0,
            undo_redo_count: 0,
            copy_paste_count: 4,
            top_elements: vec![
                ("Save".to_string(), GuiElementType::Button, 3),
                ("main.rs".to_string(), GuiElementType::TabLabel, 2),
            ],
            unmatched_click_count: 1,
            summary_line: String::new(),
        }
    }

    #[test]
    fn generate_summary_line_includes_counts() {
        let summary = make_summary();
        let line = summary.generate_summary_line();
        assert!(line.contains("13 clicks")); // 10 + 2 + 1
        assert!(line.contains("5 text entries"));
        assert!(line.contains("8 scrolls"));
        assert!(line.contains("3 saves"));
        assert!(line.contains("2 test runs"));
        assert!(line.contains("1 searches"));
        assert!(line.contains("4 copy/paste"));
    }

    #[test]
    fn generate_summary_line_empty() {
        let mut summary = make_summary();
        summary.button_clicks = 0;
        summary.text_entries = 0;
        summary.tab_switches = 0;
        summary.menu_accesses = 0;
        summary.scroll_events = 0;
        summary.save_count = 0;
        summary.test_run_count = 0;
        summary.search_count = 0;
        summary.build_count = 0;
        summary.undo_redo_count = 0;
        summary.copy_paste_count = 0;
        let line = summary.generate_summary_line();
        assert_eq!(line, "no interactions");
    }

    #[test]
    fn serde_roundtrip() {
        let summary = make_summary();
        let json = serde_json::to_string(&summary).unwrap();
        let parsed: GuiActivitySummary = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.app_name, "VS Code");
        assert_eq!(parsed.button_clicks, 10);
        assert_eq!(parsed.save_count, 3);
        assert_eq!(parsed.top_elements.len(), 2);
    }
}
