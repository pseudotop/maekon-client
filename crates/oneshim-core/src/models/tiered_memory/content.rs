use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::super::gui_activity::GuiActivitySummary;

// ---------------------------------------------------------------------------
// Content & engagement types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ContentType {
    File,
    WebPage,
    Channel,
    InnerApp,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WorkType {
    ActiveCoding,
    CodeReview,
    Writing,
    Reading,
    Designing,
    FormFilling,
    Browsing,
    PassiveMeeting,
    ActiveMeeting,
    Navigation,
    // Text-heavy app intelligence (Phase 1)
    TerminalCommands,
    LogReading,
    DocumentWriting,
    DocumentReading,
    ChatComposing,
    #[default]
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EngagementMetrics {
    pub keystrokes_per_min: f32,
    pub mouse_clicks_per_min: f32,
    pub scroll_events_per_min: f32,
    pub shortcut_ratio: f32,
    pub typing_burst_count: u32,
    pub idle_ratio: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContentActivity {
    pub content_label: String,
    pub content_type: ContentType,
    pub start_time: DateTime<Utc>,
    pub duration_secs: u64,
    pub confidence: f32,
    pub work_type: WorkType,
    pub engagement: EngagementMetrics,
    /// GUI activity summary from Phase 2 GUI Intelligence pipeline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gui_summary: Option<GuiActivitySummary>,
}

// ---------------------------------------------------------------------------
// Container detection (RDP / VM / VNC / Citrix)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ContainerType {
    Rdp,
    Vm,
    Vnc,
    Citrix,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerInfo {
    pub container_app: String,
    pub container_type: ContainerType,
    pub detected_inner_apps: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_type_new_variants_serde() {
        let val = WorkType::TerminalCommands;
        let json = serde_json::to_string(&val).unwrap();
        assert_eq!(json, r#""TERMINAL_COMMANDS""#);
        let back: WorkType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, WorkType::TerminalCommands);
    }

    #[test]
    fn work_type_unknown_variant_falls_back() {
        // Simulates a future variant name unknown to this build
        let back: WorkType = serde_json::from_str(r#""SOME_FUTURE_VARIANT""#).unwrap();
        assert_eq!(back, WorkType::Unknown);
    }

    #[test]
    fn work_type_all_new_variants_roundtrip() {
        for variant in [
            WorkType::TerminalCommands,
            WorkType::LogReading,
            WorkType::DocumentWriting,
            WorkType::DocumentReading,
            WorkType::ChatComposing,
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            let back: WorkType = serde_json::from_str(&json).unwrap();
            assert_eq!(back, variant);
        }
    }
}
