//! Tauri IPC command contract tests — CRT-PRV-IPC-001..049.
//!
//! Each `#[test]` asserts that the named IPC command module exists +
//! declares at least one `#[tauri::command]` function. These are SMOKE /
//! SURFACE tests; runtime behavior of each command lives in its own tests.
//!
//! `crt_prv_ipc_035_command_module_enumeration_matches_contract_coverage`
//! (#7718 G1) is a set-equality guard between `src/commands/`'s module list
//! and `COVERED_COMMAND_MODULES` below — it fails whenever a new command
//! module ships without a matching per-module test, closing the drift class
//! that let `audit`/`consent`/`tray` go uncovered.
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
        .expect("lib.rs must register commands through tauri::generate_handler![...]");

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
    "get_suggestions_panel_open",
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
    "run_suggestion_action",
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
    "get_pending_suggestion_count",
    "toggle_suggestions_panel",
    "show_main_window",
    "request_app_quit",
    "toggle_capture_pause",
    "set_indicator_visible",
];

/// Some modules under commands/ are pure helpers (no #[tauri::command] —
/// e.g., generate_external_cert.rs is a build-time helper, suggestion_parser.rs
/// is a JSON parser invoked by suggestion handler, privacy_audit.rs writes the
/// durable privacy-transition audit rows invoked by the pause/focus command
/// paths, #8094). For these the contract is "file exists + declares at least
/// one public function".
const HELPER_MODULES: &[&str] = &[
    "generate_external_cert",
    "privacy_audit",
    "suggestion_parser",
];

/// Every module (file or directory) under `src/commands/` that the
/// `crt_prv_ipc_0NN_*` tests below cover. This is the manifest that
/// `crt_prv_ipc_035_command_module_enumeration_matches_contract_coverage`
/// diffs against a live `read_dir` of `src/commands/` — see #7718 (G1): the
/// hand-maintained per-module test list drifted (`audit`, `consent`, `tray`
/// were live+registered command modules with no contract test coverage) with
/// no automated signal, until this enumeration test.
///
/// Adding a new module under `src/commands/`? Add both a `crt_prv_ipc_0NN_*`
/// test above AND its module name here, or this enumeration test fails.
const COVERED_COMMAND_MODULES: &[&str] = &[
    "ai_session",
    "analysis",
    "assignment_email_draft",
    "audio",
    "audit",
    "auth",
    "automation",
    "autostart",
    "bug_report",
    "build_info",
    "capture",
    "capture_status",
    "coaching",
    "consent",
    "context_home",
    "detection",
    "error_report",
    "extension",
    "focus",
    "generate_external_cert",
    "integration",
    "notification",
    "onboarding",
    "os_handoff",
    "permissions",
    "privacy_audit",
    "qc_upload_spool",
    "reauth",
    "settings",
    "shortcuts",
    "suggestion_parser",
    "suggestions",
    "sync",
    "system",
    "task",
    "tray",
    "vault",
];

/// Rust files colocated with command modules for test organization only.
///
/// Keep this allowlist explicit so the module enumeration still fails closed
/// for a newly added production command module. Each listed file is also
/// checked below to ensure it does not expose a Tauri command.
const TEST_SUPPORT_MODULES: &[&str] = &["capture_tests"];

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

