use chrono::{DateTime, Duration, Utc};
use oneshim_core::models::event::ContextEvent;
use oneshim_core::ports::vision::CaptureRequest;
use oneshim_core::ports::vision::CaptureTrigger;
use std::sync::Mutex;
use tracing::{debug, error};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TriggerType {
    WindowChange,
    ErrorDetected,
    SignificantAction,
    FormSubmission,
    ContextSwitch,
    TitleChange,
    Regular,
}

struct TriggerState {
    last_capture: Option<DateTime<Utc>>,
    prev_app_name: Option<String>,
    prev_window_title: Option<String>,
}

pub struct SmartCaptureTrigger {
    state: Mutex<TriggerState>,
    throttle_ms: u64,
}

impl SmartCaptureTrigger {
    pub fn new(throttle_ms: u64) -> Self {
        Self {
            state: Mutex::new(TriggerState {
                last_capture: None,
                prev_app_name: None,
                prev_window_title: None,
            }),
            throttle_ms,
        }
    }

    fn classify_event(event: &ContextEvent, state: &TriggerState) -> TriggerType {
        let title_lower = event.window_title.to_lowercase();
        if title_lower.contains("error") || title_lower.contains("exception") {
            return TriggerType::ErrorDetected;
        }

        // App-level change detection
        if let Some(prev) = &event.prev_app_name {
            if prev != &event.app_name {
                return TriggerType::ContextSwitch;
            }
        } else if let Some(prev) = &state.prev_app_name {
            if prev != &event.app_name {
                return TriggerType::WindowChange;
            }
        }

        // Same app, but different window title (tab switch, file switch, etc.)
        if let Some(prev_title) = &state.prev_window_title {
            if !event.window_title.is_empty()
                && !prev_title.is_empty()
                && prev_title != &event.window_title
            {
                return TriggerType::TitleChange;
            }
        }

        TriggerType::Regular
    }

    pub fn compute_importance(&self, trigger_type: &TriggerType) -> f32 {
        match trigger_type {
            TriggerType::ErrorDetected => 0.9,
            TriggerType::FormSubmission => 0.8,
            TriggerType::ContextSwitch => 0.7,
            TriggerType::WindowChange => 0.6,
            TriggerType::TitleChange => 0.5,
            TriggerType::SignificantAction => 0.5,
            TriggerType::Regular => 0.2,
        }
    }

    fn is_throttled(
        last_capture: &Option<DateTime<Utc>>,
        now: DateTime<Utc>,
        throttle_ms: u64,
    ) -> bool {
        match last_capture {
            Some(last) => {
                let elapsed = now - *last;
                elapsed < Duration::milliseconds(throttle_ms as i64)
            }
            None => false,
        }
    }
}

