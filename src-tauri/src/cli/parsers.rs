//! Debug CLI argument parsers (debug_assertions only).
//!
//! Each parser reads a gated env var and the arg iterator, returning
//! `Some(Command)` only when the gate env var is truthy.

use super::types::{
    DebugAutostartCliCommand, DebugAxTreeCliCommand, DebugNotificationBackend,
    DebugNotificationCliCommand, DebugPermissionsCliCommand, DebugPermissionsRuntimeCliCommand,
    DebugPointerCaptureCliCommand, DebugPointerCaptureRuntimeCliCommand, DebugPowerCliCommand,
};

#[cfg(debug_assertions)]
fn gate_enabled(env_value: Option<&str>) -> bool {
    env_value.map(str::trim).is_some_and(|value| {
        matches!(value, "1")
            || value.eq_ignore_ascii_case("true")
            || value.eq_ignore_ascii_case("yes")
            || value.eq_ignore_ascii_case("on")
    })
}

#[cfg(debug_assertions)]
pub(crate) fn debug_autostart_cli_command_from<I, S>(
    args: I,
    env_value: Option<&str>,
) -> Option<DebugAutostartCliCommand>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    if !gate_enabled(env_value) {
        return None;
    }

    let mut args = args.into_iter();
    if args.next().as_ref().map(AsRef::as_ref) != Some("debug-autostart") {
        return None;
    }

    match args.next().as_ref().map(AsRef::as_ref) {
        Some("status") => Some(DebugAutostartCliCommand::Status),
        Some("enable") => Some(DebugAutostartCliCommand::Enable),
        Some("disable") => Some(DebugAutostartCliCommand::Disable),
        _ => None,
    }
}

#[cfg(debug_assertions)]
pub(crate) fn debug_permissions_cli_command_from<I, S>(
    args: I,
    env_value: Option<&str>,
) -> Option<DebugPermissionsCliCommand>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    if !gate_enabled(env_value) {
        return None;
    }

    let mut args = args.into_iter();
    if args.next().as_ref().map(AsRef::as_ref) != Some("debug-permissions") {
        return None;
    }

    match args.next().as_ref().map(AsRef::as_ref) {
        Some("status") => Some(DebugPermissionsCliCommand::Status),
        Some("screen-capture-request") => Some(DebugPermissionsCliCommand::ScreenCaptureRequest),
        Some("screen-capture-attempt") => Some(DebugPermissionsCliCommand::ScreenCaptureAttempt),
        Some("accessibility-request") => Some(DebugPermissionsCliCommand::AccessibilityRequest),
        Some("open-settings") => match args.next().as_ref().map(AsRef::as_ref) {
            Some("accessibility") => Some(DebugPermissionsCliCommand::OpenAccessibilitySettings),
            Some("screen_capture" | "screen-capture") => {
                Some(DebugPermissionsCliCommand::OpenScreenCaptureSettings)
            }
            _ => None,
        },
        _ => None,
    }
}

#[cfg(debug_assertions)]
pub(crate) fn debug_permissions_runtime_cli_command_from<I, S>(
    args: I,
    env_value: Option<&str>,
) -> Option<DebugPermissionsRuntimeCliCommand>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    if !gate_enabled(env_value) {
        return None;
    }

    let mut args = args.into_iter();
    if args.next().as_ref().map(AsRef::as_ref) != Some("debug-permissions-runtime") {
        return None;
    }

    match args.next().as_ref().map(AsRef::as_ref) {
        Some("screen-capture-request") => {
            Some(DebugPermissionsRuntimeCliCommand::ScreenCaptureRequest)
        }
        _ => None,
    }
}

