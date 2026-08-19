use serde::Serialize;
use tauri::{command, AppHandle};

use crate::ipc_error::IpcError;
#[cfg(debug_assertions)]
use crate::notification_manager::NotificationActivationError;
use crate::notification_manager::NotificationActivationOutcome;

const TEST_NOTIFICATION_TITLE_FALLBACK: &str = "Maekon test notification";
const TEST_NOTIFICATION_BODY_FALLBACK: &str = "Notifications are ready.";
const TEST_NOTIFICATION_TEXT_LIMIT: usize = 240;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TestNotificationResult {
    pub delivered: bool,
}

#[cfg(not(debug_assertions))]
fn debug_notification_disabled() -> IpcError {
    IpcError::new(
        "service.unavailable",
        "notification debug IPC is only available in debug builds",
    )
}

#[cfg(debug_assertions)]
fn activation_error_to_ipc(error: NotificationActivationError) -> IpcError {
    IpcError::new("validation.invalid_arguments", error.message())
}

fn normalize_test_notification_text(raw: &str, fallback: &str) -> String {
    let normalized = raw
        .trim()
        .chars()
        .filter(|ch| !ch.is_control())
        .take(TEST_NOTIFICATION_TEXT_LIMIT)
        .collect::<String>();
    if normalized.is_empty() {
        fallback.to_string()
    } else {
        normalized
    }
}

#[command]
pub async fn send_test_notification(
    app: AppHandle,
    title: String,
    body: String,
) -> Result<TestNotificationResult, IpcError> {
    let title = normalize_test_notification_text(&title, TEST_NOTIFICATION_TITLE_FALLBACK);
    let body = normalize_test_notification_text(&body, TEST_NOTIFICATION_BODY_FALLBACK);

    crate::windows_notification_activation::show_actionable_notification(
        &app,
        &title,
        &body,
        crate::windows_notification_activation::DEFAULT_NOTIFICATION_ROUTE,
    )
    .map_err(|error| IpcError::new("service.unavailable", error))?;

    Ok(TestNotificationResult { delivered: true })
}

/// Debug-only harness that exercises the notification navigation seam
/// ([`notification_activation_outcome_from_route`] → focus main window → emit the
/// `navigate` event) as if an OS toast had been clicked.
///
/// This SIMULATES the click; it is not production click routing. Real desktop
/// toasts cannot invoke this path because `tauri-plugin-notification` 2.3.3
/// delivers no desktop click/action callback (see the routing function's doc for
/// the full rationale, #8058 P2-8) — hence the `#[cfg(debug_assertions)]` gate,
/// which keeps this dev/test-only affordance out of release builds so it cannot
/// masquerade as a shipped feature. The genuine navigation surface is the in-app
/// overlay/suggestion panel.
#[command]
pub async fn simulate_notification_activation(
    app: AppHandle,
    route: Option<String>,
) -> Result<NotificationActivationOutcome, IpcError> {
    #[cfg(debug_assertions)]
    {
        crate::windows_notification_activation::activate_notification(&app, route.as_deref())
            .map_err(activation_error_to_ipc)
    }

    #[cfg(not(debug_assertions))]
    {
        let _ = (app, route);
        Err(debug_notification_disabled())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_notification_text_falls_back_when_blank_or_control_only() {
        assert_eq!(
            super::normalize_test_notification_text("\n\t", "fallback"),
            "fallback"
        );
        assert_eq!(
            super::normalize_test_notification_text("\u{0007}Ready", "fallback"),
            "Ready"
        );
    }

    #[test]
    fn test_notification_text_is_bounded() {
        let long_text = "a".repeat(super::TEST_NOTIFICATION_TEXT_LIMIT + 10);

        assert_eq!(
            super::normalize_test_notification_text(&long_text, "fallback").len(),
            super::TEST_NOTIFICATION_TEXT_LIMIT
        );
    }
}
