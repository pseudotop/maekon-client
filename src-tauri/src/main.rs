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
//! Desktop agent migrated from the iced GUI to Tauri v2.
//! Manages the system tray, WebView dashboard, and IPC commands together.

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
mod breaker_registry;
mod bridge_cli;
mod capture_services;
mod cli_subscription_bridge;
mod codex_approval_policy;
mod commands;
mod desktop_permissions;
mod desktop_startup;
mod ext_grpc_handles;
mod fallback_stt;
mod feature_capabilities;
// E20-24 (#4816): CompositeFeedbackSink (CoachingEngine + RegimeClassifier) is the
// pure-local learning hook for accept/reject — needed by OSS local-suggestion builds,
// so it is gated on `local-suggestions`, not `server`.
#[cfg(feature = "local-suggestions")]
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
#[cfg(feature = "analysis")]
mod provider_runtime_context;
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
mod shortcut_registry;
mod skill_loader;
mod storage_runtime;
mod subprocess_provider;
mod suggestion_manager;
// E20-24 (#4816): no-op ApiClient so the local-suggestion FeedbackSender satisfies
// its required `Arc<dyn ApiClient>` with zero network. See local_api_client.rs.
#[cfg(feature = "local-suggestions")]
mod local_api_client;
mod sync_engine;
mod telemetry;
mod tray;
mod tray_icon;
mod tray_watch;
mod update_coordinator;
mod update_runtime;
mod updater;
mod web_server_runtime;
mod window_state;
#[cfg(debug_assertions)]
mod windows_gui_session_benchmark;
mod workflow_intelligence;

use tauri::{Manager, RunEvent};
#[cfg(target_os = "macos")]
use tracing::debug;
use tracing::{info, warn};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

mod cli;

// Re-export all debug-only symbols from the `cli` module so they can be referenced
// directly in the current crate namespace.
#[cfg(debug_assertions)]
use cli::*;

const OFFLINE_MODE_ENV: &str = "MAEKON_OFFLINE_MODE";
#[cfg(debug_assertions)]
const DEBUG_RUNTIME_SMOKE_ENV: &str = "MAEKON_DEBUG_RUNTIME_SMOKE_CLI";
#[cfg(debug_assertions)]
const DEBUG_RUNTIME_SMOKE_OUTPUT_ENV: &str = "MAEKON_DEBUG_RUNTIME_SMOKE_OUTPUT";

fn configure_runtime_flavor() {
    #[cfg(debug_assertions)]
    {
        // Keep local debug clients from opening the release install's data directory.
        if std::env::var_os("MAEKON_APP_FLAVOR").is_none() {
            std::env::set_var("MAEKON_APP_FLAVOR", "dev");
        }
    }
}

pub(crate) fn offline_mode_enabled() -> bool {
    let env_value = std::env::var(OFFLINE_MODE_ENV).ok();
    offline_mode_enabled_from(std::env::args().skip(1), env_value.as_deref())
}

fn offline_mode_enabled_from<I, S>(args: I, env_value: Option<&str>) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    env_value.map(str::trim).is_some_and(|value| {
        matches!(value, "1")
            || value.eq_ignore_ascii_case("true")
            || value.eq_ignore_ascii_case("yes")
            || value.eq_ignore_ascii_case("on")
    }) || args.into_iter().any(|arg| {
        let arg = arg.as_ref();
        arg == "--offline" || arg == "--offline=true"
    })
}

fn app_help_requested_from<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    matches!(
        args.into_iter().next().as_ref().map(AsRef::as_ref),
        Some("--help" | "-h")
    )
}

fn app_help_text() -> &'static str {
    concat!(
        "Maekon desktop agent\n\n",
        "Usage: maekon [OPTIONS] [COMMAND]\n\n",
        "Options:\n",
        "  -h, --help       Print help and exit\n",
        "  -V, --version    Print version and exit\n",
        "      --offline    Start the GUI in local/offline mode\n\n",
        "Commands:\n",
        "  auth                       Manage local authentication state\n",
        "  secret                     Manage local secret bindings\n",
        "  bridge                     Run local bridge utilities\n",
        "  debug-runtime-smoke        Debug builds only; requires ",
        "MAEKON_DEBUG_RUNTIME_SMOKE_CLI=1 and exits with JSON runtime evidence\n"
    )
}

