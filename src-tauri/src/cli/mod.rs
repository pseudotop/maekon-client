//! Debug CLI parser and runner module (debug_assertions only).
//!
//! ADR-013: 500L threshold applied — original 1978L file split into submodules.
//! All symbols are `pub(crate)` and re-exported for use from `main.rs`.
//!
//! Submodule layout:
//!   types.rs       — command enum definitions
//!   parsers.rs     — argument parsers + env-gate helpers
//!   output.rs      — JSON emission, path resolution, file I/O
//!   runners.rs     — run_debug_* command executors
//!   macos_notify.rs — macOS UNUserNotification delegate + send helper

#![allow(unused_imports)]

#[cfg(all(debug_assertions, target_os = "macos"))]
mod macos_notify;
#[cfg(debug_assertions)]
mod output;
#[cfg(debug_assertions)]
mod parsers;
#[cfg(debug_assertions)]
mod pointer_capture;
#[cfg(debug_assertions)]
mod runners;
#[cfg(debug_assertions)]
mod types;
#[cfg(debug_assertions)]
mod windows_sandbox_overhead;

// ── Public re-exports ────────────────────────────────────────────────────────

#[cfg(all(debug_assertions, target_os = "macos"))]
pub(crate) use macos_notify::{
    debug_macos_notification_delegate, register_debug_macos_notification_category,
};
#[cfg(all(debug_assertions, target_os = "macos"))]
pub(crate) use macos_notify::{
    install_debug_macos_notification_delegate_from_env, show_debug_macos_unuser_notification,
};

#[cfg(debug_assertions)]
pub(crate) use output::{
    append_debug_notification_audit_jsonl, debug_macos_notification_category_identifier,
    debug_macos_notification_open_action_identifier, debug_notification_audit_event_payload,
    debug_notification_cli_activation_output_path_from,
    debug_notification_cli_audit_jsonl_path_from,
    debug_notification_cli_diagnostic_jsonl_path_from,
    debug_notification_cli_marker_output_path_from, debug_notification_cli_output_path_from,
    debug_permissions_cli_output_path_from, emit_debug_notification_cli_json,
    emit_debug_notification_cli_marker_json, emit_debug_permissions_cli_json,
    hold_debug_notification_cli_if_requested, hold_debug_permissions_cli_if_requested,
};

#[cfg(debug_assertions)]
pub(crate) use parsers::{
    debug_autostart_cli_command_from, debug_ax_tree_cli_command_from,
    debug_notification_activation_route_from, debug_notification_backend_from,
    debug_notification_cli_command_from, debug_notification_cli_hold_seconds_from,
    debug_permissions_cli_command_from, debug_permissions_cli_hold_seconds_from,
    debug_permissions_runtime_cli_command_from, debug_pointer_capture_cli_command_from,
    debug_pointer_capture_runtime_cli_command_from,
    should_enable_single_instance_for_debug_runtime,
};

#[cfg(debug_assertions)]
pub(crate) use pointer_capture::{
    run_debug_pointer_capture_cli_command, run_debug_pointer_capture_runtime_cli_command,
};
#[cfg(all(debug_assertions, target_os = "macos"))]
pub(crate) use runners::emit_debug_permissions_open_settings_json;
#[cfg(debug_assertions)]
pub(crate) use runners::{
    run_debug_autostart_cli_command, run_debug_ax_tree_cli_command,
    run_debug_notification_cli_command, run_debug_permissions_cli_command,
    run_debug_permissions_runtime_cli_command,
};

#[cfg(debug_assertions)]
pub(crate) use types::{
    DebugAutostartCliCommand, DebugAxTreeCliCommand, DebugNotificationBackend,
    DebugNotificationCliCommand, DebugPermissionsCliCommand, DebugPermissionsRuntimeCliCommand,
    DebugPointerCaptureCliCommand, DebugPointerCaptureRuntimeCliCommand,
};

#[cfg(all(test, debug_assertions))]
mod cli_runtime_flag_tests {
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
    fn debug_pointer_capture_cli_requires_explicit_env_gate() {
        assert_eq!(
            debug_pointer_capture_cli_command_from(["debug-pointer-capture", "probe"], None),
            None
        );
        assert_eq!(
            debug_pointer_capture_cli_command_from(["debug-pointer-capture", "probe"], Some("1")),
            Some(DebugPointerCaptureCliCommand::Probe {
                frames: 5,
                interval_ms: 100,
            })
        );
    }

    #[test]
    fn debug_pointer_capture_cli_bounds_probe_args() {
        assert_eq!(
            debug_pointer_capture_cli_command_from(
                ["debug-pointer-capture", "probe", "0", "1"],
                Some("yes")
            ),
            Some(DebugPointerCaptureCliCommand::Probe {
                frames: 1,
                interval_ms: 16,
            })
        );
        assert_eq!(
            debug_pointer_capture_cli_command_from(
                ["debug-pointer-capture", "probe", "500", "20000"],
                Some("true")
            ),
            Some(DebugPointerCaptureCliCommand::Probe {
                frames: 30,
                interval_ms: 1_000,
            })
        );
    }

