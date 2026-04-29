use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager, Runtime,
};
use tracing::{debug, info, warn};

use crate::tray_icon::TrayIconState;
use oneshim_web::update_control::UpdatePhase;

fn focus_main_window<R: Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        window.show().unwrap_or_default();
        window.set_focus().unwrap_or_default();
    }
}

/// Read connection status flags from AppState.
///
/// Returns `(server, llm, cli)` booleans. Falls back to `(false, false, false)`
/// if AppState is not yet registered.
fn read_connection_status<R: Runtime>(app: &impl Manager<R>) -> (bool, bool, bool) {
    app.try_state::<crate::runtime_state::AppState>()
        .map(|s| {
            let ord = std::sync::atomic::Ordering::Relaxed;
            (
                s.connection.server_connected.load(ord),
                s.connection.llm_connected.load(ord),
                s.connection.cli_connected.load(ord),
            )
        })
        .unwrap_or((false, false, false))
}

/// Determine the tray icon state from capture and connection flags.
fn resolve_icon_state(paused: bool, any_disconnected: bool) -> TrayIconState {
    if paused {
        TrayIconState::Paused
    } else if any_disconnected {
        TrayIconState::Disabled
    } else {
        TrayIconState::Active
    }
}

/// Build the connection status menu items (disabled / info-only).
#[allow(clippy::type_complexity)]
fn build_connection_items<R: Runtime>(
    app: &impl Manager<R>,
    srv: bool,
    llm: bool,
    cli: bool,
) -> Result<(MenuItem<R>, MenuItem<R>, MenuItem<R>), Box<dyn std::error::Error>> {
    let srv_item = MenuItem::with_id(
        app,
        "conn-server",
        connection_item_label("Server API", srv),
        false,
        None::<&str>,
    )?;
    let llm_item = MenuItem::with_id(
        app,
        "conn-llm",
        connection_item_label("Local LLM", llm),
        false,
        None::<&str>,
    )?;
    let cli_item = MenuItem::with_id(
        app,
        "conn-cli",
        connection_item_label("CLI bridge", cli),
        false,
        None::<&str>,
    )?;
    Ok((srv_item, llm_item, cli_item))
}

fn connection_item_label(name: &str, connected: bool) -> String {
    format!(
        "  {name}: {}",
        if connected {
            "connected"
        } else {
            "unavailable"
        }
    )
}

/// Determine status text from capture/connection state (no emoji — template icon handles visual).
fn status_text(paused: bool, any_disconnected: bool) -> &'static str {
    if paused {
        "Paused"
    } else if any_disconnected {
        "Local mode"
    } else {
        "Active"
    }
}

fn dashboard_toggle_label() -> &'static str {
    "Show/Hide Dashboard"
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TrayUpdateActions {
    approve_label: &'static str,
    approve_enabled: bool,
    defer_enabled: bool,
}

fn tray_update_actions(phase: Option<&UpdatePhase>) -> TrayUpdateActions {
    match phase {
        Some(UpdatePhase::PendingApproval) => TrayUpdateActions {
            approve_label: "Download Update",
            approve_enabled: true,
            defer_enabled: true,
        },
        Some(UpdatePhase::ReadyToInstall) => TrayUpdateActions {
            approve_label: "Install Update",
            approve_enabled: true,
            defer_enabled: true,
        },
        _ => TrayUpdateActions {
            approve_label: "No Update Available",
            approve_enabled: false,
            defer_enabled: false,
        },
    }
}

fn read_update_phase<R: Runtime>(app: &impl Manager<R>) -> Option<UpdatePhase> {
    let state = app.try_state::<crate::runtime_state::AppState>()?;
    let update_control = state.update_control.as_ref()?;
    let status = update_control.state.try_read().ok()?;

    Some(status.phase.clone())
}

fn current_tray_update_actions<R: Runtime>(app: &impl Manager<R>) -> TrayUpdateActions {
    let phase = read_update_phase(app);
    tray_update_actions(phase.as_ref())
}