const SUGGESTION_SHUTDOWN_PERSIST_TIMEOUT: std::time::Duration =
    std::time::Duration::from_millis(250);
const SUGGESTION_SHUTDOWN_LOCK_RETRY: std::time::Duration = std::time::Duration::from_millis(10);

#[derive(Default)]
struct SuggestionShutdownPersistSummary {
    pending_saved: usize,
    pending_failed: usize,
    deferred_saved: usize,
    deferred_failed: usize,
    pending_lock_skipped: bool,
    deferred_lock_skipped: bool,
}

fn try_lock_until<T>(
    mutex: &tokio::sync::Mutex<T>,
    deadline: std::time::Instant,
) -> Option<tokio::sync::MutexGuard<'_, T>> {
    loop {
        match mutex.try_lock() {
            Ok(guard) => return Some(guard),
            Err(_) if std::time::Instant::now() >= deadline => return None,
            Err(_) => std::thread::sleep(SUGGESTION_SHUTDOWN_LOCK_RETRY),
        }
    }
}

fn persist_suggestions_on_shutdown(
    mgr: &crate::suggestion_manager::SuggestionManager,
) -> SuggestionShutdownPersistSummary {
    let deadline = std::time::Instant::now() + SUGGESTION_SHUTDOWN_PERSIST_TIMEOUT;
    let storage = mgr.storage().clone();
    let mut summary = SuggestionShutdownPersistSummary::default();

    let pending = match try_lock_until(mgr.queue(), deadline) {
        Some(queue) => queue.iter().cloned().collect::<Vec<_>>(),
        None => {
            summary.pending_lock_skipped = true;
            Vec::new()
        }
    };
    for suggestion in pending {
        if let Err(e) = storage.save_suggestion_with_state(&suggestion, "pending", None) {
            summary.pending_failed += 1;
            warn!(id = %suggestion.suggestion_id, "shutdown: failed to persist suggestion: {e}");
        } else {
            summary.pending_saved += 1;
        }
    }

    let deferred = match try_lock_until(mgr.deferred(), deadline) {
        Some(deferred) => deferred
            .list_deferred()
            .into_iter()
            .map(|entry| (entry.suggestion.clone(), entry.resurface_at.to_rfc3339()))
            .collect::<Vec<_>>(),
        None => {
            summary.deferred_lock_skipped = true;
            Vec::new()
        }
    };
    for (suggestion, resurface) in deferred {
        if let Err(e) =
            storage.save_suggestion_with_state(&suggestion, "deferred", Some(&resurface))
        {
            summary.deferred_failed += 1;
            warn!(id = %suggestion.suggestion_id, "shutdown: failed to persist deferred suggestion: {e}");
        } else {
            summary.deferred_saved += 1;
        }
    }

    summary
}

#[cfg(debug_assertions)]
fn debug_runtime_smoke_cli_requested_from<I, S>(args: I, env_value: Option<&str>) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let gate_enabled = env_value.map(str::trim).is_some_and(|value| {
        matches!(value, "1")
            || value.eq_ignore_ascii_case("true")
            || value.eq_ignore_ascii_case("yes")
            || value.eq_ignore_ascii_case("on")
    });
    if !gate_enabled {
        return false;
    }

    matches!(
        args.into_iter().next().as_ref().map(AsRef::as_ref),
        Some("debug-runtime-smoke")
    )
}

#[cfg(debug_assertions)]
fn consent_permissions_any_enabled(permissions: &maekon_core::consent::ConsentPermissions) -> bool {
    permissions.screen_capture
        || permissions.ocr_processing
        || permissions.telemetry
        || permissions.process_monitoring
        || permissions.input_activity
        || permissions.window_title_collection
        || permissions.app_usage_analytics
        || permissions.clipboard_monitoring
        || permissions.file_access_monitoring
        || permissions.activity_pattern_learning
        || permissions.cross_device_sync
        || permissions.full_text_extraction
        || permissions.memory_graph_enrichment
        || permissions.microphone
        || permissions.unredacted_external_ocr
}

#[cfg(debug_assertions)]
fn debug_runtime_smoke_tcp_reachable(port: u16) -> bool {
    if port == 0 {
        return false;
    }

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(500)).is_ok()
}

