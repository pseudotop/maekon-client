// OOS-TBD: ADR-013 file split (cycle 35+) — LOC: 875
use tauri::{Emitter, Manager, Runtime};
// `tauri::tray`/`menu` builder types and `image::Image` are only touched by the
// tray-builder path (`setup_tray`/`sync_tray_state`/`build_tray_menu`/
// `build_connection_items`). That path needs `tauri/tray-icon`, which is forced
// on macOS/Windows by the platform dep tables but optional on Linux via the
// `app-tray` feature (the no-gtk Linux build drops it). Gate these imports on
// the SAME predicate as the functions that use them so the no-tray Linux build
// (`--no-default-features`, `app-tray` off) has no unused imports.
#[cfg(any(not(target_os = "linux"), feature = "app-tray"))]
use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
};
use tracing::{debug, info, warn};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::tray_icon::TrayIconState;
use maekon_web::update_control::UpdatePhase;

const MAIN_TRAY_ID: &str = "main-tray";
const MAIN_TRAY_TOOLTIP: &str = "Maekon";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TrayMenuItemSnapshot {
    pub id: String,
    pub label: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TrayConnectionSnapshot {
    pub server: bool,
    pub llm: bool,
    pub cli: bool,
    pub any_disconnected: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TrayGeometrySnapshot {
    pub available: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TrayIconVisualSnapshot {
    pub variant: String,
    pub width: u32,
    pub height: u32,
    pub rgba_len: usize,
    pub rgba_sha256: String,
    pub template_icon: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TrayDebugState {
    pub id: String,
    pub registered: bool,
    pub visible: bool,
    pub tooltip: String,
    pub capture_available: bool,
    pub capture_paused: bool,
    pub indicator_visible: bool,
    pub icon_variant: String,
    pub icon_visual: TrayIconVisualSnapshot,
    pub visual_catalog: Vec<TrayIconVisualSnapshot>,
    pub connection: TrayConnectionSnapshot,
    pub menu_items: Vec<TrayMenuItemSnapshot>,
    pub geometry: TrayGeometrySnapshot,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TrayActionOutcome {
    pub action: String,
    pub handled: bool,
    pub destructive: bool,
    pub state: TrayDebugState,
}

pub(crate) fn focus_main_window<R: Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        crate::window_state::show_restore_and_focus_main_window(&window);
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

/// Whether every capture gate other than the explicit pause toggle is open.
///
/// This reuses the scheduler's effective gate so the tray cannot advertise
/// active capture when consent, configuration, or the tracking schedule blocks
/// collection. Passing `false` for pause keeps the unavailable and paused
/// states distinguishable in the tray UI.
fn capture_available<R: Runtime>(app: &impl Manager<R>) -> bool {
    app.try_state::<crate::runtime_state::AppState>()
        .map(|state| crate::magic_overlay::effective_capture_permitted(&state, false))
        .unwrap_or(false)
}

/// Determine the tray icon state from capture and connection flags.
fn resolve_icon_state(
    capture_available: bool,
    paused: bool,
    any_disconnected: bool,
) -> TrayIconState {
    if !capture_available {
        TrayIconState::Disabled
    } else if paused {
        TrayIconState::Paused
    } else if any_disconnected {
        TrayIconState::Disabled
    } else {
        TrayIconState::Active
    }
}

/// Build the connection status menu items (disabled / info-only).
///
/// Tray-builder-only: gated on the same predicate as `setup_tray`/
/// `sync_tray_state` (its only callers via `build_tray_menu`) so the no-tray
/// Linux build does not flag it as dead code.
#[cfg(any(not(target_os = "linux"), feature = "app-tray"))]
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
fn status_text(capture_available: bool, paused: bool, any_disconnected: bool) -> &'static str {
    if !capture_available {
        "Capture unavailable"
    } else if paused {
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

fn icon_state_name(state: TrayIconState) -> &'static str {
    match state {
        TrayIconState::Active => "active",
        TrayIconState::Paused => "paused",
        TrayIconState::Disabled => "disabled",
    }
}

fn tray_icon_visual_snapshot(state: TrayIconState) -> TrayIconVisualSnapshot {
    let (rgba, width, height) = crate::tray_icon::status_icon(state);
    let mut hasher = Sha256::new();
    hasher.update(&rgba);
    let digest = hasher.finalize();
    let rgba_sha256 = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();

    TrayIconVisualSnapshot {
        variant: icon_state_name(state).to_string(),
        width,
        height,
        rgba_len: rgba.len(),
        rgba_sha256,
        template_icon: true,
    }
}

fn tray_icon_visual_catalog() -> Vec<TrayIconVisualSnapshot> {
    vec![
        tray_icon_visual_snapshot(TrayIconState::Active),
        tray_icon_visual_snapshot(TrayIconState::Paused),
        tray_icon_visual_snapshot(TrayIconState::Disabled),
    ]
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

fn tray_item(
    id: impl Into<String>,
    label: impl Into<String>,
    enabled: bool,
) -> TrayMenuItemSnapshot {
    TrayMenuItemSnapshot {
        id: id.into(),
        label: label.into(),
        enabled,
    }
}

fn debug_menu_items_snapshot(
    capture_available: bool,
    paused: bool,
    indicator_visible: bool,
    srv: bool,
    llm: bool,
    cli: bool,
    update_actions: TrayUpdateActions,
) -> Vec<TrayMenuItemSnapshot> {
    let any_disconnected = !srv || !llm || !cli;
    let toggle_text = if !capture_available {
        "Capture unavailable"
    } else if paused {
        "Resume Capture"
    } else {
        "Pause Capture"
    };
    let indicator_text = if indicator_visible {
        "Hide Indicator"
    } else {
        "Show Indicator"
    };

    vec![
        tray_item(
            "capture-status",
            status_text(capture_available, paused, any_disconnected),
            false,
        ),
        tray_item(
            "conn-server",
            connection_item_label("Server API", srv),
            false,
        ),
        tray_item("conn-llm", connection_item_label("Local LLM", llm), false),
        tray_item("conn-cli", connection_item_label("CLI bridge", cli), false),
        tray_item("toggle-capture", toggle_text, capture_available),
        tray_item("toggle-indicator", indicator_text, true),
        tray_item("show", dashboard_toggle_label(), true),
        tray_item("settings", "Settings", true),
        tray_item("automation", "AI Automation Preferences", true),
        tray_item("run-preset", "Automation Page", true),
        tray_item(
            "approve_update",
            update_actions.approve_label,
            update_actions.approve_enabled,
        ),
        tray_item("defer_update", "Defer Update", update_actions.defer_enabled),
        tray_item("quit", "Quit", true),
    ]
}

/// Build the full tray menu with connection status items.
///
/// Tray-builder-only: gated on the same predicate as `setup_tray`/
/// `sync_tray_state` (its only callers) so the no-tray Linux build does not
/// flag it — or the `Menu`/`MenuItem`/`PredefinedMenuItem` imports it uses — as
/// dead.
#[cfg(any(not(target_os = "linux"), feature = "app-tray"))]
fn build_tray_menu<R: Runtime>(
    app: &impl Manager<R>,
    capture_available: bool,
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
        status_text(capture_available, paused, any_disconnected),
        false,
        None::<&str>,
    )?;
    let (srv_item, llm_item, cli_item) = build_connection_items(app, srv, llm, cli)?;

    let toggle_text = if !capture_available {
        "Capture unavailable"
    } else if paused {
        "Resume Capture"
    } else {
        "Pause Capture"
    };
    let indicator_text = if indicator_visible {
        "Hide Indicator"
    } else {
        "Show Indicator"
    };

    let toggle_capture = MenuItem::with_id(
        app,
        "toggle-capture",
        toggle_text,
        capture_available,
        None::<&str>,
    )?;
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
#[cfg(any(not(target_os = "linux"), feature = "app-tray"))]
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
    let capture_available = capture_available(app);
    let any_disconnected = !srv || !llm || !cli;

    let icon_state = resolve_icon_state(capture_available, paused, any_disconnected);
    let (rgba, w, h) = crate::tray_icon::status_icon(icon_state);
    let initial_icon = Image::new_owned(rgba, w, h);

    let menu = build_tray_menu(
        app,
        capture_available,
        paused,
        indicator_visible,
        srv,
        llm,
        cli,
    )?;

    TrayIconBuilder::with_id(MAIN_TRAY_ID)
        .icon(initial_icon)
        .icon_as_template(true)
        .menu(&menu)
        .tooltip(MAIN_TRAY_TOOLTIP)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| {
            handle_tray_menu_event(app, event.id.as_ref());
        })
        .build(app)?;

    info!("system tray initialized with template icon");
    Ok(())
}

/// No-tray fallback (Linux without `app-tray`): the desktop runs headless of a
/// tray; startup must still succeed.
#[cfg(all(target_os = "linux", not(feature = "app-tray")))]
pub fn setup_tray<R: Runtime>(_app: &tauri::App<R>) -> Result<(), Box<dyn std::error::Error>> {
    info!("tray disabled at compile time (linux without `app-tray`)");
    Ok(())
}

/// Update tray icon and menu to reflect current capture/connection state.
///
/// Combines icon swap (template shape overlay) with menu text rebuild.
/// Called from tray event handlers and IPC commands.
#[cfg(any(not(target_os = "linux"), feature = "app-tray"))]
pub fn sync_tray_state<R: Runtime>(
    app: &tauri::AppHandle<R>,
    paused: bool,
    indicator_visible: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(tray) = app.tray_by_id(MAIN_TRAY_ID) {
        let (srv, llm, cli) = read_connection_status(app);
        let capture_available = capture_available(app);
        let any_disconnected = !srv || !llm || !cli;

        // Update icon with template shape overlay
        let icon_state = resolve_icon_state(capture_available, paused, any_disconnected);
        let (rgba, w, h) = crate::tray_icon::status_icon(icon_state);
        let icon = Image::new_owned(rgba, w, h);
        tray.set_icon(Some(icon))?;
        tray.set_icon_as_template(true)?;

        // Rebuild menu with connection status
        let menu = build_tray_menu(
            app,
            capture_available,
            paused,
            indicator_visible,
            srv,
            llm,
            cli,
        )?;
        tray.set_menu(Some(menu))?;
    }
    Ok(())
}

/// No-tray fallback: state changes have no tray to mirror into.
#[cfg(all(target_os = "linux", not(feature = "app-tray")))]
pub fn sync_tray_state<R: Runtime>(
    _app: &tauri::AppHandle<R>,
    _paused: bool,
    _indicator_visible: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}

pub fn debug_tray_state<R: Runtime>(app: &tauri::AppHandle<R>) -> TrayDebugState {
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
    let capture_available = capture_available(app);
    let any_disconnected = !srv || !llm || !cli;
    let icon_state = resolve_icon_state(capture_available, paused, any_disconnected);
    #[cfg(any(not(target_os = "linux"), feature = "app-tray"))]
    let registered = app.tray_by_id(MAIN_TRAY_ID).is_some();
    #[cfg(all(target_os = "linux", not(feature = "app-tray")))]
    let registered = false;
    let update_actions = current_tray_update_actions(app);
    let icon_visual = tray_icon_visual_snapshot(icon_state);

    TrayDebugState {
        id: MAIN_TRAY_ID.to_string(),
        registered,
        visible: registered,
        tooltip: MAIN_TRAY_TOOLTIP.to_string(),
        capture_available,
        capture_paused: paused,
        indicator_visible,
        icon_variant: icon_state_name(icon_state).to_string(),
        icon_visual,
        visual_catalog: tray_icon_visual_catalog(),
        connection: TrayConnectionSnapshot {
            server: srv,
            llm,
            cli,
            any_disconnected,
        },
        menu_items: debug_menu_items_snapshot(
            capture_available,
            paused,
            indicator_visible,
            srv,
            llm,
            cli,
            update_actions,
        ),
        geometry: tray_geometry_state(),
    }
}

pub fn tray_geometry_state() -> TrayGeometrySnapshot {
    TrayGeometrySnapshot {
        available: false,
        reason: "Tauri tray API does not expose stable cross-platform tray icon geometry"
            .to_string(),
    }
}

fn tray_quit_debug_ipc_allowed(debug_assertions: bool, env_flag: Option<&str>) -> bool {
    debug_assertions
        || env_flag
            .map(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes"
                )
            })
            .unwrap_or(false)
}

fn tray_quit_debug_ipc_allowed_now() -> bool {
    tray_quit_debug_ipc_allowed(
        cfg!(debug_assertions),
        std::env::var("MAEKON_DEBUG_ALLOW_TRAY_QUIT")
            .ok()
            .as_deref(),
    )
}

fn request_tray_quit<R: Runtime>(app: &tauri::AppHandle<R>) {
    info!("tray: quit requested");
    app.exit(0);
}

fn schedule_debug_tray_quit<R: Runtime>(app: tauri::AppHandle<R>) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(75));
        request_tray_quit(&app);
    });
}

