//! Production Windows toast activation routing (#9182).
//!
//! Tauri's desktop notification plugin exposes delivery but not activation.
//! Windows already ships the lower-level WinRT adapter in the dependency graph,
//! so this module attaches the native callback directly while retaining the
//! plugin path on other operating systems.

use tauri::AppHandle;
#[cfg(any(target_os = "windows", debug_assertions))]
use tauri::{Emitter, Manager};

#[cfg(all(test, not(any(target_os = "windows", debug_assertions))))]
use crate::notification_manager::notification_activation_outcome_from_route;
#[cfg(any(target_os = "windows", debug_assertions))]
use crate::notification_manager::{
    notification_activation_outcome_from_route, NotificationActivationError,
    NotificationActivationOutcome,
};

pub(crate) const DEFAULT_NOTIFICATION_ROUTE: &str = "/replay/timeline";
#[cfg(any(target_os = "windows", debug_assertions))]
const NOTIFICATION_AUDIT_ID: &str = "system.notification_activation";

#[cfg(target_os = "windows")]
pub(crate) fn maybe_spawn_test_notification_from_env(app: &AppHandle) {
    if std::env::var("MAEKON_TEST_WINDOWS_NOTIFICATION")
        .ok()
        .as_deref()
        != Some("1")
        || std::env::var("ONESHIM_TEST_MODE").ok().as_deref() != Some("1")
    {
        return;
    }

    append_test_ready_receipt(app);

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        let token = std::env::var("MAEKON_TEST_WINDOWS_NOTIFICATION_TOKEN")
            .ok()
            .filter(|value| {
                !value.is_empty()
                    && value.len() <= 32
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            });
        let title = token.map_or_else(
            || "Maekon Windows activation test".to_string(),
            |value| format!("Maekon Windows activation test {value}"),
        );
        if let Err(error) = show_actionable_notification(
            &app,
            &title,
            "Open the activity timeline",
            DEFAULT_NOTIFICATION_ROUTE,
        ) {
            tracing::warn!(error = %error, "Windows activation test notification failed");
        }
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(any(target_os = "windows", debug_assertions))]
enum ActivationAuditStatus {
    Completed,
    Denied,
    Failed,
}

#[cfg(any(target_os = "windows", debug_assertions))]
impl ActivationAuditStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Denied => "denied",
            Self::Failed => "failed",
        }
    }
}

/// Restore the existing main window and emit exactly one allowlisted route.
/// This is the production callback target; callers must never navigate before
/// this function has validated the native payload.
#[cfg(any(target_os = "windows", debug_assertions))]
pub(crate) fn activate_notification(
    app: &AppHandle,
    route: Option<&str>,
) -> Result<NotificationActivationOutcome, NotificationActivationError> {
    let activation_id = maekon_core::id_generation::generate_id("notification_activation");
    let outcome = match notification_activation_outcome_from_route(route) {
        Ok(outcome) => outcome,
        Err(error) => {
            spawn_activation_audit(
                app,
                activation_id,
                None,
                ActivationAuditStatus::Denied,
                Some(error.message()),
            );
            return Err(error);
        }
    };

    let Some(window) = app.get_webview_window("main") else {
        spawn_activation_audit(
            app,
            activation_id,
            Some(outcome.route.clone()),
            ActivationAuditStatus::Failed,
            Some("main_window_unavailable"),
        );
        return Err(NotificationActivationError::InvalidRoute);
    };

    crate::window_state::show_restore_and_focus_main_window(&window);

    if let Err(error) = app.emit_to("main", outcome.event_name, &outcome.route) {
        tracing::warn!(error = %error, "notification activation route emit failed");
        spawn_activation_audit(
            app,
            activation_id,
            Some(outcome.route.clone()),
            ActivationAuditStatus::Failed,
            Some("route_emit_failed"),
        );
        return Err(NotificationActivationError::InvalidRoute);
    }

    spawn_activation_audit(
        app,
        activation_id,
        Some(outcome.route.clone()),
        ActivationAuditStatus::Completed,
        None,
    );
    Ok(outcome)
}

#[cfg(target_os = "windows")]
pub(crate) fn show_actionable_notification(
    app: &AppHandle,
    title: &str,
    body: &str,
    route: &str,
) -> Result<(), String> {
    notification_activation_outcome_from_route(Some(route)).map_err(|error| error.message())?;

    let app_handle = app.clone();
    let fallback_route = route.to_string();
    let app_id = windows_notification_app_id();
    tauri_winrt_notification::Toast::new(&app_id)
        .title(title)
        .text1(body)
        .add_button("Open", route)
        .on_activated(move |action| {
            let route = action.as_deref().unwrap_or(&fallback_route);
            if let Err(error) = activate_notification(&app_handle, Some(route)) {
                tracing::warn!(
                    reason = error.message(),
                    "Windows notification activation denied"
                );
            }
            Ok(())
        })
        .show()
        .map_err(|error| error.to_string())
}

