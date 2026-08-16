const APP_COMMANDS: &[&str] = &[
    "get_app_build_info",
    "login",
    "auth_status",
    "logout_all_sessions",
    "update_setting",
    "get_automation_status",
    "get_web_port",
    "get_local_auth_token",
    "get_secret_backend_capabilities",
    "get_feature_capabilities",
    "get_runtime_log_snapshot",
    "get_resource_usage_snapshot",
    "record_frontend_log",
    "get_desktop_permission_status",
    "request_desktop_notification_permission",
    "request_desktop_screen_capture_permission",
    "open_desktop_permission_settings",
    "probe_provider_surface_endpoint",
    "get_allowed_setting_keys",
    "oauth_start_flow",
    "oauth_flow_status",
    "oauth_cancel_flow",
    "oauth_revoke",
    "oauth_connection_status",
    "send_test_notification",
    "simulate_notification_activation",
    "create_ai_session",
    "send_session_message",
    "kill_ai_session",
    "list_ai_sessions",
    "retry_ai_session",
    "get_token_usage",
    "load_session_messages",
    "delete_session_history",
    "rename_ai_session",
    "interrupt_session_turn",
    "steer_session_turn",
    "respond_codex_approval",
    "get_analysis_health",
    "reload_embedding_model",
    "dismiss_coaching_message",
    "submit_coaching_feedback",
    "debug_set_overlay_interactive",
    "get_suggestions_panel_open",
    "toggle_suggestions_panel",
    "toggle_automation_confirm",
    "get_coaching_history",
    "get_goal_progress",
    "update_regime_goals",
    "get_habit_streaks",
    "get_global_shortcut_status",
    "get_capture_status",
    "toggle_capture_pause",
    "set_indicator_visible",
    "get_connection_status",
    "show_main_window",
    "debug_focus_window",
    "debug_window_state",
    "debug_set_window_bounds",
    "debug_place_overlay_for_window",
    "debug_normalize_main_window_state",
    "debug_normalize_main_window_bounds",
    "debug_set_window_fullscreen",
    "open_devtools",
    "save_panel_position",
    "get_panel_position",
    "get_onboarding_status",
    "complete_onboarding",
    "reset_onboarding",
    "toggle_focus_mode",
    "get_focus_mode_status",
    "trigger_manual_capture",
    "extract_ax_tree",
    "start_ax_focus_observer",
    "poll_ax_focus_observer",
    "stop_ax_focus_observer",
    "analyze_current_scene",
    "get_pending_suggestion_count",
    "get_pending_suggestions",
    "get_suggestion_history",
    "submit_suggestion_feedback",
    "record_suggestion_replay_event",
    "request_chat_suggestions",
    "request_current_context_suggestions",
    "explain_suggestion_in_chat",
    "get_suggestion_stats",
    "get_suggestion_daily_stats",
    "get_sync_status",
    "trigger_sync_cycle",
    "discover_sync_peers",
    "forget_sync_peer",
    "get_qc_upload_spool_status",
    "run_qc_upload_spool_step",
    "confirm_automation_command",
    "run_suggestion_action",
    "toggle_detection_overlay",
    "refresh_detection_overlay",
    "start_audio_capture",
    "stop_and_transcribe",
    "get_audio_status",
    "download_whisper_model",
    "cancel_model_download",
    "delete_whisper_model",
    "reload_stt_engine",
    "start_vad_listening",
    "stop_vad_listening",
    "autostart_capabilities",
    "disable_autostart",
    "enable_autostart",
    "get_autostart_config",
    "is_autostart_enabled",
    "mark_autostart_prompt_state",
    "export_bug_report",
    "verify_audit_log",
    "report_frontend_error",
    "get_tray_state",
    "get_tray_geometry",
    "simulate_tray_action",
    "request_app_quit",
    "get_consent",
    "set_consent",
    "withdraw_consent",
    "take_microphone_upgrade_notice",
    // #9639: MK-EXT commands retired — see lib.rs invoke_handler for why.
    "list_task_candidates",
    "list_todos",
    "confirm_task_candidate",
    "dismiss_task_candidate",
    "transition_todo",
    "delete_todo",
    "get_capture_reauth_status",
    "authenticate_capture_history",
    "register_capture_reauth_pin",
    "clear_capture_reauth_pin",
    "lock_capture_reauth",
    "set_capture_reauth_config",
    // ADR-033 memory vault mirror (#9465).
    "run_vault_mirror_cycle",
    "get_vault_mirror_settings",
    "set_vault_mirror_path",
    // OS handoff boundary (#9707).
    "open_external_target",
    // Context-home read surface (#9625).
    "fetch_context_home",
    "open_console_assignment_board",
    // Assignment email draft receipt-only surface (#9627).
    "generate_assignment_email_draft",
    "load_assignment_email_draft",
    "regenerate_assignment_email_draft",
    // Standalone native-file WBS XLSX flow (#10358).
    "generate_tmd_xlsx",
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let build_date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    println!("cargo:rustc-env=BUILD_DATE={build_date}");

    let git_sha = std::process::Command::new("git")
        .args(["rev-parse", "--short=9", "HEAD"])
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=GIT_SHA={git_sha}");

    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
    println!("cargo:rerun-if-changed=src/main.rs");

    // #8044: link LocalAuthentication.framework on macOS so `LAContext` is
    // available at load time for the capture-history biometric re-auth adapter
    // (`reauth::verifier`). Without the explicit link the class may be absent
    // and `AnyClass::get("LAContext")` returns None → biometric degrades to the
    // PIN fallback. Harmless if already linked (the linker dedups frameworks).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-lib=framework=LocalAuthentication");
    }
    // #7734: the command surface + generate_handler! moved into the `[lib]`
    // target (src/lib.rs) — track it too so ACL/APP_COMMANDS regen picks up
    // command-set changes made there, not just in the thin bin shim.
    println!("cargo:rerun-if-changed=src/lib.rs");

    // Embed the Common-Controls v6 manifest into EVERY linked binary of this
    // package on windows-msvc (tauri PR #4383): without it the loader binds
    // comctl32 v5, TaskDialogIndirect is missing, and any exe linking tauri
    // dies at startup with STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139). This must
    // be a plain `rustc-link-arg` (all link units): the `-tests` variant only
    // reaches integration-test targets, NOT the lib unittest harness — the
    // exact binary that crashes under `cargo test`. To keep the real app
    // binary from ending up with two manifests, tauri_build's own winres
    // manifest is disabled below (`new_without_app_manifest`) and this linker
    // manifest — content-identical to tauri-build's default — serves bin and
    // test binaries alike.
    let windows_msvc = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if windows_msvc {
        let manifest = std::env::current_dir()?.join("windows-app-manifest.xml");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    }

    // Keep WDIO permissions out of production ACLs. The nested capability is
    // discovered only when the explicit test-only feature is enabled.
    let capabilities_pattern = if std::env::var_os("CARGO_FEATURE_WEBDRIVER").is_some() {
        "capabilities/**/*.json"
    } else {
        "capabilities/*.json"
    };
    println!("cargo:rerun-if-changed=capabilities");

    let attrs = tauri_build::Attributes::new()
        .app_manifest(tauri_build::AppManifest::new().commands(APP_COMMANDS))
        .capabilities_path_pattern(capabilities_pattern)
        // The linker-embedded manifest above replaces the winres one; leaving
        // both in place would fail the bin link with a duplicate RT_MANIFEST.
        .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
    tauri_build::try_build(attrs)?;
    Ok(())
}