    #[test]
    fn debug_pointer_capture_runtime_cli_parses_overlay_probe() {
        assert_eq!(
            debug_pointer_capture_runtime_cli_command_from(
                ["debug-pointer-capture", "overlay-probe"],
                None
            ),
            None
        );
        assert_eq!(
            debug_pointer_capture_runtime_cli_command_from(
                ["debug-pointer-capture", "overlay-probe", "2", "32"],
                Some("1")
            ),
            Some(DebugPointerCaptureRuntimeCliCommand::OverlayProbe {
                frames: 2,
                interval_ms: 32,
            })
        );
    }

    #[test]
    fn debug_pointer_capture_runtime_cli_parses_reduced_motion_overlay_probe() {
        assert_eq!(
            debug_pointer_capture_runtime_cli_command_from(
                [
                    "debug-pointer-capture",
                    "overlay-probe-reduced-motion",
                    "3",
                    "64"
                ],
                Some("1")
            ),
            Some(
                DebugPointerCaptureRuntimeCliCommand::OverlayProbeReducedMotion {
                    frames: 3,
                    interval_ms: 64,
                }
            )
        );
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
    fn debug_ax_tree_cli_parses_windows_uia_benchmark_defaults() {
        assert_eq!(
            debug_ax_tree_cli_command_from(["debug-ax-tree", "windows-uia-benchmark"], Some("1")),
            Some(DebugAxTreeCliCommand::WindowsUiaBenchmark {
                samples: 5,
                max_depth: 3,
                max_elements: 300,
            })
        );
    }

    #[test]
    fn debug_ax_tree_cli_clamps_windows_uia_benchmark_bounds() {
        assert_eq!(
            debug_ax_tree_cli_command_from(
                [
                    "debug-ax-tree",
                    "windows-uia-benchmark",
                    "999",
                    "999",
                    "99999",
                ],
                Some("1")
            ),
            Some(DebugAxTreeCliCommand::WindowsUiaBenchmark {
                samples: 20,
                max_depth: 8,
                max_elements: 1_000,
            })
        );
    }

    #[test]
    fn debug_ax_tree_cli_parses_windows_ocr_benchmark_defaults() {
        assert_eq!(
            debug_ax_tree_cli_command_from(["debug-ax-tree", "windows-ocr-benchmark"], Some("1")),
            Some(DebugAxTreeCliCommand::WindowsOcrBenchmark {
                samples: 3,
                display_scale_x100: 150,
            })
        );
    }

    #[test]
    fn debug_ax_tree_cli_clamps_windows_ocr_benchmark_bounds() {
        assert_eq!(
            debug_ax_tree_cli_command_from(
                ["debug-ax-tree", "windows-ocr-benchmark", "999", "999"],
                Some("1")
            ),
            Some(DebugAxTreeCliCommand::WindowsOcrBenchmark {
                samples: 10,
                display_scale_x100: 300,
            })
        );
    }

    #[test]
    fn debug_ax_tree_cli_parses_windows_gui_session_e2e_defaults() {
        assert_eq!(
            debug_ax_tree_cli_command_from(["debug-ax-tree", "windows-gui-session-e2e"], Some("1")),
            Some(DebugAxTreeCliCommand::WindowsGuiSessionE2eBenchmark {
                display_scale_x100: 150,
                overlay_hold_ms: 250,
            })
        );
    }

    #[test]
    fn debug_ax_tree_cli_clamps_windows_gui_session_e2e_bounds() {
        assert_eq!(
            debug_ax_tree_cli_command_from(
                ["debug-ax-tree", "windows-gui-session-e2e", "999", "99999"],
                Some("1")
            ),
            Some(DebugAxTreeCliCommand::WindowsGuiSessionE2eBenchmark {
                display_scale_x100: 300,
                overlay_hold_ms: 3_000,
            })
        );
    }

    #[test]
    fn debug_ax_tree_cli_parses_windows_sandbox_overhead_defaults() {
        assert_eq!(
            debug_ax_tree_cli_command_from(
                ["debug-ax-tree", "windows-sandbox-overhead"],
                Some("1")
            ),
            Some(DebugAxTreeCliCommand::WindowsSandboxOverheadBenchmark {
                samples: 3,
                timeout_probe_ms: 25,
            })
        );
    }

    #[test]
    fn debug_ax_tree_cli_clamps_windows_sandbox_overhead_bounds() {
        assert_eq!(
            debug_ax_tree_cli_command_from(
                ["debug-ax-tree", "windows-sandbox-overhead", "999", "999999",],
                Some("1")
            ),
            Some(DebugAxTreeCliCommand::WindowsSandboxOverheadBenchmark {
                samples: 20,
                timeout_probe_ms: 5_000,
            })
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
}
