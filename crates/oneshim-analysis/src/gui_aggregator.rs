//! Aggregates `GuiInteractionEvent`s into `GuiActivitySummary` for a time window.
//!
//! The aggregator buffers events and flushes them into a summary when:
//! - The content label changes (new file/page/channel)
//! - The aggregation window expires
//! - The event count exceeds `max_events`
//!
//! Semantic action detection identifies high-level user intents (save, test run,
//! search, build, undo/redo, copy/paste) from interaction patterns.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use oneshim_core::config::GuiIntelligenceConfig;
use oneshim_core::models::gui_activity::GuiActivitySummary;
use oneshim_core::models::gui_interaction::{
    GuiElementType, GuiInteractionEvent, GuiInteractionType, InteractionType,
};

/// Semantic action counts detected from interaction patterns.
#[derive(Debug, Clone, Default)]
pub struct SemanticActionCounts {
    pub save_count: u32,
    pub test_run_count: u32,
    pub search_count: u32,
    pub build_count: u32,
    pub undo_redo_count: u32,
    pub copy_paste_count: u32,
}

/// Internal buffer for an active aggregation window.
struct AggregationWindow {
    app_name: String,
    window_title: String,
    content_label: String,
    start_time: DateTime<Utc>,
    events: Vec<GuiInteractionEvent>,
}

/// Aggregates GUI interaction events into structured summaries.
///
/// Events are buffered per content label and flushed into a `GuiActivitySummary`
/// when the window closes (content change, timeout, or max events).
pub struct GuiActivityAggregator {
    current_window: Option<AggregationWindow>,
    window_duration_secs: u64,
    max_events: usize,
}

impl GuiActivityAggregator {
    /// Create an aggregator from configuration.
    pub fn new(config: &GuiIntelligenceConfig) -> Self {
        Self {
            current_window: None,
            window_duration_secs: config.aggregation_window_secs,
            max_events: config.max_events_per_segment,
        }
    }

    /// Push a new event into the aggregator.
    ///
    /// Returns a summary if the previous window was flushed due to content
    /// change, time expiry, or max events.
    pub fn push(
        &mut self,
        event: GuiInteractionEvent,
        content_label: &str,
    ) -> Option<GuiActivitySummary> {
        let now = event.timestamp;
        let mut flushed = None;

        if let Some(ref window) = self.current_window {
            let content_changed = window.content_label != content_label;
            let window_expired =
                (now - window.start_time).num_seconds() as u64 >= self.window_duration_secs;
            let max_reached = window.events.len() >= self.max_events;

            if content_changed || window_expired || max_reached {
                flushed = self.flush_inner(now);
            }
        }

        // Start new window or append to current
        if self.current_window.is_none() {
            self.current_window = Some(AggregationWindow {
                app_name: event.app_name.clone(),
                window_title: event.window_title.clone().unwrap_or_default(),
                content_label: content_label.to_string(),
                start_time: now,
                events: vec![event],
            });
        } else if let Some(ref mut window) = self.current_window {
            window.events.push(event);
        }

        flushed
    }

    /// Force-flush the current window, returning the summary if any events exist.
    pub fn flush(&mut self) -> Option<GuiActivitySummary> {
        let now = self
            .current_window
            .as_ref()
            .map(|w| w.events.last().map(|e| e.timestamp).unwrap_or(w.start_time))
            .unwrap_or_else(Utc::now);
        self.flush_inner(now)
    }