#[cfg(debug_assertions)]
pub(crate) fn debug_ax_tree_cli_command_from<I, S>(
    args: I,
    env_value: Option<&str>,
) -> Option<DebugAxTreeCliCommand>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    if !gate_enabled(env_value) {
        return None;
    }

    let mut args = args.into_iter();
    if args.next().as_ref().map(AsRef::as_ref) != Some("debug-ax-tree") {
        return None;
    }

    match args.next().as_ref().map(AsRef::as_ref) {
        Some("extract") => {
            let app_name = args
                .next()
                .map(|value| value.as_ref().trim().to_string())
                .filter(|value| !value.is_empty())?;
            let max_depth = args
                .next()
                .and_then(|value| value.as_ref().parse::<u32>().ok())
                .unwrap_or(4)
                .min(8);
            let max_elements = args
                .next()
                .and_then(|value| value.as_ref().parse::<usize>().ok())
                .unwrap_or(300)
                .clamp(1, 1_000);
            Some(DebugAxTreeCliCommand::Extract {
                app_name,
                max_depth,
                max_elements,
            })
        }
        Some("windows-uia-benchmark") => {
            let samples = args
                .next()
                .and_then(|value| value.as_ref().parse::<u32>().ok())
                .unwrap_or(5)
                .clamp(1, 20);
            let max_depth = args
                .next()
                .and_then(|value| value.as_ref().parse::<u32>().ok())
                .unwrap_or(3)
                .clamp(1, 8);
            let max_elements = args
                .next()
                .and_then(|value| value.as_ref().parse::<usize>().ok())
                .unwrap_or(300)
                .clamp(1, 1_000);
            Some(DebugAxTreeCliCommand::WindowsUiaBenchmark {
                samples,
                max_depth,
                max_elements,
            })
        }
        Some("windows-ocr-benchmark") => {
            let samples = args
                .next()
                .and_then(|value| value.as_ref().parse::<u32>().ok())
                .unwrap_or(3)
                .clamp(1, 10);
            let display_scale_x100 = args
                .next()
                .and_then(|value| value.as_ref().parse::<u32>().ok())
                .unwrap_or(150)
                .clamp(100, 300);
            Some(DebugAxTreeCliCommand::WindowsOcrBenchmark {
                samples,
                display_scale_x100,
            })
        }
        Some("windows-gui-session-e2e") => {
            let display_scale_x100 = args
                .next()
                .and_then(|value| value.as_ref().parse::<u32>().ok())
                .unwrap_or(150)
                .clamp(100, 300);
            let overlay_hold_ms = args
                .next()
                .and_then(|value| value.as_ref().parse::<u64>().ok())
                .unwrap_or(250)
                .clamp(0, 3_000);
            Some(DebugAxTreeCliCommand::WindowsGuiSessionE2eBenchmark {
                display_scale_x100,
                overlay_hold_ms,
            })
        }
        Some("windows-sandbox-overhead") => {
            let samples = args
                .next()
                .and_then(|value| value.as_ref().parse::<u32>().ok())
                .unwrap_or(3)
                .clamp(1, 20);
            let timeout_probe_ms = args
                .next()
                .and_then(|value| value.as_ref().parse::<u64>().ok())
                .unwrap_or(25)
                .clamp(1, 5_000);
            Some(DebugAxTreeCliCommand::WindowsSandboxOverheadBenchmark {
                samples,
                timeout_probe_ms,
            })
        }
        _ => None,
    }
}

#[cfg(debug_assertions)]
pub(crate) fn debug_notification_cli_command_from<I, S>(
    args: I,
    env_value: Option<&str>,
) -> Option<DebugNotificationCliCommand>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    if !gate_enabled(env_value) {
        return None;
    }

    let mut args = args.into_iter();
    if args.next().as_ref().map(AsRef::as_ref) != Some("debug-notification") {
        return None;
    }

    match args.next().as_ref().map(AsRef::as_ref) {
        Some("status") => Some(DebugNotificationCliCommand::Status),
        Some("request") => Some(DebugNotificationCliCommand::Request),
        Some("send") => Some(DebugNotificationCliCommand::Send),
        _ => None,
    }
}

#[cfg(debug_assertions)]
pub(crate) fn debug_power_cli_command_from<I, S>(
    args: I,
    env_value: Option<&str>,
) -> Option<DebugPowerCliCommand>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    if !gate_enabled(env_value) {
        return None;
    }

    let mut args = args.into_iter();
    if args.next().as_ref().map(AsRef::as_ref) != Some("debug-power") {
        return None;
    }

    match args.next().as_ref().map(AsRef::as_ref) {
        Some("capture-burst-audit") => Some(DebugPowerCliCommand::CaptureBurstAudit),
        _ => None,
    }
}

