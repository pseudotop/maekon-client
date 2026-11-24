//! Capture status, connection status, panel position, and window management commands.
//!
//! ADR-013 split: 1,129-line `capture_status.rs` → focused submodules.
//!
//! | Submodule | Commands |
//! |---------|---------|
//! | `mod.rs` | `get_capture_status`, `toggle_capture_pause`, `set_indicator_visible`, `get_connection_status`, `save_panel_position`, `get_panel_position` |
//! | `window_commands` | `show_main_window`, `debug_focus_window`, `debug_window_state`, `debug_set_window_fullscreen`, `debug_set_window_bounds`, `debug_normalize_main_window_state`, `debug_normalize_main_window_bounds`, `debug_place_overlay_for_window`, `open_devtools` |
//! | `position` | Panel position validation, monitor bounds, coordinate helpers |
//! | `types` | Response DTOs and `debug_window_state_response` |

pub mod window_commands;

mod position;
mod types;

use types::{
    CaptureStatusResponse, ConnectionStatusResponse, DebugWindowFocusResponse,
    DebugWindowNormalizationResponse, DebugWindowStateResponse,
};

use std::sync::atomic::Ordering;
use tauri::{command, AppHandle, Emitter, Runtime, State};
use tauri_plugin_global_shortcut::ShortcutState;
use tracing::debug;

use crate::ipc_error::IpcError;
use crate::runtime_state::AppState;

use position::{is_position_valid, parse_position, MonitorBounds};
// ---------------------------------------------------------------------------
// Capture / connection status commands
// ---------------------------------------------------------------------------

fn capture_status_response(
    state: &AppState,
    paused: bool,
    indicator_visible: bool,
) -> CaptureStatusResponse {
    let consent = maekon_core::ports::consent_manager::ConsentGate::from_ref(
        state.capture.consent_manager.as_ref(),
    )
    .permissions_snapshot();
    CaptureStatusResponse {
        paused,
        indicator_visible,
        consent_granted: consent.screen_capture,
        permitted: crate::scheduler::capture_permitted_now(&state.config, &consent, paused),
    }
}

fn emit_capture_status_payload<R: Runtime>(app: &AppHandle<R>, payload: CaptureStatusResponse) {
    if let Err(error) = app.emit_to("magic-overlay", "overlay:capture-state-changed", payload) {
        debug!("emit magic-overlay failed: {error}");
    }
    if let Err(error) = app.emit_to("tracking-panel", "overlay:capture-state-changed", payload) {
        debug!("emit tracking-panel failed: {error}");
    }
}

pub(crate) fn emit_current_capture_status<R: Runtime>(app: &AppHandle<R>, state: &AppState) {
    let paused = state.capture_paused.load(Ordering::Relaxed);
    let indicator_visible = state.indicator_visible.load(Ordering::Relaxed);
    emit_capture_status_payload(
        app,
        capture_status_response(state, paused, indicator_visible),
    );
}

#[command]
pub async fn show_main_window(app: AppHandle) -> Result<(), IpcError> {
    window_commands::show_main_window(app).await
}

#[command]
pub async fn debug_focus_window(
    app: AppHandle,
    label: String,
) -> Result<DebugWindowFocusResponse, IpcError> {
    window_commands::debug_focus_window(app, label).await
}

#[command]
pub async fn debug_window_state(
    app: AppHandle,
    label: String,
) -> Result<DebugWindowStateResponse, IpcError> {
    window_commands::debug_window_state(app, label).await
}

