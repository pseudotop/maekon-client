use anyhow::Result;
use std::sync::Arc;
use tauri::{App, Manager};
use tracing::info;
#[cfg(target_os = "linux")]
use tracing::warn;

use crate::app_runtime_launch::{AppRuntimeLaunchBuilder, AppRuntimeLaunchResult};
use crate::bootstrap_runtime::BootstrapRuntimeBuilder;
use crate::desktop_startup::DesktopStartupCoordinator;
use crate::telemetry;

#[cfg(debug_assertions)]
fn debug_runtime_cli_requested_from<I, S>(args: I, env_value: Option<&str>) -> bool
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
        Some("debug-notification" | "debug-permissions-runtime")
    )
}

/// Tauri setup function — runs before the Agent + WebServer initialization in gui_runner.rs.
pub fn init(app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(debug_assertions)]
    {
        let debug_notification_gate = std::env::var("MAEKON_DEBUG_NOTIFICATION_CLI").ok();
        let debug_permissions_gate = std::env::var("MAEKON_DEBUG_DESKTOP_PERMISSION_CLI").ok();
        let args: Vec<String> = std::env::args().skip(1).collect();
        if debug_runtime_cli_requested_from(
            args.iter().map(String::as_str),
            debug_notification_gate.as_deref(),
        ) || debug_runtime_cli_requested_from(
            args.iter().map(String::as_str),
            debug_permissions_gate.as_deref(),
        ) {
            info!("Tauri setup: skipping full runtime for debug runtime CLI");
            return Ok(());
        }
    }

    // Per spec §5.2 mitigation #2 / I7: log warn if D-Bus session bus is
    // unavailable on Linux. Single-instance plugin will silently fail in
    // this case — duplicate processes may launch in headless sessions.
    #[cfg(target_os = "linux")]
    {
        if std::env::var("DBUS_SESSION_BUS_ADDRESS").is_err() {
            warn!(
                err.code = "single_instance_dbus_absent",
                "DBUS_SESSION_BUS_ADDRESS not set — single-instance enforcement degraded; \
                 duplicate processes may launch in headless sessions"
            );
        }
        crate::lifecycle::autostart_migration::run_startup_migration();
    }

    info!("Tauri setup: initializing Maekon agent");
    let offline_mode = crate::offline_mode_enabled();
    let bundle = BootstrapRuntimeBuilder::new()
        .with_offline_mode(offline_mode)
        .build()?;

    // Clone ConfigManager before bundle is consumed by AppRuntimeLaunchBuilder.
    // ConfigManager::clone is cheap — it shares Arc<Inner>.
    let config_manager_for_tray = bundle.config_manager.clone();
    // #5056: keep a second cheap clone for the consent-gated telemetry reconcile
    // task. That task is spawned AFTER state registration (below) because it
    // needs the `Arc<dyn ConsentManagerPort>`, which only exists once
    // `AppRuntimeLaunchBuilder::build_and_spawn` has wired the capture context.
    let config_manager_for_telemetry = bundle.config_manager.clone();

    let AppRuntimeLaunchResult {
        frontend_web_port,
        local_auth_token,
        state_builder,
        // F-RR-C36-01: destructure the handles and register them as Tauri managed
        // state. Tauri owns managed state until app shutdown, so the process
        // lifetime is guaranteed.
        #[cfg(feature = "grpc-dashboard-external")]
        ext_grpc_supervisor,
        #[cfg(feature = "grpc-dashboard-external")]
        ext_cert_watcher,
    } = AppRuntimeLaunchBuilder::new(bundle, app.handle().clone())
        .with_offline_mode(offline_mode)
        .build_and_spawn()?;

    state_builder.build().register_on(app);

    // #5056: consent-gated bus-driven telemetry reconcile task. Spawned here —
    // AFTER state registration — so the `Arc<dyn ConsentManagerPort>` wired into the
    // capture context is reachable via `AppState`. The tracing subscriber was
    // installed in main.rs with a seeded-disabled TelemetryConfig; this task
    // forwards every config change to `TelemetryHandle::apply` AFTER passing it
    // through the consent gate (`consent_gated_telemetry_config`). The first
    // iteration RECONCILES: it applies the config intent only if config.enabled
    // AND telemetry consent are both true (fail-closed bridge). When no
    // ConsentManager is wired (test/degraded builds), telemetry stays off.
    if let Some(consent_manager) = app
        .state::<crate::runtime_state::AppState>()
        .capture
        .consent_manager
        .clone()
    {
        spawn_telemetry_toggle_task(app, &config_manager_for_telemetry, consent_manager);
    } else {
        tracing::warn!(
            "telemetry reconcile task not spawned: no ConsentManager wired — \
             telemetry stays OFF (fail-closed)"
        );
    }

    // E20-41 (#4833): register the per-session local-API auth token as managed
    // state so the `get_local_auth_token` IPC (the WebView's race-safe fallback)
    // can return it. It is also injected into the main WebView below.
    app.manage(crate::runtime_state::LocalAuthTokenState(
        local_auth_token.clone(),
    ));

    // F-RR-C36-01: register the external gRPC supervisor + TLS watcher as Tauri
    // managed state. Managed state is dropped on app shutdown, so the tasks are
    // cleaned up properly. Wrapped in Option so it stays safe in builds where the
    // feature is disabled or the config is not applied.
    #[cfg(feature = "grpc-dashboard-external")]
    app.manage(crate::ext_grpc_handles::ExtGrpcHandles {
        supervisor: ext_grpc_supervisor,
        cert_watcher: ext_cert_watcher,
    });
    crate::setup_shortcuts::register_all(app);

    // 12. Desktop shell startup
    DesktopStartupCoordinator::apply(app, frontend_web_port, &local_auth_token)?;
    crate::setup_windows::prepare(app);
    crate::setup_platform::apply(app);

    // A.17: Spawn tray tooltip propagation task — watches tracking_schedule
    // sub-tree of ConfigManager and updates the tray tooltip on change.
    // Spawned AFTER setup_tray (inside DesktopStartupCoordinator::apply) so
    // the "main-tray" icon exists before the task can call set_tooltip.
    // JoinHandle is stored as Tauri managed state for teardown on app shutdown.
    let tray_watch_handle =
        crate::tray_watch::spawn_tray_watch_task(&config_manager_for_tray, app.handle().clone());
    app.manage(tray_watch_handle);

    info!("Tauri setup complete");
    crate::lifecycle::sd_notify::notify_ready();
    Ok(())
}

