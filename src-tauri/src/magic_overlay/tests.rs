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
    // Signature: (target_os, effective_capture_permitted, indicator_visible, capture_paused).
    // With capture effectively permitted + indicator visible + not paused → shown.
    assert_eq!(
        passive_tracking_surface_policy("windows", true, true, false),
        PassiveTrackingSurfacePolicy::ThinBorderWindows
    );
    // Indicator hidden → Hidden.
    assert_eq!(
        passive_tracking_surface_policy("windows", true, false, false),
        PassiveTrackingSurfacePolicy::Hidden
    );
    // Paused → Hidden.
    assert_eq!(
        passive_tracking_surface_policy("windows", true, true, true),
        PassiveTrackingSurfacePolicy::Hidden
    );
    // Non-Windows never uses the thin-border surface.
    assert_eq!(
        passive_tracking_surface_policy("macos", true, true, false),
        PassiveTrackingSurfacePolicy::Hidden
    );
}

/// #8094 regression: the passive recording border must stay HIDDEN when capture
/// is NOT effectively permitted (e.g. a fresh no-consent profile) even though the
/// indicator is visible and capture is not paused. Before the effective-gate term
/// was added this returned `ThinBorderWindows`, rendering a recording border while
/// nothing was captured (QC CRT-PRV-UX-PRIVACY-VIS-001).
#[test]
fn passive_tracking_surface_hidden_when_capture_not_effectively_permitted() {
    assert_eq!(
        passive_tracking_surface_policy(
            "windows", /* effective */ false, /* indicator_visible */ true,
            /* capture_paused */ false,
        ),
        PassiveTrackingSurfacePolicy::Hidden,
        "no-consent (effective gate closed) must hide the border despite indicator_visible"
    );
    // Sanity: flipping only the effective term to true (all else equal) shows it.
    assert_eq!(
        passive_tracking_surface_policy("windows", true, true, false),
        PassiveTrackingSurfacePolicy::ThinBorderWindows,
        "granting effective capture (all else equal) surfaces the border"
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

// ── #8849 fullscreen-policy decision (external-application scenario) ────────
//
// The pure `decide_fullscreen_policy` is the honest, deterministic oracle for
// the external-app fullscreen scenario CRT-PRV-OVL-005 requires — it does NOT
// use the Maekon main window as the sole oracle (the prior spec's only path).
// The platform probe that FEEDS `external_fullscreen` needs a real desktop and
// is validated separately per target; here we lock the decision it drives.

/// #8849: a foreground EXTERNAL application reported fullscreen (owned windows
/// all NOT fullscreen) must still SUPPRESS the overlay, with a reason that
/// attributes it to the external app — the exact gap the previous
/// owned-window-only path missed.
#[test]
fn decide_suppresses_for_external_fullscreen_without_owned_fullscreen() {
    let decision = types::decide_fullscreen_policy(false, Some(true));
    assert!(
        !decision.overlay_allowed,
        "external fullscreen must suppress the overlay"
    );
    assert!(decision.fullscreen_detected);
    assert_eq!(decision.policy, "suppress");
    assert!(
        decision.reason.contains("external"),
        "reason must attribute suppression to the external app: {}",
        decision.reason
    );
}

/// The owned-window path is preserved (ADD external detection, do not replace):
/// a Maekon-owned fullscreen window still suppresses even when the external
/// probe reports not-fullscreen.
#[test]
fn decide_suppresses_for_owned_fullscreen() {
    let decision = types::decide_fullscreen_policy(true, Some(false));
    assert!(!decision.overlay_allowed);
    assert_eq!(decision.policy, "suppress");
    assert!(decision.reason.contains("fullscreen"));
}

/// No fullscreen anywhere → overlay allowed. An UNDETERMINED external probe
/// (`None` — unsupported platform / no X server / permission) degrades to
/// allowed rather than suppressing (graceful, matches prior behavior).
#[test]
fn decide_allows_when_nothing_fullscreen_or_probe_undetermined() {
    let allowed = types::decide_fullscreen_policy(false, Some(false));
    assert!(allowed.overlay_allowed);
    assert!(!allowed.fullscreen_detected);
    assert_eq!(allowed.policy, "show_on_top");

    let undetermined = types::decide_fullscreen_policy(false, None);
    assert!(
        undetermined.overlay_allowed,
        "an undetermined external probe must not suppress the overlay"
    );
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

#[test]
fn passive_overlay_window_hides_when_no_surface_is_active() {
    assert_eq!(
        passive_overlay_window_policy(false, false),
        PassiveOverlayWindowPolicy::Hidden,
    );
    assert_eq!(
        passive_overlay_window_policy(false, true),
        PassiveOverlayWindowPolicy::Hidden,
        "CUA safe mode must not leave the passive full-screen compositor active",
    );
}

#[test]
fn passive_overlay_window_exists_only_for_visible_coaching() {
    assert_eq!(
        passive_overlay_window_policy(true, false),
        PassiveOverlayWindowPolicy::FullScreenClickThrough,
        "coaching remains visible even when capture is unavailable",
    );
    assert_eq!(
        passive_overlay_window_policy(false, false),
        PassiveOverlayWindowPolicy::Hidden,
        "no visible coaching must not create a full-screen capture source",
    );
    assert_eq!(
        passive_overlay_window_policy(true, true),
        PassiveOverlayWindowPolicy::Hidden,
        "CUA safe mode must suppress even visible coaching",
    );
}

#[test]
fn macos_pointer_context_does_not_create_a_fullscreen_overlay() {
    assert!(!pointer_context_overlay_supported("macos"));
    assert!(pointer_context_overlay_supported("windows"));
    assert!(pointer_context_overlay_supported("linux"));
}

#[test]
fn pointer_context_overlay_action_fails_closed_on_macos() {
    assert_eq!(
        pointer_context_overlay_action("macos", false),
        PointerContextOverlayAction::Suppress
    );
    assert_eq!(
        pointer_context_overlay_action("macos", true),
        PointerContextOverlayAction::Suppress
    );
}

#[test]
fn pointer_context_overlay_action_preserves_non_macos_updates() {
    assert_eq!(
        pointer_context_overlay_action("windows", false),
        PointerContextOverlayAction::EmitOnly
    );
    assert_eq!(
        pointer_context_overlay_action("windows", true),
        PointerContextOverlayAction::ShowAndEmit
    );
    assert_eq!(
        pointer_context_overlay_action("linux", true),
        PointerContextOverlayAction::ShowAndEmit
    );
}

#[test]
fn macos_setup_does_not_create_a_display_sized_native_border_window() {
    let platform_setup = include_str!("../setup/platform.rs");
    let window_setup = include_str!("../setup/windows.rs");
    let app_library = include_str!("../lib.rs");
    assert!(
        !platform_setup.contains("NativeBorderIndicator::new"),
        "macOS must not recreate the display-sized NSWindow capture source",
    );
    assert!(
        !app_library.contains("pub mod native_border;"),
        "the removed native-border runtime must not be registered again",
    );
    assert!(
        !window_setup.contains("overlay.ensure_window()"),
        "startup must not pre-create a hidden display-sized WebView that ScreenCaptureKit can enumerate",
    );
}

#[test]
fn macos_tracking_panel_fails_closed_when_capture_exclusion_is_unavailable() {
    let window_source = include_str!("window.rs");
    assert!(
        window_source.contains("setSharingType(NSWindowSharingType::None)"),
        "the tracking panel must opt out of cross-process window capture on macOS",
    );
    assert!(
        window_source.contains("sharingType() != NSWindowSharingType::None"),
        "the policy must fail closed unless the native readback confirms capture exclusion",
    );
    assert!(
        window_source.contains("configure_tracking_panel_capture_policy(&panel)"),
        "the native capture policy must be applied to the newly built tracking panel",
    );
    assert!(
        window_source.contains("failed to destroy unsafe tracking panel"),
        "a panel without enforceable capture exclusion must be destroyed, not shown",
    );
    assert_eq!(
        window_source
            .matches("setSharingType(NSWindowSharingType::None)")
            .count(),
        1,
        "capture exclusion must stay scoped to the tracking panel; the main window remains recordable",
    );
}

// ── #7076 least-privilege event-scoping policy ───────────────────────────
//
// These cover the window-label selection logic that `emit_overlay_event` uses to
// decide between `emit_to(overlay)` (screen-content) and the app-wide `emit`
// (control events). The actual cross-webview delivery boundary is exercised by
// the Tauri-backed private/integration TCs in CI; here we lock the policy that
// drives the emit choice so a regression to a global emit fails fast.

#[test]
fn screen_content_events_are_scoped_to_the_overlay_window() {
    // Every screen-content event (focus/detection/heatmap/pointer coordinates) resolves to the
    // transparent overlay webview label, never an app-wide broadcast.
    for event in OVERLAY_SCREEN_CONTENT_EVENTS {
        assert_eq!(
            screen_content_event_target(event),
            Some("magic-overlay"),
            "{event} must be scoped to the overlay webview, not broadcast app-wide",
        );
    }
}

#[test]
fn screen_content_target_excludes_lower_privilege_windows() {
    // The overlay label must differ from the `main` / `tracking-panel` webviews,
    // which also hold core:event:allow-listen but must not receive screen content.
    let target = screen_content_event_target("overlay:update-focus")
        .expect("update-focus is a screen-content event");
    assert_ne!(target, "main");
    assert_ne!(target, "tracking-panel");
}

#[test]
fn pointer_coordinates_are_scoped_to_the_overlay_window() {
    assert_eq!(
        screen_content_event_target("overlay:pointer-context-update"),
        Some("magic-overlay"),
        "pointer coordinates must never be broadcast to main or tracking-panel webviews",
    );
}

#[test]
fn control_events_keep_app_wide_broadcast() {
    // Non-screen-content control events stay un-scoped (None → app-wide emit), so
    // existing main / tracking-panel listeners keep working.
    assert_eq!(screen_content_event_target("overlay:show-coaching"), None);
    assert_eq!(screen_content_event_target("overlay:set-mode"), None);
    assert_eq!(
        screen_content_event_target("overlay:suggestions-changed"),
        None
    );
}
