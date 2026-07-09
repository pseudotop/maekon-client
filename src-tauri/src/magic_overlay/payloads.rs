use maekon_core::config::OverlayMode;
use maekon_core::models::coaching::GoalProgressView;
use serde::{Deserialize, Serialize};

/// Serializable payload emitted to the overlay WebView via Tauri events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverlayCoachingPayload {
    pub message_id: String,
    pub profile: String,
    pub trigger_type: String,
    pub text: String,
    pub auto_dismiss_secs: u64,
    pub explanation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverlayUpgradePayload {
    pub message_id: String,
    pub personalized_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverlayFocusPayload {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub border_color: String,
    pub opacity: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverlayGoalPayload {
    pub goals: Vec<GoalProgressView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverlayModePayload {
    pub mode: OverlayMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverlayFullscreenPolicyPayload {
    pub fullscreen_detected: bool,
    pub policy: String,
    pub overlay_allowed: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectionElementPayload {
    pub element_id: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub label: String,
    pub role: Option<String>,
    pub confidence: f64,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectionScenePayload {
    pub scene_id: String,
    pub app_name: Option<String>,
    pub screen_width: u32,
    pub screen_height: u32,
    pub element_count: usize,
    pub elements: Vec<DetectionElementPayload>,
}