#[cfg(debug_assertions)]
pub(crate) fn debug_pointer_capture_cli_command_from<I, S>(
    args: I,
    env_value: Option<&str>,
) -> Option<DebugPointerCaptureCliCommand>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    if !gate_enabled(env_value) {
        return None;
    }

    let mut args = args.into_iter();
    if args.next().as_ref().map(AsRef::as_ref) != Some("debug-pointer-capture") {
        return None;
    }

    match args.next().as_ref().map(AsRef::as_ref) {
        Some("probe") => {
            let frames = args
                .next()
                .and_then(|value| value.as_ref().parse::<u32>().ok())
                .unwrap_or(5)
                .clamp(1, 30);
            let interval_ms = args
                .next()
                .and_then(|value| value.as_ref().parse::<u64>().ok())
                .unwrap_or(100)
                .clamp(16, 1_000);
            Some(DebugPointerCaptureCliCommand::Probe {
                frames,
                interval_ms,
            })
        }
        _ => None,
    }
}

#[cfg(debug_assertions)]
pub(crate) fn debug_pointer_capture_runtime_cli_command_from<I, S>(
    args: I,
    env_value: Option<&str>,
) -> Option<DebugPointerCaptureRuntimeCliCommand>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    if !gate_enabled(env_value) {
        return None;
    }

    let mut args = args.into_iter();
    if args.next().as_ref().map(AsRef::as_ref) != Some("debug-pointer-capture") {
        return None;
    }

    let command = args.next().map(|value| value.as_ref().to_string());
    match command.as_deref() {
        Some("overlay-probe" | "overlay-probe-reduced-motion") => {
            let reduced_motion = command.as_deref() == Some("overlay-probe-reduced-motion");
            let frames = args
                .next()
                .and_then(|value| value.as_ref().parse::<u32>().ok())
                .unwrap_or(5)
                .clamp(1, 30);
            let interval_ms = args
                .next()
                .and_then(|value| value.as_ref().parse::<u64>().ok())
                .unwrap_or(100)
                .clamp(16, 1_000);
            if reduced_motion {
                Some(
                    DebugPointerCaptureRuntimeCliCommand::OverlayProbeReducedMotion {
                        frames,
                        interval_ms,
                    },
                )
            } else {
                Some(DebugPointerCaptureRuntimeCliCommand::OverlayProbe {
                    frames,
                    interval_ms,
                })
            }
        }
        _ => None,
    }
}

#[cfg(debug_assertions)]
pub(crate) fn debug_notification_backend_from(env_value: Option<&str>) -> DebugNotificationBackend {
    let Some(value) = env_value.map(str::trim).filter(|value| !value.is_empty()) else {
        return DebugNotificationBackend::TauriPlugin;
    };

    match value.to_ascii_lowercase().as_str() {
        "macos-unuser" | "macos_unuser" | "unuser" | "unusernotificationcenter" => {
            DebugNotificationBackend::MacosUnuser
        }
        "tauri" | "tauri-plugin" | "tauri_plugin" => DebugNotificationBackend::TauriPlugin,
        _ => DebugNotificationBackend::TauriPlugin,
    }
}

#[cfg(debug_assertions)]
pub(crate) fn should_enable_single_instance_for_debug_runtime(
    notification_command: Option<DebugNotificationCliCommand>,
    permissions_runtime_command: Option<DebugPermissionsRuntimeCliCommand>,
) -> bool {
    notification_command.is_none() && permissions_runtime_command.is_none()
}

#[cfg(debug_assertions)]
pub(crate) fn debug_notification_activation_route_from(env_value: Option<&str>) -> Option<String> {
    let route = env_value?.trim();
    crate::notification_manager::notification_activation_outcome_from_route(Some(route))
        .ok()
        .map(|outcome| outcome.route)
}

#[cfg(debug_assertions)]
pub(crate) fn debug_permissions_cli_hold_seconds_from(env_value: Option<&str>) -> u64 {
    env_value
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(0)
        .min(60)
}

#[cfg(debug_assertions)]
pub(crate) fn debug_notification_cli_hold_seconds_from(env_value: Option<&str>) -> u64 {
    env_value
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(0)
        .min(60)
}
