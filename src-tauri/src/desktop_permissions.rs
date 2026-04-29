use serde::Serialize;
use tauri::{AppHandle, Runtime};

use oneshim_core::ports::accessibility::AccessibilityExtractor;

#[cfg(target_os = "macos")]
use block2::RcBlock;
#[cfg(target_os = "macos")]
use objc2::msg_send;
#[cfg(target_os = "macos")]
use objc2::runtime::{AnyClass, AnyObject, Bool};
#[cfg(target_os = "macos")]
use std::sync::{Arc, Mutex};
#[cfg(target_os = "macos")]
use std::time::Duration;
#[cfg(target_os = "macos")]
use tracing::debug;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)] // Variants constructed per-platform; serialized to frontend via serde
pub enum DesktopPermissionState {
    Granted,
    NeedsAttention,
    NotRequired,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DesktopPermissionEntry {
    pub state: DesktopPermissionState,
    pub status_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DesktopPermissionSnapshot {
    pub platform: String,
    pub accessibility: DesktopPermissionEntry,
    pub screen_capture: DesktopPermissionEntry,
    pub notifications: DesktopPermissionEntry,
}

pub fn get_desktop_permission_snapshot<R: Runtime>(
    app_handle: &AppHandle<R>,
) -> DesktopPermissionSnapshot {
    DesktopPermissionSnapshot {
        platform: current_platform().to_string(),
        accessibility: accessibility_permission_entry(),
        screen_capture: screen_capture_permission_entry(),
        notifications: notification_permission_entry(app_handle),
    }
}

pub fn request_desktop_notification_permission<R: Runtime>(
    app_handle: &AppHandle<R>,
) -> Result<DesktopPermissionSnapshot, String> {
    #[cfg(target_os = "macos")]
    request_macos_notification_permission(app_handle)?;

    Ok(get_desktop_permission_snapshot(app_handle))
}

pub fn open_desktop_permission_settings(permission_kind: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let url = macos_permission_settings_url(permission_kind).ok_or_else(|| {
            format!("Unsupported desktop permission settings kind: {permission_kind}")
        })?;
        let status = std::process::Command::new("open")
            .arg(url)
            .status()
            .map_err(|err| format!("Failed to open macOS System Settings: {err}"))?;

        if status.success() {
            return Ok(());
        }

        Err(format!(
            "macOS System Settings returned a non-zero exit status for permission kind: {permission_kind}"
        ))
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = permission_kind;
        Err("Desktop permission shortcuts are only supported on macOS right now".to_string())
    }
}

fn current_platform() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "macos"
    }
    #[cfg(target_os = "windows")]
    {
        "windows"
    }
    #[cfg(target_os = "linux")]
    {
        "linux"
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        "unknown"
    }
}

#[cfg(target_os = "macos")]
fn granted(status_reason: Option<&str>) -> DesktopPermissionEntry {
    DesktopPermissionEntry {
        state: DesktopPermissionState::Granted,
        status_reason: status_reason.map(ToOwned::to_owned),
    }
}

