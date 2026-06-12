use super::*;

#[test]
fn debug_notification_uses_stable_macos_activation_category_ids() {
    assert_eq!(
        debug_macos_notification_category_identifier(),
        "maekon.debug.notification.activation"
    );
    assert_eq!(
        debug_macos_notification_open_action_identifier(),
        "maekon.debug.notification.open"
    );
}

#[test]
fn debug_notification_cli_accepts_optional_diagnostic_jsonl_path() {
    assert_eq!(
        debug_notification_cli_diagnostic_jsonl_path_from(None),
        None
    );
    assert_eq!(
        debug_notification_cli_diagnostic_jsonl_path_from(Some("  ")),
        None
    );
    assert_eq!(
        debug_notification_cli_diagnostic_jsonl_path_from(Some(
            "/tmp/notification-diagnostic.jsonl"
        )),
        Some(std::path::PathBuf::from(
            "/tmp/notification-diagnostic.jsonl"
        ))
    );
}

#[test]
fn offline_mode_defaults_to_false() {
    assert!(!crate::offline_mode_enabled_from(Vec::<&str>::new(), None));
}

#[test]
fn offline_mode_accepts_cli_flag() {
    assert!(crate::offline_mode_enabled_from(["--offline"], None));
    assert!(crate::offline_mode_enabled_from(["--offline=true"], None));
}

#[test]
fn offline_mode_accepts_env_override() {
    assert!(crate::offline_mode_enabled_from(
        Vec::<&str>::new(),
        Some("true")
    ));
    assert!(crate::offline_mode_enabled_from(["--other"], Some("1")));
}

#[test]
fn debug_autostart_cli_requires_explicit_env_gate() {
    assert_eq!(
        debug_autostart_cli_command_from(["debug-autostart", "status"], None),
        None
    );
    assert_eq!(
        debug_autostart_cli_command_from(["debug-autostart", "status"], Some("1")),
        Some(DebugAutostartCliCommand::Status)
    );
}

#[test]
fn debug_autostart_cli_parses_enable_disable_status() {
    assert_eq!(
        debug_autostart_cli_command_from(["debug-autostart", "enable"], Some("true")),
        Some(DebugAutostartCliCommand::Enable)
    );
    assert_eq!(
        debug_autostart_cli_command_from(["debug-autostart", "disable"], Some("yes")),
        Some(DebugAutostartCliCommand::Disable)
    );
    assert_eq!(
        debug_autostart_cli_command_from(["debug-autostart", "unknown"], Some("1")),
        None
    );
}

#[test]
fn debug_power_cli_requires_explicit_env_gate() {
    assert_eq!(
        debug_power_cli_command_from(["debug-power", "capture-burst-audit"], None),
        None
    );
    assert_eq!(
        debug_power_cli_command_from(["debug-power", "capture-burst-audit"], Some("1")),
        Some(DebugPowerCliCommand::CaptureBurstAudit)
    );
}

#[test]
fn debug_power_capture_burst_audit_payload_records_no_spurious_wake_burst() {
    let payload = debug_power_capture_burst_audit_payload();

    assert_eq!(payload["debug_power"], true);
    assert_eq!(payload["command"], "capture-burst-audit");
    assert_eq!(payload["initial_capture"], true);
    assert_eq!(payload["wake_gap_capture"], true);
    assert_eq!(payload["same_tick_burst_count"], 0);
    assert_eq!(payload["no_spurious_capture_burst"], true);
}

#[test]
fn debug_permissions_cli_requires_explicit_env_gate() {
    assert_eq!(
        debug_permissions_cli_command_from(["debug-permissions", "status"], None),
        None
    );
    assert_eq!(
        debug_permissions_cli_command_from(["debug-permissions", "status"], Some("1")),
        Some(DebugPermissionsCliCommand::Status)
    );
}