fn assert_command_module_absent(name: &str) {
    let file_path = commands_dir().join(format!("{name}.rs"));
    let dir_path = commands_dir().join(name);
    assert!(
        !file_path.exists() && !dir_path.exists(),
        "Removed duplicate IPC module src/commands/{name} must stay absent"
    );

    let mod_path = commands_dir().join("mod.rs");
    let mod_src = fs::read_to_string(&mod_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", mod_path.display(), e));
    assert!(
        !mod_src.contains(&format!("mod {name};")),
        "Removed duplicate IPC module {name} must not be registered"
    );
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

/// The dashboard IPC duplicate was removed in #7637. Keep the historical TC as
/// a negative contract so the stale command surface cannot silently return.
#[test]
fn crt_prv_ipc_012_dashboard() {
    assert_command_module_absent("dashboard");
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

/// The tracking-schedule IPC duplicate was removed in #7637. The embedded HTTP
/// API remains the delivery surface, and this TC guards that boundary.
#[test]
fn crt_prv_ipc_025_tracking_schedule() {
    assert_command_module_absent("tracking_schedule");
}

#[test]
fn crt_prv_ipc_028_notification() {
    assert_command_module("notification");

    let main_path = src_dir().join("lib.rs");
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

    let shortcuts_path = src_dir().join("setup").join("shortcuts.rs");
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
    let main_path = src_dir().join("lib.rs");
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
fn crt_prv_ipc_032_audit() {
    assert_command_module("audit");
}

#[test]
fn crt_prv_ipc_033_consent() {
    assert_command_module("consent");
}

#[test]
fn crt_prv_ipc_034_tray() {
    assert_command_module("tray");
}

/// #8044: capture-history re-authentication (biometric/PIN) IPC command
/// module coverage — get/authenticate/register-pin/clear-pin/lock/set-config.
#[test]
fn crt_prv_ipc_036_reauth() {
    assert_command_module("reauth");
}

/// ADR-033 (#9465): memory vault mirror IPC module coverage — the "Export now"
/// full-cycle trigger plus the §3 settings surface (read + the §3.3-gated
/// custom-path write). #9508 landed `vault.rs` without adding it here, which
/// left this enumeration test red on `main`; the module is now covered.
#[test]
fn crt_prv_ipc_044_vault() {
    assert_command_module("vault");
}

/// #8094: durable privacy-transition audit helper module coverage.
/// `privacy_audit.rs` is a pure helper (HELPER_MODULES) invoked by the
/// capture-pause and focus-mode command paths to write privacy-safe
/// `audit_log` rows — it must exist AND must never grow an IPC surface of
/// its own: audit writes are a side effect of existing commands, not a
/// frontend-invokable capability (#8094 shipped with a "no new IPC
/// commands" constraint).
#[test]
fn crt_prv_ipc_037_privacy_audit() {
    assert_command_module("privacy_audit");

    let path = commands_dir().join("privacy_audit.rs");
    let src = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", path.display(), e));
    assert!(
        !src.contains("#[tauri::command]") && !src.contains("#[command]"),
        "privacy_audit is a helper module by contract — it must not declare \
         #[tauri::command] functions (add a proper command module + capability \
         review instead of growing this helper into an IPC surface)"
    );
}

/// #8194: the native shortcut-collision TC requires the read-only shortcut
/// registry diagnostics command to remain part of the Tauri IPC surface.
#[test]
fn crt_prv_ipc_038_shortcuts() {
    assert_command_module("shortcuts");
}

/// #8577 (ADR-028): durable task lifecycle IPC module coverage — list/confirm/
/// dismiss/transition/delete. The commands mint their own ids and request
/// hashes, so this module must keep a real `#[command]` surface rather than
/// degrading into a helper.
/// #9639: the MK-EXT IPC surface is RETIRED — this contract now pins that.
///
/// It used to assert the eight `commands::extension::*` commands were on the
/// invoke surface (#8586, ADR-029). Measured before retiring: nothing in
/// production calls `register_package`, so `extension_installs` stays empty
/// forever. Every registered command then dead-ends —
/// `install()` → `RevisionConflict` (`load_row` → None), `list_extensions` → `[]`,
/// and skill-pack activation fails at `get_manifest(install_id)`. The chain is
/// dead at the root, so no amount of frontend work could make it do anything.
///
/// The implementation is kept (`commands/extension.rs`, storage adapters, their
/// tests). This test states the ORDER that revival requires: wire a real
/// `register_package` call site FIRST, then re-register. Flipping only the
/// registration puts the app back to advertising a feature that cannot work.
#[test]
fn crt_prv_ipc_041_extension_surface_stays_retired() {
    let lib = fs::read_to_string(src_dir().join("lib.rs")).expect("read lib.rs");
    let handler = extract_invoke_handler_commands(&lib);
    let retired = [
        "list_extensions",
        "install_extension",
        "set_extension_enablement",
        "update_extension",
        "rollback_extension",
        "uninstall_extension",
        "activate_skill_pack",
        "clear_skill_pack_activation",
    ];
    let live: Vec<&str> = retired
        .iter()
        .copied()
        .filter(|c| handler.contains(*c))
        .collect();
    assert!(
        live.is_empty(),
        "MK-EXT is retired (#9639) but these are registered again: {live:?}\n\
         Reviving requires a production `register_package` call site FIRST — \
         without it every one of these dead-ends on an empty `extension_installs`. \
         Once that exists, update this test to pin the live surface instead."
    );

    // The build manifest must agree — a command in the ACL but not the handler
    // is the half-wired state this retirement exists to prevent.
    let build = fs::read_to_string(manifest_dir().join("build.rs")).expect("read build.rs");
    let manifest = extract_build_manifest_commands(&build);
    let stale: Vec<&str> = retired
        .iter()
        .copied()
        .filter(|c| manifest.contains(*c))
        .collect();
    assert!(
        stale.is_empty(),
        "retired commands still in the build manifest (ACL): {stale:?}"
    );
}

/// #9639: the Skill Pack activation implementation survives the retirement.
///
/// #8588 pinned these commands to the invoke surface. #9639 removed them from it
/// (see the test above), but deliberately did NOT delete the code — skill-pack
/// activation is complete apart from the same missing `register_package` call
/// site, so reviving it is re-registration, not reimplementation.
///
/// This test keeps that promise honest: the module must still declare the
/// commands. If someone deletes them, the retirement has quietly become a
/// removal, and the revival cost stated above is no longer true.
#[test]
fn crt_prv_ipc_042_skill_pack_activation_impl_is_preserved() {
    let ext = fs::read_to_string(commands_dir().join("extension.rs"))
        .expect("read commands/extension.rs");
    for command in ["activate_skill_pack", "clear_skill_pack_activation"] {
        assert!(
            ext.contains(&format!("pub async fn {command}")),
            "extension module must still declare {command} — #9639 retired the \
             IPC surface, not the implementation"
        );
    }
}

/// #9305: QC upload spool recovery exposes a real frontend-invokable IPC
/// surface, so it must remain covered by the module enumeration contract.
#[test]
fn crt_prv_ipc_043_qc_upload_spool() {
    assert_command_module("qc_upload_spool");
}

#[test]
fn crt_prv_ipc_040_task() {
    assert_command_module("task");
}

/// #9707: the OS handoff boundary is the only sanctioned way out of the app,
/// so it must stay a registered command rather than drifting back into
/// per-surface `window.open` calls.
#[test]
fn crt_prv_ipc_045_os_handoff() {
    assert_command_module("os_handoff");
}

/// #9707: the handoff command must NOT be `cfg(feature = "server")`-gated.
///
/// Opening a link is a local-first capability — the connected-mode slices
/// (#9627, #9628) are its first callers, but gating it on `server` would make
/// the default build the one that cannot open anything, and every existing
/// `window.open` caller is in the default build. `auth.rs` is deliberately
/// dual-path; this one is deliberately not, and nothing else records that.
#[test]
fn crt_prv_ipc_046_os_handoff_is_not_server_feature_gated() {
    let path = commands_dir().join("os_handoff.rs");
    let src = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", path.display(), e));

    let command = src
        .split("pub async fn open_external_target")
        .next()
        .expect("os_handoff must declare open_external_target");
    let attribute_tail = command
        .rsplit("#[tauri::command]")
        .next()
        .unwrap_or_default();

    assert!(
        !attribute_tail.contains("feature = \"server\""),
        "open_external_target must stay available in the default build"
    );
}

/// #9625: the context-home read surface is the client's only route to the
/// server's context-home projection, so it must stay a registered command
/// rather than dissolving back into per-surface REST calls that each re-derive
/// their own auth handling.
#[test]
fn crt_prv_ipc_047_context_home() {
    assert_command_module("context_home");
}

/// #9625: `fetch_context_home` must take no identity argument.
///
/// The server resolves actor and organization from the JWT alone. The moment
/// this signature grows a `user_id` / `organization_id` parameter, "fetch
/// someone else's home" becomes expressible from the WebView, and the only
/// thing left between that and a cross-tenant leak is a server-side check. That
/// property is invisible in the type system and would regress silently under a
/// well-meaning "let the caller pick the org" refactor — this pins it.
#[test]
fn crt_prv_ipc_048_context_home_takes_no_identity_argument() {
    let path = commands_dir().join("context_home.rs");
    let src = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", path.display(), e));

    let signature = src
        .split("pub async fn fetch_context_home(")
        .nth(1)
        .expect("context_home must declare fetch_context_home")
        .split(')')
        .next()
        .expect("the signature must close");

    for forbidden in ["user_id", "organization_id", "actor_id", "token", "org_id"] {
        assert!(
            !signature.contains(forbidden),
            "fetch_context_home must not accept `{forbidden}` — identity comes from the JWT, \
             not from the caller. Signature was: {signature}"
        );
    }
}

/// #9627: the WebView may identify only persisted receipts and drafts.
/// Authority-bearing identity, recipient, editable content, and provider
/// selection must remain behind the authenticated Rust/server boundary.
#[test]
fn crt_prv_ipc_049_assignment_email_draft_is_receipt_only() {
    assert_command_module("assignment_email_draft");
    let path = commands_dir().join("assignment_email_draft.rs");
    let src = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", path.display(), e));

    let signature = |name: &str| {
        src.split(&format!("pub async fn {name}("))
            .nth(1)
            .unwrap_or_else(|| panic!("assignment_email_draft must declare {name}"))
            .split(") ->")
            .next()
            .expect("the command signature must close")
    };

    let signatures = [
        signature("generate_assignment_email_draft"),
        signature("load_assignment_email_draft"),
        signature("regenerate_assignment_email_draft"),
    ];
    for forbidden in [
        "organization_id",
        "actor_id",
        "user_id",
        "recipient",
        "subject",
        "body",
        "provider",
        "token",
    ] {
        assert!(
            signatures.iter().all(|value| !value.contains(forbidden)),
            "assignment email draft IPC must not accept `{forbidden}`"
        );
    }
    assert_eq!(src.matches("#[command]").count(), 3);
}

/// #8199: native fullscreen-policy diagnostics must remain debug-gated rather
/// than restoring the broad production overlay IPC removed by #7686.
#[test]
fn crt_prv_ipc_039_overlay_fullscreen_debug_gate() {
    let path = commands_dir().join("coaching.rs");
    let src = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", path.display(), e));
    let command = src
        .split("pub async fn debug_set_overlay_interactive")
        .nth(1)
        .and_then(|tail| tail.split("/// Dismiss a coaching overlay message").next())
        .expect("debug overlay interactive command must remain present");

    assert!(
        command.contains("#[cfg(not(debug_assertions))]")
            && command.contains("debug_only")
            && command.contains("overlay.set_interactive(interactive)")
            && command.contains("overlay.fullscreen_policy_state()"),
        "debug overlay policy command must be release-gated and exercise the real policy path"
    );
}

/// #8568: destructive recovery fixtures must remain compiled out of release
/// builds instead of becoming a production CLI or IPC surface.
#[test]
fn crt_prv_qc_recovery_fixture_cli_is_debug_only() {
    let src_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let lib = fs::read_to_string(src_root.join("lib.rs")).expect("read lib.rs");
    // After the ADR-003 split (#8765) the legacy-migration recovery fixture
    // lives in the `qc_fixture_cli/recovery.rs` directory module.
    let fixture = fs::read_to_string(src_root.join("qc_fixture_cli").join("recovery.rs"))
        .expect("read qc_fixture_cli/recovery.rs");
    let upload_spool =
        fs::read_to_string(src_root.join("qc_upload_spool.rs")).expect("read qc_upload_spool.rs");

    // The debug gate must sit in the module's attribute STACK, not be the
    // immediately-adjacent line: #9071 stacked `#[cfg(feature = "analysis")]`
    // (plus a comment) between `#[cfg(debug_assertions)]` and
    // `mod qc_upload_spool;` — cfg attributes AND-compose, so the release
    // guarantee held, but the old exact-adjacency string match went stale and
    // latently broke `cargo test --workspace` (#9083 follow-on).
    assert!(
        module_attribute_stack(&lib, "pub(crate) mod qc_fixture_cli;")
            .contains("#[cfg(debug_assertions)]"),
        "the QC fixture module must remain absent from release builds"
    );
    assert!(
        fixture.contains("debug-prepare-qc-legacy-migration")
            && fixture.contains("debug-verify-qc-legacy-migration")
            && !fixture.contains("#[tauri::command]"),
        "the legacy migration fixture must remain a debug CLI without an IPC surface"
    );
    assert!(
        module_attribute_stack(&lib, "mod qc_upload_spool;").contains("#[cfg(debug_assertions)]")
            && upload_spool.contains("debug-prepare-qc-upload-spool")
            && upload_spool.contains("debug-verify-qc-upload-spool")
            && upload_spool.contains("MAEKON_DEBUG_QC_UPLOAD_SPOOL_FIXTURE")
            && !upload_spool.contains("#[tauri::command]"),
        "the upload-spool fixture must remain a debug-only CLI without an IPC surface"
    );
}

/// Collect the contiguous attribute/comment lines directly above a module
/// declaration. cfg attributes on the same item AND-compose, so gate checks
/// must look at the whole stack instead of demanding exact line adjacency.
fn module_attribute_stack(source: &str, declaration: &str) -> String {
    let declaration_start = source
        .lines()
        .position(|line| line.trim() == declaration)
        .unwrap_or_else(|| panic!("module declaration not found: {declaration}"));
    let lines: Vec<&str> = source.lines().collect();
    let mut stack: Vec<&str> = Vec::new();
    for line in lines[..declaration_start].iter().rev() {
        let trimmed = line.trim();
        if trimmed.starts_with("#[") || trimmed.starts_with("//") {
            stack.push(trimmed);
        } else {
            break;
        }
    }
    stack.reverse();
    stack.join("\n")
}

/// G1 (#7718) enumeration guard: the module set under `src/commands/` must
/// exactly match `COVERED_COMMAND_MODULES`. This catches the drift class that
/// let `audit`/`consent`/`tray` sit uncovered for multiple modules' worth of
/// history — a future `commands/<new>.rs` addition now fails this test
/// immediately instead of silently expanding the uncovered set.
#[test]
fn crt_prv_ipc_035_command_module_enumeration_matches_contract_coverage() {
    let dir = commands_dir();
    let entries =
        fs::read_dir(&dir).unwrap_or_else(|e| panic!("Failed to read {}: {}", dir.display(), e));

    let mut discovered: BTreeSet<String> = BTreeSet::new();
    let mut discovered_test_support: BTreeSet<String> = BTreeSet::new();
    for entry in entries {
        let entry = entry.unwrap_or_else(|e| panic!("Failed to read dir entry: {e}"));
        let path = entry.path();
        if path.is_dir() {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .expect("command directory module must have a valid UTF-8 name");
            discovered.insert(name.to_owned());
            continue;
        }
        if path.extension().is_some_and(|ext| ext == "rs") {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .expect("command file module must have a valid UTF-8 stem");
            // `mod.rs` is the directory-module wiring file for `commands/`
            // itself (not a command module in its own right).
            if stem == "mod" {
                continue;
            }
            if TEST_SUPPORT_MODULES.contains(&stem) {
                let support_src = fs::read_to_string(&path)
                    .unwrap_or_else(|e| panic!("Failed to read {}: {}", path.display(), e));
                assert!(
                    !support_src.contains("#[tauri::command]")
                        && !support_src.contains("#[command]"),
                    "Test-support module {stem} must not expose a Tauri command"
                );
                discovered_test_support.insert(stem.to_owned());
                continue;
            }
            discovered.insert(stem.to_owned());
        }
    }

    let covered: BTreeSet<String> = COVERED_COMMAND_MODULES
        .iter()
        .map(|s| (*s).to_owned())
        .collect();

    let missing_coverage: Vec<&String> = discovered.difference(&covered).collect();
    let stale_coverage: Vec<&String> = covered.difference(&discovered).collect();
    let expected_test_support: BTreeSet<String> = TEST_SUPPORT_MODULES
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    let missing_test_support: Vec<&String> = expected_test_support
        .difference(&discovered_test_support)
        .collect();

    assert!(
        missing_coverage.is_empty()
            && stale_coverage.is_empty()
            && missing_test_support.is_empty(),
        "src/commands/ module set drifted from COVERED_COMMAND_MODULES — \
         modules on disk but uncovered (add a crt_prv_ipc_0NN_* test + list entry): {missing_coverage:?}; \
         modules listed but no longer on disk (remove the list entry, and the test if orphaned): {stale_coverage:?}; \
         allowlisted test-support modules missing from disk: {missing_test_support:?}"
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