#[cfg(debug_assertions)]
fn debug_runtime_smoke_http_get_ok(port: u16, path: &str, local_auth_token: Option<&str>) -> bool {
    for _ in 0..10 {
        if debug_runtime_smoke_http_get_once_ok(port, path, local_auth_token) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    false
}

#[cfg(debug_assertions)]
fn debug_runtime_smoke_http_get_once_ok(
    port: u16,
    path: &str,
    local_auth_token: Option<&str>,
) -> bool {
    use std::io::{Read, Write};

    if port == 0 {
        return false;
    }

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut stream) =
        std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(500))
    else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(2)));

    let request = debug_runtime_smoke_http_request(port, path, local_auth_token);
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }

    let mut response = [0_u8; 64];
    match stream.read(&mut response) {
        Ok(0) | Err(_) => false,
        Ok(n) => std::str::from_utf8(&response[..n])
            .map(|head| head.starts_with("HTTP/1.1 200") || head.starts_with("HTTP/1.0 200"))
            .unwrap_or(false),
    }
}

#[cfg(debug_assertions)]
fn debug_runtime_smoke_http_request(
    port: u16,
    path: &str,
    local_auth_token: Option<&str>,
) -> String {
    let auth_header = local_auth_token
        .filter(|token| !token.is_empty())
        .map(|token| format!("x-local-auth: {token}\r\n"))
        .unwrap_or_default();

    format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{auth_header}Connection: close\r\n\r\n"
    )
}