/// Build the full tray menu with connection status items.
fn build_tray_menu<R: Runtime>(
    app: &impl Manager<R>,
    paused: bool,
    indicator_visible: bool,
    srv: bool,
    llm: bool,
    cli: bool,
) -> Result<Menu<R>, Box<dyn std::error::Error>> {
    let any_disconnected = !srv || !llm || !cli;

    let capture_status = MenuItem::with_id(
        app,
        "capture-status",
        status_text(paused, any_disconnected),
        false,
        None::<&str>,
    )?;
    let (srv_item, llm_item, cli_item) = build_connection_items(app, srv, llm, cli)?;

    let toggle_text = if paused {
        "Resume Capture"
    } else {
        "Pause Capture"
    };
    let indicator_text = if indicator_visible {
        "Hide Indicator"
    } else {
        "Show Indicator"
    };

    let toggle_capture = MenuItem::with_id(app, "toggle-capture", toggle_text, true, None::<&str>)?;
    let toggle_indicator =
        MenuItem::with_id(app, "toggle-indicator", indicator_text, true, None::<&str>)?;

    let show = MenuItem::with_id(app, "show", dashboard_toggle_label(), true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
    let automation = MenuItem::with_id(
        app,
        "automation",
        "AI Automation Preferences",
        true,
        None::<&str>,
    )?;
    let run_preset = MenuItem::with_id(app, "run-preset", "Automation Page", true, None::<&str>)?;
    let update_actions = current_tray_update_actions(app);
    let approve = MenuItem::with_id(
        app,
        "approve_update",
        update_actions.approve_label,
        update_actions.approve_enabled,
        None::<&str>,
    )?;
    let defer = MenuItem::with_id(
        app,
        "defer_update",
        "Defer Update",
        update_actions.defer_enabled,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &capture_status,
            &srv_item,
            &llm_item,
            &cli_item,
            &PredefinedMenuItem::separator(app)?,
            &toggle_capture,
            &toggle_indicator,
            &PredefinedMenuItem::separator(app)?,
            &show,
            &PredefinedMenuItem::separator(app)?,
            &settings,
            &automation,
            &run_preset,
            &approve,
            &defer,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    Ok(menu)
}

/// System tray setup — template icon + menu + event handler.
/// tauri.conf.json trayIcon is null; everything is handled here.
pub fn setup_tray<R: Runtime>(app: &tauri::App<R>) -> Result<(), Box<dyn std::error::Error>> {
    // Read initial state from AppState (config-driven, registered before tray setup)
    let (paused, indicator_visible) = app
        .try_state::<crate::runtime_state::AppState>()
        .map(|s| {
            (
                s.capture_paused.load(std::sync::atomic::Ordering::Relaxed),
                s.indicator_visible
                    .load(std::sync::atomic::Ordering::Relaxed),
            )
        })
        .unwrap_or((false, true));

    let (srv, llm, cli) = read_connection_status(app);
    let any_disconnected = !srv || !llm || !cli;

    let icon_state = resolve_icon_state(paused, any_disconnected);
    let (rgba, w, h) = crate::tray_icon::status_icon(icon_state);
    let initial_icon = Image::new_owned(rgba, w, h);

    let menu = build_tray_menu(app, paused, indicator_visible, srv, llm, cli)?;

    TrayIconBuilder::with_id("main-tray")
        .icon(initial_icon)
        .icon_as_template(true)
        .menu(&menu)
        .tooltip("Maekon")
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "toggle-capture" => {
                if let Some(state) = app.try_state::<crate::runtime_state::AppState>() {
                    let was_paused = state
                        .capture_paused
                        .fetch_xor(true, std::sync::atomic::Ordering::Relaxed);
                    let new_paused = !was_paused;
                    let indicator_visible = state
                        .indicator_visible
                        .load(std::sync::atomic::Ordering::Relaxed);
                    let payload = serde_json::json!({
                        "paused": new_paused,
                        "indicator_visible": indicator_visible
                    });
                    if let Err(e) =
                        app.emit_to("magic-overlay", "overlay:capture-state-changed", &payload)
                    {
                        debug!("emit magic-overlay failed: {e}");
                    }
                    let _ =
                        app.emit_to("tracking-panel", "overlay:capture-state-changed", &payload);
                    if let Err(e) = sync_tray_state(app, new_paused, indicator_visible) {
                        debug!("sync_tray_state failed: {e}");
                    }
                    #[cfg(target_os = "macos")]
                    if let Some(border) = app.try_state::<crate::native_border::NativeBorderState>()
                    {
                        border.0.set_paused(new_paused);
                    }
                }
            }
            "toggle-indicator" => {
                if let Some(state) = app.try_state::<crate::runtime_state::AppState>() {
                    let was_visible = state
                        .indicator_visible
                        .fetch_xor(true, std::sync::atomic::Ordering::Relaxed);
                    let new_visible = !was_visible;
                    let paused = state
                        .capture_paused
                        .load(std::sync::atomic::Ordering::Relaxed);
                    let payload = serde_json::json!({
                        "paused": paused,
                        "indicator_visible": new_visible
                    });
                    if let Err(e) =
                        app.emit_to("magic-overlay", "overlay:capture-state-changed", &payload)
                    {
                        debug!("emit magic-overlay failed: {e}");
                    }
                    let _ =
                        app.emit_to("tracking-panel", "overlay:capture-state-changed", &payload);
                    if let Some(panel) = app.get_webview_window("tracking-panel") {
                        if new_visible {
                            if let Err(e) = panel.show() {
                                debug!("window show failed: {e}");
                            }
                        } else if let Err(e) = panel.hide() {
                            debug!("window hide failed: {e}");
                        }
                    }
                    if let Err(e) = sync_tray_state(app, paused, new_visible) {
                        debug!("sync_tray_state failed: {e}");
                    }
                    #[cfg(target_os = "macos")]
                    if let Some(border) = app.try_state::<crate::native_border::NativeBorderState>()
                    {
                        if new_visible && !crate::app_runtime_launch::cua_safe_mode_enabled() {
                            border.0.show();
                        } else {
                            border.0.hide();
                        }
                    }
                }
            }
            "show" => {
                if let Some(w) = app.get_webview_window("main") {
                    if w.is_visible().unwrap_or(false) {
                        w.hide().unwrap_or_default();
                    } else {
                        w.show().unwrap_or_default();
                        w.set_focus().unwrap_or_default();
                    }
                }
            }
            // All three menu items emit the standard `navigate` event with a
            // deep-link path, matching the "Settings" entry. Earlier revisions
            // used custom `tray-toggle-automation` / `automation:quick-access`
            // events that were bridged back to navigate calls in the frontend
            // listener — the indirection added nothing beyond a React Query
            // invalidation the target routes did not even consume. Unified so
            // new tray entries only need a path, not a bespoke event name.
            "settings" => {
                focus_main_window(app);
                app.emit_to("main", "navigate", "/settings")
                    .unwrap_or_default();
            }
            "automation" => {
                focus_main_window(app);
                app.emit_to("main", "navigate", "/settings/ai-automation")
                    .unwrap_or_default();
            }
            "run-preset" => {
                focus_main_window(app);
                app.emit_to("main", "navigate", "/automation")
                    .unwrap_or_default();
            }
            "approve_update" => {
                if current_tray_update_actions(app).approve_enabled {
                    focus_main_window(app);
                    if let Some(state) = app.try_state::<crate::runtime_state::AppState>() {
                        use oneshim_web::update_control::UpdateAction;
                        if let Err(e) = state.update_action_tx.send(UpdateAction::Approve) {
                            warn!("tray: approve_update send failed: {e}");
                        }
                    }
                    app.emit_to("main", "tray-approve-update", ())
                        .unwrap_or_default();
                } else {
                    debug!("tray: approve_update ignored because no update action is available");
                }
            }
            "defer_update" => {
                if current_tray_update_actions(app).defer_enabled {
                    focus_main_window(app);
                    if let Some(state) = app.try_state::<crate::runtime_state::AppState>() {
                        use oneshim_web::update_control::UpdateAction;
                        if let Err(e) = state.update_action_tx.send(UpdateAction::Defer) {
                            warn!("tray: defer_update send failed: {e}");
                        }
                    }
                    app.emit_to("main", "tray-defer-update", ())
                        .unwrap_or_default();
                } else {
                    debug!("tray: defer_update ignored because no update action is available");
                }
            }
            "quit" => {
                info!("tray: quit requested");
                app.exit(0);
            }
            _ => {}
        })
        .build(app)?;

    info!("system tray initialized with template icon");
    Ok(())
}