    /// Internal flush: aggregate buffered events into a summary.
    fn flush_inner(&mut self, end_time: DateTime<Utc>) -> Option<GuiActivitySummary> {
        let window = self.current_window.take()?;
        if window.events.is_empty() {
            return None;
        }

        let duration_secs = (end_time - window.start_time).num_seconds().max(0) as u64;

        // Count interaction types
        let mut button_clicks: u32 = 0;
        let mut text_entries: u32 = 0;
        let mut tab_switches: u32 = 0;
        let mut menu_accesses: u32 = 0;
        let mut tree_navigations: u32 = 0;
        let mut scroll_events: u32 = 0;
        let mut unmatched_click_count: u32 = 0;

        // Track element frequency: (text, element_type) -> count
        let mut element_freq: HashMap<(String, GuiElementType), u32> = HashMap::new();

        for event in &window.events {
            // Count by element type
            match event.element.element_type {
                GuiElementType::Button | GuiElementType::Link => button_clicks += 1,
                GuiElementType::TextInput | GuiElementType::TextRegion => text_entries += 1,
                GuiElementType::TabLabel => tab_switches += 1,
                GuiElementType::MenuItem => menu_accesses += 1,
                GuiElementType::TreeItem => tree_navigations += 1,
                // ScrollBar element clicks are counted via InteractionType::Scroll
                // below to avoid double-counting.
                GuiElementType::ScrollBar => {}
                GuiElementType::ToolbarIcon | GuiElementType::StatusBar => button_clicks += 1,
                GuiElementType::Unknown => {
                    // Count unmatched clicks
                    if matches!(
                        event.interaction_type,
                        GuiInteractionType::Click
                            | GuiInteractionType::DoubleClick
                            | GuiInteractionType::RightClick
                    ) {
                        unmatched_click_count += 1;
                    }
                }
                GuiElementType::TitleBar => {}
            }

            // Also count scroll from structured interaction type
            if let Some(InteractionType::Scroll { .. }) = &event.interaction {
                scroll_events += 1;
            }

            // Track element frequency
            if !event.element.text.is_empty()
                && event.element.element_type != GuiElementType::Unknown
            {
                *element_freq
                    .entry((
                        event.element.text.clone(),
                        event.element.element_type.clone(),
                    ))
                    .or_insert(0) += 1;
            }
        }

        // Top 5 elements by frequency
        let mut top_elements: Vec<(String, GuiElementType, u32)> = element_freq
            .into_iter()
            .map(|((text, etype), count)| (text, etype, count))
            .collect();
        top_elements.sort_by_key(|e| std::cmp::Reverse(e.2));
        top_elements.truncate(5);

        // Detect semantic actions
        let semantic = Self::detect_semantic_actions(&window.events);

        let mut summary = GuiActivitySummary {
            app_name: window.app_name,
            window_title: window.window_title,
            content_label: window.content_label,
            start_time: window.start_time,
            end_time,
            duration_secs,
            button_clicks,
            text_entries,
            tab_switches,
            menu_accesses,
            tree_navigations,
            scroll_events,
            save_count: semantic.save_count,
            test_run_count: semantic.test_run_count,
            search_count: semantic.search_count,
            build_count: semantic.build_count,
            undo_redo_count: semantic.undo_redo_count,
            copy_paste_count: semantic.copy_paste_count,
            top_elements,
            unmatched_click_count,
            summary_line: String::new(),
        };

        summary.summary_line = summary.generate_summary_line();
        Some(summary)
    }