#[cfg(target_os = "macos")]
fn macos_permission_settings_url(permission_kind: &str) -> Option<&'static str> {
    match permission_kind {
        "accessibility" => {
            Some("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        }
        "screen_capture" => {
            Some("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
        }
        _ => None,
    }
}

fn needs_attention(status_reason: &str) -> DesktopPermissionEntry {
    DesktopPermissionEntry {
        state: DesktopPermissionState::NeedsAttention,
        status_reason: Some(status_reason.to_string()),
    }
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
fn not_required(status_reason: Option<&str>) -> DesktopPermissionEntry {
    DesktopPermissionEntry {
        state: DesktopPermissionState::NotRequired,
        status_reason: status_reason.map(ToOwned::to_owned),
    }
}

fn unavailable(status_reason: &str) -> DesktopPermissionEntry {
    DesktopPermissionEntry {
        state: DesktopPermissionState::Unavailable,
        status_reason: Some(status_reason.to_string()),
    }
}

#[cfg(target_os = "macos")]
fn accessibility_permission_entry() -> DesktopPermissionEntry {
    let extractor = oneshim_vision::accessibility::MacOsNativeAccessibility::new();
    if extractor.has_permission() {
        granted(Some("macos_accessibility_granted"))
    } else {
        needs_attention("macos_accessibility_missing")
    }
}

#[cfg(target_os = "windows")]
fn accessibility_permission_entry() -> DesktopPermissionEntry {
    not_required(Some("windows_uia_no_permission_required"))
}

#[cfg(target_os = "linux")]
fn accessibility_permission_entry() -> DesktopPermissionEntry {
    let extractor = oneshim_vision::accessibility::LinuxAccessibility::new();
    if extractor.has_permission() {
        not_required(Some("linux_atspi_ready"))
    } else {
        needs_attention("linux_atspi_session_unavailable")
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn accessibility_permission_entry() -> DesktopPermissionEntry {
    unavailable("accessibility_unsupported")
}

#[cfg(target_os = "macos")]
fn screen_capture_permission_entry() -> DesktopPermissionEntry {
    if macos_screen_capture_access_granted() {
        granted(Some("macos_screen_capture_granted"))
    } else {
        needs_attention("macos_screen_capture_missing")
    }
}

#[cfg(not(target_os = "macos"))]
fn screen_capture_permission_entry() -> DesktopPermissionEntry {
    match oneshim_vision::capture::ScreenCapture::monitor_count() {
        Ok(count) if count > 0 => not_required(Some("screen_capture_ready")),
        Ok(_) => unavailable("screen_capture_no_monitors"),
        Err(_) => unavailable("screen_capture_probe_failed"),
    }
}

#[cfg(target_os = "macos")]
fn notification_permission_entry<R: Runtime>(app_handle: &AppHandle<R>) -> DesktopPermissionEntry {
    match macos_notification_authorization_status(app_handle) {
        Ok(0) => needs_attention("macos_notifications_not_determined"),
        Ok(1) => needs_attention("macos_notifications_denied"),
        Ok(2) => granted(Some("macos_notifications_granted")),
        Ok(3) => granted(Some("macos_notifications_provisional")),
        Ok(4) => granted(Some("macos_notifications_ephemeral")),
        Ok(_) => unavailable("macos_notifications_unknown_status"),
        Err(_) => unavailable("macos_notifications_probe_failed"),
    }
}

#[cfg(target_os = "windows")]
fn notification_permission_entry<R: Runtime>(_app_handle: &AppHandle<R>) -> DesktopPermissionEntry {
    not_required(Some("windows_notifications_managed_by_os"))
}

#[cfg(target_os = "linux")]
fn notification_permission_entry<R: Runtime>(_app_handle: &AppHandle<R>) -> DesktopPermissionEntry {
    not_required(Some("linux_notifications_managed_by_session"))
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn notification_permission_entry<R: Runtime>(_app_handle: &AppHandle<R>) -> DesktopPermissionEntry {
    unavailable("notifications_unsupported")
}

#[cfg(target_os = "macos")]
fn request_macos_notification_permission<R: Runtime>(
    app_handle: &AppHandle<R>,
) -> Result<(), String> {
    const UN_AUTHORIZATION_OPTION_BADGE: usize = 1 << 0;
    const UN_AUTHORIZATION_OPTION_SOUND: usize = 1 << 1;
    const UN_AUTHORIZATION_OPTION_ALERT: usize = 1 << 2;
    const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let tx = Arc::new(Mutex::new(Some(tx)));
    let request_sender = Arc::clone(&tx);

    app_handle
        .run_on_main_thread(move || unsafe {
            let Some(notification_center_class) = AnyClass::get(c"UNUserNotificationCenter") else {
                send_once(
                    &request_sender,
                    Err("macos notification center unavailable".to_string()),
                );
                return;
            };

            let notification_center: *mut AnyObject =
                msg_send![notification_center_class, currentNotificationCenter];
            if notification_center.is_null() {
                send_once(
                    &request_sender,
                    Err("macos notification center probe failed".to_string()),
                );
                return;
            }

            let sender = Arc::clone(&request_sender);
            let handler = RcBlock::new(move |_granted: Bool, error: *mut AnyObject| {
                let result = if error.is_null() {
                    Ok(())
                } else {
                    Err("macos notification authorization failed".to_string())
                };
                send_once(&sender, result);
            });

            let options = UN_AUTHORIZATION_OPTION_BADGE
                | UN_AUTHORIZATION_OPTION_SOUND
                | UN_AUTHORIZATION_OPTION_ALERT;
            let _: () = msg_send![
                notification_center,
                requestAuthorizationWithOptions: options,
                completionHandler: &*handler
            ];
        })
        .map_err(|error| error.to_string())?;

    rx.recv_timeout(REQUEST_TIMEOUT)
        .map_err(|_| "timed out waiting for macOS notification authorization".to_string())?
}

#[cfg(target_os = "macos")]
fn macos_notification_authorization_status<R: Runtime>(
    app_handle: &AppHandle<R>,
) -> Result<isize, String> {
    const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let tx = Arc::new(Mutex::new(Some(tx)));
    let probe_sender = Arc::clone(&tx);

    app_handle
        .run_on_main_thread(move || unsafe {
            let Some(notification_center_class) = AnyClass::get(c"UNUserNotificationCenter") else {
                send_once(
                    &probe_sender,
                    Err("macos notification center unavailable".to_string()),
                );
                return;
            };

            let notification_center: *mut AnyObject =
                msg_send![notification_center_class, currentNotificationCenter];
            if notification_center.is_null() {
                send_once(
                    &probe_sender,
                    Err("macos notification center probe failed".to_string()),
                );
                return;
            }

            let sender = Arc::clone(&probe_sender);
            let handler = RcBlock::new(move |settings: *mut AnyObject| {
                let result = if settings.is_null() {
                    Err("macos notification settings unavailable".to_string())
                } else {
                    let status: isize = msg_send![settings, authorizationStatus];
                    Ok(status)
                };
                send_once(&sender, result);
            });

            let _: () = msg_send![
                notification_center,
                getNotificationSettingsWithCompletionHandler: &*handler
            ];
        })
        .map_err(|error| error.to_string())?;

    rx.recv_timeout(PROBE_TIMEOUT)
        .map_err(|_| "timed out waiting for macOS notification settings".to_string())?
}

#[cfg(target_os = "macos")]
fn send_once<T>(sender: &Arc<Mutex<Option<std::sync::mpsc::SyncSender<T>>>>, value: T) {
    if let Ok(mut guard) = sender.lock() {
        if let Some(sender) = guard.take() {
            if let Err(e) = sender.send(value) {
                debug!("channel send failed: {e}");
            }
        }
    }
}

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
}

#[cfg(target_os = "macos")]
fn macos_screen_capture_access_granted() -> bool {
    unsafe { CGPreflightScreenCaptureAccess() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_name_is_known() {
        assert!(!current_platform().is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_permission_settings_urls_match_expected_targets() {
        assert_eq!(
            macos_permission_settings_url("accessibility"),
            Some("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        );
        assert_eq!(
            macos_permission_settings_url("screen_capture"),
            Some("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
        );
        assert_eq!(macos_permission_settings_url("notifications"), None);
    }
}