#[test]
fn debug_permissions_cli_parses_permission_commands() {
    assert_eq!(
        debug_permissions_cli_command_from(
            ["debug-permissions", "screen-capture-request"],
            Some("true")
        ),
        Some(DebugPermissionsCliCommand::ScreenCaptureRequest)
    );
    assert_eq!(
        debug_permissions_cli_command_from(
            ["debug-permissions", "screen-capture-attempt"],
            Some("true")
        ),
        Some(DebugPermissionsCliCommand::ScreenCaptureAttempt)
    );
    assert_eq!(
        debug_permissions_cli_command_from(
            ["debug-permissions", "accessibility-request"],
            Some("yes")
        ),
        Some(DebugPermissionsCliCommand::AccessibilityRequest)
    );
    assert_eq!(
        debug_permissions_cli_command_from(
            ["debug-permissions", "open-settings", "accessibility"],
            Some("yes")
        ),
        Some(DebugPermissionsCliCommand::OpenAccessibilitySettings)
    );
    assert_eq!(
        debug_permissions_cli_command_from(
            ["debug-permissions", "open-settings", "screen_capture"],
            Some("yes")
        ),
        Some(DebugPermissionsCliCommand::OpenScreenCaptureSettings)
    );
    assert_eq!(
        debug_permissions_cli_command_from(
            ["debug-permissions", "open-settings", "screen-capture"],
            Some("yes")
        ),
        Some(DebugPermissionsCliCommand::OpenScreenCaptureSettings)
    );
    assert_eq!(
        debug_permissions_cli_command_from(["debug-permissions", "unknown"], Some("1")),
        None
    );
    assert_eq!(
        debug_permissions_cli_command_from(
            ["debug-permissions", "open-settings", "camera"],
            Some("1")
        ),
        None
    );
}

#[test]
fn debug_permissions_cli_accepts_optional_output_path() {
    assert_eq!(debug_permissions_cli_output_path_from(None), None);
    assert_eq!(debug_permissions_cli_output_path_from(Some("  ")), None);
    assert_eq!(
        debug_permissions_cli_output_path_from(Some("/tmp/permission.json")),
        Some(std::path::PathBuf::from("/tmp/permission.json"))
    );
}

#[test]
fn debug_permissions_cli_hold_seconds_are_bounded() {
    assert_eq!(debug_permissions_cli_hold_seconds_from(None), 0);
    assert_eq!(debug_permissions_cli_hold_seconds_from(Some("bad")), 0);
    assert_eq!(debug_permissions_cli_hold_seconds_from(Some("8")), 8);
    assert_eq!(debug_permissions_cli_hold_seconds_from(Some("600")), 60);
}

#[test]
fn debug_permissions_runtime_cli_parses_screen_capture_request() {
    assert_eq!(
        debug_permissions_runtime_cli_command_from(
            ["debug-permissions-runtime", "screen-capture-request"],
            None
        ),
        None
    );
    assert_eq!(
        debug_permissions_runtime_cli_command_from(
            ["debug-permissions-runtime", "screen-capture-request"],
            Some("1")
        ),
        Some(DebugPermissionsRuntimeCliCommand::ScreenCaptureRequest)
    );
    assert_eq!(
        debug_permissions_runtime_cli_command_from(
            ["debug-permissions-runtime", "screen-capture-attempt"],
            Some("1")
        ),
        None
    );
}

#[test]
fn debug_notification_cli_requires_explicit_env_gate() {
    assert_eq!(
        debug_notification_cli_command_from(["debug-notification", "status"], None),
        None
    );
    assert_eq!(
        debug_notification_cli_command_from(["debug-notification", "status"], Some("1")),
        Some(DebugNotificationCliCommand::Status)
    );
}

#[test]
fn debug_notification_cli_parses_notification_commands() {
    assert_eq!(
        debug_notification_cli_command_from(["debug-notification", "request"], Some("true")),
        Some(DebugNotificationCliCommand::Request)
    );
    assert_eq!(
        debug_notification_cli_command_from(["debug-notification", "send"], Some("yes")),
        Some(DebugNotificationCliCommand::Send)
    );
    assert_eq!(
        debug_notification_cli_command_from(["debug-notification", "unknown"], Some("1")),
        None
    );
}

#[test]
fn debug_notification_cli_disables_single_instance_lock() {
    assert!(should_enable_single_instance_for_debug_runtime(None, None));
    assert!(!should_enable_single_instance_for_debug_runtime(
        Some(DebugNotificationCliCommand::Request),
        None
    ));
    assert!(!should_enable_single_instance_for_debug_runtime(
        Some(DebugNotificationCliCommand::Send),
        None
    ));
    assert!(!should_enable_single_instance_for_debug_runtime(
        None,
        Some(DebugPermissionsRuntimeCliCommand::ScreenCaptureRequest)
    ));
}

