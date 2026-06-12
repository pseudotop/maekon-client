use super::{
    DebugAutostartCliCommand, DebugAxTreeCliCommand, DebugNotificationBackend,
    DebugNotificationCliCommand, DebugPermissionsCliCommand, DebugPermissionsRuntimeCliCommand,
    DebugPowerCliCommand,
};
use std::path::PathBuf;

fn debug_gate_enabled(env_value: Option<&str>) -> bool {
    env_value.map(str::trim).is_some_and(|value| {
        matches!(value, "1")
            || value.eq_ignore_ascii_case("true")
            || value.eq_ignore_ascii_case("yes")
            || value.eq_ignore_ascii_case("on")
    })
}

fn optional_path_from(env_value: Option<&str>) -> Option<PathBuf> {
    let value = env_value?.trim();
    if value.is_empty() {
        None
    } else {
        Some(PathBuf::from(value))
    }
}

pub(crate) fn debug_autostart_cli_command_from<I, S>(
    args: I,
    env_value: Option<&str>,
) -> Option<DebugAutostartCliCommand>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    if !debug_gate_enabled(env_value) {
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

pub(crate) fn debug_permissions_cli_command_from<I, S>(
    args: I,
    env_value: Option<&str>,
) -> Option<DebugPermissionsCliCommand>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    if !debug_gate_enabled(env_value) {
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

pub(crate) fn debug_permissions_runtime_cli_command_from<I, S>(
    args: I,
    env_value: Option<&str>,
) -> Option<DebugPermissionsRuntimeCliCommand>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    if !debug_gate_enabled(env_value) {
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

pub(crate) fn debug_ax_tree_cli_command_from<I, S>(
    args: I,
    env_value: Option<&str>,
) -> Option<DebugAxTreeCliCommand>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    if !debug_gate_enabled(env_value) {
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
        _ => None,
    }
}

pub(crate) fn debug_notification_cli_command_from<I, S>(
    args: I,
    env_value: Option<&str>,
) -> Option<DebugNotificationCliCommand>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    if !debug_gate_enabled(env_value) {
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

pub(crate) fn debug_power_cli_command_from<I, S>(
    args: I,
    env_value: Option<&str>,
) -> Option<DebugPowerCliCommand>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    if !debug_gate_enabled(env_value) {
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

pub(crate) fn should_enable_single_instance_for_debug_runtime(
    notification_command: Option<DebugNotificationCliCommand>,
    permissions_runtime_command: Option<DebugPermissionsRuntimeCliCommand>,
) -> bool {
    notification_command.is_none() && permissions_runtime_command.is_none()
}

pub(crate) fn debug_permissions_cli_output_path_from(env_value: Option<&str>) -> Option<PathBuf> {
    optional_path_from(env_value)
}

pub(crate) fn debug_notification_cli_output_path_from(env_value: Option<&str>) -> Option<PathBuf> {
    optional_path_from(env_value)
}

pub(crate) fn debug_ax_tree_cli_output_path_from(env_value: Option<&str>) -> Option<PathBuf> {
    optional_path_from(env_value)
}

pub(crate) fn debug_power_cli_output_path_from(env_value: Option<&str>) -> Option<PathBuf> {
    optional_path_from(env_value)
}

pub(crate) fn debug_notification_cli_marker_output_path_from(
    env_value: Option<&str>,
) -> Option<PathBuf> {
    optional_path_from(env_value)
}

pub(crate) fn debug_notification_cli_activation_output_path_from(
    env_value: Option<&str>,
) -> Option<PathBuf> {
    optional_path_from(env_value)
}

pub(crate) fn debug_notification_cli_audit_jsonl_path_from(
    env_value: Option<&str>,
) -> Option<PathBuf> {
    optional_path_from(env_value)
}

pub(crate) fn debug_notification_cli_diagnostic_jsonl_path_from(
    env_value: Option<&str>,
) -> Option<PathBuf> {
    optional_path_from(env_value)
}

pub(crate) fn debug_notification_activation_route_from(env_value: Option<&str>) -> Option<String> {
    let route = env_value?.trim();
    crate::notification_manager::notification_activation_outcome_from_route(Some(route))
        .ok()
        .map(|outcome| outcome.route)
}

pub(crate) fn debug_permissions_cli_hold_seconds_from(env_value: Option<&str>) -> u64 {
    env_value
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(0)
        .min(60)
}

pub(crate) fn debug_notification_cli_hold_seconds_from(env_value: Option<&str>) -> u64 {
    env_value
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(0)
        .min(60)
}
