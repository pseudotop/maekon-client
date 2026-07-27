use serde::Serialize;
use tauri::command;

use crate::ipc_error::IpcError;

#[derive(Debug, Serialize)]
pub struct GlobalShortcutStatusResponse {
    pub ok: bool,
    pub records: Vec<crate::shortcut_registry::ShortcutRegistrationRecord>,
}

/// Return the read-only registration diagnostics used by the native shortcut
/// collision TC and by any UI notice that needs to surface a fallback chord.
#[command]
pub async fn get_global_shortcut_status() -> Result<GlobalShortcutStatusResponse, IpcError> {
    Ok(GlobalShortcutStatusResponse {
        ok: true,
        records: crate::shortcut_registry::records(),
    })
}
