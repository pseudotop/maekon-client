use super::{
    emit_debug_permissions_cli_json, hold_debug_permissions_cli_if_requested,
    DebugPermissionsCliCommand, DebugPermissionsRuntimeCliCommand,
};

pub(crate) fn run_debug_permissions_cli_command(command: DebugPermissionsCliCommand) -> i32 {
    #[cfg(target_os = "macos")]
    {
        use maekon_core::ports::accessibility::AccessibilityExtractor;

        match command {
            DebugPermissionsCliCommand::Status => {
                let accessibility =
                    maekon_vision::accessibility::MacOsNativeAccessibility::new().has_permission();
                let screen_capture = unsafe { CGPreflightScreenCaptureAccess() };
                let payload = serde_json::json!({
                    "debug_permissions": true,
                    "command": "status",
                    "accessibility_granted": accessibility,
                    "screen_capture_granted": screen_capture,
                })
                .to_string();
                emit_debug_permissions_cli_json(&payload)
            }
            DebugPermissionsCliCommand::ScreenCaptureRequest => {
                let granted = unsafe { CGRequestScreenCaptureAccess() };
                let payload = serde_json::json!({
                    "debug_permissions": true,
                    "command": "screen-capture-request",
                    "granted": granted,
                })
                .to_string();
                let exit_code = emit_debug_permissions_cli_json(&payload);
                if exit_code == 0 {
                    hold_debug_permissions_cli_if_requested();
                }
                exit_code
            }
            DebugPermissionsCliCommand::ScreenCaptureAttempt => {
                let payload =
                    match maekon_vision::capture::ScreenCapture::new().capture_primary() {
                        Ok(image) => serde_json::json!({
                            "debug_permissions": true,
                            "command": "screen-capture-attempt",
                            "ok": true,
                            "width": image.width(),
                            "height": image.height(),
                        }),
                        Err(error) => serde_json::json!({
                            "debug_permissions": true,
                            "command": "screen-capture-attempt",
                            "ok": false,
                            "error": error.to_string(),
                        }),
                    }
                    .to_string();
                let exit_code = emit_debug_permissions_cli_json(&payload);
                if exit_code == 0 {
                    hold_debug_permissions_cli_if_requested();
                }
                exit_code
            }
            DebugPermissionsCliCommand::AccessibilityRequest => {
                let granted = maekon_vision::accessibility::MacOsNativeAccessibility::new()
                    .request_permission();
                let payload = serde_json::json!({
                    "debug_permissions": true,
                    "command": "accessibility-request",
                    "granted": granted,
                })
                .to_string();
                let exit_code = emit_debug_permissions_cli_json(&payload);
                if exit_code == 0 {
                    hold_debug_permissions_cli_if_requested();
                }
                exit_code
            }
            DebugPermissionsCliCommand::OpenAccessibilitySettings => {
                emit_debug_permissions_open_settings_json("accessibility")
            }
            DebugPermissionsCliCommand::OpenScreenCaptureSettings => {
                emit_debug_permissions_open_settings_json("screen_capture")
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("debug-permissions is only supported on macOS");
        let _ = command;
        2
    }
}

pub(crate) fn run_debug_permissions_runtime_cli_command<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    command: DebugPermissionsRuntimeCliCommand,
) -> i32 {
    match command {
        DebugPermissionsRuntimeCliCommand::ScreenCaptureRequest => {
            let payload =
                match crate::desktop_permissions::request_desktop_screen_capture_permission(
                    app_handle,
                ) {
                    Ok(snapshot) => serde_json::json!({
                        "debug_permissions": true,
                        "command": "screen-capture-request-runtime",
                        "ok": true,
                        "snapshot": snapshot,
                    }),
                    Err(error) => serde_json::json!({
                        "debug_permissions": true,
                        "command": "screen-capture-request-runtime",
                        "ok": false,
                        "error": error,
                    }),
                };
            let payload = serde_json::to_string(&payload).unwrap_or_else(|_| {
                "{\"debug_permissions\":true,\"command\":\"screen-capture-request-runtime\",\"ok\":false,\"error\":\"json serialization failed\"}".to_string()
            });
            let exit_code = emit_debug_permissions_cli_json(&payload);
            if exit_code == 0 {
                hold_debug_permissions_cli_if_requested();
            }
            exit_code
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn emit_debug_permissions_open_settings_json(permission_kind: &str) -> i32 {
    let payload =
        match crate::desktop_permissions::open_desktop_permission_settings(permission_kind) {
            Ok(()) => serde_json::json!({
                "debug_permissions": true,
                "command": "open-settings",
                "permission_kind": permission_kind,
                "ok": true,
            }),
            Err(error) => serde_json::json!({
                "debug_permissions": true,
                "command": "open-settings",
                "permission_kind": permission_kind,
                "ok": false,
                "error": error,
            }),
        };
    let payload = payload.to_string();

    emit_debug_permissions_cli_json(&payload)
}

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    /// Returns the current macOS Screen Recording grant without prompting.
    ///
    /// # Safety
    /// This CoreGraphics symbol is available on macOS 10.15+. It takes no
    /// pointers and returns only the current grant state. Callers must invoke it
    /// only on macOS and treat the result as advisory because the user can change
    /// TCC state outside the process at any time.
    fn CGPreflightScreenCaptureAccess() -> bool;

    /// Requests macOS Screen Recording access and may trigger a system prompt.
    ///
    /// # Safety
    /// This CoreGraphics symbol is available on macOS 10.15+. It takes no
    /// pointers and returns the current grant state after the request attempt.
    /// Callers must invoke it only from code paths where a user-visible TCC
    /// prompt is expected and acceptable.
    fn CGRequestScreenCaptureAccess() -> bool;
}
