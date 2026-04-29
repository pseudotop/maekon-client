#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![allow(unexpected_cfgs)]
// Cast safety: UI metrics, scheduler counters, coordinates — precision loss acceptable.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
// P2 nursery-hardening (PR-B): derive Eq alongside PartialEq when possible.
#![deny(clippy::derive_partial_eq_without_eq)]

//! Maekon Desktop Agent - Tauri v2 entry point
//!
//! iced GUI에서 Tauri v2로 마이그레이션된 데스크톱 에이전트.
//! 시스템 트레이, WebView 대시보드, IPC 커맨드를 통합 관리합니다.

mod agent_runtime;
mod agent_runtime_support;
mod app_runtime_launch;
mod app_runtime_launch_health_probe;
mod audit_query;
mod auditing_session;
mod auth_cli;
mod automation_controller_builder;
mod automation_runtime;
mod autostart;
mod background_runtime;
mod bootstrap_preflight;
mod bootstrap_runtime;
mod bridge_cli;
mod capture_services;
mod cli_subscription_bridge;
mod commands;
mod desktop_permissions;
mod desktop_startup;
mod fallback_stt;
mod feature_capabilities;
#[cfg(feature = "server")]
mod feedback_sink;
mod focus_analyzer;
mod focus_auto;
mod focus_mode;
mod focus_probe_adapter;
#[cfg(feature = "server")]
mod integration_insight_source;
mod integration_policy;
#[cfg(feature = "server")]
mod integration_prompt_delivery;
#[cfg(feature = "server")]
mod integration_runtime;
mod integrity_guard;
mod ipc_error;
mod launch_resources;
mod lifecycle;
mod log_retention;
#[cfg(target_os = "macos")]
mod macos_integration;
mod magic_overlay;
mod magic_overlay_driver;
mod memory_profiler;
mod native_border;
mod notification_manager;
mod oauth_provider_registry;
mod platform_accessibility;
mod platform_overlay;
mod provider_adapters;
mod provider_secret_backend;
mod runtime_bridges;
mod runtime_state;
mod scheduler;
mod secret_cli;
#[cfg(feature = "server")]
mod server_runtime_context;
mod services;
mod session_adapters;
mod session_context;
mod session_manager;
mod setup;
mod setup_platform;
mod setup_shortcuts;
mod setup_windows;
mod skill_loader;
mod storage_runtime;
mod subprocess_provider;
mod suggestion_manager;
mod sync_engine;
mod telemetry;
mod tray;
mod tray_icon;
mod tray_watch;
mod update_coordinator;
mod update_runtime;
mod updater;
mod web_server_runtime;
mod workflow_intelligence;

use tauri::{Manager, RunEvent};
#[cfg(target_os = "macos")]
use tracing::debug;
use tracing::{info, warn};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

fn configure_runtime_flavor() {
    #[cfg(debug_assertions)]
    {
        // Keep local debug clients from opening the release install's data directory.
        if std::env::var_os("ONESHIM_APP_FLAVOR").is_none() {
            std::env::set_var("ONESHIM_APP_FLAVOR", "dev");
        }
    }
}

/// Wrapper for `tracing_appender::non_blocking::WorkerGuard`.
///
/// Stored as Tauri managed state so it is dropped (and flushed) when the
/// app exits rather than leaked.  The inner field is intentionally never
/// read — its purpose is to keep the guard alive for the duration of the
/// process.
#[allow(dead_code)] // RAII: inner guard kept alive for log flushing on Drop
pub(crate) struct LogWorkerGuard(tracing_appender::non_blocking::WorkerGuard);