#[cfg(debug_assertions)]
fn run_debug_runtime_smoke_cli_command(app_handle: &tauri::AppHandle) -> i32 {
    use maekon_core::consent::{ConsentPermissions, ConsentStatus};
    use std::sync::atomic::Ordering;

    let app_flavor = std::env::var("MAEKON_APP_FLAVOR").unwrap_or_default();
    let (window_exists, window_visible, window_width, window_height) =
        if let Some(window) = app_handle.get_webview_window("main") {
            let visible = window.is_visible().unwrap_or(false);
            let size = window.inner_size().ok();
            (
                true,
                visible,
                size.map(|s| s.width).unwrap_or_default(),
                size.map(|s| s.height).unwrap_or_default(),
            )
        } else {
            (false, false, 0, 0)
        };

    let (web_port, automation_enabled, sandbox_enabled, sandbox_profile) = app_handle
        .try_state::<runtime_state::ConfigRuntimeState>()
        .map(|state| {
            let config = state.config_manager().get();
            (
                state.web_port(),
                config.automation.enabled,
                config.automation.sandbox.enabled,
                format!("{:?}", config.automation.sandbox.profile),
            )
        })
        .unwrap_or_else(|| (0, true, true, "Unavailable".to_string()));

    let local_auth_token = app_handle
        .try_state::<runtime_state::LocalAuthTokenState>()
        .map(|state| state.0.clone());

    let (
        consent_status,
        consent_permissions,
        effective_permissions,
        capture_paused,
        data_dir_resolved,
        app_flavor_isolated,
    ) = app_handle
        .try_state::<runtime_state::AppState>()
        .and_then(|state| {
            let manager = state.capture.consent_manager.as_ref()?;
            let (status, permissions) = manager.status_and_permissions();
            let data_dir = maekon_core::config_manager::ConfigManager::data_dir().ok();
            let app_flavor_isolated = data_dir.as_ref().is_some_and(|path| {
                !app_flavor.is_empty() && path.display().to_string().contains(&app_flavor)
            });
            Some((
                status,
                permissions,
                manager.effective_permissions(),
                state.capture_paused.load(Ordering::Relaxed),
                data_dir.is_some(),
                app_flavor_isolated,
            ))
        })
        .unwrap_or_else(|| {
            (
                ConsentStatus::NotGranted,
                ConsentPermissions::default(),
                ConsentPermissions::default(),
                false,
                false,
                false,
            )
        });

    let web_port_reachable = debug_runtime_smoke_tcp_reachable(web_port);
    let settings_endpoint_ok =
        debug_runtime_smoke_http_get_ok(web_port, "/api/settings", local_auth_token.as_deref());
    let raw_consent_any_enabled = consent_permissions_any_enabled(&consent_permissions);
    let effective_consent_any_enabled = consent_permissions_any_enabled(&effective_permissions);
    let default_consent_closed = consent_status == ConsentStatus::NotGranted
        && !raw_consent_any_enabled
        && !effective_consent_any_enabled;
    let ok = window_exists
        && window_visible
        && web_port_reachable
        && settings_endpoint_ok
        && data_dir_resolved
        && app_flavor_isolated
        && !automation_enabled
        && !sandbox_enabled
        && default_consent_closed;

    let payload = serde_json::json!({
        "ok": ok,
        "command": "debug-runtime-smoke",
        "processId": std::process::id(),
        "appFlavor": app_flavor,
        "offlineMode": offline_mode_enabled(),
        "window": {
            "exists": window_exists,
            "visible": window_visible,
            "innerWidth": window_width,
            "innerHeight": window_height,
        },
        "web": {
            "port": web_port,
            "portReachable": web_port_reachable,
            "settingsEndpointOk": settings_endpoint_ok,
        },
        "automation": {
            "enabled": automation_enabled,
            "sandboxEnabled": sandbox_enabled,
            "sandboxProfile": sandbox_profile,
        },
        "consent": {
            "status": consent_status,
            "rawPermissions": consent_permissions,
            "effectivePermissions": effective_permissions,
            "defaultClosed": default_consent_closed,
        },
        "privacy": {
            "capturePaused": capture_paused,
            "collectsScreenContent": false,
            "collectsUserInput": false,
            "usesBroadDesktopScreenshot": false,
        },
        "paths": {
            "dataDirResolved": data_dir_resolved,
            "appFlavorIsolated": app_flavor_isolated,
            "outputEnv": DEBUG_RUNTIME_SMOKE_OUTPUT_ENV,
        },
        "shutdownMethod": "app_handle.exit",
    });

    let serialized = serde_json::to_string_pretty(&payload).unwrap_or_else(|error| {
        format!(
            "{{\"ok\":false,\"command\":\"debug-runtime-smoke\",\"error\":\"serialize failed: {error}\"}}"
        )
    });

    if let Some(output_path) = std::env::var_os(DEBUG_RUNTIME_SMOKE_OUTPUT_ENV) {
        if let Some(parent) = std::path::Path::new(&output_path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(error) = std::fs::write(&output_path, &serialized) {
            eprintln!("failed to write runtime smoke output: {error}");
            println!("{serialized}");
            return 1;
        }
    } else {
        println!("{serialized}");
    }

    if ok {
        0
    } else {
        1
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

    // `--version` / `-V` CLI handler. Build date and git SHA are embedded by
    // build.rs at compile time, so this exits before starting the webview.
    {
        let args: Vec<String> = std::env::args().collect();
        if args.iter().skip(1).any(|a| a == "--version" || a == "-V") {
            let info = crate::commands::build_info::AppBuildInfo::current();
            println!(
                "maekon {} (build: {} | commit: {})",
                info.version, info.build_date, info.git_sha
            );
            std::process::exit(0);
        }
        if app_help_requested_from(args.iter().skip(1).map(String::as_str)) {
            println!("{}", app_help_text());
            std::process::exit(0);
        }
    }

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

    #[cfg(debug_assertions)]
    {
        let args: Vec<String> = std::env::args().collect();
        let debug_permissions_gate = std::env::var("MAEKON_DEBUG_DESKTOP_PERMISSION_CLI").ok();
        if let Some(command) = debug_permissions_cli_command_from(
            args.iter().skip(1).map(String::as_str),
            debug_permissions_gate.as_deref(),
        ) {
            std::process::exit(run_debug_permissions_cli_command(command));
        }

        let debug_ax_tree_gate = std::env::var("MAEKON_DEBUG_AX_TREE_CLI").ok();
        if let Some(command) = debug_ax_tree_cli_command_from(
            args.iter().skip(1).map(String::as_str),
            debug_ax_tree_gate.as_deref(),
        ) {
            std::process::exit(run_debug_ax_tree_cli_command(command));
        }

        let debug_power_gate = std::env::var("MAEKON_DEBUG_POWER_CLI").ok();
        if let Some(command) = debug_power_cli_command_from(
            args.iter().skip(1).map(String::as_str),
            debug_power_gate.as_deref(),
        ) {
            std::process::exit(run_debug_power_cli_command(command));
        }

        let debug_pointer_capture_gate = std::env::var("MAEKON_DEBUG_POINTER_CAPTURE_CLI").ok();
        if let Some(command) = debug_pointer_capture_cli_command_from(
            args.iter().skip(1).map(String::as_str),
            debug_pointer_capture_gate.as_deref(),
        ) {
            std::process::exit(run_debug_pointer_capture_cli_command(command));
        }
    }

    // Windows DLL search order hardening (Spec Section 9.2):
    // Remove CWD from DLL search path to prevent DLL hijacking.
    #[cfg(target_os = "windows")]
    unsafe {
        windows_sys::Win32::System::LibraryLoader::SetDllDirectoryW(windows_sys::core::w!(""));
    }

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("maekon=info,maekon_app=info,maekon_core=info,maekon_monitor=info,maekon_vision=info,maekon_storage=info,maekon_network=info,maekon_suggestion=info")
    });

    // Console layer — writes to stderr (same as previous fmt() subscriber).
    let console_layer = tracing_subscriber::fmt::layer().with_ansi(true);

    // File layer — daily rolling log files in {data_dir}/logs/.
    // WorkerGuard MUST outlive the subscriber; we store it in Tauri state.
    let log_dir = maekon_core::config_manager::ConfigManager::data_dir()
        .map(|d| d.join("logs"))
        .unwrap_or_else(|_| std::path::PathBuf::from("logs"));

    std::fs::create_dir_all(&log_dir).ok();

    // Cleanup old log files before creating new appender
    let deleted = log_retention::cleanup_old_logs(&log_dir, log_retention::DEFAULT_MAX_AGE_DAYS);
    if deleted > 0 {
        // Cannot use tracing yet — subscriber not initialized.
        eprintln!("[maekon] startup log cleanup: deleted {deleted} old log file(s)");
    }

    let file_appender = tracing_appender::rolling::daily(&log_dir, "maekon.log");
    let (non_blocking, worker_guard) = tracing_appender::non_blocking(file_appender);

    let file_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(non_blocking);

    // Telemetry layer + handle. ConfigManager does not exist yet at this
    // point (it is built during bootstrap below), so we seed with an explicit
    // disabled config — no exporter is built before the consent gate runs.
    // The bus-driven reconcile task spawned in bootstrap_runtime.rs picks up
    // the user's real setting on its first iteration and applies it.
    let telemetry_data_dir = maekon_core::config_manager::ConfigManager::data_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."));
    std::fs::create_dir_all(&telemetry_data_dir).ok();
    let disabled_telemetry = maekon_core::config::TelemetryConfig::disabled();
    let (telemetry_layer, telemetry_handle) =
        telemetry::Handle::new_with_layer(&disabled_telemetry, &telemetry_data_dir)
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
    #[cfg(debug_assertions)]
    {
        let debug_autostart_gate = std::env::var("MAEKON_DEBUG_AUTOSTART_CLI").ok();
        if let Some(command) = debug_autostart_cli_command_from(
            args.iter().skip(1).map(String::as_str),
            debug_autostart_gate.as_deref(),
        ) {
            std::process::exit(run_debug_autostart_cli_command(command));
        }
    }
    if args.len() > 1 && args[1] == "auth" {
        let config_dir = maekon_core::config_manager::ConfigManager::config_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."));
        let exit_code = auth_cli::run(&args[2..], &config_dir);
        std::process::exit(exit_code);
    }
    if args.len() > 1 && args[1] == "secret" {
        let config_dir = maekon_core::config_manager::ConfigManager::config_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."));
        let exit_code = secret_cli::run(&args[2..], &config_dir);
        std::process::exit(exit_code);
    }
    if args.len() > 1 && args[1] == "bridge" {
        let data_dir = maekon_core::config_manager::ConfigManager::data_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."));
        let exit_code = bridge_cli::run(&args[2..], &data_dir);
        std::process::exit(exit_code);
    }

    #[cfg(debug_assertions)]
    let debug_notification_command = {
        let debug_notification_gate = std::env::var("MAEKON_DEBUG_NOTIFICATION_CLI").ok();
        debug_notification_cli_command_from(
            args.iter().skip(1).map(String::as_str),
            debug_notification_gate.as_deref(),
        )
    };

    #[cfg(debug_assertions)]
    let debug_permissions_runtime_command = {
        let debug_permissions_gate = std::env::var("MAEKON_DEBUG_DESKTOP_PERMISSION_CLI").ok();
        debug_permissions_runtime_cli_command_from(
            args.iter().skip(1).map(String::as_str),
            debug_permissions_gate.as_deref(),
        )
    };

    #[cfg(debug_assertions)]
    let debug_pointer_capture_runtime_command = {
        let debug_pointer_capture_gate = std::env::var("MAEKON_DEBUG_POINTER_CAPTURE_CLI").ok();
        debug_pointer_capture_runtime_cli_command_from(
            args.iter().skip(1).map(String::as_str),
            debug_pointer_capture_gate.as_deref(),
        )
    };

    #[cfg(debug_assertions)]
    let debug_runtime_smoke_requested = {
        let debug_runtime_smoke_gate = std::env::var(DEBUG_RUNTIME_SMOKE_ENV).ok();
        debug_runtime_smoke_cli_requested_from(
            args.iter().skip(1).map(String::as_str),
            debug_runtime_smoke_gate.as_deref(),
        )
    };

    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default();

    #[cfg(debug_assertions)]
    let enable_single_instance = should_enable_single_instance_for_debug_runtime(
        debug_notification_command,
        debug_permissions_runtime_command,
    ) && !debug_runtime_smoke_requested
        && debug_pointer_capture_runtime_command.is_none();
    #[cfg(not(debug_assertions))]
    let enable_single_instance = true;

    if enable_single_instance {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
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
        }));
    }

    #[allow(unused_mut)]
    let mut builder = builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(LogWorkerGuard(worker_guard))
        .manage(telemetry_handle)
        // OOS-TBD-N15-UI-EXPOSURE (2026-05-05): TokenManagerState used by the
        // logout_all_sessions Tauri command. Registered ONCE here as an empty
        // interior-mutable slot; app_runtime_launch populates it via
        // `TokenManagerState::set(..)` after server bootstrap creates the token
        // manager. A second `manage(..)` would be a silent no-op (Tauri does not
        // overwrite an already-managed type), so the slot is the populate path.
        // Until populated (or for disabled features / bootstrap failures) the
        // slot stays empty and the command fails immediately.
        .manage(commands::auth::TokenManagerState::empty());

    // WebDriver server plugin — for E2E tests (MUST never be included in production builds)
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
        .setup(|app| {
            #[cfg(all(debug_assertions, target_os = "macos"))]
            install_debug_macos_notification_delegate_from_env();
            setup::init(app)
        })
        .on_window_event(|window, event| {
            // Close-to-tray: hide the window on close (not an actual exit).
            match event {
                tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_) => {
                    crate::window_state::ensure_main_window_on_available_monitor(window);
                    crate::window_state::persist_main_window_state(window);
                }
                tauri::WindowEvent::CloseRequested { api, .. } => {
                    crate::window_state::persist_main_window_state(window);
                    window.hide().unwrap_or_default();
                    api.prevent_close();
                }
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::build_info::get_app_build_info,
            commands::auth::logout_all_sessions,
            commands::settings::update_setting,
            commands::system::get_automation_status,
            commands::settings::get_web_port,
            commands::settings::get_local_auth_token,
            commands::system::get_secret_backend_capabilities,
            commands::system::get_feature_capabilities,
            commands::system::get_runtime_log_snapshot,
            commands::system::record_frontend_log,
            commands::permissions::get_desktop_permission_status,
            commands::permissions::request_desktop_notification_permission,
            commands::permissions::request_desktop_screen_capture_permission,
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
            commands::notification::send_test_notification,
            commands::notification::simulate_notification_activation,
            commands::ai_session::create_ai_session,
            commands::ai_session::send_session_message,
            commands::ai_session::kill_ai_session,
            commands::ai_session::list_ai_sessions,
            commands::ai_session::retry_ai_session,
            commands::ai_session::get_token_usage,
            commands::ai_session::load_session_messages,
            commands::ai_session::delete_session_history,
            commands::ai_session::rename_ai_session,
            commands::ai_session::interrupt_session_turn,
            commands::ai_session::steer_session_turn,
            commands::ai_session::respond_codex_approval,
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
            commands::coaching::get_overlay_fullscreen_policy_state,
            commands::coaching::toggle_suggestions_panel,
            commands::coaching::toggle_automation_confirm,
            commands::coaching::get_coaching_history,
            commands::coaching::get_goal_progress,
            commands::coaching::update_regime_goals,
            commands::coaching::get_habit_streaks,
            commands::shortcuts::get_global_shortcut_status,
            commands::capture_status::get_capture_status,
            commands::capture_status::toggle_capture_pause,
            commands::capture_status::set_indicator_visible,
            commands::capture_status::get_connection_status,
            commands::capture_status::show_main_window,
            commands::capture_status::debug_focus_window,
            commands::capture_status::debug_window_state,
            commands::capture_status::debug_set_window_bounds,
            commands::capture_status::debug_place_overlay_for_window,
            commands::capture_status::debug_normalize_main_window_state,
            commands::capture_status::debug_normalize_main_window_bounds,
            commands::capture_status::debug_set_window_fullscreen,
            commands::capture_status::open_devtools,
            commands::capture_status::save_panel_position,
            commands::capture_status::get_panel_position,
            commands::onboarding::get_onboarding_status,
            commands::onboarding::complete_onboarding,
            commands::onboarding::reset_onboarding,
            commands::focus::toggle_focus_mode,
            commands::focus::get_focus_mode_status,
            commands::capture::trigger_manual_capture,
            commands::capture::extract_ax_tree,
            commands::capture::start_ax_focus_observer,
            commands::capture::poll_ax_focus_observer,
            commands::capture::stop_ax_focus_observer,
            commands::capture::analyze_current_scene,
            commands::suggestions::queries::get_pending_suggestions,
            commands::suggestions::queries::get_suggestion_history,
            commands::suggestions::feedback::submit_suggestion_feedback,
            commands::suggestions::replay::record_suggestion_replay_event,
            commands::suggestions::chat_suggestions::request_chat_suggestions,
            commands::suggestions::chat_suggestions::explain_suggestion_in_chat,
            commands::suggestions::queries::save_suggestion_state,
            commands::suggestions::queries::get_suggestion_stats,
            commands::suggestions::queries::get_deferred_suggestions,
            commands::suggestions::queries::get_suggestion_daily_stats,
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
            commands::audit::export_audit_log,
            commands::audit::verify_audit_log,
            commands::error_report::report_frontend_error,
            commands::tracking_schedule::get_tracking_schedule,
            commands::tracking_schedule::set_tracking_schedule,
            commands::tracking_schedule::get_tracking_schedule_status,
            commands::tray::get_tray_state,
            commands::tray::get_tray_geometry,
            commands::tray::simulate_tray_action,
            commands::consent::get_consent,
            commands::consent::set_consent,
            commands::consent::withdraw_consent,
            commands::consent::take_microphone_upgrade_notice,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Maekon");

    app.run(move |app_handle, event| match event {
        #[cfg(debug_assertions)]
        RunEvent::Ready if debug_notification_command.is_some() => {
            let command = debug_notification_command.expect("notification debug command exists");
            let app_handle = app_handle.clone();
            std::thread::spawn(move || {
                let exit_code = run_debug_notification_cli_command(&app_handle, command);
                app_handle.exit(exit_code);
            });
        }
        #[cfg(debug_assertions)]
        RunEvent::Ready if debug_permissions_runtime_command.is_some() => {
            let command =
                debug_permissions_runtime_command.expect("permission debug command exists");
            let app_handle = app_handle.clone();
            std::thread::spawn(move || {
                let exit_code = run_debug_permissions_runtime_cli_command(&app_handle, command);
                app_handle.exit(exit_code);
            });
        }
        #[cfg(debug_assertions)]
        RunEvent::Ready if debug_pointer_capture_runtime_command.is_some() => {
            let command = debug_pointer_capture_runtime_command
                .expect("pointer capture debug command exists");
            let app_handle = app_handle.clone();
            std::thread::spawn(move || {
                let exit_code =
                    run_debug_pointer_capture_runtime_cli_command(&app_handle, command);
                app_handle.exit(exit_code);
            });
        }
        #[cfg(debug_assertions)]
        RunEvent::Ready if debug_runtime_smoke_requested => {
            let app_handle = app_handle.clone();
            std::thread::spawn(move || {
                let exit_code = run_debug_runtime_smoke_cli_command(&app_handle);
                app_handle.exit(exit_code);
            });
        }
        RunEvent::Exit => {
            info!("Tauri exit: sending shutdown signal");

            // Persist suggestion queue before shutdown (best-effort).
            if let Some(srs) = app_handle.try_state::<runtime_state::SuggestionRuntimeState>() {
                if let Some(ref mgr) = srs.manager() {
                    let summary = persist_suggestions_on_shutdown(mgr);
                    if summary.pending_lock_skipped {
                        warn!("shutdown: skipped pending suggestion persistence because queue lock stayed busy");
                    }
                    if summary.deferred_lock_skipped {
                        warn!("shutdown: skipped deferred suggestion persistence because queue lock stayed busy");
                    }
                    info!(
                        pending_saved = summary.pending_saved,
                        pending_failed = summary.pending_failed,
                        deferred_saved = summary.deferred_saved,
                        deferred_failed = summary.deferred_failed,
                        pending_lock_skipped = summary.pending_lock_skipped,
                        deferred_lock_skipped = summary.deferred_lock_skipped,
                        "shutdown suggestion persistence completed"
                    );
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
                //
                // #4345: the `RunEvent::Exit` callback runs on the synchronous
                // Tauri main thread, so `tokio::runtime::Handle::try_current()`
                // always returned `Err` — there is no entered tokio runtime on
                // the main thread. As a result the `shutdown_all()` cleanup was
                // dead code that never ran.
                //
                // Instead we `block_on` from a separate multi-thread background
                // runtime handle. At this point we are still before the
                // `shutdown_blocking()` call below, so the background runtime is
                // still alive and its worker threads drive the await/spawn calls
                // inside `shutdown_all`. Calling `block_on` on a *separate*
                // runtime handle from the main thread is valid (no
                // `block_in_place`/`Handle::current` panic risk).
                if let Some(ai_session_state) =
                    app_handle.try_state::<runtime_state::AiSessionRuntimeState>()
                {
                    state
                        .background_runtime
                        .handle()
                        .block_on(async { ai_session_state.shutdown_all().await });
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
            #[cfg(debug_assertions)]
            if matches!(debug_notification_command, Some(DebugNotificationCliCommand::Send)) {
                debug_macos_notification_delegate::record_reopen_activation();
            }

            // Show the main window when the macOS dock icon is clicked.
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

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    #[test]
    fn app_help_requested_from_recognizes_exit_flags() {
        assert!(crate::app_help_requested_from(["--help"]));
        assert!(crate::app_help_requested_from(["-h"]));
        assert!(!crate::app_help_requested_from(["--offline"]));
        assert!(!crate::app_help_requested_from(["auth", "--help"]));
        assert!(!crate::app_help_requested_from(["debug-runtime-smoke"]));
    }

    #[test]
    fn app_help_text_mentions_debug_runtime_smoke() {
        let help = crate::app_help_text();

        assert!(help.contains("Usage: maekon"));
        assert!(help.contains("--version"));
        assert!(help.contains("debug-runtime-smoke"));
        assert!(help.contains("MAEKON_DEBUG_RUNTIME_SMOKE_CLI"));
    }

    #[test]
    fn try_lock_until_is_bounded_when_mutex_is_busy() {
        let mutex = tokio::sync::Mutex::new(1);
        let guard = mutex.try_lock().expect("test mutex must lock");

        let result = crate::try_lock_until(&mutex, Instant::now() + Duration::from_millis(25));

        assert!(result.is_none());
        drop(guard);
        assert!(crate::try_lock_until(&mutex, Instant::now()).is_some());
    }

    #[cfg(debug_assertions)]
    #[test]
    fn debug_runtime_smoke_http_request_sends_local_auth_header() {
        let request =
            crate::debug_runtime_smoke_http_request(10090, "/api/settings", Some("token-123"));

        assert!(request.starts_with("GET /api/settings HTTP/1.1\r\n"));
        assert!(request.contains("Host: 127.0.0.1:10090\r\n"));
        assert!(request.contains("x-local-auth: token-123\r\n"));
        assert!(!request.contains("local_auth=token-123"));
    }

    #[cfg(debug_assertions)]
    #[test]
    fn debug_runtime_smoke_http_request_omits_empty_local_auth_header() {
        let request = crate::debug_runtime_smoke_http_request(10090, "/api/settings", Some(""));

        assert!(!request.contains("x-local-auth:"));
        assert!(request.ends_with("Connection: close\r\n\r\n"));
    }
}
