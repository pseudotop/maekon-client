use serde::Serialize;
use tauri::command;

use crate::ipc_error::IpcError;
use crate::runtime_state::AppState;

#[derive(Serialize)]
pub struct FocusModeResponse {
    pub active: bool,
    pub auto: bool,
    pub remaining_minutes: Option<u32>,
    pub activated_at: Option<String>,
}

#[command]
pub async fn toggle_focus_mode(
    state: tauri::State<'_, AppState>,
    active: bool,
    duration_minutes: Option<u32>,
) -> Result<FocusModeResponse, IpcError> {
    if active {
        state
            .focus_mode
            .activate(duration_minutes.unwrap_or(0), false);
        if let Some(ref overlay) = state.magic_overlay {
            overlay.emit_focus_mode(true, false);
        }
    } else {
        state.focus_mode.deactivate();
        if let Some(ref overlay) = state.magic_overlay {
            overlay.emit_focus_mode(false, false);
        }
    }
    get_focus_mode_status(state).await
}

#[command]
pub async fn get_focus_mode_status(
    state: tauri::State<'_, AppState>,
) -> Result<FocusModeResponse, IpcError> {
    Ok(FocusModeResponse {
        active: state.focus_mode.is_active(),
        auto: state.focus_mode.is_auto_activated(),
        remaining_minutes: state.focus_mode.remaining_minutes(),
        activated_at: state.focus_mode.activated_at().map(|t| t.to_rfc3339()),
    })
}
