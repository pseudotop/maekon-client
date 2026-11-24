use super::validate_capture_scene;
use maekon_core::config::PrivacyConfig;
use maekon_core::error::CoreError;
use maekon_core::error_codes::PermissionCode;
use maekon_core::models::context::WindowBounds;
use maekon_core::ports::vision::{CaptureScene, CaptureWindow};

fn scene() -> CaptureScene {
    let foreground = CaptureWindow {
        native_id: 17,
        pid: 41,
        app_name: "Editor".into(),
        title: "Work note".into(),
        bounds: WindowBounds {
            x: -1800,
            y: 20,
            width: 800,
            height: 600,
        },
    };
    let background = CaptureWindow {
        native_id: 23,
        pid: 42,
        app_name: "Notes".into(),
        title: "Shared document".into(),
        bounds: WindowBounds {
            x: -1000,
            y: 40,
            width: 400,
            height: 300,
        },
    };
    CaptureScene {
        display_id: 0,
        display_bounds: WindowBounds {
            x: -1920,
            y: 0,
            width: 1920,
            height: 1080,
        },
        windows: vec![foreground.clone(), background],
        foreground,
    }
}

fn assert_denied(policy: &PrivacyConfig, observed: &CaptureScene) {
    let error = validate_capture_scene(policy, observed).unwrap_err();
    match error {
        CoreError::PermissionDenied { code, message } => {
            assert_eq!(code, PermissionCode::PrivacyDenied);
            assert_eq!(message, "capture denied: capture scene policy denied");
        }
        other => panic!("expected capture privacy denial, got {other:?}"),
    }
}

#[test]
fn capture_scene_accepts_complete_observation_and_negative_monitor_coordinates() {
    let policy = PrivacyConfig::default();
    let mut observed = scene();
    validate_capture_scene(&policy, &observed).unwrap();
    observed.display_id = u32::MAX;
    observed.windows.reverse();
    validate_capture_scene(&policy, &observed).unwrap();
}

#[test]
fn capture_scene_requires_nonempty_display_geometry() {
    let policy = PrivacyConfig::default();
    for width in [true, false] {
        let mut observed = scene();
        if width {
            observed.display_bounds.width = 0;
        } else {
            observed.display_bounds.height = 0;
        }
        assert_denied(&policy, &observed);
    }
    validate_capture_scene(&policy, &scene()).unwrap();
}

#[test]
fn capture_scene_requires_foreground_in_complete_window_inventory() {
    let policy = PrivacyConfig::default();
    let mut observed = scene();
    observed.windows.clear();
    assert_denied(&policy, &observed);
    observed = scene();
    observed.windows.remove(0);
    assert_denied(&policy, &observed);
    validate_capture_scene(&policy, &scene()).unwrap();
}

#[test]
fn capture_scene_binds_every_foreground_identity_and_geometry_field() {
    let changes: &[fn(&mut CaptureWindow)] = &[
        |w| w.native_id += 1,
        |w| w.pid += 1,
        |w| w.app_name.push_str(" changed"),
        |w| w.title.push_str(" changed"),
        |w| w.bounds.x += 1,
        |w| w.bounds.y += 1,
        |w| w.bounds.width += 1,
        |w| w.bounds.height += 1,
    ];
    let policy = PrivacyConfig::default();
    for change in changes {
        let mut observed = scene();
        change(&mut observed.foreground);
        assert_denied(&policy, &observed);
        observed.windows[0] = observed.foreground.clone();
        validate_capture_scene(&policy, &observed).unwrap();
    }
}

#[test]
fn capture_scene_rejects_unknown_background_owners_and_geometry() {
    let changes: &[fn(&mut CaptureWindow)] = &[
        |w| w.native_id = 0,
        |w| w.pid = 0,
        |w| w.app_name.clear(),
        |w| w.app_name = " \t\n".into(),
        |w| w.bounds.width = 0,
        |w| w.bounds.height = 0,
    ];
    let policy = PrivacyConfig::default();
    for change in changes {
        let mut observed = scene();
        change(&mut observed.windows[1]);
        assert_denied(&policy, &observed);
    }
    validate_capture_scene(&policy, &scene()).unwrap();
}

#[test]
fn capture_scene_rejects_duplicate_native_identity_without_rejecting_shared_pid() {
    let policy = PrivacyConfig::default();
    let mut observed = scene();
    observed.windows[1].pid = observed.foreground.pid;
    validate_capture_scene(&policy, &observed).unwrap();
    observed.windows[1].native_id = observed.foreground.native_id;
    assert_denied(&policy, &observed);
    observed = scene();
    observed.windows.push(observed.foreground.clone());
    assert_denied(&policy, &observed);
}

#[test]
fn capture_scene_applies_exact_app_patterns_and_title_policy_to_background_windows() {
    let original = scene();
    let mut policy = PrivacyConfig::default();
    policy.excluded_apps.push("Notes".into());
    assert_denied(&policy, &original);
    let mut allowed = original.clone();
    allowed.windows[1].app_name = "Other Notes".into();
    validate_capture_scene(&policy, &allowed).unwrap();
    policy.excluded_apps.clear();
    policy.excluded_app_patterns.push("Note*".into());
    assert_denied(&policy, &original);
    validate_capture_scene(&policy, &allowed).unwrap();
    policy.excluded_app_patterns.clear();
    policy.excluded_title_patterns.push("Private*".into());
    allowed.windows[1].title = "Private fixture document".into();
    assert_denied(&policy, &allowed);
    validate_capture_scene(&policy, &original).unwrap();
}

#[test]
fn capture_scene_preserves_sensitive_app_opt_out_and_unicode_names() {
    let mut policy = PrivacyConfig::default();
    let mut observed = scene();
    observed.windows[1].app_name = "1Password".into();
    policy.auto_exclude_sensitive = true;
    assert_denied(&policy, &observed);
    policy.auto_exclude_sensitive = false;
    validate_capture_scene(&policy, &observed).unwrap();
    observed.windows[1].app_name = "한글 메모".into();
    validate_capture_scene(&policy, &observed).unwrap();
    policy.excluded_apps.push("한글 메모".into());
    assert_denied(&policy, &observed);
}

#[test]
fn capture_scene_checks_last_window_in_a_large_inventory() {
    let mut policy = PrivacyConfig::default();
    policy.excluded_apps.push("Private fixture".into());
    let mut observed = scene();
    for native_id in 100..4196 {
        let mut window = observed.windows[1].clone();
        window.native_id = native_id;
        observed.windows.push(window);
    }
    validate_capture_scene(&policy, &observed).unwrap();
    observed.windows.last_mut().unwrap().app_name = "Private fixture".into();
    assert_denied(&policy, &observed);
}