impl CaptureTrigger for SmartCaptureTrigger {
    // P2 PR-A: state mutex is held across classify + importance calc + throttle
    // check + mutation as one atomic "observe-then-update" window. Tightening
    // would create a race where two events see the same prev_app_name.
    #[allow(clippy::significant_drop_tightening)]
    fn should_capture(&self, event: &ContextEvent) -> Option<CaptureRequest> {
        let mut state = self
            .state
            .lock()
            .map_err(|e| {
                error!("SmartCaptureTrigger state lock poisoned: {e}");
            })
            .ok()?;
        let now = event.timestamp;
        let trigger_type = Self::classify_event(event, &state);
        let base_importance = self.compute_importance(&trigger_type);
        // Boost importance based on input activity: up to +0.3 when user is active
        let input_boost = (event.input_activity_level * 0.3).min(0.3);
        let importance = (base_importance + input_boost).min(1.0);

        if importance < 0.8 && Self::is_throttled(&state.last_capture, now, self.throttle_ms) {
            debug!(
                "capture: {:?} (in progress {:.1})",
                trigger_type, importance
            );
            return None;
        }

        state.last_capture = Some(now);
        state.prev_app_name = Some(event.app_name.clone());
        state.prev_window_title = Some(event.window_title.clone());

        let trigger_type_str = format!("{:?}", trigger_type);
        debug!(
            "capture approval: {} (in progress {:.1})",
            trigger_type_str, importance
        );

        Some(CaptureRequest {
            trigger_type: trigger_type_str,
            importance,
            app_name: event.app_name.clone(),
            window_title: event.window_title.clone(),
            // Window bounds are not available from ContextEvent; the caller
            // sets this field after receiving the CaptureRequest.
            window_bounds: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_event(app: &str, title: &str, prev: Option<&str>) -> ContextEvent {
        ContextEvent {
            app_name: app.to_string(),
            window_title: title.to_string(),
            prev_app_name: prev.map(String::from),
            timestamp: Utc::now(),
            ..Default::default()
        }
    }

    #[test]
    fn window_change_trigger() {
        let trigger = SmartCaptureTrigger::new(5000);
        let event = make_event("Code", "test.rs", Some("Firefox"));
        let req = trigger.should_capture(&event);
        assert!(req.is_some());
        let req = req.unwrap();
        assert_eq!(req.trigger_type, "ContextSwitch");
        assert!(req.importance >= 0.7);
    }

    #[test]
    fn error_detection() {
        let trigger = SmartCaptureTrigger::new(5000);
        let event = make_event("Terminal", "Error: command failed", None);
        let req = trigger.should_capture(&event);
        assert!(req.is_some());
        assert!(req.unwrap().importance >= 0.8);
    }

    #[test]
    fn throttle_low_importance() {
        let trigger = SmartCaptureTrigger::new(5000);

        let event1 = make_event("Code", "main.rs", None);
        assert!(trigger.should_capture(&event1).is_some());

        let event2 = make_event("Code", "lib.rs", None);
        assert!(trigger.should_capture(&event2).is_none());
    }

    #[test]
    fn high_importance_bypasses_throttle() {
        let trigger = SmartCaptureTrigger::new(5000);

        let event1 = make_event("Code", "main.rs", None);
        trigger.should_capture(&event1);

        let event2 = make_event("Terminal", "Error: panic", None);
        assert!(trigger.should_capture(&event2).is_some());
    }

    #[test]
    fn title_change_within_same_app() {
        // Use 0ms throttle so title change isn't suppressed
        let trigger = SmartCaptureTrigger::new(0);

        let event1 = make_event("Chrome", "Gmail - Inbox", None);
        let req1 = trigger.should_capture(&event1);
        assert!(req1.is_some());

        // Same app, different title (tab switch)
        let event2 = make_event("Chrome", "GitHub - Pull Requests", None);
        let req2 = trigger.should_capture(&event2);
        assert!(req2.is_some());
        let req2 = req2.unwrap();
        assert_eq!(req2.trigger_type, "TitleChange");
        assert!((req2.importance - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn title_change_same_title_stays_regular() {
        let trigger = SmartCaptureTrigger::new(0);

        let event1 = make_event("Code", "main.rs", None);
        trigger.should_capture(&event1);

        // Same app, same title — Regular
        let event2 = make_event("Code", "main.rs", None);
        let req2 = trigger.should_capture(&event2);
        assert!(req2.is_some());
        assert_eq!(req2.unwrap().trigger_type, "Regular");
    }

    #[test]
    fn input_activity_boosts_importance() {
        let trigger = SmartCaptureTrigger::new(0);

        // First capture to establish state
        let event1 = make_event("Code", "main.rs", None);
        trigger.should_capture(&event1);

        // Same app/title (Regular=0.2) but with high input activity
        let event2 = ContextEvent {
            app_name: "Code".to_string(),
            window_title: "main.rs".to_string(),
            prev_app_name: None,
            timestamp: Utc::now(),
            input_activity_level: 1.0, // max activity
        };
        let req = trigger.should_capture(&event2).unwrap();
        // Regular (0.2) + boost (1.0 * 0.3 = 0.3) = 0.5
        assert!((req.importance - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn input_activity_zero_no_boost() {
        let trigger = SmartCaptureTrigger::new(0);

        let event = ContextEvent {
            app_name: "Code".to_string(),
            window_title: "main.rs".to_string(),
            prev_app_name: None,
            timestamp: Utc::now(),
            input_activity_level: 0.0,
        };
        let req = trigger.should_capture(&event).unwrap();
        // Regular (0.2) + no boost = 0.2
        assert!((req.importance - 0.2).abs() < f32::EPSILON);
    }

    #[test]
    fn importance_scores() {
        let trigger = SmartCaptureTrigger::new(5000);
        assert_eq!(trigger.compute_importance(&TriggerType::ErrorDetected), 0.9);
        assert_eq!(trigger.compute_importance(&TriggerType::ContextSwitch), 0.7);
        assert_eq!(trigger.compute_importance(&TriggerType::TitleChange), 0.5);
        assert_eq!(trigger.compute_importance(&TriggerType::Regular), 0.2);
    }

    /// Verifies that a high throttle_ms value suppresses a second capture attempt
    /// of the same low-importance event type that arrives before the window expires.
    ///
    /// The trigger uses event.timestamp as the clock, so we control the apparent
    /// wall-clock time by constructing events with explicit timestamps — no real
    /// sleeping required.
    #[test]
    fn throttle_suppresses_second_capture_within_window() {
        // 10 000 ms throttle — any same-app Regular event within 10 s is suppressed.
        let trigger = SmartCaptureTrigger::new(10_000);

        let t0 = Utc::now();

        // First call: no prior capture recorded — must produce a CaptureRequest.
        let event1 = ContextEvent {
            app_name: "Code".to_string(),
            window_title: "main.rs".to_string(),
            prev_app_name: None,
            timestamp: t0,
            ..Default::default()
        };
        let first = trigger.should_capture(&event1);
        assert!(
            first.is_some(),
            "first call with no prior capture should always return Some(CaptureRequest)"
        );

        // Second call: same app/title, timestamp only 1 ms later — well inside the
        // 10 000 ms throttle window, importance == Regular (0.2) < 0.8 threshold.
        let event2 = ContextEvent {
            app_name: "Code".to_string(),
            window_title: "main.rs".to_string(),
            prev_app_name: None,
            timestamp: t0 + chrono::Duration::milliseconds(1),
            ..Default::default()
        };
        let second = trigger.should_capture(&event2);
        assert!(
            second.is_none(),
            "second call within the throttle window for a low-importance event must return None"
        );
    }
}
