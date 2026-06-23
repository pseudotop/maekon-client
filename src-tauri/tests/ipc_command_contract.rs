//! Tauri IPC command contract tests — CRT-PRV-IPC-001..031.
//!
//! Each `#[test]` asserts that the named IPC command module exists +
//! declares at least one `#[tauri::command]` function. These are SMOKE /
//! SURFACE tests; runtime behavior of each command lives in its own tests.
//!
//! Run via:
//!   cargo test -p maekon-app --test ipc_command_contract

use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

fn commands_dir() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.join("src/commands")
}

fn src_dir() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.join("src")
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_dir() -> PathBuf {
    manifest_dir()
        .parent()
        .expect("src-tauri manifest must live under the repository root")
        .to_path_buf()
}

fn extract_invoke_handler_commands(src: &str) -> BTreeSet<String> {
    let command_block = src
        .split(".invoke_handler(tauri::generate_handler![")
        .nth(1)
        .and_then(|tail| tail.split("])").next())
        .expect("main.rs must register commands through tauri::generate_handler![...]");

    command_block
        .lines()
        .filter_map(|line| {
            let line = line.split("//").next().unwrap_or_default().trim();
            if line.is_empty() {
                return None;
            }
            let command_path = line.trim_end_matches(',');
            command_path.rsplit("::").next().map(str::to_owned)
        })
        .collect()
}

fn extract_build_manifest_commands(src: &str) -> BTreeSet<String> {
    let command_block = src
        .split("const APP_COMMANDS: &[&str] = &[")
        .nth(1)
        .and_then(|tail| tail.split("];").next())
        .expect("build.rs must declare APP_COMMANDS for Tauri capability generation");

    command_block
        .lines()
        .filter_map(|line| {
            let line = line.split("//").next().unwrap_or_default().trim();
            if line.is_empty() {
                return None;
            }
            Some(
                line.trim_end_matches(',')
                    .trim_matches('"')
                    .trim()
                    .to_owned(),
            )
        })
        .collect()
}

fn read_capability(name: &str) -> Value {
    let path = manifest_dir().join("capabilities").join(name);
    let src = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", path.display(), e));
    serde_json::from_str(&src)
        .unwrap_or_else(|e| panic!("Failed to parse {}: {}", path.display(), e))
}

