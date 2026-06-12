//! Payload types and internal state for the MagicOverlay module.

use maekon_core::config::OverlayMode;
use maekon_core::models::coaching::GoalProgressView;
use serde::{Deserialize, Serialize};

// ── Serializable event payloads emitted to the overlay WebView ───────────────

/// Coaching message shown on the overlay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverlayCoachingPayload {
    pub message_id: String,
    pub profile: String,
    pub trigger_type: String,
    pub text: String,
    pub auto_dismiss_secs: u64,
    pub explanation: String,
}

/// LLM-personalized coaching text upgrade.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverlayUpgradePayload {
    pub message_id: String,
    pub personalized_text: String,
}

/// Focus highlight bounding box.
#[allow(dead_code)] // retained for future IPC command usage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverlayFocusPayload {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub border_color: String,
    pub opacity: f32,
}

/// Goal progress update payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverlayGoalPayload {
    pub goals: Vec<GoalProgressView>,
}

/// Overlay display mode change.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverlayModePayload {
    pub mode: OverlayMode,
}

/// Transient pointer-context highlight payload for capture/evidence overlays.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OverlayPointerContextPayload {
    pub enabled: bool,
    pub x: Option<f32>,
    pub y: Option<f32>,
    pub click_count: u32,
    pub click_pulse: bool,
    pub reduced_motion: bool,
    pub ttl_ms: u64,
    pub sample_rate_hz: u32,
}

impl OverlayPointerContextPayload {
    pub fn hidden(click_count: u32) -> Self {
        Self {
            enabled: false,
            x: None,
            y: None,
            click_count,
            click_pulse: false,
            reduced_motion: false,
            ttl_ms: 900,
            sample_rate_hz: 30,
        }
    }
}

/// Fullscreen detection policy result emitted to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverlayFullscreenPolicyPayload {
    pub fullscreen_detected: bool,
    pub policy: String,
    pub overlay_allowed: bool,
    pub reason: String,
}

/// A single UI element in the detection overlay scene.
#[allow(dead_code)] // used by detection overlay IPC commands
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

/// Full UI scene sent to the detection overlay.
#[allow(dead_code)] // used by detection overlay IPC commands
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectionScenePayload {
    pub scene_id: String,
    pub app_name: Option<String>,
    pub screen_width: u32,
    pub screen_height: u32,
    pub element_count: usize,
    pub elements: Vec<DetectionElementPayload>,
}

// ── Internal overlay window state ────────────────────────────────────────────

#[derive(Debug)]
pub(super) struct OverlayState {
    pub(super) mode: OverlayMode,
    pub(super) visible: bool,
    pub(super) current_message_id: Option<String>,
    pub(super) detection_active: bool,
    pub(super) suggestions_panel_open: bool,
    pub(super) automation_confirm_active: bool,
}
