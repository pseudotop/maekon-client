//! Tauri IPC command contract tests — CRT-PRV-IPC-001..028.
//!
//! Each `#[test]` asserts that the named IPC command module exists +
//! declares at least one `#[tauri::command]` function. These are SMOKE /
//! SURFACE tests; runtime behavior of each command lives in its own tests.
//!
//! Run via:
//!   cargo test -p maekon-app --test ipc_command_contract

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