    /// Detect high-level semantic actions from interaction event patterns.
    pub fn detect_semantic_actions(events: &[GuiInteractionEvent]) -> SemanticActionCounts {
        let mut counts = SemanticActionCounts::default();

        for event in events {
            let text_lower = event.element.text.to_lowercase();

            // Check structured interaction for keyboard shortcuts
            if let Some(InteractionType::KeyboardShortcut { ref keys }) = event.interaction {
                let keys_lower = keys.to_lowercase();

                // Save: Cmd+S / Ctrl+S
                if keys_lower == "cmd+s" || keys_lower == "ctrl+s" {
                    counts.save_count += 1;
                    continue;
                }
                // Search: Cmd+F / Ctrl+F
                if keys_lower == "cmd+f" || keys_lower == "ctrl+f" {
                    counts.search_count += 1;
                    continue;
                }
                // Undo/Redo: Cmd+Z / Ctrl+Z / Cmd+Shift+Z
                if keys_lower == "cmd+z"
                    || keys_lower == "ctrl+z"
                    || keys_lower == "cmd+shift+z"
                    || keys_lower == "ctrl+shift+z"
                    || keys_lower == "cmd+y"
                    || keys_lower == "ctrl+y"
                {
                    counts.undo_redo_count += 1;
                    continue;
                }
                // Copy/Paste: Cmd+C / Cmd+V / Ctrl+C / Ctrl+V
                if keys_lower == "cmd+c"
                    || keys_lower == "ctrl+c"
                    || keys_lower == "cmd+v"
                    || keys_lower == "ctrl+v"
                {
                    counts.copy_paste_count += 1;
                    continue;
                }
            }

            // Check for click-based semantic actions
            if matches!(
                event.interaction_type,
                GuiInteractionType::Click | GuiInteractionType::DoubleClick
            ) {
                // Save: click on element with "save" / "저장" text
                if text_lower.contains("save") || text_lower.contains("저장") {
                    counts.save_count += 1;
                    continue;
                }
                // Test run: click on test-related elements
                if text_lower.contains("test")
                    || text_lower.contains("run test")
                    || text_lower.contains("cargo test")
                    || text_lower.contains("pytest")
                    || text_lower.contains("npm test")
                {
                    counts.test_run_count += 1;
                    continue;
                }
                // Search: click on search/find
                if text_lower.contains("search") || text_lower.contains("find") {
                    counts.search_count += 1;
                    continue;
                }
                // Build: click on build/compile
                if text_lower.contains("build")
                    || text_lower.contains("compile")
                    || text_lower.contains("cargo build")
                {
                    counts.build_count += 1;
                    continue;
                }
            }
        }

        counts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use oneshim_core::models::frame::BoundingBox;
    use oneshim_core::models::gui_interaction::GuiElement;

    fn make_config() -> GuiIntelligenceConfig {
        GuiIntelligenceConfig {
            enabled: true,
            aggregation_window_secs: 60,
            max_events_per_segment: 100,
            proximity_threshold_px: 40,
            ml_model_path: String::new(),
        }
    }

    fn make_event(
        text: &str,
        element_type: GuiElementType,
        interaction_type: GuiInteractionType,
        timestamp: DateTime<Utc>,
        app_name: &str,
    ) -> GuiInteractionEvent {
        GuiInteractionEvent {
            timestamp,
            element: GuiElement {
                text: text.to_string(),
                bbox: BoundingBox {
                    x: 100,
                    y: 200,
                    width: 80,
                    height: 30,
                },
                element_type,
                confidence: 0.9,
                type_confidence: 1.0,
            },
            interaction_type,
            app_name: app_name.to_string(),
            window_title: Some("Test Window".to_string()),
            screen_position: Some((100, 200)),
            interaction: None,
        }
    }

    fn make_shortcut_event(keys: &str, timestamp: DateTime<Utc>) -> GuiInteractionEvent {
        GuiInteractionEvent {
            timestamp,
            element: GuiElement {
                text: String::new(),
                bbox: BoundingBox {
                    x: 0,
                    y: 0,
                    width: 0,
                    height: 0,
                },
                element_type: GuiElementType::Unknown,
                confidence: 0.0,
                type_confidence: 1.0,
            },
            interaction_type: GuiInteractionType::Click,
            app_name: "VS Code".to_string(),
            window_title: Some("main.rs".to_string()),
            screen_position: None,
            interaction: Some(InteractionType::KeyboardShortcut {
                keys: keys.to_string(),
            }),
        }
    }

    #[test]
    fn single_event_produces_correct_counts() {
        let config = make_config();
        let mut agg = GuiActivityAggregator::new(&config);
        let now = Utc::now();

        let event = make_event(
            "Save",
            GuiElementType::Button,
            GuiInteractionType::Click,
            now,
            "VS Code",
        );
        agg.push(event, "main.rs");

        let summary = agg.flush().unwrap();
        assert_eq!(summary.button_clicks, 1);
        assert_eq!(summary.save_count, 1);
        assert_eq!(summary.content_label, "main.rs");
        assert_eq!(summary.app_name, "VS Code");
    }

    #[test]
    fn window_flush_on_content_label_change() {
        let config = make_config();
        let mut agg = GuiActivityAggregator::new(&config);
        let t0 = Utc::now();

        agg.push(
            make_event(
                "Save",
                GuiElementType::Button,
                GuiInteractionType::Click,
                t0,
                "VS Code",
            ),
            "main.rs",
        );

        // Push event with different content label
        let flushed = agg.push(
            make_event(
                "OK",
                GuiElementType::Button,
                GuiInteractionType::Click,
                t0 + Duration::seconds(5),
                "VS Code",
            ),
            "lib.rs",
        );

        let summary = flushed.expect("should flush on content change");
        assert_eq!(summary.content_label, "main.rs");
        assert_eq!(summary.button_clicks, 1);
    }

    #[test]
    fn window_flush_on_time_expiry() {
        let mut config = make_config();
        config.aggregation_window_secs = 10;
        let mut agg = GuiActivityAggregator::new(&config);
        let t0 = Utc::now();

        agg.push(
            make_event(
                "OK",
                GuiElementType::Button,
                GuiInteractionType::Click,
                t0,
                "App",
            ),
            "file.rs",
        );

        // Push event after window expires
        let flushed = agg.push(
            make_event(
                "Cancel",
                GuiElementType::Button,
                GuiInteractionType::Click,
                t0 + Duration::seconds(15),
                "App",
            ),
            "file.rs",
        );

        let summary = flushed.expect("should flush on time expiry");
        assert_eq!(summary.button_clicks, 1);
    }

    #[test]
    fn semantic_action_detection_save() {
        let now = Utc::now();
        let events = vec![
            make_event(
                "Save All",
                GuiElementType::Button,
                GuiInteractionType::Click,
                now,
                "VS Code",
            ),
            make_shortcut_event("Cmd+S", now + Duration::seconds(1)),
        ];

        let counts = GuiActivityAggregator::detect_semantic_actions(&events);
        assert_eq!(counts.save_count, 2);
    }

    #[test]
    fn semantic_action_detection_test_run() {
        let now = Utc::now();
        let events = vec![make_event(
            "Run Test",
            GuiElementType::Button,
            GuiInteractionType::Click,
            now,
            "VS Code",
        )];

        let counts = GuiActivityAggregator::detect_semantic_actions(&events);
        assert_eq!(counts.test_run_count, 1);
    }

    #[test]
    fn semantic_action_detection_search() {
        let now = Utc::now();
        let events = vec![
            make_shortcut_event("Ctrl+F", now),
            make_event(
                "Search",
                GuiElementType::Button,
                GuiInteractionType::Click,
                now + Duration::seconds(1),
                "Chrome",
            ),
        ];

        let counts = GuiActivityAggregator::detect_semantic_actions(&events);
        assert_eq!(counts.search_count, 2);
    }

    #[test]
    fn semantic_action_detection_undo_redo_copy_paste() {
        let now = Utc::now();
        let events = vec![
            make_shortcut_event("Cmd+Z", now),
            make_shortcut_event("Cmd+Y", now + Duration::seconds(1)),
            make_shortcut_event("Ctrl+C", now + Duration::seconds(2)),
            make_shortcut_event("Ctrl+V", now + Duration::seconds(3)),
        ];

        let counts = GuiActivityAggregator::detect_semantic_actions(&events);
        assert_eq!(counts.undo_redo_count, 2);
        assert_eq!(counts.copy_paste_count, 2);
    }

    #[test]
    fn top_elements_sorted_by_frequency() {
        let config = make_config();
        let mut agg = GuiActivityAggregator::new(&config);
        let t0 = Utc::now();

        // Push 3 clicks on "Save", 1 on "OK"
        for i in 0..3 {
            agg.push(
                make_event(
                    "Save",
                    GuiElementType::Button,
                    GuiInteractionType::Click,
                    t0 + Duration::seconds(i),
                    "App",
                ),
                "file.rs",
            );
        }
        agg.push(
            make_event(
                "OK",
                GuiElementType::Button,
                GuiInteractionType::Click,
                t0 + Duration::seconds(3),
                "App",
            ),
            "file.rs",
        );

        let summary = agg.flush().unwrap();
        assert_eq!(summary.top_elements[0].0, "Save");
        assert_eq!(summary.top_elements[0].2, 3);
        assert_eq!(summary.top_elements[1].0, "OK");
        assert_eq!(summary.top_elements[1].2, 1);
    }

    #[test]
    fn summary_line_format() {
        let config = make_config();
        let mut agg = GuiActivityAggregator::new(&config);
        let now = Utc::now();

        agg.push(
            make_event(
                "Save",
                GuiElementType::Button,
                GuiInteractionType::Click,
                now,
                "VS Code",
            ),
            "main.rs",
        );

        let summary = agg.flush().unwrap();
        assert!(summary.summary_line.contains("clicks"));
        assert!(summary.summary_line.contains("saves"));
    }

    #[test]
    fn empty_events_returns_none() {
        let config = make_config();
        let mut agg = GuiActivityAggregator::new(&config);
        assert!(agg.flush().is_none());
    }

    #[test]
    fn unmatched_click_count() {
        let config = make_config();
        let mut agg = GuiActivityAggregator::new(&config);
        let now = Utc::now();

        agg.push(
            make_event(
                "",
                GuiElementType::Unknown,
                GuiInteractionType::Click,
                now,
                "App",
            ),
            "file.rs",
        );

        let summary = agg.flush().unwrap();
        assert_eq!(summary.unmatched_click_count, 1);
    }

    #[test]
    fn max_events_triggers_flush() {
        let mut config = make_config();
        config.max_events_per_segment = 3;
        let mut agg = GuiActivityAggregator::new(&config);
        let t0 = Utc::now();

        for i in 0..3 {
            agg.push(
                make_event(
                    "OK",
                    GuiElementType::Button,
                    GuiInteractionType::Click,
                    t0 + Duration::seconds(i),
                    "App",
                ),
                "file.rs",
            );
        }

        // 4th event should trigger flush
        let flushed = agg.push(
            make_event(
                "OK",
                GuiElementType::Button,
                GuiInteractionType::Click,
                t0 + Duration::seconds(3),
                "App",
            ),
            "file.rs",
        );

        assert!(flushed.is_some());
        let summary = flushed.unwrap();
        assert_eq!(summary.button_clicks, 3);
    }

    #[test]
    fn toolbar_icon_counts_as_button_click() {
        let config = make_config();
        let mut agg = GuiActivityAggregator::new(&config);
        let now = Utc::now();

        agg.push(
            make_event(
                "X",
                GuiElementType::ToolbarIcon,
                GuiInteractionType::Click,
                now,
                "App",
            ),
            "file.rs",
        );

        let summary = agg.flush().unwrap();
        assert_eq!(summary.button_clicks, 1);
    }

    #[test]
    fn status_bar_counts_as_button_click() {
        let config = make_config();
        let mut agg = GuiActivityAggregator::new(&config);
        let now = Utc::now();

        agg.push(
            make_event(
                "Ln 42",
                GuiElementType::StatusBar,
                GuiInteractionType::Click,
                now,
                "VS Code",
            ),
            "file.rs",
        );

        let summary = agg.flush().unwrap();
        assert_eq!(summary.button_clicks, 1);
    }

    #[test]
    fn scroll_bar_element_without_scroll_interaction_not_counted() {
        let config = make_config();
        let mut agg = GuiActivityAggregator::new(&config);
        let now = Utc::now();

        // A click on a ScrollBar element without a Scroll interaction type
        // should NOT increment scroll_events (avoids double-counting).
        agg.push(
            make_event(
                "",
                GuiElementType::ScrollBar,
                GuiInteractionType::Click,
                now,
                "App",
            ),
            "file.rs",
        );

        let summary = agg.flush().unwrap();
        assert_eq!(summary.scroll_events, 0);
    }

    #[test]
    fn scroll_interaction_counted_once() {
        let config = make_config();
        let mut agg = GuiActivityAggregator::new(&config);
        let now = Utc::now();

        // Event with ScrollBar element AND Scroll interaction should count once
        let mut event = make_event(
            "",
            GuiElementType::ScrollBar,
            GuiInteractionType::Click,
            now,
            "App",
        );
        event.interaction = Some(InteractionType::Scroll {
            direction: oneshim_core::models::gui_interaction::ScrollDirection::Down,
            amount: 3.0,
        });

        agg.push(event, "file.rs");

        let summary = agg.flush().unwrap();
        // Only counted once via InteractionType::Scroll, not also via ScrollBar element
        assert_eq!(summary.scroll_events, 1);
    }

    #[test]
    fn korean_save_detected() {
        let now = Utc::now();
        let events = vec![make_event(
            "저장",
            GuiElementType::Button,
            GuiInteractionType::Click,
            now,
            "App",
        )];
        let counts = GuiActivityAggregator::detect_semantic_actions(&events);
        assert_eq!(counts.save_count, 1);
    }
}