fn main() {
    configure_runtime_flavor();

    // D13 Task 13: `generate-external-cert` CLI subcommand — dispatched BEFORE
    // any Tauri initialization so we never spawn the webview runtime for
    // pure-utility invocations. Tauri itself does not parse CLI args for
    // arbitrary subcommands; we do it here.
    #[cfg(feature = "external-grpc-tools")]
    {
        let args: Vec<String> = std::env::args().collect();
        if args.get(1).map(|s| s.as_str()) == Some("generate-external-cert") {
            match crate::commands::generate_external_cert::cli::run(&args[2..]) {
                Ok(()) => std::process::exit(0),
                Err(e) => {
                    eprintln!("{e:#}");
                    std::process::exit(1);
                }
            }
        }
    }

    // Windows DLL search order hardening (Spec Section 9.2):
    // Remove CWD from DLL search path to prevent DLL hijacking.
    #[cfg(target_os = "windows")]
    unsafe {
        windows_sys::Win32::System::LibraryLoader::SetDllDirectoryW(windows_sys::core::w!(""));
    }

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("oneshim=info,oneshim_app=info,oneshim_core=info,oneshim_monitor=info,oneshim_vision=info,oneshim_storage=info,oneshim_network=info,oneshim_suggestion=info")
    });

    // Console layer — writes to stderr (same as previous fmt() subscriber).
    let console_layer = tracing_subscriber::fmt::layer().with_ansi(true);

    // File layer — daily rolling log files in {data_dir}/logs/.
    // WorkerGuard MUST outlive the subscriber; we store it in Tauri state.
    let log_dir = oneshim_core::config_manager::ConfigManager::data_dir()
        .map(|d| d.join("logs"))
        .unwrap_or_else(|_| std::path::PathBuf::from("logs"));

    std::fs::create_dir_all(&log_dir).ok();

    // Cleanup old log files before creating new appender
    let deleted = log_retention::cleanup_old_logs(&log_dir, log_retention::DEFAULT_MAX_AGE_DAYS);
    if deleted > 0 {
        // Cannot use tracing yet — subscriber not initialized.
        eprintln!("[oneshim] startup log cleanup: deleted {deleted} old log file(s)");
    }

    let file_appender = tracing_appender::rolling::daily(&log_dir, "oneshim.log");
    let (non_blocking, worker_guard) = tracing_appender::non_blocking(file_appender);

    let file_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(non_blocking);

    // Telemetry layer + handle. ConfigManager does not exist yet at this
    // point (it is built during bootstrap below), so we seed with
    // TelemetryConfig::default() which is disabled — no exporter is built.
    // The bus-driven reconcile task spawned in bootstrap_runtime.rs picks up
    // the user's real setting on its first iteration and applies it.
    let telemetry_data_dir = oneshim_core::config_manager::ConfigManager::data_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."));
    std::fs::create_dir_all(&telemetry_data_dir).ok();
    let (telemetry_layer, telemetry_handle) = telemetry::Handle::new_with_layer(
        &oneshim_core::config::TelemetryConfig::default(),
        &telemetry_data_dir,
    )
    .expect("disabled-at-boot telemetry construction is infallible");
    let telemetry_handle = std::sync::Arc::new(telemetry_handle);

    // Layer order matters: the OTel layer's type is tied to `Registry` as its
    // Subscriber param (see tracing_opentelemetry::OpenTelemetryLayer<S, T>),
    // so the reload wrapper around it must attach directly on top of Registry.
    // Higher layers (env_filter, console, file) stack above as plain
    // Layer<Registry> impls and don't change the Subscriber type the OTel
    // layer sees.
    tracing_subscriber::registry()
        .with(telemetry_layer)
        .with(env_filter)
        .with(console_layer)
        .with(file_layer)
        .init();

    info!(log_dir = %log_dir.display(), "persistent file logging initialized");

    // CLI pre-dispatch: handle "auth" subcommand before Tauri boot
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && args[1] == "auth" {
        let config_dir = oneshim_core::config_manager::ConfigManager::config_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."));
        let exit_code = auth_cli::run(&args[2..], &config_dir);
        std::process::exit(exit_code);
    }
    if args.len() > 1 && args[1] == "secret" {
        let config_dir = oneshim_core::config_manager::ConfigManager::config_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."));
        let exit_code = secret_cli::run(&args[2..], &config_dir);
        std::process::exit(exit_code);
    }
    if args.len() > 1 && args[1] == "bridge" {
        let data_dir = oneshim_core::config_manager::ConfigManager::data_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."));
        let exit_code = bridge_cli::run(&args[2..], &data_dir);
        std::process::exit(exit_code);
    }

    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // Callback runs in 1st instance when 2nd instance launches.
            // Must be cheap + synchronous (no async, no DB calls).
            // Order matters per spec §5.2 mitigation #1: show() → unminimize() → set_focus().
            // Reverse order can leave window unfocused on Linux/X11.
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
            // _args, _cwd reserved for future CLI command extension (NG3).
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(LogWorkerGuard(worker_guard))
        .manage(telemetry_handle);

    // WebDriver 서버 플러그인 — E2E 테스트용 (production 빌드에 절대 포함 금지)
    #[cfg(feature = "webdriver")]
    {
        let port = std::env::var("TAURI_WEBDRIVER_PORT")
            .ok()
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(4445);
        info!("WebDriver plugin enabled on port {port}");
        builder = builder.plugin(tauri_plugin_webdriver::init_with_port(port));
    }

    let app = builder
        .setup(setup::init)
        .on_window_event(|window, event| {
            // Close-to-tray: 윈도우 닫기 시 숨기기 (실제 종료 아님)
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                window.hide().unwrap_or_default();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::settings::update_setting,
            commands::system::get_automation_status,
            commands::settings::get_web_port,
            commands::system::get_secret_backend_capabilities,
            commands::system::get_feature_capabilities,
            commands::system::get_runtime_log_snapshot,
            commands::system::record_frontend_log,
            commands::permissions::get_desktop_permission_status,
            commands::permissions::request_desktop_notification_permission,
            commands::permissions::open_desktop_permission_settings,
            commands::system::probe_provider_surface_endpoint,
            commands::system::preview_update,
            commands::settings::get_allowed_setting_keys,
            commands::integration::integration_auth_status,
            commands::integration::integration_start_device_authorization,
            commands::integration::integration_poll_device_authorization,
            commands::integration::integration_cancel_device_authorization,
            commands::integration::integration_reset_auth_state,
            commands::integration::oauth_start_flow,
            commands::integration::oauth_flow_status,
            commands::integration::oauth_cancel_flow,
            commands::integration::oauth_revoke,
            commands::integration::oauth_connection_status,
            commands::ai_session::create_ai_session,
            commands::ai_session::send_session_message,
            commands::ai_session::kill_ai_session,
            commands::ai_session::list_ai_sessions,
            commands::ai_session::retry_ai_session,
            commands::ai_session::get_token_usage,
            commands::ai_session::load_session_messages,
            commands::ai_session::delete_session_history,
            commands::ai_session::rename_ai_session,
            commands::analysis::get_analysis_config,
            commands::analysis::update_analysis_config,
            commands::analysis::get_analysis_status,
            commands::analysis::get_analysis_health,
            commands::analysis::reload_embedding_model,
            commands::dashboard::semantic_search,
            commands::dashboard::get_weekly_digest,
            commands::dashboard::get_dashboard_day,
            commands::dashboard::get_daily_digest,
            commands::dashboard::create_override,
            commands::dashboard::delete_override,
            commands::dashboard::list_overrides,
            commands::dashboard::trigger_recluster,
            commands::coaching::dismiss_coaching_message,
            commands::coaching::submit_coaching_feedback,
            commands::coaching::set_overlay_mode,
            commands::coaching::toggle_overlay_mode,
            commands::coaching::get_overlay_state,
            commands::coaching::toggle_overlay_interactive,
            commands::coaching::toggle_suggestions_panel,
            commands::coaching::toggle_automation_confirm,
            commands::coaching::get_coaching_history,
            commands::coaching::get_goal_progress,
            commands::coaching::update_regime_goals,
            commands::coaching::get_habit_streaks,
            commands::capture_status::get_capture_status,
            commands::capture_status::toggle_capture_pause,
            commands::capture_status::set_indicator_visible,
            commands::capture_status::get_connection_status,
            commands::capture_status::show_main_window,
            commands::capture_status::open_devtools,
            commands::capture_status::save_panel_position,
            commands::capture_status::get_panel_position,
            commands::onboarding::get_onboarding_status,
            commands::onboarding::complete_onboarding,
            commands::onboarding::reset_onboarding,
            commands::focus::toggle_focus_mode,
            commands::focus::get_focus_mode_status,
            commands::capture::trigger_manual_capture,
            commands::capture::analyze_current_scene,
            commands::suggestions::get_pending_suggestions,
            commands::suggestions::get_suggestion_history,
            commands::suggestions::submit_suggestion_feedback,
            commands::suggestions::request_chat_suggestions,
            commands::suggestions::explain_suggestion_in_chat,
            commands::suggestions::save_suggestion_state,
            commands::suggestions::get_suggestion_stats,
            commands::suggestions::get_deferred_suggestions,
            commands::suggestions::get_suggestion_daily_stats,
            commands::sync::get_sync_status,
            commands::sync::trigger_sync_cycle,
            commands::sync::discover_sync_peers,
            commands::sync::set_sync_enabled,
            commands::sync::forget_peer,
            commands::automation::check_automation_available,
            commands::automation::list_automation_presets,
            commands::automation::run_automation_preset,
            commands::automation::execute_automation_hint,
            commands::automation::analyze_automation_scene,
            commands::automation::get_pending_confirmations,
            commands::automation::confirm_automation_command,
            commands::detection::toggle_detection_overlay,
            commands::detection::refresh_detection_overlay,
            commands::audio::start_audio_capture,
            commands::audio::stop_and_transcribe,
            commands::audio::get_audio_status,
            commands::audio::download_whisper_model,
            commands::audio::cancel_model_download,
            commands::audio::delete_whisper_model,
            commands::audio::reload_stt_engine,
            commands::audio::start_vad_listening,
            commands::audio::stop_vad_listening,
            commands::autostart::autostart_capabilities,
            commands::autostart::disable_autostart,
            commands::autostart::enable_autostart,
            commands::autostart::get_autostart_config,
            commands::autostart::is_autostart_enabled,
            commands::autostart::mark_autostart_prompt_state,
            commands::bug_report::export_bug_report,
            commands::error_report::report_frontend_error,
            commands::tracking_schedule::get_tracking_schedule,
            commands::tracking_schedule::set_tracking_schedule,
            commands::tracking_schedule::get_tracking_schedule_status,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Maekon");

    app.run(|app_handle, event| match event {
        RunEvent::Exit => {
            info!("Tauri exit: sending shutdown signal");

            // Persist suggestion queue before shutdown (best-effort).
            if let Some(srs) = app_handle.try_state::<runtime_state::SuggestionRuntimeState>() {
                if let Some(ref mgr) = srs.manager() {
                    let storage = mgr.storage();
                    // Save pending queue items.
                    if let Ok(queue) = mgr.queue().try_lock() {
                        for suggestion in queue.iter() {
                            if let Err(e) = storage.save_suggestion_with_state(suggestion, "pending", None) {
                                warn!(id = %suggestion.suggestion_id, "shutdown: failed to persist suggestion: {e}");
                            }
                        }
                    }
                    // Save deferred items with their resurface time.
                    if let Ok(deferred) = mgr.deferred().try_lock() {
                        for entry in deferred.list_deferred() {
                            let resurface = entry.resurface_at.to_rfc3339();
                            if let Err(e) = storage.save_suggestion_with_state(
                                &entry.suggestion,
                                "deferred",
                                Some(&resurface),
                            ) {
                                warn!(id = %entry.suggestion.suggestion_id, "shutdown: failed to persist deferred suggestion: {e}");
                            }
                        }
                    }
                }
            }

            // A.17: Abort the tray-watch task before the background runtime shuts
            // down, preventing a spurious "config channel closed" warn log on exit.
            if let Some(tray_watch) =
                app_handle.try_state::<crate::tray_watch::TrayWatchHandle>()
            {
                tray_watch.0.abort();
            }

            if let Some(state) = app_handle.try_state::<runtime_state::AppState>() {
                // Terminate all active AI sessions before shutdown.
                if let Some(ai_session_state) =
                    app_handle.try_state::<runtime_state::AiSessionRuntimeState>()
                {
                    if let Ok(handle) = tokio::runtime::Handle::try_current() {
                        handle.block_on(async { ai_session_state.shutdown_all().await });
                    }
                }
                if state.shutdown_tx.send(true).is_err() {
                    warn!("shutdown signal send failed (receivers already dropped)");
                }
                state.background_runtime.shutdown_blocking();

                // Checkpoint WAL BEFORE the regime save so a stalled save
                // cannot hold the shared `Arc<Mutex<Connection>>` and block
                // the checkpoint indefinitely. Note that `save_all` in
                // `SqliteRegimeManagerStateStore` is sync-inside-async
                // (`std::sync::Mutex::lock()` + `conn.execute()` with no
                // `.await`), so the `tokio::time::timeout` wrapping it is
                // advisory — it cannot cancel the in-flight SQL. Running
                // the checkpoint first gives it a guaranteed-unblocked
                // window on the mutex; the save that follows simply writes
                // into the fresh WAL, which is idempotently replayed on
                // next startup if the process is killed mid-write.
                if let Err(e) = state.storage.wal_checkpoint_truncate() {
                    warn!("WAL checkpoint on shutdown failed: {e}");
                }

                // Persist RegimeManager state (best-effort, 4s watchdog).
                //
                // Uses the Phase-2 pattern: offload the save to a dedicated
                // std thread that owns its own tokio runtime + timeout, then
                // join with a wall-clock deadline. Matches
                // `src-tauri/src/telemetry/otlp.rs::shutdown` and avoids
                // deadlocking by calling block_on on the background_runtime
                // handle from the Tauri callback thread when that same
                // runtime may be draining its tasks.
                //
                // The 4s tokio timeout cannot actually preempt the sync
                // SQL `execute` (no `.await` point), so this watchdog
                // bounds the main thread's *wait* rather than the save
                // itself. A genuinely stalled save will outlive the
                // wait — the OS reaps it when the process exits. Data
                // is either fully committed (execute returned) or not
                // at all (SQLite journal rolls back), so there is no
                // torn-write risk. See ADR-018 "Consequences".
                //
                // Both fields are None until Task 13 (composition-root wiring)
                // populates them, making this a runtime no-op in the interim.
                if let (Some(regime_storage), Some(regime_manager)) = (
                    state.regime_storage.clone(),
                    state.regime_manager_snapshot.clone(),
                ) {
                    let regimes = {
                        let guard = regime_manager.lock();
                        guard.all_regimes().to_vec()
                    };
                    let regime_count = regimes.len();
                    let (tx, rx) = std::sync::mpsc::channel();
                    std::thread::spawn(move || {
                        let rt = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .expect("regime-save shutdown runtime");
                        let result = rt.block_on(async move {
                            tokio::time::timeout(
                                std::time::Duration::from_secs(4),
                                regime_storage.save_all(&regimes),
                            )
                            .await
                        });
                        let _ = tx.send(result);
                    });

                    // 4s timeout + 500ms slack for thread scheduling.
                    match rx.recv_timeout(std::time::Duration::from_millis(4500)) {
                        Ok(Ok(Ok(()))) => info!(count = regime_count, "regime state persisted"),
                        Ok(Ok(Err(e))) => {
                            warn!(error = %e, "regime state save failed")
                        }
                        Ok(Err(_timeout)) => {
                            warn!("regime state save exceeded 4s; proceeding with shutdown")
                        }
                        Err(_channel) => {
                            warn!("regime state save thread did not respond within 4.5s; proceeding with shutdown")
                        }
                    }
                }
            }
        }
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => {
            // macOS dock 아이콘 클릭 시 메인 윈도우 표시
            if let Some(w) = app_handle.get_webview_window("main") {
                if let Err(e) = w.show() {
                    debug!("window show failed: {e}");
                }
                if let Err(e) = w.set_focus() {
                    debug!("set_focus failed: {e}");
                }
            }
        }
        _ => {}
    });
}
