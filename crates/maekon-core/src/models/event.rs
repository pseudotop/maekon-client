use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

use super::app_registry::KeystrokeProfile;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Event {
    User(UserEvent),
    System(SystemEvent),
    Context(ContextEvent),
    Input(InputActivityEvent),
    Process(ProcessSnapshotEvent),
    Window(WindowLayoutEvent),
    Clipboard(ClipboardEvent),
    FileAccess(FileAccessEvent),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ClipboardContentType {
    Text,
    Image,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardEvent {
    pub timestamp: DateTime<Utc>,
    pub content_type: ClipboardContentType,
    pub char_count: usize,
    pub preview: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FileEventType {
    Created,
    Modified,
    Deleted,
    Renamed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileAccessEvent {
    pub timestamp: DateTime<Utc>,
    pub relative_path: PathBuf,
    pub event_type: FileEventType,
    pub extension: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserEvent {
    pub event_id: Uuid,
    pub event_type: UserEventType,
    pub timestamp: DateTime<Utc>,
    pub app_name: String,
    pub window_title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UserEventType {
    WindowChange,
    AppSwitch,
    SignificantAction,
    FormSubmission,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemEvent {
    pub event_id: Uuid,
    pub event_type: SystemEventType,
    pub timestamp: DateTime<Utc>,
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SystemEventType {
    MetricsUpdate,
    Alert,
    NetworkChange,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ContextEvent {
    pub app_name: String,
    pub window_title: String,
    pub prev_app_name: Option<String>,
    pub timestamp: DateTime<Utc>,
    /// Normalized input activity level (0.0–1.0) from InputActivityCollector.
    /// Used by capture trigger to boost importance when user is actively interacting.
    #[serde(default)]
    pub input_activity_level: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventBatch {
    pub session_id: String,
    pub events: Vec<Event>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputActivityEvent {
    pub timestamp: DateTime<Utc>,
    pub period_secs: u32,
    pub mouse: MouseActivity,
    pub keyboard: KeyboardActivity,
    pub app_name: String,
    /// Keystroke profile with key-category ratios.
    /// Present only when `input_pattern_detail` is enabled in config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keystroke_profile: Option<KeystrokeProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MouseActivity {
    pub click_count: u32,
    pub move_distance: f64,
    pub scroll_count: u32,
    pub last_position: Option<(f32, f32)>,
    pub double_click_count: u32,
    pub right_click_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KeyboardActivity {
    pub keystrokes_per_min: u32,
    pub total_keystrokes: u32,
    pub typing_bursts: u32,
    pub shortcut_count: u32,
    pub correction_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessSnapshotEvent {
    pub timestamp: DateTime<Utc>,
    pub processes: Vec<ProcessDetail>,
    pub total_process_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessDetail {
    pub name: String,
    /// PID
    pub pid: u32,
    pub cpu_percent: f32,
    pub memory_mb: f64,
    pub window_count: u32,
    pub is_foreground: bool,
    pub running_secs: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executable_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowLayoutEvent {
    pub timestamp: DateTime<Utc>,
    pub event_type: WindowLayoutEventType,
    pub window: WindowInfo,
    pub screen_resolution: (u32, u32),
    pub monitor_index: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WindowLayoutEventType {
    Focus,
    Resize,
    Move,
    Maximize,
    Minimize,
    Restore,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowInfo {
    pub app_name: String,
    pub window_title: String,
    pub position: (i32, i32),
    pub size: (u32, u32),
    pub screen_ratio: f32,
    pub is_fullscreen: bool,
    pub z_order: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_event_without_keystroke_profile_deserializes() {
        let json = r#"{
            "timestamp": "2026-03-19T00:00:00Z",
            "period_secs": 30,
            "mouse": {"click_count":0,"move_distance":0.0,"scroll_count":0,"last_position":null,"double_click_count":0,"right_click_count":0},
            "keyboard": {"keystrokes_per_min":0,"total_keystrokes":0,"typing_bursts":0,"shortcut_count":0,"correction_count":0},
            "app_name": "Code"
        }"#;
        let event: InputActivityEvent = serde_json::from_str(json).unwrap();
        assert!(event.keystroke_profile.is_none());
    }
}
