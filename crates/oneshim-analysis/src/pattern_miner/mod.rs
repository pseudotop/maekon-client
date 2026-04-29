mod detectors;
mod gui_patterns;
mod itemset;
mod sequential;

pub(crate) use detectors::is_communication_app;
pub use gui_patterns::{detect_gui_patterns, GuiPattern};

use chrono::{DateTime, Utc};
use oneshim_core::models::analysis::ActivityPattern;
use oneshim_core::models::event::Event;

/// Pure algorithmic pattern detector. No external I/O dependencies.
#[derive(Default)]
pub struct PatternMiner;

impl PatternMiner {
    pub fn new() -> Self {
        Self
    }

    /// Detect activity patterns from raw events.
    pub fn detect(&self, events: &[Event]) -> Vec<ActivityPattern> {
        let app_switches = self.extract_app_sequence(events);
        let mut patterns = Vec::new();
        patterns.extend(detectors::detect_context_switching(&app_switches));
        patterns.extend(detectors::detect_work_modes(&app_switches));
        patterns.extend(detectors::detect_deep_work_blocks(&app_switches));
        patterns.extend(sequential::detect_sequential_patterns(&app_switches));
        patterns.extend(itemset::detect_co_occurrence_patterns(&app_switches));
        patterns.extend(detectors::detect_communication_bursts(&app_switches));
        patterns
    }

    fn extract_app_sequence(&self, events: &[Event]) -> Vec<(DateTime<Utc>, String)> {
        events
            .iter()
            .filter_map(|e| match e {
                Event::Context(ctx) => Some((ctx.timestamp, ctx.app_name.clone())),
                _ => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use oneshim_core::models::analysis::PatternType;
    use oneshim_core::models::event::ContextEvent;

    pub(crate) fn make_ctx_event(app: &str, mins_offset: i64) -> Event {
        Event::Context(ContextEvent {
            app_name: app.to_string(),
            window_title: format!("{app} Window"),
            prev_app_name: None,
            timestamp: Utc::now() - Duration::minutes(mins_offset),
            ..Default::default()
        })
    }

    fn make_ctx_event_at(app: &str, ts: DateTime<Utc>) -> Event {
        Event::Context(ContextEvent {
            app_name: app.to_string(),
            window_title: format!("{app} Window"),
            prev_app_name: None,
            timestamp: ts,
            ..Default::default()
        })
    }

    #[test]
    fn detect_context_switching_pattern() {
        let events: Vec<Event> = (0..12)
            .map(|i| {
                if i % 2 == 0 {
                    make_ctx_event("Slack", 60 - i)
                } else {
                    make_ctx_event("VSCode", 60 - i)
                }
            })
            .collect();

        let miner = PatternMiner::new();
        let patterns = miner.detect(&events);

        let ctx_switches: Vec<_> = patterns
            .iter()
            .filter(|p| p.pattern_type == PatternType::ContextSwitch)
            .collect();

        assert!(!ctx_switches.is_empty(), "should detect context switching");
        assert!(ctx_switches[0].frequency >= 3);
    }

    #[test]
    fn detect_coding_work_mode() {
        let events: Vec<Event> = (0..10).map(|i| make_ctx_event("VSCode", 30 - i)).collect();

        let miner = PatternMiner::new();
        let patterns = miner.detect(&events);

        let modes: Vec<_> = patterns
            .iter()
            .filter(|p| p.pattern_type == PatternType::WorkMode)
            .collect();

        assert!(!modes.is_empty());
        assert!(modes[0].description.contains("coding"));
    }

    #[test]
    fn no_patterns_for_few_events() {
        let events = vec![make_ctx_event("VSCode", 1)];
        let miner = PatternMiner::new();
        let patterns = miner.detect(&events);
        let ctx_switches: Vec<_> = patterns
            .iter()
            .filter(|p| p.pattern_type == PatternType::ContextSwitch)
            .collect();
        assert!(ctx_switches.is_empty());
    }

    #[test]
    fn empty_events_returns_empty() {
        let miner = PatternMiner::new();
        let patterns = miner.detect(&[]);
        assert!(patterns.is_empty());
    }

    #[test]
    fn handles_identical_timestamps() {
        let now = Utc::now();
        let events = vec![
            make_ctx_event_at("VSCode", now),
            make_ctx_event_at("Slack", now),
            make_ctx_event_at("Chrome", now),
            make_ctx_event_at("VSCode", now),
        ];
        let miner = PatternMiner::new();
        let patterns = miner.detect(&events);
        // Should not panic, may or may not detect patterns
        // The important thing is no crash
        let _ = patterns;
    }

    #[test]
    fn all_six_detectors_run() {
        // Ensure detect() calls all detectors without panic
        let apps = ["Slack", "Chrome", "VSCode"];
        let events: Vec<Event> = (0..12)
            .map(|i| make_ctx_event(apps[i as usize % 3], 60 - i))
            .collect();

        let miner = PatternMiner::new();
        let patterns = miner.detect(&events);
        // At least context-switch and sequential patterns should fire
        assert!(!patterns.is_empty());
    }
}