pub fn simulate_tray_action<R: Runtime>(
    app: &tauri::AppHandle<R>,
    action: &str,
) -> Result<TrayActionOutcome, String> {
    if action == "quit" {
        if !tray_quit_debug_ipc_allowed_now() {
            return Err(
                "quit debug IPC is disabled outside debug/test mode; set MAEKON_DEBUG_ALLOW_TRAY_QUIT=1 to allow destructive quit TC"
                    .to_string(),
            );
        }
        let state = debug_tray_state(app);
        schedule_debug_tray_quit(app.clone());
        return Ok(TrayActionOutcome {
            action: action.to_string(),
            handled: true,
            destructive: true,
            state,
        });
    }

    let handled = handle_tray_menu_event(app, action);
    if !handled {
        return Err(format!("unknown tray action: {action}"));
    }

    Ok(TrayActionOutcome {
        action: action.to_string(),
        handled,
        destructive: action == "quit",
        state: debug_tray_state(app),
    })
}

fn handle_tray_menu_event<R: Runtime>(app: &tauri::AppHandle<R>, id: &str) -> bool {
    match id {
        "toggle-capture" => {
            if let Some(state) = app.try_state::<crate::runtime_state::AppState>() {
                if !crate::magic_overlay::effective_capture_permitted(&state, false) {
                    debug!("tray: toggle-capture ignored because capture is unavailable");
                    return true;
                }
                let was_paused = state
                    .capture_paused
                    .fetch_xor(true, std::sync::atomic::Ordering::Relaxed);
                let new_paused = !was_paused;
                let indicator_visible = state
                    .indicator_visible
                    .load(std::sync::atomic::Ordering::Relaxed);
                crate::commands::capture_status::emit_current_capture_status(app, &state);
                if let Err(e) = sync_tray_state(app, new_paused, indicator_visible) {
                    debug!("sync_tray_state failed: {e}");
                }
                crate::magic_overlay::sync_passive_tracking_surface(
                    app,
                    new_paused,
                    indicator_visible,
                );
                #[cfg(target_os = "macos")]
                if let Some(border) = app.try_state::<crate::native_border::NativeBorderState>() {
                    border.0.set_paused(new_paused);
                }
                // Privacy: re-gate any running VAD listener at once so the tray
                // pause tears down the mic immediately (not after the ≤2 s
                // backstop tick), AND remember/auto-rearm VAD across the pause
                // toggle. Shared helper — every pause site must route through this.
                crate::commands::audio::on_capture_pause_toggled(app, new_paused);
            }
            true
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
                crate::commands::capture_status::emit_current_capture_status(app, &state);
                if let Err(e) = crate::magic_overlay::set_tracking_panel_visible(app, new_visible) {
                    debug!("tracking panel visibility reconcile failed: {e}");
                }
                if let Err(e) = sync_tray_state(app, paused, new_visible) {
                    debug!("sync_tray_state failed: {e}");
                }
                crate::magic_overlay::sync_passive_tracking_surface(app, paused, new_visible);
                #[cfg(target_os = "macos")]
                if let Some(border) = app.try_state::<crate::native_border::NativeBorderState>() {
                    if new_visible && !crate::app_runtime_launch::cua_safe_mode_enabled() {
                        border.0.show();
                    } else {
                        border.0.hide();
                    }
                }
            }
            true
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
            true
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
            true
        }
        "automation" => {
            focus_main_window(app);
            app.emit_to("main", "navigate", "/settings/ai-automation")
                .unwrap_or_default();
            true
        }
        "run-preset" => {
            focus_main_window(app);
            app.emit_to("main", "navigate", "/automation")
                .unwrap_or_default();
            true
        }
        "approve_update" => {
            if current_tray_update_actions(app).approve_enabled {
                focus_main_window(app);
                if let Some(state) = app.try_state::<crate::runtime_state::AppState>() {
                    use maekon_web::update_control::UpdateAction;
                    if let Err(e) = state.update_action_tx.send(UpdateAction::Approve) {
                        warn!("tray: approve_update send failed: {e}");
                    }
                }
                app.emit_to("main", "tray-approve-update", ())
                    .unwrap_or_default();
            } else {
                debug!("tray: approve_update ignored because no update action is available");
            }
            true
        }
        "defer_update" => {
            if current_tray_update_actions(app).defer_enabled {
                focus_main_window(app);
                if let Some(state) = app.try_state::<crate::runtime_state::AppState>() {
                    use maekon_web::update_control::UpdateAction;
                    if let Err(e) = state.update_action_tx.send(UpdateAction::Defer) {
                        warn!("tray: defer_update send failed: {e}");
                    }
                }
                app.emit_to("main", "tray-defer-update", ())
                    .unwrap_or_default();
            } else {
                debug!("tray: defer_update ignored because no update action is available");
            }
            true
        }
        "quit" => {
            request_tray_quit(app);
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnected_services_use_local_mode_status() {
        assert_eq!(status_text(true, false, true), "Local mode");
    }

    #[test]
    fn all_healthy_flags_render_active_not_local_mode() {
        // #8050 regression: once server/llm/cli all read healthy, the tray must
        // render "Active" — the permanent "Local mode"/disconnected state
        // (caused by dead adapter health writers) is gone. The two off-by-one
        // half-states below prove any single unhealthy flag still degrades.
        let all_healthy = debug_menu_items_snapshot(
            true,
            false,
            true,
            true,
            true,
            true,
            tray_update_actions(None),
        );
        let status = all_healthy
            .iter()
            .find(|item| item.id == "capture-status")
            .expect("capture-status item");
        assert_eq!(
            status.label, "Active",
            "all-connected flags must render Active, not Local mode (#8050)"
        );

        // Any single unhealthy flag still degrades to Local mode.
        for (srv, llm, cli) in [
            (false, true, true),
            (true, false, true),
            (true, true, false),
        ] {
            let items = debug_menu_items_snapshot(
                true,
                false,
                true,
                srv,
                llm,
                cli,
                tray_update_actions(None),
            );
            let label = &items
                .iter()
                .find(|item| item.id == "capture-status")
                .expect("capture-status item")
                .label;
            assert_eq!(
                label, "Local mode",
                "one unhealthy flag (srv={srv}, llm={llm}, cli={cli}) must degrade to Local mode"
            );
        }
    }

    #[test]
    fn unavailable_capture_uses_disabled_truthful_state() {
        let items = debug_menu_items_snapshot(
            false,
            false,
            true,
            true,
            true,
            true,
            tray_update_actions(None),
        );
        let by_id = |id: &str| {
            items
                .iter()
                .find(|item| item.id == id)
                .unwrap_or_else(|| panic!("missing tray item {id}"))
        };

        assert_eq!(by_id("capture-status").label, "Capture unavailable");
        assert_eq!(by_id("toggle-capture").label, "Capture unavailable");
        assert!(!by_id("toggle-capture").enabled);
        assert_eq!(
            resolve_icon_state(false, false, false),
            TrayIconState::Disabled
        );
    }

    #[test]
    fn available_paused_capture_remains_resumable() {
        let items = debug_menu_items_snapshot(
            true,
            true,
            true,
            true,
            true,
            true,
            tray_update_actions(None),
        );
        let by_id = |id: &str| {
            items
                .iter()
                .find(|item| item.id == id)
                .unwrap_or_else(|| panic!("missing tray item {id}"))
        };

        assert_eq!(by_id("capture-status").label, "Paused");
        assert_eq!(by_id("toggle-capture").label, "Resume Capture");
        assert!(by_id("toggle-capture").enabled);
        assert_eq!(resolve_icon_state(true, true, false), TrayIconState::Paused);
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
            &maekon_web::update_control::UpdatePhase::PendingApproval,
        ));

        assert_eq!(actions.approve_label, "Download Update");
        assert!(actions.approve_enabled);
        assert!(actions.defer_enabled);
    }

    #[test]
    fn update_actions_describe_ready_to_install_phase() {
        let actions = tray_update_actions(Some(
            &maekon_web::update_control::UpdatePhase::ReadyToInstall,
        ));

        assert_eq!(actions.approve_label, "Install Update");
        assert!(actions.approve_enabled);
        assert!(actions.defer_enabled);
    }

    #[test]
    fn debug_menu_snapshot_lists_current_tray_items() {
        let items = debug_menu_items_snapshot(
            true,
            false,
            true,
            true,
            false,
            true,
            tray_update_actions(None),
        );

        let by_id = |id: &str| {
            items
                .iter()
                .find(|item| item.id == id)
                .unwrap_or_else(|| panic!("missing tray item {id}"))
        };

        assert_eq!(by_id("capture-status").label, "Local mode");
        assert!(!by_id("capture-status").enabled);
        assert_eq!(by_id("toggle-capture").label, "Pause Capture");
        assert!(by_id("toggle-capture").enabled);
        assert_eq!(by_id("toggle-indicator").label, "Hide Indicator");
        assert!(by_id("toggle-indicator").enabled);
        assert_eq!(by_id("show").label, dashboard_toggle_label());
        assert_eq!(by_id("settings").label, "Settings");
        assert_eq!(by_id("approve_update").label, "No Update Available");
        assert!(!by_id("approve_update").enabled);
        assert_eq!(by_id("quit").label, "Quit");
    }

    #[test]
    fn native_tray_menu_stays_minimal_control_surface() {
        let items = debug_menu_items_snapshot(
            true,
            false,
            true,
            true,
            true,
            true,
            tray_update_actions(None),
        );
        let labels = items
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>();

        assert!(labels.contains(&"Pause Capture"));
        assert!(labels.contains(&"Hide Indicator"));
        assert!(labels.contains(&"Show/Hide Dashboard"));
        assert!(labels.contains(&"Settings"));
        assert!(labels.contains(&"Quit"));

        for rich_surface_label in [
            "Suggestion queue",
            "Current GUI context",
            "External egress blocked",
            "Audit ready",
            "Capture now",
        ] {
            assert!(
                !labels.contains(&rich_surface_label),
                "native tray menu must not duplicate rich tray dashboard content: {rich_surface_label}"
            );
        }
    }

    #[test]
    fn tray_icon_visual_catalog_has_distinct_state_fingerprints() {
        let catalog = tray_icon_visual_catalog();

        assert_eq!(catalog.len(), 3);
        assert_eq!(catalog[0].variant, "active");
        assert_eq!(catalog[1].variant, "paused");
        assert_eq!(catalog[2].variant, "disabled");

        for snapshot in &catalog {
            println!(
                "tray_icon_visual_catalog variant={} size={}x{} rgba_len={} sha256={} template_icon={}",
                snapshot.variant,
                snapshot.width,
                snapshot.height,
                snapshot.rgba_len,
                snapshot.rgba_sha256,
                snapshot.template_icon
            );
            assert_eq!(snapshot.width, 44);
            assert_eq!(snapshot.height, 44);
            assert_eq!(snapshot.rgba_len, 44 * 44 * 4);
            assert!(snapshot.template_icon);
            assert_eq!(snapshot.rgba_sha256.len(), 64);
        }

        assert_ne!(catalog[0].rgba_sha256, catalog[1].rgba_sha256);
        assert_ne!(catalog[0].rgba_sha256, catalog[2].rgba_sha256);
        assert_ne!(catalog[1].rgba_sha256, catalog[2].rgba_sha256);
    }

    #[test]
    fn tray_icon_visual_snapshot_matches_resolved_state() {
        let active = tray_icon_visual_snapshot(resolve_icon_state(true, false, false));
        let paused = tray_icon_visual_snapshot(resolve_icon_state(true, true, false));
        let disabled = tray_icon_visual_snapshot(resolve_icon_state(false, false, false));

        assert_eq!(active.variant, "active");
        assert_eq!(paused.variant, "paused");
        assert_eq!(disabled.variant, "disabled");
        assert_ne!(active.rgba_sha256, paused.rgba_sha256);
        assert_ne!(active.rgba_sha256, disabled.rgba_sha256);
    }

    #[test]
    fn tray_quit_debug_ipc_is_blocked_without_debug_or_env_flag() {
        assert!(
            !tray_quit_debug_ipc_allowed(false, None),
            "debug IPC quit must stay blocked outside debug/test mode unless explicitly enabled"
        );
    }

    #[test]
    fn tray_quit_debug_ipc_is_allowed_in_debug_mode() {
        assert!(
            tray_quit_debug_ipc_allowed(true, None),
            "debug IPC quit should be available in debug/test builds for cleanup TC execution"
        );
    }

    #[test]
    fn tray_quit_debug_ipc_can_be_enabled_by_env_flag() {
        assert!(tray_quit_debug_ipc_allowed(false, Some("1")));
        assert!(tray_quit_debug_ipc_allowed(false, Some("true")));
        assert!(tray_quit_debug_ipc_allowed(false, Some("yes")));
    }
}
