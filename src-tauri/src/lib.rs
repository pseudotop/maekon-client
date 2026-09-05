#![allow(unexpected_cfgs)]
// Cast safety: UI metrics, scheduler counters, coordinates — precision loss acceptable.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
// P2 nursery-hardening (PR-B): derive Eq alongside PartialEq when possible.
// (Enforced workspace-wide via `[workspace.lints.clippy]`, #7719.)
// H1 (#7719): `significant_drop_tightening` is now enforced workspace-wide too
// (this binary crate never declared it before). Re-measured with the full
// `--workspace --all-targets` build (a partial/aborted-early build under-
// counted this on the first pass): 52 flagged sites across 21 production
// files (notification_manager, update_coordinator/*, magic_overlay,
// session_manager/*, session_adapters/subprocess_session, scheduler loops,
// focus_analyzer/*, commands/*, skill_loader, app_runtime_launch) plus ~107
// more inside `#[cfg(test)] mod tests` blocks. This binary is the DI/
// orchestration root wiring every `Arc<Mutex/RwLock<...>>`-backed adapter
// together — the dominant pattern is "acquire guard, read/mutate in-memory
// state, drop at scope end", where the nursery lint's "tighten the drop
// point" rewrite either produces invalid code (guard borrowed across an
// early return) or trades one atomicity guarantee for a micro-optimization
// that is negligible for in-memory, non-blocking critical sections — the
// same false-positive profile documented in maekon-analysis/-automation/
// -network/-storage's crate-wide allow for this lint. Accepted crate-wide;
// narrow, more surprising cases keep the site-level rationale they already
// had before #7719.
#![allow(clippy::significant_drop_tightening)]

//! Maekon Desktop Agent — application library.
//!
//! Internal application library backing the `maekon` binary (Tauri v2
//! desktop shell). This crate exists so the thin composition-root binary
//! and this crate's own integration test suite have a compilable target to
//! link against; it carries NO external stability contract. Module and item
//! visibility here follow internal composition-root and test-suite needs,
//! not semantic-versioning discipline — nothing in this crate should be
//! treated as a published API.
//!
//! Desktop agent migrated from the iced GUI to Tauri v2.
//! Manages the system tray, WebView dashboard, and IPC commands together.

pub mod agent_runtime;
pub mod agent_runtime_support;
pub mod ai_readiness;
pub mod app_runtime_launch;
pub mod app_runtime_launch_health_probe;
pub mod audit_query;
pub mod auditing_session;
pub mod auth_cli;
pub mod automation_controller_builder;
pub mod automation_runtime;
pub mod autostart;
pub mod background_runtime;
pub mod bootstrap_preflight;
pub mod bootstrap_runtime;
pub mod breaker_registry;
pub mod bridge_cli;
/// #9659: build-capability markers embedded in the binary so an operator can
/// tell a login-capable artifact from a login-less one without launching it.
pub mod build_capabilities;
pub mod capture_scale;
pub mod capture_services;
pub mod cli_subscription_bridge;
pub mod codex_approval_policy;
pub mod commands;
pub mod desktop_permissions;
pub mod desktop_startup;
pub mod fallback_stt;
pub mod feature_capabilities;
// #7916: GUI HITL ticket HMAC secret auto-provisioning (keychain, env override).
pub mod gui_ticket_secret;
// E20-24 (#4816): CompositeFeedbackSink (CoachingEngine + RegimeClassifier) is the
// pure-local learning hook for accept/reject — needed by OSS local-suggestion builds,
// so it is gated on `local-suggestions`, not `server`.
#[cfg(feature = "local-suggestions")]
pub mod feedback_sink;
pub mod focus_auto;
pub mod focus_mode;
pub mod focus_probe_adapter;
pub mod inflight_registry;
#[cfg(feature = "server")]
pub mod integration_insight_source;
pub mod integration_policy;
#[cfg(feature = "server")]
pub mod integration_prompt_delivery;
#[cfg(feature = "server")]
pub mod integration_runtime;
pub mod integrity_guard;
pub mod ipc_error;
pub mod launch_resources;
pub mod lifecycle;
pub mod local_analysis_status;
pub mod log_retention;
#[cfg(target_os = "macos")]
pub mod macos_integration;
pub mod magic_overlay;
pub mod magic_overlay_driver;
pub mod memory_profiler;
pub mod notification_manager;
pub mod oauth_provider_registry;
pub mod platform_accessibility;
pub mod platform_overlay;
pub mod provider_adapters;
#[cfg(feature = "analysis")]
pub mod provider_runtime_context;
pub mod provider_secret_backend;
#[cfg(all(debug_assertions, feature = "audio"))]
pub(crate) mod qc_audio_fixture;
#[cfg(debug_assertions)]
pub(crate) mod qc_fixture_cli;
#[cfg(debug_assertions)]
mod qc_sync_peer;
#[cfg(debug_assertions)]
// QC upload spool needs maekon-network's BatchUploader — only present when the
// `analysis` feature (default-on) links maekon-network (#8685 no-default build).
#[cfg(feature = "analysis")]
mod qc_upload_spool;
pub mod reauth;
pub mod runtime_bridges;
pub mod runtime_state;
pub mod scheduler;
pub mod secret_cli;
#[cfg(feature = "server")]
pub mod server_runtime_context;
pub mod services;
pub mod session_adapters;
pub mod session_context;
pub mod session_manager;
pub mod setup;
pub mod shortcut_registry;
pub mod skill_loader;
pub mod skill_pack_resolver;
mod startup_logging;
// Cfg-free formatting for a startup failure the user can act on (#10985), so
// the message is unit-tested here rather than only observable by crashing.
pub mod startup_failure;
pub mod storage_runtime;
pub mod subprocess_provider;
pub mod suggestion_manager;
pub mod windows_notification_activation;
// E20-24 (#4816): no-op ApiClient so the local-suggestion FeedbackSender satisfies
// its required `Arc<dyn ApiClient>` with zero network. See local_api_client.rs.
#[cfg(feature = "local-suggestions")]
pub mod local_api_client;
pub mod telemetry;
pub mod tray;
pub mod tray_icon;
pub mod tray_watch;
pub mod update_coordinator;
pub mod update_runtime;
pub mod updater;
pub(crate) mod vault_wiring;
pub mod web_server_runtime;
pub mod window_state;
#[cfg(debug_assertions)]
pub mod windows_gui_session_benchmark;

