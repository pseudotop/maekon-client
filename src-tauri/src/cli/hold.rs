use super::{debug_notification_cli_hold_seconds_from, debug_permissions_cli_hold_seconds_from};

pub(crate) fn hold_debug_permissions_cli_if_requested() {
    let hold_seconds = debug_permissions_cli_hold_seconds_from(
        std::env::var("MAEKON_DEBUG_DESKTOP_PERMISSION_CLI_HOLD_SECONDS")
            .ok()
            .as_deref(),
    );
    if hold_seconds > 0 {
        std::thread::sleep(std::time::Duration::from_secs(hold_seconds));
    }
}

pub(crate) fn hold_debug_notification_cli_if_requested() {
    let hold_seconds = debug_notification_cli_hold_seconds_from(
        std::env::var("MAEKON_DEBUG_NOTIFICATION_CLI_HOLD_SECONDS")
            .ok()
            .as_deref(),
    );
    if hold_seconds > 0 {
        std::thread::sleep(std::time::Duration::from_secs(hold_seconds));
    }
}