/// Update tray icon and menu to reflect current capture/connection state.
///
/// Combines icon swap (template shape overlay) with menu text rebuild.
/// Called from tray event handlers and IPC commands.
pub fn sync_tray_state<R: Runtime>(
    app: &tauri::AppHandle<R>,
    paused: bool,
    indicator_visible: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(tray) = app.tray_by_id("main-tray") {
        let (srv, llm, cli) = read_connection_status(app);
        let any_disconnected = !srv || !llm || !cli;

        // Update icon with template shape overlay
        let icon_state = resolve_icon_state(paused, any_disconnected);
        let (rgba, w, h) = crate::tray_icon::status_icon(icon_state);
        let icon = Image::new_owned(rgba, w, h);
        tray.set_icon(Some(icon))?;
        tray.set_icon_as_template(true)?;

        // Rebuild menu with connection status
        let menu = build_tray_menu(app, paused, indicator_visible, srv, llm, cli)?;
        tray.set_menu(Some(menu))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnected_services_use_local_mode_status() {
        assert_eq!(status_text(false, true), "Local mode");
    }

    #[test]
    fn dashboard_toggle_label_describes_the_target_window() {
        assert_eq!(dashboard_toggle_label(), "Show/Hide Dashboard");
    }

    #[test]
    fn connection_item_label_uses_words_instead_of_raw_marks() {
        assert_eq!(
            connection_item_label("Server API", false),
            "  Server API: unavailable"
        );
        assert_eq!(
            connection_item_label("Local LLM", true),
            "  Local LLM: connected"
        );
    }

    #[test]
    fn update_actions_are_disabled_without_actionable_update() {
        let actions = tray_update_actions(None);

        assert_eq!(actions.approve_label, "No Update Available");
        assert!(!actions.approve_enabled);
        assert!(!actions.defer_enabled);
    }

    #[test]
    fn update_actions_describe_pending_update_phase() {
        let actions = tray_update_actions(Some(
            &oneshim_web::update_control::UpdatePhase::PendingApproval,
        ));

        assert_eq!(actions.approve_label, "Download Update");
        assert!(actions.approve_enabled);
        assert!(actions.defer_enabled);
    }

    #[test]
    fn update_actions_describe_ready_to_install_phase() {
        let actions = tray_update_actions(Some(
            &oneshim_web::update_control::UpdatePhase::ReadyToInstall,
        ));

        assert_eq!(actions.approve_label, "Install Update");
        assert!(actions.approve_enabled);
        assert!(actions.defer_enabled);
    }
}
