use chrono::{DateTime, Utc};
use oneshim_core::config::PiiFilterLevel;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputCollectionConfig {
    #[serde(default)]
    pub collect_keystroke_timing: bool,
    #[serde(default)]
    pub collect_mouse_trajectory: bool,
    #[serde(default)]
    pub collect_click_coordinates: bool,
    #[serde(default)]
    pub collect_scroll_patterns: bool,
    #[serde(default = "default_trajectory_sample_rate")]
    pub trajectory_sample_rate_ms: u64,
    #[serde(default)]
    pub pii_filter_level: PiiFilterLevel,
}

impl Default for InputCollectionConfig {
    fn default() -> Self {
        Self {
            collect_keystroke_timing: false,
            collect_mouse_trajectory: false,
            collect_click_coordinates: false,
            collect_scroll_patterns: false,
            trajectory_sample_rate_ms: default_trajectory_sample_rate(),
            pii_filter_level: PiiFilterLevel::Standard,
        }
    }
}

fn default_trajectory_sample_rate() -> u64 {
    50
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum KeyEventType {
    Press,
    Release,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeystrokeEvent {
    pub timestamp: DateTime<Utc>,
    pub key_code: String,
    pub event_type: KeyEventType,
    pub interval_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MouseTrajectoryPoint {
    pub timestamp: DateTime<Utc>,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClickEvent {
    pub timestamp: DateTime<Utc>,
    pub button: String,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScrollEvent {
    pub timestamp: DateTime<Utc>,
    pub delta_x: i64,
    pub delta_y: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DetailedInputEvent {
    Keystroke(KeystrokeEvent),
    MouseMove(MouseTrajectoryPoint),
    Click(ClickEvent),
    Scroll(ScrollEvent),
}

pub fn sanitize_key_name(key_name: &str, level: PiiFilterLevel) -> String {
    match level {
        PiiFilterLevel::Off => key_name.to_string(),
        _ => {
            let allowed_keys = [
                "Control",
                "ControlLeft",
                "ControlRight",
                "Shift",
                "ShiftLeft",
                "ShiftRight",
                "Alt",
                "AltGr",
                "Meta",
                "MetaLeft",
                "MetaRight",
                "Tab",
                "Return",
                "Enter",
                "Escape",
                "Backspace",
                "Delete",
                "Space",
                "ArrowUp",
                "ArrowDown",
                "ArrowLeft",
                "ArrowRight",
                "Home",
                "End",
                "PageUp",
                "PageDown",
                "CapsLock",
                "F1",
                "F2",
                "F3",
                "F4",
                "F5",
                "F6",
                "F7",
                "F8",
                "F9",
                "F10",
                "F11",
                "F12",
            ];

            if allowed_keys
                .iter()
                .any(|k| k.eq_ignore_ascii_case(key_name))
            {
                key_name.to_string()
            } else {
                "[MASKED]".to_string()
            }
        }
    }
}

pub struct InputEventBuffer {
    config: InputCollectionConfig,
    events: Vec<DetailedInputEvent>,
    max_buffer_size: usize,
}

impl InputEventBuffer {
    pub fn new(config: InputCollectionConfig, max_buffer_size: usize) -> Self {
        Self {
            config,
            events: Vec::with_capacity(max_buffer_size),
            max_buffer_size,
        }
    }

    pub fn push_keystroke(&mut self, key_name: &str, event_type: KeyEventType, interval_ms: u64) {
        if !self.config.collect_keystroke_timing {
            return;
        }
        if self.events.len() >= self.max_buffer_size {
            return; // buffer full
        }

        let sanitized = sanitize_key_name(key_name, self.config.pii_filter_level);
        self.events
            .push(DetailedInputEvent::Keystroke(KeystrokeEvent {
                timestamp: Utc::now(),
                key_code: sanitized,
                event_type,
                interval_ms,
            }));
    }

    pub fn push_mouse_move(&mut self, x: f64, y: f64) {
        if !self.config.collect_mouse_trajectory {
            return;
        }
        if self.events.len() >= self.max_buffer_size {
            return;
        }

        self.events
            .push(DetailedInputEvent::MouseMove(MouseTrajectoryPoint {
                timestamp: Utc::now(),
                x,
                y,
            }));
    }

    pub fn push_click(&mut self, button: &str, x: f64, y: f64) {
        if !self.config.collect_click_coordinates {
            return;
        }
        if self.events.len() >= self.max_buffer_size {
            return;
        }

        self.events.push(DetailedInputEvent::Click(ClickEvent {
            timestamp: Utc::now(),
            button: button.to_string(),
            x,
            y,
        }));
    }

    pub fn push_scroll(&mut self, delta_x: i64, delta_y: i64) {
        if !self.config.collect_scroll_patterns {
            return;
        }
        if self.events.len() >= self.max_buffer_size {
            return;
        }

        self.events.push(DetailedInputEvent::Scroll(ScrollEvent {
            timestamp: Utc::now(),
            delta_x,
            delta_y,
        }));
    }

    pub fn drain(&mut self) -> Vec<DetailedInputEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_modifier_keys_allowed() {
        let result = sanitize_key_name("ControlLeft", PiiFilterLevel::Standard);
        assert_eq!(result, "ControlLeft");
    }

    #[test]
    fn sanitize_character_keys_masked() {
        let result = sanitize_key_name("KeyA", PiiFilterLevel::Standard);
        assert_eq!(result, "[MASKED]");
    }

    #[test]
    fn sanitize_off_allows_all() {
        let result = sanitize_key_name("KeyA", PiiFilterLevel::Off);
        assert_eq!(result, "KeyA");
    }

    #[test]
    fn buffer_respects_config() {
        let config = InputCollectionConfig {
            collect_keystroke_timing: true,
            collect_mouse_trajectory: false,
            ..Default::default()
        };
        let mut buffer = InputEventBuffer::new(config, 100);

        buffer.push_keystroke("Tab", KeyEventType::Press, 50);
        buffer.push_mouse_move(100.0, 200.0);
        assert_eq!(buffer.len(), 1);
    }

    #[test]
    fn buffer_max_size() {
        let config = InputCollectionConfig {
            collect_keystroke_timing: true,
            ..Default::default()
        };
        let mut buffer = InputEventBuffer::new(config, 2);

        buffer.push_keystroke("Tab", KeyEventType::Press, 10);
        buffer.push_keystroke("Enter", KeyEventType::Press, 20);
        buffer.push_keystroke("Space", KeyEventType::Press, 30);
        assert_eq!(buffer.len(), 2);
    }

    #[test]
    fn buffer_drain() {
        let config = InputCollectionConfig {
            collect_keystroke_timing: true,
            ..Default::default()
        };
        let mut buffer = InputEventBuffer::new(config, 100);
        buffer.push_keystroke("Tab", KeyEventType::Press, 10);

        let events = buffer.drain();
        assert_eq!(events.len(), 1);
        assert!(buffer.is_empty());
    }
}
