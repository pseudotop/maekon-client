/// Tray tooltip propagation via `ConfigManager::subscribe`.
///
/// Spawns a background async task that watches the whole-config `watch` channel
/// and updates the tray tooltip whenever the `tracking_schedule` sub-section
/// changes. The task is narrow-filtered: only `tracking_schedule` diff triggers
/// a tray update (A.18 will additionally check `notification.tracking_schedule_enabled`).
///
/// Per CONS-PI12: `tray.rs` stays sync. The `tokio::spawn` lives here, wired
/// from `setup.rs` (the composition root) after `setup_tray` returns.
///
/// Watch coalescence is accepted as non-goal per CONS-PM01 / spec §3.7a:
/// rapid enable→disable→enable mutations within a single tick may coalesce.
use oneshim_core::config::TrackingScheduleConfig;
use oneshim_core::config_manager::ConfigManager;
use tauri::{AppHandle, Runtime};
use tracing::{debug, warn};

/// Owned wrapper around the tray-watch task `JoinHandle`.
///
/// Stored as Tauri managed state so that `main.rs` shutdown code can call
/// `abort()` cleanly on app exit.
pub(crate) struct TrayWatchHandle(pub(crate) tauri::async_runtime::JoinHandle<()>);

impl Drop for TrayWatchHandle {
    fn drop(&mut self) {
        // Best-effort abort on drop; task is also aborted explicitly in the
        // RunEvent::Exit handler in main.rs.
        self.0.abort();
    }
}

/// Pure predicate: should the tray tooltip be updated?
///
/// Returns `true` when the new `tracking_schedule` sub-section differs from
/// the previous one. Extracted as a named function so unit tests can exercise
/// the diff logic without any async machinery.
///
/// A.18 will extend this predicate to also check
/// `notification.tracking_schedule_enabled`.
pub(crate) fn tray_diff_detects_tracking_schedule_change(
    prev: &TrackingScheduleConfig,
    next: &TrackingScheduleConfig,
) -> bool {
    prev != next
}

/// Build the tray tooltip string for the current `tracking_schedule` config.
///
/// When the schedule is enabled and has at least one window, the tooltip shows
/// the schedule status ("Tracking Schedule Active"). Otherwise it shows the
/// default app name.
///
/// Q7 / CONS-M12 resolution: reuses Paused icon label wording.
fn build_tray_tooltip(cfg: &TrackingScheduleConfig) -> String {
    if cfg.enabled && !cfg.windows.is_empty() {
        "Maekon - Tracking Schedule Active".to_string()
    } else {
        "Maekon".to_string()
    }
}

/// Apply a tooltip update to the main-tray icon.
fn apply_tray_tooltip<R: Runtime>(app: &AppHandle<R>, tooltip: &str) {
    if let Some(tray) = app.tray_by_id("main-tray") {
        if let Err(e) = tray.set_tooltip(Some(tooltip)) {
            debug!("tray tooltip update failed: {e}");
        }
    }
}