#[command]
pub async fn debug_set_window_bounds(
    app: AppHandle,
    label: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<DebugWindowStateResponse, IpcError> {
    window_commands::debug_set_window_bounds(app, label, x, y, width, height).await
}

#[command]
pub async fn debug_place_overlay_for_window(
    app: AppHandle,
    target_label: String,
    interactive: bool,
) -> Result<DebugWindowStateResponse, IpcError> {
    window_commands::debug_place_overlay_for_window(app, target_label, interactive).await
}

#[command]
pub async fn debug_normalize_main_window_state(
    app: AppHandle,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<DebugWindowNormalizationResponse, IpcError> {
    window_commands::debug_normalize_main_window_state(app, x, y, width, height).await
}

#[command]
pub async fn debug_normalize_main_window_bounds(
    app: AppHandle,
) -> Result<DebugWindowStateResponse, IpcError> {
    window_commands::debug_normalize_main_window_bounds(app).await
}

#[command]
pub async fn debug_set_window_fullscreen(
    app: AppHandle,
    label: String,
    fullscreen: bool,
) -> Result<DebugWindowStateResponse, IpcError> {
    window_commands::debug_set_window_fullscreen(app, label, fullscreen).await
}

#[command]
pub async fn open_devtools(app: AppHandle, label: Option<String>) -> Result<(), IpcError> {
    window_commands::open_devtools(app, label).await
}

#[command]
pub async fn get_capture_status(
    state: State<'_, AppState>,
) -> Result<CaptureStatusResponse, IpcError> {
    let paused = state.capture_paused.load(Ordering::Relaxed);
    let indicator_visible = state.indicator_visible.load(Ordering::Relaxed);
    Ok(capture_status_response(&state, paused, indicator_visible))
}

/// Actual route decisions live with the shared transition, not in UI adapters.
#[derive(Clone, Copy)]
pub(crate) enum CapturePauseRequest {
    Ipc,
    Shortcut(ShortcutState),
    Tray,
}

pub(crate) fn apply_capture_pause_request(
    state: &AppState,
    request: CapturePauseRequest,
) -> Result<Option<bool>, IpcError> {
    match request {
        CapturePauseRequest::Shortcut(phase) if phase != ShortcutState::Pressed => Ok(None),
        CapturePauseRequest::Tray
            if !crate::magic_overlay::effective_capture_permitted(state, false) =>
        {
            Ok(None)
        }
        _ => state
            .capture_pause_state
            .toggle()
            .map(Some)
            .map_err(IpcError::from),
    }
}

#[command]
pub async fn toggle_capture_pause(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<CaptureStatusResponse, IpcError> {
    let new_paused =
        apply_capture_pause_request(&state, CapturePauseRequest::Ipc)?.ok_or_else(|| {
            IpcError::from(maekon_core::ports::vision::capture_denied(
                "IPC pause transition unavailable",
            ))
        })?;
    let indicator_visible = state.indicator_visible.load(Ordering::Relaxed);

    let payload = capture_status_response(&state, new_paused, indicator_visible);
    emit_capture_status_payload(&app, payload);
    if let Err(e) = crate::tray::sync_tray_state(&app, new_paused, indicator_visible) {
        debug!("sync_tray_state failed: {e}");
    }
    crate::magic_overlay::sync_passive_tracking_surface(&app, new_paused, indicator_visible);

    // Privacy: re-gate any running VAD listener at once so the Privacy page /
    // tracking-panel pause button tears down the mic immediately (not after the
    // ≤2 s backstop tick), AND remember/auto-rearm VAD across the pause toggle.
    // Shared helper — every pause site must route through this.
    crate::commands::audio::on_capture_pause_toggled(&app, new_paused);

    Ok(payload)
}

#[command]
pub async fn set_indicator_visible(
    app: AppHandle,
    state: State<'_, AppState>,
    visible: bool,
) -> Result<(), IpcError> {
    state.indicator_visible.store(visible, Ordering::Relaxed);
    let paused = state.capture_paused.load(Ordering::Relaxed);

    emit_capture_status_payload(&app, capture_status_response(&state, paused, visible));

    if let Err(e) = crate::magic_overlay::set_tracking_panel_visible(&app, visible) {
        debug!("tracking panel visibility reconcile failed: {e}");
    }
    if let Err(e) = crate::tray::sync_tray_state(&app, paused, visible) {
        debug!("sync_tray_state failed: {e}");
    }
    crate::magic_overlay::sync_passive_tracking_surface(&app, paused, visible);

    Ok(())
}

#[command]
pub async fn get_connection_status(
    state: State<'_, AppState>,
) -> Result<ConnectionStatusResponse, IpcError> {
    Ok(ConnectionStatusResponse {
        server: state.connection.server_connected.load(Ordering::Relaxed),
        llm: state.connection.llm_connected.load(Ordering::Relaxed),
        cli: state.connection.cli_connected.load(Ordering::Relaxed),
    })
}

// ---------------------------------------------------------------------------
// Panel position commands
// ---------------------------------------------------------------------------

/// F-RR-06: set_meta acquires a parking_lot write lock (blocking).
/// Move it off the tokio reactor thread via spawn_blocking.
#[command]
pub async fn save_panel_position(
    state: State<'_, AppState>,
    x: f64,
    y: f64,
) -> Result<(), IpcError> {
    let storage = state.storage.clone();
    let pos = format!("{x},{y}");
    tokio::task::spawn_blocking(move || {
        storage.set_meta("tracking_panel_position", &pos);
    })
    .await
    .map_err(|join_err| {
        IpcError::new(
            "internal.generic",
            format!("save_panel_position task join failed: {join_err}"),
        )
    })
}

/// F-RR-06: get_meta acquires a parking_lot read lock (blocking).
/// Move it off the tokio reactor thread via spawn_blocking.
#[command]
pub async fn get_panel_position(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<String>, IpcError> {
    let storage = state.storage.clone();
    let raw_opt: Option<String> =
        tokio::task::spawn_blocking(move || storage.get_meta("tracking_panel_position"))
            .await
            .map_err(|join_err| {
                IpcError::new(
                    "internal.generic",
                    format!("get_panel_position task join failed: {join_err}"),
                )
            })?;
    let raw: String = match raw_opt {
        Some(v) => v,
        None => return Ok(None),
    };

    let (x, y) = match parse_position(&raw) {
        Some(pos) => pos,
        None => {
            debug!("Saved panel position is malformed: {raw:?}, resetting to default");
            return Ok(None);
        }
    };

    let monitors: Vec<MonitorBounds> = app
        .available_monitors()
        .unwrap_or_else(|e| {
            debug!("Failed to query monitors: {e}");
            Vec::new()
        })
        .iter()
        .map(|m| MonitorBounds {
            x: m.position().x as f64,
            y: m.position().y as f64,
            width: m.size().width as f64,
            height: m.size().height as f64,
        })
        .collect();

    if is_position_valid(x, y, &monitors) {
        Ok(Some(raw))
    } else {
        debug!("Saved panel position ({x},{y}) is off-screen, resetting to default");
        Ok(None)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::{
        position::{
            is_position_valid, parse_position, resolve_point_monitor_index,
            resolve_window_monitor_index, MonitorBounds,
        },
        types::CaptureStatusResponse,
    };

    use super::{apply_capture_pause_request, CapturePauseRequest};
    use crate::runtime_state::{AppState, CaptureContext, ConnectionStatus};
    use maekon_core::consent::{ConsentManager, ConsentPermissions};
    use maekon_core::ports::vision::CapturePauseState;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tauri_plugin_global_shortcut::ShortcutState;

    pub(crate) struct PauseFixture {
        pub(crate) state: AppState,
        _runtime: tokio::runtime::Runtime,
        _dir: tempfile::TempDir,
    }

    impl PauseFixture {
        pub(crate) fn new(granted: bool) -> Self {
            let runtime = tokio::runtime::Runtime::new().unwrap();
            let dir = tempfile::tempdir().unwrap();
            let storage = Arc::new(
                maekon_storage::sqlite::SqliteStorage::open(&dir.path().join("pause.db"), 1, None)
                    .unwrap(),
            );
            let consent = Arc::new(ConsentManager::new(dir.path().join("consent.json")));
            if granted {
                consent
                    .grant_consent(
                        ConsentPermissions {
                            screen_capture: true,
                            ..Default::default()
                        },
                        30,
                    )
                    .unwrap();
            }
            let paused = Arc::new(AtomicBool::new(false));
            let pause = Arc::new(CapturePauseState::new(paused.clone()));
            let (update_action_tx, _) = tokio::sync::mpsc::unbounded_channel();
            let (shutdown_tx, _) = tokio::sync::watch::channel(false);
            let state = AppState {
                runtime_handle: runtime.handle().clone(),
                background_runtime: crate::bootstrap_runtime::spawn_background_runtime().unwrap(),
                config: maekon_core::config::AppConfig::default_config(),
                storage,
                update_control: None,
                update_action_tx,
                shutdown_tx,
                recluster_requested: Arc::new(AtomicBool::new(false)),
                magic_overlay: None,
                coaching_engine: None,
                capture_paused: paused,
                capture_pause_state: pause,
                indicator_visible: Arc::new(AtomicBool::new(true)),
                connection: ConnectionStatus {
                    server_connected: Arc::new(AtomicBool::new(false)),
                    llm_connected: Arc::new(AtomicBool::new(false)),
                    cli_connected: Arc::new(AtomicBool::new(false)),
                },
                focus_mode: Arc::new(crate::focus_mode::FocusModeState::new()),
                focus_analyzer: None,
                capture: CaptureContext {
                    frame_processor: None,
                    frame_storage: None,
                    activity_monitor: None,
                    accessibility_extractor: None,
                    consent_manager: Some(consent),
                    work_classifier: None,
                },
                analysis_health: None,
                regime_storage: None,
                regime_manager_snapshot: None,
            };
            Self {
                state,
                _runtime: runtime,
                _dir: dir,
            }
        }

        pub(crate) fn scheduler(&self) -> crate::scheduler::Scheduler {
            let process = Arc::new(maekon_monitor::process::ProcessTracker::new());
            crate::scheduler::Scheduler::new(
                crate::scheduler::SchedulerConfig::default(),
                Arc::new(maekon_monitor::system::SysInfoMonitor::new()),
                Arc::new(maekon_monitor::activity::ActivityTracker::new(
                    process.clone(),
                )),
                process,
                Arc::new(maekon_vision::trigger::SmartCaptureTrigger::new(100)),
                Arc::new(maekon_vision::processor::EdgeFrameProcessor::new(
                    1, 1, None,
                )),
                self.state.storage.clone(),
                self.state.storage.clone(),
                None,
                None,
            )
            .with_capture_paused(
                self.state.capture_paused.clone(),
                self.state.capture_pause_state.clone(),
            )
        }
    }

    #[test]
    fn pause_routes_preserve_pressed_filter_and_real_tray_availability() {
        let fixture = PauseFixture::new(true);
        let state = &fixture.state;
        let origin = state.capture_pause_state.snapshot().unwrap();
        assert_eq!(
            apply_capture_pause_request(
                state,
                CapturePauseRequest::Shortcut(ShortcutState::Released)
            )
            .unwrap(),
            None
        );
        assert_eq!(state.capture_pause_state.snapshot().unwrap(), origin);
        for request in [
            CapturePauseRequest::Ipc,
            CapturePauseRequest::Shortcut(ShortcutState::Pressed),
            CapturePauseRequest::Tray,
        ] {
            let before = state.capture_pause_state.snapshot().unwrap();
            assert_eq!(
                apply_capture_pause_request(state, request).unwrap(),
                Some(true)
            );
            assert!(state.capture_paused.load(Ordering::Acquire));
            assert_eq!(
                apply_capture_pause_request(state, request).unwrap(),
                Some(false)
            );
            assert!(!state.capture_paused.load(Ordering::Acquire));
            assert_ne!(state.capture_pause_state.snapshot().unwrap(), before);
        }
        let mut blocked = PauseFixture::new(false);
        let before = blocked.state.capture_pause_state.snapshot().unwrap();
        assert_eq!(
            apply_capture_pause_request(&blocked.state, CapturePauseRequest::Tray).unwrap(),
            None
        );
        assert_eq!(
            blocked.state.capture_pause_state.snapshot().unwrap(),
            before
        );
        assert!(!blocked.state.capture_paused.load(Ordering::Acquire));
        blocked
            .state
            .capture
            .consent_manager
            .as_ref()
            .unwrap()
            .grant_consent(
                ConsentPermissions {
                    screen_capture: true,
                    ..Default::default()
                },
                30,
            )
            .unwrap();
        blocked.state.config.vision.capture_enabled = false;
        assert_eq!(
            apply_capture_pause_request(&blocked.state, CapturePauseRequest::Tray).unwrap(),
            None
        );
        assert_eq!(
            blocked.state.capture_pause_state.snapshot().unwrap(),
            before
        );
        // IPC/shortcut retain their existing ability to change pause while capture is disabled.
        assert_eq!(
            apply_capture_pause_request(&blocked.state, CapturePauseRequest::Ipc).unwrap(),
            Some(true)
        );
        assert_eq!(
            apply_capture_pause_request(
                &blocked.state,
                CapturePauseRequest::Shortcut(ShortcutState::Pressed)
            )
            .unwrap(),
            Some(false)
        );
    }

    #[test]
    fn capture_status_response_serializes_effective_gate_fields() {
        let response = CaptureStatusResponse {
            paused: false,
            indicator_visible: true,
            consent_granted: false,
            permitted: false,
        };
        let value = serde_json::to_value(response).expect("serialize capture status");

        assert_eq!(value["paused"], false);
        assert_eq!(value["indicator_visible"], true);
        assert_eq!(value["consent_granted"], false);
        assert_eq!(value["permitted"], false);
    }

    #[test]
    fn native_capture_state_entrypoints_use_typed_event_chokepoint() {
        for (entrypoint, source) in [
            ("global shortcut", include_str!("../../setup/shortcuts.rs")),
            ("tray", include_str!("../../tray.rs")),
        ] {
            assert!(
                source.contains("commands::capture_status::emit_current_capture_status"),
                "{entrypoint} must emit the canonical CaptureStatusResponse"
            );
            assert!(
                !source.contains("\"overlay:capture-state-changed\""),
                "{entrypoint} must not hand-roll a partial capture-state event"
            );
        }
    }

    // --- parse_position tests ---

    #[test]
    fn test_parse_valid() {
        assert_eq!(parse_position("100.5,200.0"), Some((100.5, 200.0)));
    }

    #[test]
    fn test_parse_integers() {
        assert_eq!(parse_position("500,300"), Some((500.0, 300.0)));
    }

    #[test]
    fn test_parse_negative() {
        assert_eq!(parse_position("-100,50"), Some((-100.0, 50.0)));
    }

    #[test]
    fn test_parse_invalid_format() {
        assert_eq!(parse_position("not_a_number"), None);
    }

    #[test]
    fn test_parse_empty() {
        assert_eq!(parse_position(""), None);
    }

    #[test]
    fn test_parse_nan() {
        assert_eq!(parse_position("NaN,100"), None);
    }

    #[test]
    fn test_parse_infinity() {
        assert_eq!(parse_position("inf,100"), None);
    }

    #[test]
    fn test_parse_single_value() {
        assert_eq!(parse_position("100"), None);
    }

    // --- is_position_valid tests ---

    fn single_monitor() -> Vec<MonitorBounds> {
        vec![MonitorBounds {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        }]
    }

    #[test]
    fn test_within_single_monitor() {
        assert!(is_position_valid(500.0, 300.0, &single_monitor()));
    }

    #[test]
    fn test_outside_all_monitors() {
        assert!(!is_position_valid(5000.0, 5000.0, &single_monitor()));
    }

    #[test]
    fn test_right_edge_within_margin() {
        // Right edge: x = mon_w - MARGIN = 1920 - 100 = 1820
        assert!(is_position_valid(1820.0, 300.0, &single_monitor()));
    }

    #[test]
    fn test_right_edge_beyond_margin() {
        // Right edge: x = mon_w - MARGIN + 1 = 1821
        assert!(!is_position_valid(1821.0, 300.0, &single_monitor()));
    }

    #[test]
    fn test_left_edge_within_margin() {
        // Left bound: x >= (0 - 260 + 100) = -160
        assert!(is_position_valid(-160.0, 300.0, &single_monitor()));
    }

    #[test]
    fn test_left_edge_beyond_margin() {
        // Left bound: x >= -160, so -161 is out
        assert!(!is_position_valid(-161.0, 300.0, &single_monitor()));
    }

    #[test]
    fn test_top_edge_exact() {
        // y = mon_y = 0 (exactly at top)
        assert!(is_position_valid(500.0, 0.0, &single_monitor()));
    }

    #[test]
    fn test_above_top_edge() {
        // y = -1 (above monitor top)
        assert!(!is_position_valid(500.0, -1.0, &single_monitor()));
    }

    #[test]
    fn test_multi_monitor_secondary() {
        let monitors = vec![
            MonitorBounds {
                x: 0.0,
                y: 0.0,
                width: 1920.0,
                height: 1080.0,
            },
            MonitorBounds {
                x: 1920.0,
                y: 0.0,
                width: 1920.0,
                height: 1080.0,
            },
        ];
        assert!(is_position_valid(2500.0, 300.0, &monitors));
    }

    #[test]
    fn test_negative_monitor_coords() {
        let monitors = vec![
            MonitorBounds {
                x: 0.0,
                y: 0.0,
                width: 1920.0,
                height: 1080.0,
            },
            MonitorBounds {
                x: -1920.0,
                y: 0.0,
                width: 1920.0,
                height: 1080.0,
            },
        ];
        assert!(is_position_valid(-1000.0, 100.0, &monitors));
    }

    #[test]
    fn test_empty_monitors() {
        assert!(!is_position_valid(500.0, 300.0, &[]));
    }

    #[test]
    fn debug_window_state_monitor_resolution_selects_secondary_by_center() {
        let monitors = vec![
            MonitorBounds {
                x: 0.0,
                y: 0.0,
                width: 1920.0,
                height: 1080.0,
            },
            MonitorBounds {
                x: 1920.0,
                y: 0.0,
                width: 1920.0,
                height: 1080.0,
            },
        ];

        let selected = resolve_window_monitor_index(2100.0, 120.0, 800.0, 600.0, &monitors);

        assert_eq!(selected, Some(1));
    }

    #[test]
    fn debug_window_state_cursor_resolution_selects_secondary_point() {
        let monitors = vec![
            MonitorBounds {
                x: 0.0,
                y: 0.0,
                width: 1920.0,
                height: 1080.0,
            },
            MonitorBounds {
                x: 1920.0,
                y: 0.0,
                width: 1920.0,
                height: 1080.0,
            },
        ];

        let selected = resolve_point_monitor_index(2400.0, 500.0, &monitors);

        assert_eq!(selected, Some(1));
    }
}