/// Spawn the bus-driven telemetry reconcile task. Must be called AFTER both
/// `bundle.config_manager` is fully constructed AND the `Arc<dyn ConsentManagerPort>`
/// is available (the latter is built during `AppRuntimeLaunchBuilder` and read
/// back from `AppState`). The task captures a subscriber via
/// `config_manager.subscribe()` and keeps a clone of the `Arc<TelemetryHandle>`
/// that `main.rs` stashed in Tauri managed state, plus the `Arc<dyn ConsentManagerPort>`.
///
/// #5056 — consent→exporter bridge. The applied config is NOT the raw config:
/// it is run through [`telemetry::consent_gated_telemetry_config`], which ANDs
/// `config.telemetry.enabled` with the fail-closed `consent.telemetry`
/// permission. So a config flip to `enabled=true` while telemetry consent is
/// absent/revoked leaves the exporter OFF — `permissions.telemetry` now
/// actually gates the OTLP exporter, not just the consent UI.
///
/// The first iteration is a synchronous reconcile: seed `prev` with the
/// explicit disabled bootstrap config (matching what `main.rs` used for the
/// subscriber), then compare the GATED startup config against it and apply if it
/// differs. This is how a startup config with `enabled=true` AND telemetry
/// consent granted actually activates the exporter — without it, the config
/// intent would stay dormant until the user next touched the settings.
///
/// Note: this task only re-evaluates the gate on a CONFIG change. Consent
/// changes (grant/revoke) are pushed by the IPC commands in
/// `commands::consent` (which call `consent_gated_telemetry_config` + `apply`
/// directly), so a revoke shuts the exporter down immediately rather than
/// waiting for the next config edit.
fn spawn_telemetry_toggle_task(
    app: &App,
    config_manager: &maekon_core::config_manager::ConfigManager,
    consent_manager: Arc<dyn maekon_core::ports::consent_manager::ConsentManagerPort>,
) {
    let handle: Arc<telemetry::Handle> = app.state::<Arc<telemetry::Handle>>().inner().clone();
    let mut rx = config_manager.subscribe();

    tauri::async_runtime::spawn(async move {
        use maekon_core::config::TelemetryConfig;

        // Seed with the boot-time disabled state so the first iteration
        // reconciles any consent-approved config intent.
        let mut prev = TelemetryConfig::disabled();

        // Synchronous reconcile before entering the await loop. Apply the
        // CONSENT-GATED config, not the raw config (#5056 fail-closed bridge).
        let initial = telemetry::consent_gated_telemetry_config(
            &rx.borrow_and_update().telemetry,
            &*consent_manager,
        );
        if initial != prev {
            if let Err(e) = handle.apply(&initial) {
                tracing::warn!(error = %e, "initial telemetry apply failed");
            }
            prev = initial;
        }

        while rx.changed().await.is_ok() {
            // Re-read both config and consent on every tick — consent may have
            // changed since the last apply (the IPC re-apply is best-effort, so
            // the config-change path must also honour the live consent gate).
            let current = telemetry::consent_gated_telemetry_config(
                &rx.borrow_and_update().telemetry,
                &*consent_manager,
            );
            if current != prev {
                if let Err(e) = handle.apply(&current) {
                    tracing::warn!(error = %e, "telemetry apply failed");
                }
                prev = current;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    #[cfg(debug_assertions)]
    use super::debug_runtime_cli_requested_from;

    #[cfg(debug_assertions)]
    #[test]
    fn debug_runtime_cli_setup_skip_requires_gate_and_supported_subcommand() {
        assert!(!debug_runtime_cli_requested_from(
            ["debug-notification", "send"],
            None
        ));
        assert!(!debug_runtime_cli_requested_from(
            ["debug-autostart", "status"],
            Some("1")
        ));
        assert!(debug_runtime_cli_requested_from(
            ["debug-notification", "request"],
            Some("yes")
        ));
        assert!(debug_runtime_cli_requested_from(
            ["debug-permissions-runtime", "screen-capture-request"],
            Some("1")
        ));
    }

    /// Verifies that the window config in tauri.conf.json is consistent with the
    /// show() logic in setup::init(). The pattern of visible=false plus a show()
    /// call in setup must be preserved.
    #[test]
    fn tauri_conf_window_starts_hidden_for_setup_controlled_show() {
        let conf_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json");
        let conf_str = std::fs::read_to_string(&conf_path).expect("tauri.conf.json must exist");
        let conf: serde_json::Value =
            serde_json::from_str(&conf_str).expect("tauri.conf.json must be valid JSON");

        let windows = conf["app"]["windows"]
            .as_array()
            .expect("app.windows must be an array");
        assert!(!windows.is_empty(), "at least one window must be defined");

        let main_window = windows
            .iter()
            .find(|w| w["label"].as_str() == Some("main"))
            .expect("main window must be defined in tauri.conf.json");

        // Verify visible=false — the pattern where setup::init() calls show()
        assert_eq!(
            main_window["visible"].as_bool(),
            Some(false),
            "main window must start hidden (visible=false); setup::init() calls show() after initialization"
        );
    }

    fn load_tauri_csp(file_name: &str) -> String {
        let conf_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(file_name);
        let conf_str = std::fs::read_to_string(&conf_path).expect("tauri config must exist");
        let conf: serde_json::Value =
            serde_json::from_str(&conf_str).expect("tauri config must be valid JSON");
        conf["app"]["security"]["csp"]
            .as_str()
            .expect("app.security.csp must exist")
            .to_string()
    }

    fn csp_directive_tokens(csp: &str, directive: &str) -> Vec<String> {
        csp.split(';')
            .filter_map(|part| {
                let mut tokens = part.split_whitespace();
                let name = tokens.next()?;
                if name == directive {
                    Some(tokens.map(str::to_string).collect())
                } else {
                    None
                }
            })
            .next()
            .unwrap_or_default()
    }

    #[test]
    fn tauri_prod_csp_disallows_eval_and_scopes_inline_styles_to_style_attrs() {
        let prod_csp = load_tauri_csp("tauri.conf.json");

        assert!(
            !csp_directive_tokens(&prod_csp, "script-src").contains(&"'unsafe-eval'".to_string()),
            "prod CSP script-src must not allow unsafe-eval"
        );
        assert!(
            !csp_directive_tokens(&prod_csp, "style-src").contains(&"'unsafe-inline'".to_string()),
            "prod CSP style-src must not allow broad unsafe-inline; use style-src-attr for React dynamic style attributes"
        );
        assert!(
            csp_directive_tokens(&prod_csp, "style-src-attr")
                .contains(&"'unsafe-inline'".to_string()),
            "prod CSP must scope the inline-style compatibility exception to style attributes only"
        );
    }

    #[test]
    fn tauri_prod_csp_allows_loopback_api_for_desktop_dashboard() {
        let prod_csp = load_tauri_csp("tauri.conf.json");
        let connect_src = csp_directive_tokens(&prod_csp, "connect-src");
        let img_src = csp_directive_tokens(&prod_csp, "img-src");

        for port in 10090..=10099 {
            let origin = format!("http://127.0.0.1:{port}");
            assert!(
                connect_src.contains(&origin),
                "prod CSP connect-src must allow local Axum API/SSE origin {origin}"
            );
            assert!(
                img_src.contains(&origin),
                "prod CSP img-src must allow local frame image origin {origin}"
            );
        }
    }

    #[test]
    fn tauri_dev_csp_is_the_only_config_that_allows_eval() {
        let prod_csp = load_tauri_csp("tauri.conf.json");
        let dev_csp = load_tauri_csp("tauri.dev.conf.json");

        assert!(
            !prod_csp.contains("'unsafe-eval'"),
            "prod CSP must never include unsafe-eval"
        );
        assert!(
            csp_directive_tokens(&dev_csp, "script-src").contains(&"'unsafe-eval'".to_string()),
            "dev CSP may allow unsafe-eval for Vite/dev tooling only"
        );
    }

    /// Statically verifies that the desktop startup helper contains a window.show()
    /// call. Prevents the show() call from being accidentally removed in a future
    /// refactor.
    #[test]
    fn desktop_startup_contains_window_show_call() {
        let setup_src = include_str!("desktop_startup.rs");

        assert!(
            setup_src.contains("window.show()"),
            "desktop startup must call window.show() — without this, the GUI window is invisible on launch"
        );
        assert!(
            setup_src.contains("window.set_focus()"),
            "desktop startup must call window.set_focus() after show()"
        );
    }

    /// Verifies that main.rs has a RunEvent::Reopen handler.
    /// Required to re-show the window when the macOS dock icon is clicked.
    #[test]
    fn main_contains_reopen_handler() {
        let main_src = include_str!("main.rs");

        assert!(
            main_src.contains("RunEvent::Reopen"),
            "main.rs must handle RunEvent::Reopen for macOS dock icon clicks"
        );
    }
}
