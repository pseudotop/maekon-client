use serde::Serialize;
use tauri::{AppHandle, Runtime};

#[cfg(any(target_os = "macos", target_os = "linux"))]
use maekon_core::ports::accessibility::AccessibilityExtractor;

#[cfg(target_os = "macos")]
use block2::RcBlock;
#[cfg(target_os = "macos")]
use objc2::msg_send;
#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2::runtime::{AnyClass, AnyObject, Bool};
#[cfg(target_os = "macos")]
use objc2_foundation::{ns_string, NSString};
#[cfg(target_os = "macos")]
use std::sync::{Arc, Mutex};
#[cfg(target_os = "macos")]
use std::time::Duration;
#[cfg(target_os = "macos")]
use tracing::debug;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
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
    pub microphone: DesktopPermissionEntry,
    pub input_monitoring: DesktopPermissionEntry,
    pub notifications: DesktopPermissionEntry,
}

pub fn get_desktop_permission_snapshot<R: Runtime>(
    app_handle: &AppHandle<R>,
) -> DesktopPermissionSnapshot {
    DesktopPermissionSnapshot {
        platform: current_platform().to_string(),
        accessibility: accessibility_permission_entry(),
        screen_capture: screen_capture_permission_entry(),
        microphone: microphone_permission_entry(),
        input_monitoring: input_monitoring_permission_entry(),
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

pub fn request_desktop_screen_capture_permission<R: Runtime>(
    app_handle: &AppHandle<R>,
) -> Result<DesktopPermissionSnapshot, String> {
    #[cfg(target_os = "macos")]
    request_macos_screen_capture_permission(app_handle)?;

    Ok(get_desktop_permission_snapshot(app_handle))
}

pub fn open_desktop_permission_settings(permission_kind: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let url = macos_permission_settings_url(permission_kind).ok_or_else(|| {
            format!("Unsupported desktop permission settings kind: {permission_kind}")
        })?;
        // SEC-MON-01: resolve against the shared trusted-directory allowlist
        // (maekon-monitor) instead of spawning a bare `open` — fail closed
        // (no PATH fallback) when the binary is not found under any trusted
        // system directory.
        let open_path = maekon_monitor::resolve_trusted_binary("open")
            .ok_or_else(|| "open not found under the trusted directory allowlist".to_string())?;
        let status = std::process::Command::new(open_path)
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
        "microphone" => {
            Some("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
        }
        "input_monitoring" => {
            Some("x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent")
        }
        "notifications" => Some("x-apple.systempreferences:com.apple.preference.notifications"),
        _ => None,
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
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
    let extractor = maekon_vision::accessibility::MacOsNativeAccessibility::new();
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
    let extractor = maekon_vision::accessibility::LinuxAccessibility::new();
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
    match maekon_vision::capture::ScreenCapture::monitor_count() {
        Ok(count) if count > 0 => not_required(Some("screen_capture_ready")),
        Ok(_) => unavailable("screen_capture_no_monitors"),
        Err(_) => unavailable("screen_capture_probe_failed"),
    }
}

#[cfg(target_os = "macos")]
fn microphone_permission_entry() -> DesktopPermissionEntry {
    match macos_microphone_authorization_status() {
        Ok(3) => granted(Some("macos_microphone_granted")),
        Ok(0) => needs_attention("macos_microphone_not_determined"),
        Ok(1) => needs_attention("macos_microphone_restricted"),
        Ok(2) => needs_attention("macos_microphone_denied"),
        Ok(_) => unavailable("macos_microphone_unknown_status"),
        Err(_) => unavailable("macos_microphone_probe_failed"),
    }
}

#[cfg(target_os = "windows")]
fn microphone_permission_entry() -> DesktopPermissionEntry {
    not_required(Some("windows_microphone_managed_by_os"))
}

#[cfg(target_os = "linux")]
fn microphone_permission_entry() -> DesktopPermissionEntry {
    not_required(Some("linux_microphone_managed_by_session"))
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn microphone_permission_entry() -> DesktopPermissionEntry {
    unavailable("microphone_unsupported")
}

#[cfg(target_os = "macos")]
fn input_monitoring_permission_entry() -> DesktopPermissionEntry {
    match macos_input_monitoring_access_status() {
        Ok(0) => granted(Some("macos_input_monitoring_granted")),
        Ok(1) => needs_attention("macos_input_monitoring_denied"),
        Ok(2) => needs_attention("macos_input_monitoring_unknown"),
        Ok(_) => unavailable("macos_input_monitoring_unknown_status"),
        Err(_) => unavailable("macos_input_monitoring_probe_failed"),
    }
}

#[cfg(target_os = "windows")]
fn input_monitoring_permission_entry() -> DesktopPermissionEntry {
    not_required(Some("windows_input_monitoring_managed_by_os"))
}

#[cfg(target_os = "linux")]
fn input_monitoring_permission_entry() -> DesktopPermissionEntry {
    not_required(Some("linux_input_monitoring_managed_by_session"))
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn input_monitoring_permission_entry() -> DesktopPermissionEntry {
    unavailable("input_monitoring_unsupported")
}

#[cfg(target_os = "macos")]
fn request_macos_screen_capture_permission<R: Runtime>(
    app_handle: &AppHandle<R>,
) -> Result<(), String> {
    const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let tx = Arc::new(Mutex::new(Some(tx)));
    let request_sender = Arc::clone(&tx);

    app_handle
        .run_on_main_thread(move || {
            // SAFETY: FFI call into CoreGraphics (declared in the extern block
            // below). It takes no arguments and returns a `bool` by value, so
            // there are no pointer/lifetime obligations; macOS only requires it
            // on the main thread, which `run_on_main_thread` guarantees.
            let granted = unsafe { CGRequestScreenCaptureAccess() };
            send_once(&request_sender, granted);
        })
        .map_err(|error| error.to_string())?;

    let _granted = rx
        .recv_timeout(REQUEST_TIMEOUT)
        .map_err(|_| "timed out waiting for macOS screen capture authorization".to_string())?;
    Ok(())
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
    let request_timeout = macos_notification_request_timeout();

    macos_notification_center_available_for_current_process()?;

    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let tx = Arc::new(Mutex::new(Some(tx)));
    let request_sender = Arc::clone(&tx);

    app_handle
        // SAFETY: this closure sends Objective-C messages (msg_send!) to
        // UNUserNotificationCenter on the main thread (guaranteed by
        // run_on_main_thread). The class is looked up via AnyClass::get and the
        // returned receiver pointer is null-checked before any selector is sent;
        // each selector (currentNotificationCenter / requestAuthorization…)
        // matches the receiver's real signature. The RcBlock completion handler
        // is leaked via mem::forget because the framework invokes it
        // asynchronously after this closure returns.
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
            let handler = RcBlock::new(move |granted: Bool, error: *mut AnyObject| {
                let result = macos_notification_authorization_request_result(
                    granted.as_bool(),
                    macos_notification_error_message(error),
                );
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

            // UNUserNotificationCenter completes asynchronously; keep the
            // Objective-C block alive beyond this main-thread closure.
            std::mem::forget(handler);
        })
        .map_err(|error| error.to_string())?;

    rx.recv_timeout(request_timeout)
        .map_err(|_| "timed out waiting for macOS notification authorization".to_string())?
}

#[cfg(target_os = "macos")]
fn macos_notification_request_timeout() -> Duration {
    macos_notification_request_timeout_from(
        std::env::var("MAEKON_DEBUG_NOTIFICATION_REQUEST_TIMEOUT_SECONDS")
            .ok()
            .as_deref(),
    )
}

#[cfg(target_os = "macos")]
fn macos_notification_request_timeout_from(env_value: Option<&str>) -> Duration {
    let seconds = env_value
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .map(|seconds| seconds.min(60))
        .unwrap_or(5);
    Duration::from_secs(seconds)
}

#[cfg(target_os = "macos")]
fn macos_notification_authorization_request_result(
    granted: bool,
    error_message: Option<String>,
) -> Result<(), String> {
    if let Some(error_message) = error_message
        .map(|message| message.trim().to_string())
        .filter(|message| !message.is_empty())
    {
        return Err(format!(
            "macos notification authorization failed: {error_message}"
        ));
    }

    if granted {
        Ok(())
    } else {
        Err("macos notification authorization was not granted".to_string())
    }
}

#[cfg(target_os = "macos")]
fn macos_notification_error_message(error: *mut AnyObject) -> Option<String> {
    if error.is_null() {
        return None;
    }

    // SAFETY: `error` was confirmed non-null above and points to a live NSError
    // (an `NSError *` handed in by the framework). `&*error` reborrows that
    // checked pointer only for the message sends; localizedDescription/domain
    // (-> NSString) and code (-> isize) are valid NSError selectors whose return
    // types match the bindings.
    unsafe {
        let desc: Retained<NSString> = msg_send![&*error, localizedDescription];
        let domain: Retained<NSString> = msg_send![&*error, domain];
        let code: isize = msg_send![&*error, code];
        Some(format!("{} (domain: {}, code: {})", desc, domain, code))
    }
}

#[cfg(target_os = "macos")]
fn macos_notification_authorization_status<R: Runtime>(
    app_handle: &AppHandle<R>,
) -> Result<isize, String> {
    const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

    macos_notification_center_available_for_current_process()?;

    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let tx = Arc::new(Mutex::new(Some(tx)));
    let probe_sender = Arc::clone(&tx);

    app_handle
        // SAFETY: this closure sends Objective-C messages (msg_send!) to
        // UNUserNotificationCenter on the main thread (guaranteed by
        // run_on_main_thread). The class is looked up via AnyClass::get and the
        // returned receiver pointer is null-checked before any selector is sent;
        // each selector (currentNotificationCenter /
        // getNotificationSettingsWithCompletionHandler:) matches the receiver's
        // real signature. The RcBlock settings handler is leaked via mem::forget
        // because the framework invokes it asynchronously after this closure
        // returns.
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

            // UNUserNotificationCenter invokes this completion handler
            // asynchronously, so it must outlive the main-thread closure.
            std::mem::forget(handler);
        })
        .map_err(|error| error.to_string())?;

    rx.recv_timeout(PROBE_TIMEOUT)
        .map_err(|_| "timed out waiting for macOS notification settings".to_string())?
}

#[cfg(target_os = "macos")]
fn macos_notification_center_available_for_current_process() -> Result<(), String> {
    if std::env::var_os("MAEKON_ALLOW_UNBUNDLED_NOTIFICATION_CENTER").is_some() {
        return Ok(());
    }

    let exe = std::env::current_exe()
        .map_err(|error| format!("macos notification bundle probe failed: {error}"))?;
    if macos_path_has_app_bundle(&exe) {
        Ok(())
    } else {
        Err(format!(
            "macos notification center unavailable for unbundled executable: {}",
            exe.display()
        ))
    }
}

#[cfg(target_os = "macos")]
fn macos_path_has_app_bundle(path: &std::path::Path) -> bool {
    path.ancestors().any(|ancestor| {
        ancestor
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
    })
}

#[cfg(target_os = "macos")]
fn macos_microphone_authorization_status() -> Result<isize, String> {
    // SAFETY: AnyClass::get returns a valid +0 class pointer (else we early
    // return), and `authorizationStatusForMediaType:` is a real AVCaptureDevice
    // class method taking an NSString (the static `ns_string!` lives for the
    // program) and returning the `AVAuthorizationStatus` raw value as isize.
    unsafe {
        let Some(capture_device_class) = AnyClass::get(c"AVCaptureDevice") else {
            return Err("macos microphone authorization class unavailable".to_string());
        };

        let status: isize = msg_send![
            capture_device_class,
            authorizationStatusForMediaType: ns_string!("soun")
        ];
        Ok(status)
    }
}

#[cfg(target_os = "macos")]
fn macos_input_monitoring_access_status() -> Result<u32, String> {
    const K_IOHID_REQUEST_TYPE_LISTEN_EVENT: u32 = 1;
    // SAFETY: FFI call into IOKit (declared in the extern block below). It takes
    // a single `u32` request-type by value and returns a `u32` status; the
    // constant is the documented kIOHIDRequestTypeListenEvent value, so there
    // are no pointer or lifetime obligations.
    Ok(unsafe { IOHIDCheckAccess(K_IOHID_REQUEST_TYPE_LISTEN_EVENT) })
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

// SAFETY: these declarations match the CoreGraphics C ABI — both
// CGPreflight/CGRequestScreenCaptureAccess take no arguments and return a C
// `bool` — so calls through them are sound.
#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

// SAFETY: empty extern block declares no callable items; it exists only to link
// the AVFoundation framework so msg_send! to AVCaptureDevice resolves at load.
#[cfg(target_os = "macos")]
#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" {}

// SAFETY: the declaration matches IOKit's C ABI — IOHIDCheckAccess takes a
// single `u32` request type and returns a `u32` status — so calls are sound.
#[cfg(target_os = "macos")]
#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOHIDCheckAccess(request_type: u32) -> u32;
}

#[cfg(target_os = "macos")]
fn macos_screen_capture_access_granted() -> bool {
    // SAFETY: FFI call into CoreGraphics; takes no arguments and returns a bool
    // by value. This is a read-only preflight with no pointer/lifetime or
    // thread-affinity obligations.
    unsafe { CGPreflightScreenCaptureAccess() }
}

/// Cheap per-tick OS screen-capture permission probe for the monitor loop
/// (#8686 AC4 mid-session revocation fail-closed axis).
///
/// macOS: `CGPreflightScreenCaptureAccess` — a read-only syscall safe to call
/// every second. Other platforms have no revocable screen-recording permission
/// concept we can preflight (Windows/Linux report `not_required`/`unavailable`
/// in the snapshot), so the axis is always open there; xcap-level capture
/// errors keep their existing warn-and-skip handling.
pub(crate) fn screen_capture_os_permission_ok() -> bool {
    #[cfg(target_os = "macos")]
    {
        macos_screen_capture_access_granted()
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_name_is_known() {
        assert!(!current_platform().is_empty());
    }

    #[test]
    fn desktop_permission_snapshot_serializes_voice_and_input_entries() {
        let snapshot = DesktopPermissionSnapshot {
            platform: "macos".to_string(),
            accessibility: DesktopPermissionEntry {
                state: DesktopPermissionState::Granted,
                status_reason: Some("macos_accessibility_granted".to_string()),
            },
            screen_capture: DesktopPermissionEntry {
                state: DesktopPermissionState::Granted,
                status_reason: Some("macos_screen_capture_granted".to_string()),
            },
            microphone: DesktopPermissionEntry {
                state: DesktopPermissionState::NeedsAttention,
                status_reason: Some("macos_microphone_not_determined".to_string()),
            },
            input_monitoring: DesktopPermissionEntry {
                state: DesktopPermissionState::NeedsAttention,
                status_reason: Some("macos_input_monitoring_unknown".to_string()),
            },
            notifications: DesktopPermissionEntry {
                state: DesktopPermissionState::Granted,
                status_reason: Some("macos_notifications_granted".to_string()),
            },
        };

        let json = serde_json::to_value(&snapshot).expect("snapshot must serialize");
        assert_eq!(
            json["microphone"]["status_reason"],
            "macos_microphone_not_determined"
        );
        assert_eq!(
            json["input_monitoring"]["status_reason"],
            "macos_input_monitoring_unknown"
        );
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
        assert_eq!(
            macos_permission_settings_url("microphone"),
            Some("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
        );
        assert_eq!(
            macos_permission_settings_url("input_monitoring"),
            Some("x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent")
        );
        assert_eq!(
            macos_permission_settings_url("notifications"),
            Some("x-apple.systempreferences:com.apple.preference.notifications")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_notification_authorization_result_preserves_denial_and_error_details() {
        assert_eq!(
            macos_notification_authorization_request_result(false, None).unwrap_err(),
            "macos notification authorization was not granted"
        );
        assert_eq!(
            macos_notification_authorization_request_result(
                false,
                Some("notifications are disabled for this bundle".to_string())
            )
            .unwrap_err(),
            "macos notification authorization failed: notifications are disabled for this bundle"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_notification_request_timeout_accepts_bounded_debug_override() {
        assert_eq!(
            macos_notification_request_timeout_from(None),
            std::time::Duration::from_secs(5)
        );
        assert_eq!(
            macos_notification_request_timeout_from(Some(" 20 ")),
            std::time::Duration::from_secs(20)
        );
        assert_eq!(
            macos_notification_request_timeout_from(Some("0")),
            std::time::Duration::from_secs(5)
        );
        assert_eq!(
            macos_notification_request_timeout_from(Some("600")),
            std::time::Duration::from_secs(60)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_notification_bundle_probe_detects_app_ancestor() {
        assert!(macos_path_has_app_bundle(std::path::Path::new(
            "/Applications/Maekon.app/Contents/MacOS/maekon"
        )));
        assert!(!macos_path_has_app_bundle(std::path::Path::new(
            "/tmp/target/debug/maekon"
        )));
    }
}
