use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::OnceLock;
use tauri::{Monitor, PhysicalPosition, PhysicalSize, WebviewWindow, Window};
use tracing::{debug, warn};

const MAIN_WINDOW_LABEL: &str = "main";
const WINDOW_STATE_FILE: &str = "window-state-main.json";
const MIN_RESTORABLE_WIDTH: u32 = 400;
const MIN_RESTORABLE_HEIGHT: u32 = 300;

/// Background writer for main-window geometry persistence.
///
/// Moved/Resized fire many times per second while dragging, so doing a
/// synchronous `fs::write` inline on the Tauri event thread (the main UI
/// thread) blocks it on disk I/O for every event. Instead we compute the
/// geometry on the event thread (cheap window queries) and hand the latest
/// `(path, state)` to a dedicated background thread over a channel. The
/// writer coalesces by draining the channel and only writing the most
/// recent state, so a burst of move/resize events collapses to a single
/// disk write.
static STATE_WRITER: OnceLock<Sender<(PathBuf, MainWindowState)>> = OnceLock::new();

fn state_writer() -> &'static Sender<(PathBuf, MainWindowState)> {
    STATE_WRITER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<(PathBuf, MainWindowState)>();
        std::thread::Builder::new()
            .name("window-state-writer".into())
            .spawn(move || {
                // Block on the first message, then coalesce any others already
                // queued so only the newest geometry is written to disk.
                while let Ok(mut latest) = rx.recv() {
                    while let Ok(next) = rx.try_recv() {
                        latest = next;
                    }
                    let (path, state) = latest;
                    match save_state_to_path(&path, state) {
                        Ok(()) => debug!(
                            path = %path.display(),
                            x = state.x,
                            y = state.y,
                            width = state.width,
                            height = state.height,
                            "main window state persisted"
                        ),
                        Err(error) => {
                            debug!(error = %error, "main window state persist skipped")
                        }
                    }
                }
            })
            .expect("spawn window-state-writer thread");
        tx
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct MonitorBounds {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MainWindowState {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

impl MainWindowState {
    fn is_restorable(self) -> bool {
        self.width >= MIN_RESTORABLE_WIDTH && self.height >= MIN_RESTORABLE_HEIGHT
    }
}

pub(crate) fn persist_main_window_state(window: &Window) {
    if window.label() != MAIN_WINDOW_LABEL {
        return;
    }

    // Compute geometry + path on the event thread (cheap window queries), then
    // hand the disk write to the background writer so the main UI thread is
    // never blocked on `fs::write` while the window is being moved/resized.
    match state_from_window(window).and_then(|state| window_state_path().map(|path| (path, state)))
    {
        Ok((path, state)) => {
            if state_writer().send((path, state)).is_err() {
                debug!("main window state persist skipped: writer channel closed");
            }
        }
        Err(error) => debug!(error = %error, "main window state persist skipped"),
    }
}

pub(crate) fn restore_main_window_state(window: &WebviewWindow) {
    if window.label() != MAIN_WINDOW_LABEL {
        return;
    }

    match window_state_path()
        .and_then(|path| load_state_from_path(&path).map(|state| (path, state)))
    {
        Ok((path, Some(state))) => {
            let state = match normalized_state_for_webview_window(window, state) {
                Ok(normalized) => {
                    if normalized != state {
                        debug!(
                            path = %path.display(),
                            from_x = state.x,
                            from_y = state.y,
                            to_x = normalized.x,
                            to_y = normalized.y,
                            "main window state normalized to available monitor"
                        );
                    }
                    normalized
                }
                Err(error) => {
                    debug!(path = %path.display(), error = %error, "main window monitor normalization skipped");
                    state
                }
            };
            if let Err(error) = apply_state_to_webview_window(window, state) {
                warn!(
                    path = %path.display(),
                    error = %error,
                    "main window state restore failed"
                );
            } else {
                debug!(
                    path = %path.display(),
                    x = state.x,
                    y = state.y,
                    width = state.width,
                    height = state.height,
                    "main window state restored"
                );
            }
        }
        Ok((_path, None)) => {}
        Err(error) => debug!(error = %error, "main window state restore skipped"),
    }
}

pub(crate) fn ensure_main_window_on_available_monitor(window: &Window) {
    if window.label() != MAIN_WINDOW_LABEL {
        return;
    }

    let Ok(state) = state_from_window(window) else {
        return;
    };
    let Ok(normalized) = normalized_state_for_window(window, state) else {
        return;
    };
    if normalized == state {
        return;
    }
    if let Err(error) = apply_state_to_window(window, normalized) {
        debug!(error = %error, "main window monitor normalization failed");
    } else {
        debug!(
            from_x = state.x,
            from_y = state.y,
            to_x = normalized.x,
            to_y = normalized.y,
            "main window normalized to available monitor"
        );
    }
}

pub(crate) fn ensure_main_webview_window_on_available_monitor(window: &WebviewWindow) {
    if window.label() != MAIN_WINDOW_LABEL {
        return;
    }

    let Ok(state) = state_from_webview_window(window) else {
        return;
    };
    let Ok(normalized) = normalized_state_for_webview_window(window, state) else {
        return;
    };
    if normalized == state {
        return;
    }
    if let Err(error) = apply_state_to_webview_window(window, normalized) {
        debug!(error = %error, "main webview window monitor normalization failed");
    } else {
        debug!(
            from_x = state.x,
            from_y = state.y,
            to_x = normalized.x,
            to_y = normalized.y,
            "main webview window normalized to available monitor"
        );
    }
}

pub(crate) fn normalize_state_for_available_monitors(
    state: MainWindowState,
    monitors: &[MonitorBounds],
) -> MainWindowState {
    if monitors.is_empty() || !state.is_restorable() {
        return state;
    }

    let target = monitor_containing_state_center(state, monitors).unwrap_or(&monitors[0]);
    clamp_state_to_monitor(state, *target)
}

fn state_from_window(window: &Window) -> Result<MainWindowState, String> {
    let position = window
        .outer_position()
        .map_err(|error| format!("outer_position failed: {error}"))?;
    let size = window
        .inner_size()
        .map_err(|error| format!("inner_size failed: {error}"))?;
    let state = MainWindowState {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
    };

    if state.is_restorable() {
        Ok(state)
    } else {
        Err(format!(
            "window state too small to persist: {}x{}",
            state.width, state.height
        ))
    }
}

fn state_from_webview_window(window: &WebviewWindow) -> Result<MainWindowState, String> {
    let position = window
        .outer_position()
        .map_err(|error| format!("outer_position failed: {error}"))?;
    let size = window
        .inner_size()
        .map_err(|error| format!("inner_size failed: {error}"))?;
    let state = MainWindowState {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
    };

    if state.is_restorable() {
        Ok(state)
    } else {
        Err(format!(
            "window state too small to persist: {}x{}",
            state.width, state.height
        ))
    }
}

fn apply_state_to_webview_window(
    window: &WebviewWindow,
    state: MainWindowState,
) -> Result<(), String> {
    if !state.is_restorable() {
        return Ok(());
    }

    window
        .set_size(PhysicalSize::new(state.width, state.height))
        .map_err(|error| format!("set_size failed: {error}"))?;
    window
        .set_position(PhysicalPosition::new(state.x, state.y))
        .map_err(|error| format!("set_position failed: {error}"))?;
    Ok(())
}

fn apply_state_to_window(window: &Window, state: MainWindowState) -> Result<(), String> {
    if !state.is_restorable() {
        return Ok(());
    }

    window
        .set_size(PhysicalSize::new(state.width, state.height))
        .map_err(|error| format!("set_size failed: {error}"))?;
    window
        .set_position(PhysicalPosition::new(state.x, state.y))
        .map_err(|error| format!("set_position failed: {error}"))?;
    Ok(())
}

fn normalized_state_for_webview_window(
    window: &WebviewWindow,
    state: MainWindowState,
) -> Result<MainWindowState, String> {
    let monitors = window
        .available_monitors()
        .map_err(|error| format!("available_monitors failed: {error}"))?;
    Ok(normalize_state_for_available_monitors(
        state,
        &monitor_bounds_from(&monitors),
    ))
}

fn normalized_state_for_window(
    window: &Window,
    state: MainWindowState,
) -> Result<MainWindowState, String> {
    let monitors = window
        .available_monitors()
        .map_err(|error| format!("available_monitors failed: {error}"))?;
    Ok(normalize_state_for_available_monitors(
        state,
        &monitor_bounds_from(&monitors),
    ))
}

fn monitor_bounds_from(monitors: &[Monitor]) -> Vec<MonitorBounds> {
    monitors
        .iter()
        .map(|monitor| MonitorBounds {
            x: monitor.position().x,
            y: monitor.position().y,
            width: monitor.size().width,
            height: monitor.size().height,
        })
        .collect()
}

fn monitor_containing_state_center(
    state: MainWindowState,
    monitors: &[MonitorBounds],
) -> Option<&MonitorBounds> {
    let center_x = i64::from(state.x) + i64::from(state.width / 2);
    let center_y = i64::from(state.y) + i64::from(state.height / 2);
    monitors.iter().find(|monitor| {
        center_x >= i64::from(monitor.x)
            && center_x < i64::from(monitor.x) + i64::from(monitor.width)
            && center_y >= i64::from(monitor.y)
            && center_y < i64::from(monitor.y) + i64::from(monitor.height)
    })
}

fn clamp_state_to_monitor(state: MainWindowState, monitor: MonitorBounds) -> MainWindowState {
    let width = clamp_dimension_to_monitor(state.width, monitor.width, MIN_RESTORABLE_WIDTH);
    let height = clamp_dimension_to_monitor(state.height, monitor.height, MIN_RESTORABLE_HEIGHT);
    let x = clamp_coordinate_to_monitor(state.x, monitor.x, monitor.width, width);
    let y = clamp_coordinate_to_monitor(state.y, monitor.y, monitor.height, height);

    MainWindowState {
        x,
        y,
        width,
        height,
    }
}

fn clamp_dimension_to_monitor(value: u32, monitor_value: u32, minimum: u32) -> u32 {
    if monitor_value < minimum {
        monitor_value
    } else {
        value.clamp(minimum, monitor_value)
    }
}

fn clamp_coordinate_to_monitor(
    value: i32,
    monitor_origin: i32,
    monitor_size: u32,
    window_size: u32,
) -> i32 {
    let min = i64::from(monitor_origin);
    let max = min + i64::from(monitor_size.saturating_sub(window_size));
    let clamped = i64::from(value).clamp(min, max.max(min));
    clamped.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

fn window_state_path() -> Result<PathBuf, String> {
    maekon_core::config_manager::ConfigManager::data_dir()
        .map(|path| state_path_for_data_dir(&path))
        .map_err(|error| error.to_string())
}

fn state_path_for_data_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(WINDOW_STATE_FILE)
}

fn save_state_to_path(path: &Path, state: MainWindowState) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create state directory failed: {error}"))?;
    }
    let bytes = serde_json::to_vec_pretty(&state)
        .map_err(|error| format!("serialize window state failed: {error}"))?;
    fs::write(path, bytes).map_err(|error| format!("write window state failed: {error}"))
}

