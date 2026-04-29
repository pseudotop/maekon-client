use tauri::{command, AppHandle, Runtime};

use crate::desktop_permissions::{
    get_desktop_permission_snapshot,
    open_desktop_permission_settings as open_desktop_permission_settings_impl,
    request_desktop_notification_permission as request_notification_permission_snapshot,
    DesktopPermissionSnapshot,
};
use crate::ipc_error::IpcError;

/// Wrap a desktop-permissions helper's String error into the canonical
/// PermissionCode::PermissionDenied wire code. The upstream helpers return
/// `Result<_, String>` for historical reasons; we preserve that surface but
/// promote the error at the IPC boundary so the frontend sees a typed code.
fn permission_error(msg: String) -> IpcError {
    IpcError::new("permission.permission_denied", msg)
}

#[command]
pub async fn get_desktop_permission_status<R: Runtime>(
    app: AppHandle<R>,
) -> Result<DesktopPermissionSnapshot, IpcError> {
    Ok(get_desktop_permission_snapshot(&app))
}

#[command]
pub async fn request_desktop_notification_permission<R: Runtime>(
    app: AppHandle<R>,
) -> Result<DesktopPermissionSnapshot, IpcError> {
    request_notification_permission_snapshot(&app).map_err(permission_error)
}

#[command]
pub async fn open_desktop_permission_settings(permission_kind: String) -> Result<(), IpcError> {
    open_desktop_permission_settings_impl(&permission_kind).map_err(permission_error)
}
