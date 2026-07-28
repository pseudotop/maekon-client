use serde::Serialize;
use tauri::{command, AppHandle};
#[cfg(debug_assertions)]
use tauri::{Emitter, Manager};
use tauri_plugin_notification::NotificationExt;
#[cfg(debug_assertions)]
use tracing::debug;

use crate::ipc_error::IpcError;
use crate::notification_manager::NotificationActivationOutcome;
#[cfg(debug_assertions)]
use crate::notification_manager::{
    notification_activation_outcome_from_route, NotificationActivationError,
};

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

    app.notification()
        .builder()
        .title(&title)
        .body(&body)
        .show()
        .map_err(|error| IpcError::new("service.unavailable", error.to_string()))?;

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
        let outcome = notification_activation_outcome_from_route(route.as_deref())
            .map_err(activation_error_to_ipc)?;

        if outcome.focus_main_window {
            if let Some(window) = app.get_webview_window("main") {
                if let Err(e) = window.show() {
                    debug!("notification activation window show failed: {e}");
                }
                if let Err(e) = window.set_focus() {
                    debug!("notification activation window focus failed: {e}");
                }
            }
        }

        app.emit_to("main", outcome.event_name, &outcome.route)
            .map_err(|e| IpcError::new("internal.generic", e.to_string()))?;
        Ok(outcome)
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