pub mod ext_grpc_handles {
    #[cfg(feature = "grpc-dashboard-external")]
    pub(crate) use crate::setup::ext_grpc_handles::*;
}

pub mod setup_platform {
    pub(crate) use crate::setup::platform::*;
}

pub mod setup_shortcuts {
    pub(crate) use crate::setup::shortcuts::*;
}

pub mod setup_windows {
    pub(crate) use crate::setup::windows::*;
}

use tauri::{Manager, RunEvent};
#[cfg(target_os = "macos")]
use tracing::debug;
use tracing::{info, warn};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

mod cli;
#[cfg(debug_assertions)]
mod runtime_flavor;

// Re-export all debug-only symbols from the `cli` module so they can be referenced
// directly in the current crate namespace.
#[cfg(debug_assertions)]
use cli::*;

const OFFLINE_MODE_ENV: &str = "MAEKON_OFFLINE_MODE";
#[cfg(debug_assertions)]
const DEBUG_RUNTIME_SMOKE_ENV: &str = "MAEKON_DEBUG_RUNTIME_SMOKE_CLI";
#[cfg(debug_assertions)]
const DEBUG_RUNTIME_SMOKE_OUTPUT_ENV: &str = "MAEKON_DEBUG_RUNTIME_SMOKE_OUTPUT";

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
                // Debug-smoke diagnostic tool reporting BOTH raw (`permissions`
                // above) and effective consent side-by-side; `manager` is
                // guaranteed present (post `?` above), so there is no
                // missing-manager default to diverge on (#7728).
                // lint:allow-effective-permissions-composition
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