fn capability_permission_ids(capability: &Value) -> BTreeSet<String> {
    capability
        .get("permissions")
        .and_then(Value::as_array)
        .expect("capability must contain a permissions array")
        .iter()
        .map(|permission| {
            permission
                .as_str()
                .map(str::to_owned)
                .or_else(|| {
                    permission
                        .get("identifier")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .expect("permission entries must be strings or objects with identifier")
        })
        .collect()
}

fn capability_windows(capability: &Value) -> BTreeSet<String> {
    capability
        .get("windows")
        .and_then(Value::as_array)
        .expect("capability must contain a windows array")
        .iter()
        .map(|window| {
            window
                .as_str()
                .expect("capability windows must be strings")
                .to_owned()
        })
        .collect()
}

fn app_command_permission(action: &str, command: &str) -> String {
    format!("{action}-{}", command.replace('_', "-"))
}

fn app_command_permission_set(commands: &[&str]) -> BTreeSet<String> {
    commands
        .iter()
        .map(|command| app_command_permission("allow", command))
        .collect()
}

fn declared_app_command_permissions(capability: &Value) -> BTreeSet<String> {
    capability_permission_ids(capability)
        .into_iter()
        .filter(|permission| {
            !permission.contains(':')
                && (permission.starts_with("allow-") || permission.starts_with("deny-"))
        })
        .collect()
}

const OVERLAY_APP_COMMANDS: &[&str] = &[
    "toggle_suggestions_panel",
    "toggle_automation_confirm",
    "get_pending_suggestions",
    "refresh_detection_overlay",
    "toggle_detection_overlay",
    "get_capture_status",
    "dismiss_coaching_message",
    "submit_coaching_feedback",
    "record_suggestion_replay_event",
    "explain_suggestion_in_chat",
    "submit_suggestion_feedback",
    "get_suggestion_history",
    "get_suggestion_stats",
    "get_suggestion_daily_stats",
    "confirm_automation_command",
    "respond_codex_approval",
];

const TRACKING_PANEL_APP_COMMANDS: &[&str] = &[
    "get_capture_status",
    "get_connection_status",
    "get_panel_position",
    "save_panel_position",
    "trigger_manual_capture",
    "analyze_current_scene",
    "get_focus_mode_status",
    "toggle_focus_mode",
    "toggle_suggestions_panel",
    "show_main_window",
    "simulate_tray_action",
    "toggle_capture_pause",
    "set_indicator_visible",
];

/// Some modules under commands/ are pure helpers (no #[tauri::command] —
/// e.g., generate_external_cert.rs is a build-time helper, suggestion_parser.rs
/// is a JSON parser invoked by suggestion handler). For these the contract is
/// "file exists + declares at least one public function".
const HELPER_MODULES: &[&str] = &["generate_external_cert", "suggestion_parser"];

fn assert_command_module(name: &str) {
    let file_path = commands_dir().join(format!("{name}.rs"));
    let dir_path = commands_dir().join(name);
    let mod_path = dir_path.join("mod.rs");
    let src = if file_path.exists() {
        let src = fs::read_to_string(&file_path)
            .unwrap_or_else(|e| panic!("Failed to read {}: {}", file_path.display(), e));
        src
    } else {
        assert!(
            mod_path.exists(),
            "Expected src/commands/{name}.rs or src/commands/{name}/mod.rs to exist"
        );
        let mut module_src = String::new();
        let mut entries = fs::read_dir(&dir_path)
            .unwrap_or_else(|e| panic!("Failed to read {}: {}", dir_path.display(), e))
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
            .collect::<Vec<_>>();
        entries.sort();
        for entry in entries {
            module_src.push_str(
                &fs::read_to_string(&entry)
                    .unwrap_or_else(|e| panic!("Failed to read {}: {}", entry.display(), e)),
            );
            module_src.push('\n');
        }
        module_src
    };

    if HELPER_MODULES.contains(&name) {
        assert!(
            src.contains("pub fn") || src.contains("pub async fn"),
            "Expected src/commands/{name} module (helper module) to declare at least one pub fn"
        );
    } else {
        assert!(
            src.contains("#[tauri::command]") || src.contains("#[command]"),
            "Expected src/commands/{name} module to declare at least one #[tauri::command] fn"
        );
    }
}

#[test]
fn crt_prv_ipc_001_ai_session() {
    assert_command_module("ai_session");
}

#[test]
fn crt_prv_ipc_002_analysis() {
    assert_command_module("analysis");
}

#[test]
fn crt_prv_ipc_003_audio() {
    assert_command_module("audio");
}

#[test]
fn crt_prv_ipc_004_auth() {
    assert_command_module("auth");
}

#[test]
fn crt_prv_ipc_005_automation() {
    assert_command_module("automation");
}

#[test]
fn crt_prv_ipc_006_autostart() {
    assert_command_module("autostart");
}

#[test]
fn crt_prv_ipc_007_bug_report() {
    assert_command_module("bug_report");
}

#[test]
fn crt_prv_ipc_008_build_info() {
    assert_command_module("build_info");
}

#[test]
fn crt_prv_ipc_009_capture() {
    assert_command_module("capture");
}

#[test]
fn crt_prv_ipc_010_capture_status() {
    assert_command_module("capture_status");
}

#[test]
fn crt_prv_ipc_011_coaching() {
    assert_command_module("coaching");
}

#[test]
fn crt_prv_ipc_012_dashboard() {
    assert_command_module("dashboard");
}

#[test]
fn crt_prv_ipc_013_detection() {
    assert_command_module("detection");
}

#[test]
fn crt_prv_ipc_014_error_report() {
    assert_command_module("error_report");
}

#[test]
fn crt_prv_ipc_015_focus() {
    assert_command_module("focus");
}

#[test]
fn crt_prv_ipc_016_generate_external_cert() {
    assert_command_module("generate_external_cert");
}

#[test]
fn crt_prv_ipc_017_integration() {
    assert_command_module("integration");
}

#[test]
fn crt_prv_ipc_018_onboarding() {
    assert_command_module("onboarding");
}

#[test]
fn crt_prv_ipc_019_permissions() {
    assert_command_module("permissions");
}

#[test]
fn crt_prv_ipc_020_settings() {
    assert_command_module("settings");
}

#[test]
fn crt_prv_ipc_021_suggestion_parser() {
    assert_command_module("suggestion_parser");
}

#[test]
fn crt_prv_ipc_022_suggestions() {
    assert_command_module("suggestions");
}

#[test]
fn crt_prv_ipc_023_sync() {
    assert_command_module("sync");
}

#[test]
fn crt_prv_ipc_024_system() {
    assert_command_module("system");
}

#[test]
fn crt_prv_ipc_025_tracking_schedule() {
    assert_command_module("tracking_schedule");
}

#[test]
fn crt_prv_ipc_028_notification() {
    assert_command_module("notification");

    let main_path = src_dir().join("main.rs");
    let main_src = fs::read_to_string(&main_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", main_path.display(), e));
    assert!(
        main_src.contains("commands::notification::simulate_notification_activation"),
        "notification activation debug IPC must be registered in the Tauri invoke handler"
    );
}

#[test]
fn crt_prv_ipc_027_detection_activation_waits_for_visible_scene() {
    let detection_path = commands_dir().join("detection.rs");
    let detection_src = fs::read_to_string(&detection_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", detection_path.display(), e));
    let active_branch = detection_src
        .split("if active {")
        .nth(1)
        .and_then(|tail| tail.split("} else {").next())
        .expect("detection command must still branch on active state");
    assert!(
        !active_branch.contains("set_interactive(true)"),
        "detection command must not make the full-screen overlay interactive before scene analysis yields visible elements"
    );

    let shortcuts_path = src_dir().join("setup_shortcuts.rs");
    let shortcuts_src = fs::read_to_string(&shortcuts_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", shortcuts_path.display(), e));
    let shortcut_active_branch = shortcuts_src
        .split("if now_active {")
        .nth(1)
        .and_then(|tail| tail.split("} else {").next())
        .expect("detection shortcut must still branch on active state");
    assert!(
        !shortcut_active_branch.contains("set_interactive(true)"),
        "detection shortcut must not make the full-screen overlay interactive before scene analysis yields visible elements"
    );

    let overlay_path = src_dir().join("magic_overlay").join("mod.rs");
    let overlay_src = fs::read_to_string(&overlay_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", overlay_path.display(), e));
    assert!(
        overlay_src.contains("build_detection_payload(scene)")
            && overlay_src.contains("state.detection_active = true;")
            && overlay_src.contains("self.apply_window_layout(&state);"),
        "magic overlay must activate the interactive layout only after building a visible detection payload"
    );
}

#[test]
fn crt_prv_ipc_029_build_manifest_matches_invoke_handler() {
    let main_path = src_dir().join("main.rs");
    let main_src = fs::read_to_string(&main_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", main_path.display(), e));
    let build_path = manifest_dir().join("build.rs");
    let build_src = fs::read_to_string(&build_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", build_path.display(), e));

    let invoke_commands = extract_invoke_handler_commands(&main_src);
    let manifest_commands = extract_build_manifest_commands(&build_src);

    assert!(
        !invoke_commands.is_empty(),
        "invoke handler command inventory must not be empty"
    );
    assert_eq!(
        invoke_commands, manifest_commands,
        "Tauri build manifest commands must mirror invoke_handler so generated app-command ACLs stay complete"
    );
}

#[test]
fn crt_prv_ipc_030_main_capability_scopes_all_app_commands() {
    let build_path = manifest_dir().join("build.rs");
    let build_src = fs::read_to_string(&build_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", build_path.display(), e));
    let commands = extract_build_manifest_commands(&build_src);
    assert!(
        !commands.is_empty(),
        "build.rs APP_COMMANDS must list every app IPC command"
    );

    let default = read_capability("default.json");
    assert_eq!(
        capability_windows(&default),
        BTreeSet::from(["main".to_owned()]),
        "default capability must only target the main window"
    );

    let default_permissions = capability_permission_ids(&default);
    for command in &commands {
        let permission = app_command_permission("allow", command);
        assert!(
            default_permissions.contains(&permission),
            "main window capability must explicitly allow app command {permission}"
        );
    }

    for command in OVERLAY_APP_COMMANDS
        .iter()
        .chain(TRACKING_PANEL_APP_COMMANDS)
    {
        assert!(
            commands.contains(*command),
            "window-scoped command {command} must exist in build.rs APP_COMMANDS"
        );
    }

    let overlay = read_capability("overlay.json");
    assert_eq!(
        declared_app_command_permissions(&overlay),
        app_command_permission_set(OVERLAY_APP_COMMANDS),
        "overlay capability must allow exactly the app commands used by overlay.html"
    );

    let tracking_panel = read_capability("tracking-panel.json");
    assert_eq!(
        declared_app_command_permissions(&tracking_panel),
        app_command_permission_set(TRACKING_PANEL_APP_COMMANDS),
        "tracking-panel capability must allow exactly the app commands used by tracking-panel.html"
    );
}

#[test]
fn crt_prv_ipc_031_tauri_origin_confusion_patch_guard() {
    let patch_path = repo_dir()
        .join("patches")
        .join("tauri-2.11.2")
        .join("src")
        .join("webview")
        .join("mod.rs");
    let patch_src = fs::read_to_string(&patch_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", patch_path.display(), e));

    assert!(
        patch_src.contains("current_url.domain() == protocol_url.domain()"),
        "patched Tauri is_local_url must compare full protocol domains"
    );
    assert!(
        patch_src.contains("strip_suffix(\".localhost\")"),
        "patched Tauri custom-protocol check must require a .localhost suffix"
    );
    assert!(
        patch_src.contains("myproto.evil.com"),
        "patched Tauri tests must reject spoofed custom protocol domains"
    );
    assert!(
        patch_src.contains("notregistered.localhost"),
        "patched Tauri tests must reject unregistered localhost protocol labels"
    );
}
