use anyhow::Result;
use std::sync::Arc;
use tauri::{App, Manager};
use tracing::info;
#[cfg(target_os = "linux")]
use tracing::warn;

use crate::app_runtime_launch::{AppRuntimeLaunchBuilder, AppRuntimeLaunchResult};
use crate::bootstrap_runtime::{BootstrapRuntimeBuilder, BootstrapRuntimeBundle};
use crate::desktop_startup::DesktopStartupCoordinator;
use crate::telemetry;

/// Tauri setup 함수 — gui_runner.rs의 Agent + WebServer 초기화 이전
pub fn init(app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
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
    let bundle = BootstrapRuntimeBuilder::new().build()?;

    // Bus-driven telemetry reconcile task. ConfigManager now exists (built
    // inside bundle); the tracing subscriber was installed in main.rs with a
    // seeded-disabled TelemetryConfig. Spawn a task that forwards every
    // ConfigChangeBus update to `TelemetryHandle::apply`. The first iteration
    // RECONCILES: it compares `TelemetryConfig::default()` against the real
    // startup config and applies the user's opt-in if present.
    spawn_telemetry_toggle_task(app, &bundle);

    // Clone ConfigManager before bundle is consumed by AppRuntimeLaunchBuilder.
    // ConfigManager::clone is cheap — it shares Arc<Inner>.
    let config_manager_for_tray = bundle.config_manager.clone();

    let AppRuntimeLaunchResult {
        frontend_web_port,
        state_builder,
    } = AppRuntimeLaunchBuilder::new(bundle, app.handle().clone()).build_and_spawn()?;

    state_builder.build().register_on(app);
    crate::setup_shortcuts::register_all(app);

    // 12. Desktop shell startup
    DesktopStartupCoordinator::apply(app, frontend_web_port)?;
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

/// Spawn the bus-driven telemetry reconcile task. Must be called AFTER
/// `bundle.config_manager` is fully constructed — the task captures a
/// subscriber via `config_manager.subscribe()` and keeps a clone of the
/// `Arc<TelemetryHandle>` that `main.rs` stashed in Tauri managed state.
///
/// The first iteration is a synchronous reconcile: seed `prev` with
/// `TelemetryConfig::default()` (matching what `main.rs` used for the
/// subscriber), then compare against the real startup config and apply if it
/// differs. This is how a startup config with `enabled=true` actually
/// activates the exporter — without it, the user's opt-in would stay dormant
/// until they next touched the settings.
fn spawn_telemetry_toggle_task(app: &App, bundle: &BootstrapRuntimeBundle) {
    let handle: Arc<telemetry::Handle> = app.state::<Arc<telemetry::Handle>>().inner().clone();
    let mut rx = bundle.config_manager.subscribe();

    tauri::async_runtime::spawn(async move {
        use oneshim_core::config::TelemetryConfig;

        // Seed with the boot-time default so the first iteration reconciles
        // any user-saved opt-in.
        let mut prev = TelemetryConfig::default();

        // Synchronous reconcile before entering the await loop.
        let initial = rx.borrow_and_update().telemetry.clone();
        if initial != prev {
            if let Err(e) = handle.apply(&initial) {
                tracing::warn!(error = %e, "initial telemetry apply failed");
            }
            prev = initial;
        }

        while rx.changed().await.is_ok() {
            let current = rx.borrow_and_update().telemetry.clone();
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
    /// tauri.conf.json의 window 설정이 setup::init()의 show() 로직과 일관성 있는지 검증.
    /// visible=false + setup에서 show() 호출하는 패턴이 유지되어야 함.
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

        // visible=false 확인 — setup::init()에서 show()를 호출하는 패턴
        assert_eq!(
            main_window["visible"].as_bool(),
            Some(false),
            "main window must start hidden (visible=false); setup::init() calls show() after initialization"
        );
    }

    /// desktop startup helper에 window.show() 호출이 포함되어 있는지 정적 검증.
    /// 향후 리팩토링 시 show() 호출이 실수로 제거되는 것을 방지.
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

    /// main.rs에 RunEvent::Reopen 핸들러가 있는지 검증.
    /// macOS dock 아이콘 클릭 시 윈도우를 다시 표시하기 위해 필수.
    #[test]
    fn main_contains_reopen_handler() {
        let main_src = include_str!("main.rs");

        assert!(
            main_src.contains("RunEvent::Reopen"),
            "main.rs must handle RunEvent::Reopen for macOS dock icon clicks"
        );
    }
}