/// Composition-root entry point invoked by the thin `src/main.rs` binary
/// shim. Builds the Tauri app (DI wiring, IPC command registration, tray,
/// window lifecycle) and runs it to completion.
pub fn run() {
    #[cfg(debug_assertions)]
    runtime_flavor::configure();

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

        let debug_pointer_capture_gate = std::env::var("MAEKON_DEBUG_POINTER_CAPTURE_CLI").ok();
        if let Some(command) = debug_pointer_capture_cli_command_from(
            args.iter().skip(1).map(String::as_str),
            debug_pointer_capture_gate.as_deref(),
        ) {
            std::process::exit(run_debug_pointer_capture_cli_command(command));
        }

        #[cfg(feature = "analysis")]
        {
            if crate::qc_upload_spool::prepare_command_requested(
                args.iter().skip(1).map(String::as_str),
            ) {
                match crate::qc_upload_spool::run_prepare_from_env() {
                    Ok(report) => {
                        println!("{report}");
                        std::process::exit(0);
                    }
                    Err(error) => {
                        eprintln!("debug QC upload-spool preparation failed: {error:#}");
                        std::process::exit(2);
                    }
                }
            }

            if crate::qc_upload_spool::verify_command_requested(
                args.iter().skip(1).map(String::as_str),
            ) {
                match crate::qc_upload_spool::run_verify_from_env() {
                    Ok(report) => {
                        println!("{report}");
                        std::process::exit(0);
                    }
                    Err(error) => {
                        eprintln!("debug QC upload-spool verification failed: {error:#}");
                        std::process::exit(2);
                    }
                }
            }
        }

        if crate::qc_fixture_cli::command_requested(args.iter().skip(1).map(String::as_str)) {
            match crate::qc_fixture_cli::run_from_env() {
                Ok(report) => {
                    println!("{report}");
                    std::process::exit(0);
                }
                Err(error) => {
                    eprintln!("debug QC fixture seed failed: {error:#}");
                    std::process::exit(1);
                }
            }
        }

        if crate::qc_fixture_cli::suggestion_command_requested(
            args.iter().skip(1).map(String::as_str),
        ) {
            match crate::qc_fixture_cli::run_suggestion_from_env() {
                Ok(report) => {
                    println!("{report}");
                    std::process::exit(0);
                }
                Err(error) => {
                    eprintln!("debug QC suggestion fixture seed failed: {error:#}");
                    std::process::exit(1);
                }
            }
        }

        if crate::qc_fixture_cli::action_suggestion_command_requested(
            args.iter().skip(1).map(String::as_str),
        ) {
            match crate::qc_fixture_cli::run_action_suggestion_from_env() {
                Ok(report) => {
                    println!("{report}");
                    std::process::exit(0);
                }
                Err(error) => {
                    eprintln!("debug QC action suggestion fixture seed failed: {error:#}");
                    std::process::exit(1);
                }
            }
        }
        if crate::qc_fixture_cli::claims_command_requested(args.iter().skip(1).map(String::as_str))
        {
            match crate::qc_fixture_cli::run_claims_from_env() {
                Ok(report) => {
                    println!("{report}");
                    std::process::exit(0);
                }
                Err(error) => {
                    eprintln!("debug QC claims fixture seed failed: {error:#}");
                    std::process::exit(2);
                }
            }
        }

        if crate::qc_fixture_cli::audio_command_requested(args.iter().skip(1).map(String::as_str)) {
            match crate::qc_fixture_cli::run_audio_from_env() {
                Ok(report) => {
                    println!("{report}");
                    std::process::exit(0);
                }
                Err(error) => {
                    eprintln!("debug QC audio fixture seed failed: {error:#}");
                    std::process::exit(2);
                }
            }
        }

        if crate::qc_fixture_cli::sync_peer_command_requested(
            args.iter().skip(1).map(String::as_str),
        ) {
            match crate::qc_fixture_cli::run_sync_peer_from_env() {
                Ok(report) => {
                    println!("{report}");
                    std::process::exit(0);
                }
                Err(error) => {
                    eprintln!("debug QC sync-peer fixture seed failed: {error:#}");
                    std::process::exit(2);
                }
            }
        }

        if crate::qc_fixture_cli::legacy_migration_prepare_command_requested(
            args.iter().skip(1).map(String::as_str),
        ) {
            match crate::qc_fixture_cli::run_legacy_migration_prepare_from_env() {
                Ok(report) => {
                    println!("{report}");
                    std::process::exit(0);
                }
                Err(error) => {
                    eprintln!("debug QC legacy migration preparation failed: {error:#}");
                    std::process::exit(2);
                }
            }
        }

        if crate::qc_fixture_cli::legacy_migration_verify_command_requested(
            args.iter().skip(1).map(String::as_str),
        ) {
            match crate::qc_fixture_cli::run_legacy_migration_verify_from_env() {
                Ok(report) => {
                    println!("{report}");
                    std::process::exit(0);
                }
                Err(error) => {
                    eprintln!("debug QC legacy migration verification failed: {error:#}");
                    std::process::exit(2);
                }
            }
        }
    }

    // Windows DLL search order hardening (Spec Section 9.2):
    // Remove CWD from DLL search path to prevent DLL hijacking.
    // SAFETY: FFI call into the Win32 LibraryLoader API. `w!("")` expands to a
    // `'static`, NUL-terminated UTF-16 string literal pointer that outlives the
    // call; SetDllDirectoryW only reads it (the empty string is the documented
    // value that removes the current directory from the DLL search path).
    #[cfg(target_os = "windows")]
    unsafe {
        windows_sys::Win32::System::LibraryLoader::SetDllDirectoryW(windows_sys::core::w!(""));
    }

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("maekon=info,maekon_app=info,maekon_core=info,maekon_monitor=info,maekon_vision=info,maekon_storage=info,maekon_network=info,maekon_suggestion=info")
    });

    let console_layer = tracing_subscriber::fmt::layer().with_ansi(true);

    let log_dir = maekon_core::config_manager::ConfigManager::data_dir()
        .map(|d| d.join("logs"))
        .unwrap_or_else(|_| std::path::PathBuf::from("logs"));

    let deleted = log_retention::cleanup_old_logs(&log_dir, log_retention::DEFAULT_MAX_AGE_DAYS);
    if deleted > 0 {
        // Cannot use tracing yet — subscriber not initialized.
        eprintln!("[maekon] startup log cleanup: deleted {deleted} old log file(s)");
    }

    let (file_layer, worker_guard, file_logging_error) =
        match startup_logging::try_file_log_writer(&log_dir) {
            Ok((non_blocking, worker_guard)) => (
                Some(
                    tracing_subscriber::fmt::layer()
                        .with_ansi(false)
                        .with_writer(non_blocking),
                ),
                Some(worker_guard),
                None,
            ),
            Err(error) => (None, None, Some(error)),
        };

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
        telemetry::Handle::new_with_layer(&disabled_telemetry, &telemetry_data_dir).unwrap_or_else(
            |error| panic!("disabled-at-boot telemetry construction is infallible: {error}"),
        );
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

    if let Some(error) = file_logging_error {
        warn!(
            error = %error,
            "persistent file logging unavailable; continuing without file logging"
        );
    } else {
        info!(log_dir = %log_dir.display(), "persistent file logging initialized");
    }

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
    if args.len() > 1 && (args[1] == "auth" || args[1] == "secret") {
        // #9523: fail loud instead of falling back to `.` — a credential CLI
        // silently operating on ./maekon-keychain-registry.json (a DIFFERENT
        // file than the GUI's config-dir registry) would revoke/inspect the
        // wrong inventory while reporting success.
        let config_dir = match maekon_core::config_manager::ConfigManager::config_dir() {
            Ok(dir) => dir,
            Err(e) => {
                eprintln!("error: cannot resolve the config directory ({e}); refusing to operate on a fallback registry path");
                std::process::exit(2);
            }
        };
        let exit_code = if args[1] == "auth" {
            auth_cli::run(&args[2..], &config_dir)
        } else {
            secret_cli::run(&args[2..], &config_dir)
        };
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
                tracing::info!(
                    window_label = window.label(),
                    "single-instance activation callback restoring main window"
                );
                crate::window_state::show_restore_and_focus_main_window(&window);
            } else {
                tracing::warn!("single-instance activation callback found no main window");
            }
            // _args, _cwd reserved for future CLI command extension (NG3).
        }));
    }

    #[allow(unused_mut)]
    let mut builder = builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(startup_logging::LogWorkerGuard(worker_guard))
        .manage(telemetry_handle)
        // OOS-TBD-N15-UI-EXPOSURE (2026-05-05): TokenManagerState used by the
        // logout_all_sessions Tauri command. Registered ONCE here as an empty
        // interior-mutable slot; app_runtime_launch populates it via
        // `TokenManagerState::set(..)` after server bootstrap creates the token
        // manager. A second `manage(..)` would be a silent no-op (Tauri does not
        // overwrite an already-managed type), so the slot is the populate path.
        // Until populated (or for disabled features / bootstrap failures) the
        // slot stays empty and the command fails immediately.
        .manage(commands::auth::TokenManagerState::empty())
        // #9627: receipt-only draft transport, populated from the same shared
        // authenticated client as context home. No bearer crosses IPC.
        .manage(commands::assignment_email_draft::AssignmentEmailDraftState::empty())
        // #9628: authenticated pending handoff + one-window guard. Populated
        // from the same shared login session as Context Home.
        .manage(commands::console_handoff::ConsoleHandoffState::empty())
        // #10358: shared authenticated transport + process-wide SQLite receipt spool.
        .manage(commands::tmd_xlsx::TmdXlsxState::empty())
        // #9625: same slot discipline for the context-home transport. Registered
        // empty here; `app_runtime_launch::auth_wiring` populates it from the
        // shared login session so one sign-in serves this surface too.
        .manage(commands::context_home::ContextHomeState::empty());

    // WebdriverIO plugins — test-only and excluded from production builds.
    // The service supplies TAURI_WEBDRIVER_PORT and owns app lifecycle.
    #[cfg(feature = "webdriver")]
    {
        info!("WebdriverIO test plugins enabled");
        builder = builder
            .plugin(tauri_plugin_wdio::init())
            .plugin(tauri_plugin_wdio_webdriver::init());
    }

    let app = builder
        .setup(|app| {
            #[cfg(all(debug_assertions, target_os = "macos"))]
            install_debug_macos_notification_delegate_from_env();
            // #10985: returning Err here reaches Tauri's
            // `panic!("Failed to setup app: {e}")` inside a non-unwinding
            // extern boundary, so the process aborts with `Abort trap: 6` and a
            // raw backtrace. That is what a user rolling back to an older build
            // sees, with no hint that the profile belongs to a newer version or
            // that a pre-migration backup exists. Report something actionable
            // and exit cleanly instead.
            match setup::init(app) {
                Ok(()) => Ok(()),
                Err(error) => {
                    let data_dir = maekon_core::config_manager::ConfigManager::data_dir()
                        .unwrap_or_else(|_| std::path::PathBuf::from("."));
                    let backup = std::fs::read_dir(&data_dir)
                        .map(|entries| {
                            startup_failure::newest_backup_name(
                                entries
                                    .flatten()
                                    .map(|e| e.file_name().to_string_lossy().into_owned()),
                            )
                        })
                        .unwrap_or_default();
                    let message = startup_failure::format_startup_failure(
                        &error.to_string(),
                        &data_dir,
                        backup.as_deref(),
                    );
                    // Both sinks on purpose: stderr for a terminal launch, the
                    // tracing log for a Finder/Explorer launch where stderr goes
                    // nowhere the user will ever look.
                    tracing::error!(%error, "startup failed");
                    eprintln!("{message}");
                    std::process::exit(1);
                }
            }
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
            commands::auth::login,
            commands::auth::auth_status,
            commands::auth::logout_all_sessions,
            commands::settings::update_setting,
            commands::system::get_automation_status,
            commands::settings::get_web_port,
            commands::settings::get_local_auth_token,
            commands::system::get_secret_backend_capabilities,
            commands::system::get_feature_capabilities,
            commands::system::get_runtime_log_snapshot,
            commands::system::get_resource_usage_snapshot,
            commands::system::record_frontend_log,
            commands::permissions::get_desktop_permission_status,
            commands::permissions::request_desktop_notification_permission,
            commands::permissions::request_desktop_screen_capture_permission,
            commands::permissions::open_desktop_permission_settings,
            commands::system::probe_provider_surface_endpoint,
            commands::settings::get_allowed_setting_keys,
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
            commands::analysis::get_analysis_health,
            commands::analysis::reload_embedding_model,
            commands::coaching::dismiss_coaching_message,
            commands::coaching::submit_coaching_feedback,
            commands::coaching::debug_set_overlay_interactive,
            commands::coaching::toggle_suggestions_panel,
            commands::coaching::get_suggestions_panel_open,
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
            commands::suggestions::queries::get_pending_suggestion_count,
            commands::suggestions::queries::get_pending_suggestions,
            commands::suggestions::queries::get_suggestion_history,
            commands::suggestions::feedback::submit_suggestion_feedback,
            commands::suggestions::replay::record_suggestion_replay_event,
            commands::suggestions::chat_suggestions::request_chat_suggestions,
            commands::suggestions::current_context::request_current_context_suggestions,
            commands::suggestions::chat_suggestions::explain_suggestion_in_chat,
            commands::suggestions::queries::get_suggestion_stats,
            commands::suggestions::queries::get_suggestion_daily_stats,
            commands::sync::get_sync_status,
            commands::sync::trigger_sync_cycle,
            commands::sync::discover_sync_peers,
            commands::sync::forget_sync_peer,
            commands::qc_upload_spool::get_qc_upload_spool_status,
            commands::qc_upload_spool::run_qc_upload_spool_step,
            commands::automation::confirm_automation_command,
            commands::automation::run_suggestion_action,
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
            commands::audit::verify_audit_log,
            commands::error_report::report_frontend_error,
            commands::tray::get_tray_state,
            commands::tray::request_app_quit,
            commands::tray::get_tray_geometry,
            commands::tray::simulate_tray_action,
            commands::consent::get_consent,
            commands::consent::set_consent,
            commands::consent::withdraw_consent,
            commands::consent::take_microphone_upgrade_notice,
            // #9639: the MK-EXT IPC surface is RETIRED — the eight
            // `commands::extension::*` commands used to be registered here.
            //
            // Measured: nothing in production calls `register_package`, so
            // `extension_installs` is permanently empty. `install()` then returns
            // `RevisionConflict` on every call (`load_row` → None), `list_extensions`
            // returns `[]`, and skill-pack activation fails at
            // `get_manifest(install_id)` — the whole chain is dead at the root, not
            // at the leaf. Registering them made the app advertise a feature that
            // could never do anything.
            //
            // The implementation stays as directly testable Rust functions
            // (`commands/extension.rs`, the storage adapters, their tests), without
            // `#[command]` annotations that would falsely describe a live IPC surface.
            // Reviving requires restoring those annotations, re-adding these lines,
            // AND wiring a real `register_package` call site — the guard in
            // `tests/ipc_command_contract.rs` names that order so the surface cannot
            // come back half-wired again.
            commands::task::list_task_candidates,
            commands::task::list_todos,
            commands::task::confirm_task_candidate,
            commands::task::dismiss_task_candidate,
            commands::task::transition_todo,
            commands::task::delete_todo,
            commands::reauth::get_capture_reauth_status,
            commands::reauth::authenticate_capture_history,
            commands::reauth::register_capture_reauth_pin,
            commands::reauth::clear_capture_reauth_pin,
            commands::reauth::lock_capture_reauth,
            commands::reauth::set_capture_reauth_config,
            // ADR-033 memory vault mirror (#9465).
            commands::vault::run_vault_mirror_cycle,
            commands::vault::get_vault_mirror_settings,
            commands::vault::set_vault_mirror_path,
            // OS handoff boundary (#9707).
            commands::os_handoff::open_external_target,
            // Context-home read surface (#9625).
            commands::context_home::fetch_context_home,
            // Fixed-route Maekon→Console continuity handoff (#9628).
            commands::console_handoff::open_console_assignment_board,
            // Receipt-only assignment draft surface (#9627).
            commands::assignment_email_draft::generate_assignment_email_draft,
            commands::assignment_email_draft::load_assignment_email_draft,
            commands::assignment_email_draft::regenerate_assignment_email_draft,
            // Standalone native-file WBS XLSX flow (#10358).
            commands::tmd_xlsx::generate_tmd_xlsx,
        ])
        .build(tauri::generate_context!())
        .unwrap_or_else(|error| panic!("error while building Maekon: {error}"));

    app.run(move |app_handle, event| match event {
        #[cfg(debug_assertions)]
        RunEvent::Ready if debug_notification_command.is_some() => {
            let Some(command) = debug_notification_command else {
                panic!("notification debug command exists");
            };
            let app_handle = app_handle.clone();
            std::thread::spawn(move || {
                let exit_code = run_debug_notification_cli_command(&app_handle, command);
                app_handle.exit(exit_code);
            });
        }
        #[cfg(debug_assertions)]
        RunEvent::Ready if debug_permissions_runtime_command.is_some() => {
            let Some(command) = debug_permissions_runtime_command else {
                panic!("permission debug command exists");
            };
            let app_handle = app_handle.clone();
            std::thread::spawn(move || {
                let exit_code = run_debug_permissions_runtime_cli_command(&app_handle, command);
                app_handle.exit(exit_code);
            });
        }
        #[cfg(debug_assertions)]
        RunEvent::Ready if debug_pointer_capture_runtime_command.is_some() => {
            let Some(command) = debug_pointer_capture_runtime_command else {
                panic!("pointer capture debug command exists");
            };
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
            crate::app_runtime_launch_health_probe::mark_clean_shutdown();

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
                            .unwrap_or_else(|error| {
                                panic!("regime-save shutdown runtime: {error}")
                            });
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
