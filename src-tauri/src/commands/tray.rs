use tauri::{command, AppHandle};

use crate::ipc_error::IpcError;
use crate::tray::{TrayActionOutcome, TrayDebugState, TrayGeometrySnapshot};

#[cfg(not(debug_assertions))]
fn debug_tray_disabled() -> IpcError {
    IpcError::new(
        "service.unavailable",
        "tray debug IPC is only available in debug builds",
    )
}

#[command]
pub async fn get_tray_state(app: AppHandle) -> Result<TrayDebugState, IpcError> {
    #[cfg(debug_assertions)]
    {
        Ok(crate::tray::debug_tray_state(&app))
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = app;
        Err(debug_tray_disabled())
    }
}

#[command]
pub async fn get_tray_geometry(_app: AppHandle) -> Result<TrayGeometrySnapshot, IpcError> {
    #[cfg(debug_assertions)]
    {
        Ok(crate::tray::tray_geometry_state())
    }
    #[cfg(not(debug_assertions))]
    {
        Err(debug_tray_disabled())
    }
}

/// #9634: production quit for UI surfaces — NOT debug-gated. The tracking
/// panel's quit button must work in release builds; `simulate_tray_action`
/// below stays QC-only.
#[command]
pub async fn request_app_quit(app: AppHandle) -> Result<(), IpcError> {
    crate::tray::request_app_quit(&app);
    Ok(())
}

#[command]
pub async fn simulate_tray_action(
    app: AppHandle,
    action: String,
) -> Result<TrayActionOutcome, IpcError> {
    #[cfg(debug_assertions)]
    {
        crate::tray::simulate_tray_action(&app, &action)
            .map_err(|message| IpcError::new("validation.invalid_arguments", message))
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = (app, action);
        Err(debug_tray_disabled())
    }
}