#[cfg(target_os = "windows")]
fn windows_notification_app_id() -> String {
    // An unpackaged debug binary has no Start-menu shortcut registering the
    // production AUMID. Use Windows' documented PowerShell fallback only in
    // the isolated test profile so the shell can display the real WinRT toast;
    // installed/release builds retain the bundle identifier registered by the
    // Tauri installer and therefore appear as Maekon.
    if std::env::var("ONESHIM_TEST_MODE").ok().as_deref() == Some("1") {
        tauri_winrt_notification::Toast::POWERSHELL_APP_ID.to_string()
    } else {
        "com.maekon.app".to_string()
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn show_actionable_notification(
    app: &AppHandle,
    title: &str,
    body: &str,
    _route: &str,
) -> Result<(), String> {
    use tauri_plugin_notification::NotificationExt;

    app.notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .map_err(|error| error.to_string())
}

#[cfg(any(target_os = "windows", debug_assertions))]
fn spawn_activation_audit(
    app: &AppHandle,
    activation_id: String,
    route: Option<String>,
    status: ActivationAuditStatus,
    reason: Option<&'static str>,
) {
    let Some(state) = app.try_state::<crate::runtime_state::AppState>() else {
        tracing::warn!("notification activation audit unavailable: AppState missing");
        return;
    };
    let storage = state.storage.clone();
    let reason = reason.map(str::to_string);
    tauri::async_runtime::spawn(async move {
        let result = tokio::task::spawn_blocking(move || {
            use maekon_core::models::audit::{AuditEntry, AuditStatus};

            let audit_status = match status {
                ActivationAuditStatus::Completed => AuditStatus::Completed,
                ActivationAuditStatus::Denied => AuditStatus::Denied,
                ActivationAuditStatus::Failed => AuditStatus::Failed,
            };
            let receipt_activation_id = activation_id.clone();
            let receipt_route = route.clone();
            storage.save_audit_entry(&AuditEntry {
                entry_id: maekon_core::id_generation::generate_id("audit"),
                timestamp: chrono::Utc::now(),
                session_id: NOTIFICATION_AUDIT_ID.into(),
                command_id: activation_id,
                action_type: "windows_notification_activated".into(),
                status: audit_status,
                details: Some(
                    serde_json::json!({
                        "source": "windows_winrt_toast",
                        "route": route,
                        "reason": reason,
                    })
                    .to_string(),
                ),
                execution_time_ms: None,
            });
            if status == ActivationAuditStatus::Completed {
                let persisted = storage
                    .entries_by_command_id(&receipt_activation_id, 1)
                    .into_iter()
                    .next()
                    .is_some_and(|entry| {
                        entry.action_type == "windows_notification_activated"
                            && entry.status == AuditStatus::Completed
                    });
                if persisted {
                    if let Some(route) = receipt_route.as_deref() {
                        append_test_receipt(&receipt_activation_id, route, status);
                    }
                } else {
                    tracing::warn!(
                        activation_id = %receipt_activation_id,
                        "notification activation durable audit receipt unavailable"
                    );
                }
            }
        })
        .await;
        if let Err(error) = result {
            tracing::warn!(error = %error, "notification activation audit task failed");
        }
    });
}

/// Optional, sanitized JSONL observer for the Windows UIA proof lane. It is
/// inert unless the isolated test process supplies an explicit receipt path.
#[cfg(any(target_os = "windows", debug_assertions))]
fn append_test_receipt(activation_id: &str, route: &str, status: ActivationAuditStatus) {
    let Ok(path) = std::env::var("MAEKON_NOTIFICATION_ACTIVATION_RECEIPT_PATH") else {
        return;
    };
    if std::env::var("ONESHIM_TEST_MODE").ok().as_deref() != Some("1") {
        return;
    }

    let receipt = serde_json::json!({
        "schema_version": "maekon.windows-notification-activation-receipt.v1",
        "activation_id": activation_id,
        "source": "windows_winrt_toast",
        "route": route,
        "status": status.as_str(),
        "process_id": std::process::id(),
    });
    let write_result = (|| -> std::io::Result<()> {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        writeln!(file, "{receipt}")
    })();
    if let Err(error) = write_result {
        tracing::warn!(error = %error, "notification activation test receipt write failed");
    }
}

#[cfg(target_os = "windows")]
fn append_test_ready_receipt(app: &AppHandle) {
    let Ok(path) = std::env::var("MAEKON_NOTIFICATION_ACTIVATION_RECEIPT_PATH") else {
        return;
    };
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let Some(handle) = crate::window_state::windows_webview_handle(&window) else {
        return;
    };
    let receipt = serde_json::json!({
        "schema_version": "maekon.windows-notification-activation-receipt.v1",
        "source": "windows_winrt_toast",
        "route": DEFAULT_NOTIFICATION_ROUTE,
        "status": "ready",
        "process_id": std::process::id(),
        "main_window_handle": handle as usize,
    });
    let write_result = (|| -> std::io::Result<()> {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        writeln!(file, "{receipt}")
    })();
    if let Err(error) = write_result {
        tracing::warn!(error = %error, "notification activation ready receipt write failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_default_route_is_allowlisted() {
        let outcome = notification_activation_outcome_from_route(Some(DEFAULT_NOTIFICATION_ROUTE))
            .expect("default notification route must remain allowlisted");
        assert_eq!(outcome.route, "/replay/timeline");
    }

    #[test]
    fn production_activation_id_prefix_satisfies_adr_022() {
        let id = maekon_core::id_generation::generate_id("notification_activation");
        assert!(id.starts_with("notification_activation_"));
    }
}