/// Spawn the tray-watch async task.
///
/// Captures:
/// - `config_manager` — subscribes to whole-config changes.
/// - `app_handle` — used to reach the tray icon.
///
/// Returns a `TrayWatchHandle` wrapping the `JoinHandle`. The caller registers
/// it as Tauri managed state for lifetime management.
pub(crate) fn spawn_tray_watch_task(
    config_manager: &ConfigManager,
    app_handle: AppHandle,
) -> TrayWatchHandle {
    let mut rx = config_manager.subscribe();
    let initial_cfg = config_manager.snapshot();
    let mut last_ts_cfg = initial_cfg.tracking_schedule.clone();

    let handle = tauri::async_runtime::spawn(async move {
        while rx.changed().await.is_ok() {
            let cfg = rx.borrow_and_update().clone();
            // Narrow filter per spec §3.11a — only tracking_schedule sub-tree.
            if tray_diff_detects_tracking_schedule_change(&last_ts_cfg, &cfg.tracking_schedule) {
                last_ts_cfg = cfg.tracking_schedule.clone();
                let tooltip = build_tray_tooltip(&cfg.tracking_schedule);
                apply_tray_tooltip(&app_handle, &tooltip);
                debug!(
                    enabled = cfg.tracking_schedule.enabled,
                    windows = cfg.tracking_schedule.windows.len(),
                    "tray watch: tracking_schedule changed, tooltip updated"
                );
            }
        }
        warn!("tray watch: config channel closed, task exiting");
    });

    TrayWatchHandle(handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oneshim_core::config::TrackingWindow;

    fn make_window() -> TrackingWindow {
        TrackingWindow {
            start: "09:00".to_string(),
            end: "17:00".to_string(),
            days_of_week: vec![],
            label: String::new(),
        }
    }

    fn disabled_cfg() -> TrackingScheduleConfig {
        TrackingScheduleConfig::default()
    }

    fn enabled_cfg_with_window() -> TrackingScheduleConfig {
        TrackingScheduleConfig {
            enabled: true,
            windows: vec![make_window()],
            timezone: "Local".to_string(),
        }
    }

    /// tray_diff_detects_tracking_schedule_change: identical configs → no update.
    #[test]
    fn tray_diff_detects_tracking_schedule_change_same_config_no_update() {
        let cfg = disabled_cfg();
        assert!(
            !tray_diff_detects_tracking_schedule_change(&cfg, &cfg.clone()),
            "identical configs must not trigger a tray update"
        );
    }

    /// tray_diff_detects_tracking_schedule_change: enabled toggle → triggers update.
    #[test]
    fn tray_diff_detects_tracking_schedule_change_enabled_toggle_triggers_update() {
        let prev = disabled_cfg();
        let next = TrackingScheduleConfig {
            enabled: true,
            ..disabled_cfg()
        };
        assert!(
            tray_diff_detects_tracking_schedule_change(&prev, &next),
            "toggling enabled=true must trigger a tray update"
        );
    }

    /// tray_diff_detects_tracking_schedule_change: adding a window → triggers update.
    #[test]
    fn tray_diff_detects_tracking_schedule_change_window_added_triggers_update() {
        let prev = disabled_cfg();
        let next = enabled_cfg_with_window();
        assert!(
            tray_diff_detects_tracking_schedule_change(&prev, &next),
            "adding a tracking window must trigger a tray update"
        );
    }

    /// tray_diff_detects_tracking_schedule_change: enabled=true → disabled=false (reverse).
    #[test]
    fn tray_diff_detects_tracking_schedule_change_disable_triggers_update() {
        let prev = enabled_cfg_with_window();
        let next = disabled_cfg();
        assert!(
            tray_diff_detects_tracking_schedule_change(&prev, &next),
            "disabling an active schedule must trigger a tray update"
        );
    }

    /// build_tray_tooltip: disabled schedule -> default "Maekon" tooltip.
    #[test]
    fn build_tray_tooltip_disabled_returns_default() {
        let cfg = disabled_cfg();
        assert_eq!(build_tray_tooltip(&cfg), "Maekon");
    }

    /// build_tray_tooltip: enabled with window → active schedule tooltip.
    #[test]
    fn build_tray_tooltip_enabled_with_window_returns_active_label() {
        let cfg = enabled_cfg_with_window();
        let tooltip = build_tray_tooltip(&cfg);
        assert!(
            tooltip.contains("Tracking Schedule Active"),
            "active schedule tooltip must mention 'Tracking Schedule Active', got: {tooltip}"
        );
    }

    /// build_tray_tooltip: enabled=true but no windows → still returns default.
    #[test]
    fn build_tray_tooltip_enabled_no_windows_returns_default() {
        let cfg = TrackingScheduleConfig {
            enabled: true,
            windows: vec![],
            timezone: "Local".to_string(),
        };
        assert_eq!(
            build_tray_tooltip(&cfg),
            "Maekon",
            "enabled with no windows should still return default tooltip"
        );
    }
}
