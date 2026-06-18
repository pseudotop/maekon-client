use tauri::{command, AppHandle, Runtime};

use crate::desktop_permissions::{
    get_desktop_permission_snapshot,
    open_desktop_permission_settings as open_desktop_permission_settings_impl,
    request_desktop_notification_permission as request_notification_permission_snapshot,
    request_desktop_screen_capture_permission as request_screen_capture_permission_snapshot,
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

/// Wrap a `spawn_blocking` JoinError (task panic / cancel) into a typed IPC error.
fn probe_task_error(err: tokio::task::JoinError) -> IpcError {
    IpcError::new(
        "internal.generic",
        format!("desktop permission probe task failed: {err}"),
    )
}

#[command]
pub async fn get_desktop_permission_status<R: Runtime>(
    app: AppHandle<R>,
) -> Result<DesktopPermissionSnapshot, IpcError> {
    // #6266: the macOS probe blocks on `recv_timeout` (desktop_permissions.rs)
    // waiting for the OS dialog/TCC response; run it off the tokio reactor so a
    // pending dialog cannot stall an async worker thread.
    tokio::task::spawn_blocking(move || get_desktop_permission_snapshot(&app))
        .await
        .map_err(probe_task_error)
}

#[command]
pub async fn request_desktop_notification_permission<R: Runtime>(
    app: AppHandle<R>,
) -> Result<DesktopPermissionSnapshot, IpcError> {
    // #6266: offload the blocking permission-request probe (see above).
    tokio::task::spawn_blocking(move || request_notification_permission_snapshot(&app))
        .await
        .map_err(probe_task_error)?
        .map_err(permission_error)
}

#[command]
pub async fn request_desktop_screen_capture_permission<R: Runtime>(
    app: AppHandle<R>,
) -> Result<DesktopPermissionSnapshot, IpcError> {
    // #6266: offload the blocking permission-request probe (see above).
    tokio::task::spawn_blocking(move || request_screen_capture_permission_snapshot(&app))
        .await
        .map_err(probe_task_error)?
        .map_err(permission_error)
}

#[command]
pub async fn open_desktop_permission_settings(permission_kind: String) -> Result<(), IpcError> {
    open_desktop_permission_settings_impl(&permission_kind).map_err(permission_error)
}
