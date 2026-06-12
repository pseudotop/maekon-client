//! Unit tests for MagicOverlay handle, payloads, and monitor layout.
//! Extracted from mod.rs (ADR-013 split).

use super::*;
use window::{overlay_monitor_layout_from_parts, tracking_border_window_specs};

#[test]
fn overlay_state_default_mode() {
    let state = types::OverlayState {
        mode: OverlayMode::Minimal,
        visible: false,
        current_message_id: None,
        detection_active: false,
        suggestions_panel_open: false,
        automation_confirm_active: false,
    };
    assert_eq!(state.mode, OverlayMode::Minimal);
    assert!(!state.visible);
    assert!(state.current_message_id.is_none());
    assert!(!state.detection_active);
    assert!(!state.suggestions_panel_open);
    assert!(!state.automation_confirm_active);
}

#[test]
fn overlay_monitor_layout_uses_target_monitor_origin_and_scale() {
    let layout = overlay_monitor_layout_from_parts(2880, 0, 2560, 1600, 2.0);
    assert_eq!(layout.origin_x, 1440.0);
    assert_eq!(layout.logical_width, 1280.0);
}

#[test]
fn overlay_monitor_layout_treats_invalid_scale_as_one() {
    let layout = overlay_monitor_layout_from_parts(1440, 0, 1280, 800, 0.0);
    assert_eq!(layout.origin_x, 1440.0);
    assert_eq!(layout.logical_width, 1280.0);
}

#[test]
fn passive_tracking_surface_uses_capture_compatible_windows_on_windows() {
    assert_eq!(
        passive_tracking_surface_policy("windows", true, false),
        PassiveTrackingSurfacePolicy::ThinBorderWindows
    );
    assert_eq!(
        passive_tracking_surface_policy("windows", false, false),
        PassiveTrackingSurfacePolicy::Hidden
    );
    assert_eq!(
        passive_tracking_surface_policy("windows", true, true),
        PassiveTrackingSurfacePolicy::Hidden
    );
    assert_eq!(
        passive_tracking_surface_policy("macos", true, false),
        PassiveTrackingSurfacePolicy::Hidden
    );
}

#[test]
fn tracking_border_window_specs_do_not_create_fullscreen_capture_targets() {
    let layout = overlay_monitor_layout_from_parts(0, 0, 1920, 1080, 1.0);
    let specs = tracking_border_window_specs(layout, 16.0);

    assert_eq!(specs.len(), 4);
    assert_eq!(specs[0].x, 16.0);
    assert_eq!(specs[0].width, layout.logical_width - 32.0);
    assert_eq!(specs[0].height, 16.0);
    assert_eq!(specs[1].x, 16.0);
    assert_eq!(specs[1].width, layout.logical_width - 32.0);
    assert_eq!(specs[1].height, 16.0);
    assert_eq!(specs[2].width, 16.0);
    assert_eq!(specs[2].y, 16.0);
    assert_eq!(specs[2].height, layout.logical_height - 32.0);
    assert_eq!(specs[3].width, 16.0);
    assert_eq!(specs[3].y, 16.0);
    assert_eq!(specs[3].height, layout.logical_height - 32.0);
    for spec in specs {
        assert!(
            spec.width < layout.logical_width || spec.height < layout.logical_height,
            "tracking border spec must not be a full-screen capture target: {spec:?}"
        );
    }
}

#[test]
fn overlay_coaching_payload_serde_roundtrip() {
    let payload = OverlayCoachingPayload {
        message_id: "msg-001".to_string(),
        profile: "FocusGuard".to_string(),
        trigger_type: "RegimeDrift".to_string(),
        text: "Take a break from coding.".to_string(),
        auto_dismiss_secs: 15,
        explanation: "Frequent app switching detected.".to_string(),
    };
    let json = serde_json::to_string(&payload).expect("serialize");
    let restored: OverlayCoachingPayload = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(restored.message_id, "msg-001");
    assert_eq!(restored.auto_dismiss_secs, 15);
}

#[test]
fn overlay_upgrade_payload_serde_roundtrip() {
    let payload = OverlayUpgradePayload {
        message_id: "msg-002".to_string(),
        personalized_text: "Great focus session! Time for a well-earned break.".to_string(),
    };
    let json = serde_json::to_string(&payload).expect("serialize");
    let restored: OverlayUpgradePayload = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(restored.message_id, "msg-002");
    assert!(restored.personalized_text.contains("well-earned"));
}

#[test]
fn overlay_focus_payload_serde_roundtrip() {
    let payload = OverlayFocusPayload {
        x: 100,
        y: 200,
        width: 800,
        height: 600,
        border_color: "#3b82f6".to_string(),
        opacity: 0.8,
    };
    let json = serde_json::to_string(&payload).expect("serialize");
    let restored: OverlayFocusPayload = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(restored.x, 100);
    assert!((restored.opacity - 0.8).abs() < f32::EPSILON);
}

#[test]
fn overlay_goal_payload_serde_roundtrip() {
    let payload = OverlayGoalPayload {
        goals: vec![maekon_core::models::coaching::GoalProgressView {
            regime_label: "Deep Work".to_string(),
            current_minutes: 45,
            target_minutes: 120,
            percentage: 37,
            display_color: "#3b82f6".to_string(),
        }],
    };
    let json = serde_json::to_string(&payload).expect("serialize");
    let restored: OverlayGoalPayload = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(restored.goals[0].regime_label, "Deep Work");
}

#[test]
fn pointer_context_payload_serde_roundtrip() {
    let payload = types::OverlayPointerContextPayload {
        enabled: true,
        x: Some(120.0),
        y: Some(240.0),
        click_count: 3,
        click_pulse: true,
        reduced_motion: false,
        ttl_ms: 900,
        sample_rate_hz: 30,
    };

    let json = serde_json::to_string(&payload).expect("serialize");
    let restored: types::OverlayPointerContextPayload =
        serde_json::from_str(&json).expect("deserialize");

    assert!(restored.enabled);
    assert_eq!(restored.x, Some(120.0));
    assert_eq!(restored.y, Some(240.0));
    assert!(restored.click_pulse);
    assert_eq!(restored.sample_rate_hz, 30);
}

#[test]
fn pointer_context_hidden_payload_carries_no_coordinates() {
    let payload = types::OverlayPointerContextPayload::hidden(7);

    assert!(!payload.enabled);
    assert!(payload.x.is_none());
    assert!(payload.y.is_none());
    assert_eq!(payload.click_count, 7);
    assert!(!payload.click_pulse);
}