#[test]
fn debug_notification_cli_accepts_optional_output_path() {
    assert_eq!(debug_notification_cli_output_path_from(None), None);
    assert_eq!(debug_notification_cli_output_path_from(Some("  ")), None);
    assert_eq!(
        debug_notification_cli_output_path_from(Some("/tmp/notification.json")),
        Some(std::path::PathBuf::from("/tmp/notification.json"))
    );
}

#[test]
fn debug_notification_cli_accepts_optional_marker_output_path() {
    assert_eq!(debug_notification_cli_marker_output_path_from(None), None);
    assert_eq!(
        debug_notification_cli_marker_output_path_from(Some("  ")),
        None
    );
    assert_eq!(
        debug_notification_cli_marker_output_path_from(Some("/tmp/notification-started.json")),
        Some(std::path::PathBuf::from("/tmp/notification-started.json"))
    );
}

#[test]
fn debug_notification_cli_accepts_optional_activation_output_path() {
    assert_eq!(
        debug_notification_cli_activation_output_path_from(None),
        None
    );
    assert_eq!(
        debug_notification_cli_activation_output_path_from(Some("  ")),
        None
    );
    assert_eq!(
        debug_notification_cli_activation_output_path_from(Some(
            "/tmp/notification-activated.json"
        )),
        Some(std::path::PathBuf::from("/tmp/notification-activated.json"))
    );
}

#[test]
fn debug_notification_cli_accepts_optional_audit_jsonl_path() {
    assert_eq!(debug_notification_cli_audit_jsonl_path_from(None), None);
    assert_eq!(
        debug_notification_cli_audit_jsonl_path_from(Some("  ")),
        None
    );
    assert_eq!(
        debug_notification_cli_audit_jsonl_path_from(Some("/tmp/notification-audit.jsonl")),
        Some(std::path::PathBuf::from("/tmp/notification-audit.jsonl"))
    );
}

#[test]
fn debug_notification_audit_payload_records_metadata_without_raw_body() {
    let payload = debug_notification_audit_event_payload(
        "send",
        DebugNotificationBackend::MacosUnuser,
        true,
        "Review draft",
        "Secret meeting body",
        Some("/replay/timeline"),
    );

    assert_eq!(payload["event"], "debug_notification.send");
    assert_eq!(payload["backend"], "macos-unuser");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["title_present"], true);
    assert_eq!(payload["body_present"], true);
    assert_eq!(payload["body_len"], 19);
    assert_eq!(payload["activation_route"], "/replay/timeline");
    let serialized = serde_json::to_string(&payload).unwrap();
    assert!(
        !serialized.contains("Secret meeting body"),
        "audit payload must not store raw notification body: {serialized}"
    );
}

#[test]
fn debug_notification_cli_accepts_safe_activation_route() {
    assert_eq!(debug_notification_activation_route_from(None), None);
    assert_eq!(debug_notification_activation_route_from(Some("  ")), None);
    assert_eq!(
        debug_notification_activation_route_from(Some("/replay/timeline")),
        Some("/replay/timeline".to_string())
    );
    assert_eq!(
        debug_notification_activation_route_from(Some("https://example.com")),
        None
    );
    assert_eq!(
        debug_notification_activation_route_from(Some("//evil")),
        None
    );
}

#[test]
fn debug_notification_cli_hold_seconds_are_bounded() {
    assert_eq!(debug_notification_cli_hold_seconds_from(None), 0);
    assert_eq!(debug_notification_cli_hold_seconds_from(Some("bad")), 0);
    assert_eq!(debug_notification_cli_hold_seconds_from(Some("9")), 9);
    assert_eq!(debug_notification_cli_hold_seconds_from(Some("600")), 60);
}

#[test]
fn debug_notification_backend_defaults_to_tauri_plugin() {
    assert_eq!(
        debug_notification_backend_from(None),
        DebugNotificationBackend::TauriPlugin
    );
    assert_eq!(
        debug_notification_backend_from(Some("  ")),
        DebugNotificationBackend::TauriPlugin
    );
    assert_eq!(
        debug_notification_backend_from(Some("unknown")),
        DebugNotificationBackend::TauriPlugin
    );
}

#[test]
fn debug_notification_backend_accepts_macos_unuser_aliases() {
    assert_eq!(
        debug_notification_backend_from(Some("macos-unuser")),
        DebugNotificationBackend::MacosUnuser
    );
    assert_eq!(
        debug_notification_backend_from(Some("UNUserNotificationCenter")),
        DebugNotificationBackend::MacosUnuser
    );
}
