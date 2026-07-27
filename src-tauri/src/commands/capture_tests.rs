use super::*;
use maekon_core::config::AppConfig;
use maekon_core::consent::ConsentPermissions;
use maekon_core::models::context::WindowBounds;

fn allowed_permissions() -> ConsentPermissions {
    ConsentPermissions {
        screen_capture: true,
        ..ConsentPermissions::default()
    }
}

fn bounds() -> WindowBounds {
    WindowBounds {
        x: 10,
        y: 20,
        width: 640,
        height: 480,
    }
}

#[test]
fn manual_capture_gate_blocks_when_capture_disabled() {
    let mut config = AppConfig::default_config();
    config.vision.capture_enabled = false;

    let result = manual_capture_privacy_gate(
        &config,
        &allowed_permissions(),
        false,
        Some(&bounds()),
        "TextEdit",
        "Untitled",
    );

    assert_eq!(result, Err(ManualCaptureGateError::CaptureDisabled));
}

#[test]
fn manual_capture_gate_blocks_without_screen_capture_consent() {
    let mut config = AppConfig::default_config();
    config.vision.capture_enabled = true;

    let result = manual_capture_privacy_gate(
        &config,
        &ConsentPermissions::default(),
        false,
        Some(&bounds()),
        "TextEdit",
        "Untitled",
    );

    assert_eq!(
        result,
        Err(ManualCaptureGateError::ScreenCaptureConsentMissing)
    );
}

#[test]
fn manual_capture_gate_blocks_when_capture_is_paused() {
    let mut config = AppConfig::default_config();
    config.vision.capture_enabled = true;

    let result = manual_capture_privacy_gate(
        &config,
        &allowed_permissions(),
        true,
        Some(&bounds()),
        "TextEdit",
        "Untitled",
    );

    assert_eq!(result, Err(ManualCaptureGateError::CapturePaused));
}

#[test]
fn manual_capture_gate_requires_window_bounds() {
    let mut config = AppConfig::default_config();
    config.vision.capture_enabled = true;

    let result = manual_capture_privacy_gate(
        &config,
        &allowed_permissions(),
        false,
        None,
        "TextEdit",
        "Untitled",
    );

    assert_eq!(result, Err(ManualCaptureGateError::WindowBoundsRequired));
}

#[test]
fn manual_capture_gate_allows_when_scheduled_capture_gate_allows() {
    let mut config = AppConfig::default_config();
    config.vision.capture_enabled = true;

    let result = manual_capture_privacy_gate(
        &config,
        &allowed_permissions(),
        false,
        Some(&bounds()),
        "TextEdit",
        "Untitled",
    );

    assert_eq!(result, Ok(()));
}

/// #7909 (T1.1): the exclusion policy applies at capture time — a manual,
/// user-initiated capture of an app on the excluded list must be blocked.
#[test]
fn manual_capture_gate_blocks_excluded_app() {
    let mut config = AppConfig::default_config();
    config.vision.capture_enabled = true;
    config.privacy.excluded_apps = vec!["Slack".to_string()];

    let result = manual_capture_privacy_gate(
        &config,
        &allowed_permissions(),
        false,
        Some(&bounds()),
        "Slack",
        "General",
    );

    assert_eq!(result, Err(ManualCaptureGateError::ExcludedApp));
}

/// #8879: Windows reports the executable identity with `.exe`, while the
/// settings surface stores the human-facing application name.
#[test]
fn manual_capture_gate_blocks_excluded_windows_executable() {
    let mut config = AppConfig::default_config();
    config.vision.capture_enabled = true;
    config.privacy.excluded_apps = vec!["Notepad".to_string()];

    let result = manual_capture_privacy_gate(
        &config,
        &allowed_permissions(),
        false,
        Some(&bounds()),
        "Notepad.exe",
        "Untitled - Notepad",
    );

    assert_eq!(result, Err(ManualCaptureGateError::ExcludedApp));
}

/// #7909 (T1.1): `auto_exclude_sensitive` defaults on, so sensitive apps
/// (password managers, banking) are blocked from manual capture without
/// any explicit list entry.
#[test]
fn manual_capture_gate_auto_blocks_sensitive_app() {
    let mut config = AppConfig::default_config();
    config.vision.capture_enabled = true;

    let result = manual_capture_privacy_gate(
        &config,
        &allowed_permissions(),
        false,
        Some(&bounds()),
        "1Password",
        "Unlock",
    );

    assert_eq!(result, Err(ManualCaptureGateError::ExcludedApp));
}

/// Own-field gate (#4802): when only screen_capture is granted
/// (ocr_processing=false), the manual-capture OCR text must be discarded (None).
#[test]
fn manual_ocr_not_collected_with_only_screen_capture() {
    let perms = allowed_permissions();
    assert!(perms.screen_capture, "composite gate passes");
    assert!(!perms.ocr_processing, "ocr_processing defaults to false");
    let gated = gate_manual_ocr_text(Some("user@example.com".to_string()), perms.ocr_processing);
    assert!(
        gated.is_none(),
        "without ocr_processing, manual-capture OCR text is None (no leak)"
    );
}

/// Own-field gate (#4802): when ocr_processing is granted, the manual-capture OCR text must be preserved.
#[test]
fn manual_ocr_collected_when_own_field_granted() {
    let perms = ConsentPermissions {
        screen_capture: true,
        ocr_processing: true,
        ..ConsentPermissions::default()
    };
    let gated = gate_manual_ocr_text(Some("agenda 2026".to_string()), perms.ocr_processing);
    assert_eq!(
        gated.as_deref(),
        Some("agenda 2026"),
        "with ocr_processing granted, manual-capture OCR text is preserved"
    );
}

fn sample_ocr_regions() -> Vec<OcrRegionDto> {
    vec![OcrRegionDto {
        text: "user@example.com".to_string(),
        x: 1,
        y: 2,
        width: 100,
        height: 20,
        confidence: 0.9,
    }]
}

/// Own-field gate (#4802): when only screen_capture is granted
/// (ocr_processing=false), the OCR region text of analyze_current_scene must be
/// discarded (empty Vec).
#[test]
fn scene_ocr_not_collected_with_only_screen_capture() {
    let perms = allowed_permissions();
    assert!(perms.screen_capture, "composite gate passes");
    assert!(!perms.ocr_processing, "ocr_processing defaults to false");
    let gated = gate_scene_ocr_regions(sample_ocr_regions(), perms.ocr_processing);
    assert!(
        gated.is_empty(),
        "without ocr_processing, scene OCR regions are an empty Vec (no leak)"
    );
}

/// Own-field gate (#4802): when ocr_processing is granted, the OCR regions of analyze_current_scene are preserved.
#[test]
fn scene_ocr_collected_when_own_field_granted() {
    let perms = ConsentPermissions {
        screen_capture: true,
        ocr_processing: true,
        ..ConsentPermissions::default()
    };
    let gated = gate_scene_ocr_regions(sample_ocr_regions(), perms.ocr_processing);
    assert_eq!(
        gated.len(),
        1,
        "with ocr_processing granted, scene OCR regions are preserved"
    );
    assert_eq!(gated[0].text, "user@example.com");
}