fn load_state_from_path(path: &Path) -> Result<Option<MainWindowState>, String> {
    if !path.exists() {
        return Ok(None);
    }

    let bytes = fs::read(path).map_err(|error| format!("read window state failed: {error}"))?;
    let state: MainWindowState = serde_json::from_slice(&bytes)
        .map_err(|error| format!("parse window state failed: {error}"))?;
    Ok(state.is_restorable().then_some(state))
}

#[cfg(test)]
mod tests {
    use super::{
        load_state_from_path, normalize_state_for_available_monitors, save_state_to_path,
        state_path_for_data_dir, MainWindowState, MonitorBounds,
    };

    #[test]
    fn state_path_uses_data_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = state_path_for_data_dir(dir.path());

        assert_eq!(path, dir.path().join("window-state-main.json"));
    }

    #[test]
    fn state_file_round_trips_valid_geometry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = state_path_for_data_dir(dir.path());
        let state = MainWindowState {
            x: 120,
            y: 80,
            width: 1280,
            height: 800,
        };

        save_state_to_path(&path, state).expect("save state");

        assert_eq!(
            load_state_from_path(&path).expect("load state"),
            Some(state)
        );
    }

    #[test]
    fn tiny_state_is_not_restored() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = state_path_for_data_dir(dir.path());
        let state = MainWindowState {
            x: 10,
            y: 10,
            width: 200,
            height: 100,
        };

        save_state_to_path(&path, state).expect("save state");

        assert_eq!(load_state_from_path(&path).expect("load state"), None);
    }

    #[test]
    fn removed_secondary_display_state_snaps_to_primary_monitor() {
        let state = MainWindowState {
            x: 4_080,
            y: 240,
            width: 1_280,
            height: 800,
        };
        let monitors = [MonitorBounds {
            x: 0,
            y: 0,
            width: 1_920,
            height: 1_080,
        }];

        let normalized = normalize_state_for_available_monitors(state, &monitors);

        assert_eq!(normalized.x, 640);
        assert_eq!(normalized.y, 240);
        assert_eq!(normalized.width, 1_280);
        assert_eq!(normalized.height, 800);
    }

    #[test]
    fn available_secondary_display_state_is_preserved() {
        let state = MainWindowState {
            x: 4_080,
            y: 240,
            width: 1_280,
            height: 800,
        };
        let monitors = [
            MonitorBounds {
                x: 0,
                y: 0,
                width: 1_920,
                height: 1_080,
            },
            MonitorBounds {
                x: 3_840,
                y: 0,
                width: 3_840,
                height: 2_160,
            },
        ];

        assert_eq!(
            normalize_state_for_available_monitors(state, &monitors),
            state
        );
    }

    #[test]
    fn oversized_restored_state_fits_inside_remaining_monitor() {
        let state = MainWindowState {
            x: -200,
            y: -100,
            width: 4_000,
            height: 3_000,
        };
        let monitors = [MonitorBounds {
            x: 0,
            y: 0,
            width: 1_920,
            height: 1_080,
        }];

        let normalized = normalize_state_for_available_monitors(state, &monitors);

        assert_eq!(
            normalized,
            MainWindowState {
                x: 0,
                y: 0,
                width: 1_920,
                height: 1_080,
            }
        );
    }
}
