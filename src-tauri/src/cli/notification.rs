use super::{
    append_debug_notification_audit_jsonl, debug_notification_activation_route_from,
    debug_notification_backend_from, debug_notification_cli_activation_output_path_from,
    debug_notification_cli_diagnostic_jsonl_path_from, emit_debug_notification_cli_json,
    emit_debug_notification_cli_marker_json, hold_debug_notification_cli_if_requested,
    DebugNotificationBackend, DebugNotificationCliCommand,
};

pub(crate) fn debug_notification_audit_event_payload(
    command: &str,
    backend: DebugNotificationBackend,
    ok: bool,
    title: &str,
    body: &str,
    activation_route: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "event": format!("debug_notification.{command}"),
        "backend": backend.as_str(),
        "ok": ok,
        "title_present": !title.is_empty(),
        "title_len": title.len(),
        "body_present": !body.is_empty(),
        "body_len": body.len(),
        "activation_route": activation_route,
    })
}

pub(crate) fn debug_macos_notification_category_identifier() -> &'static str {
    "maekon.debug.notification.activation"
}

pub(crate) fn debug_macos_notification_open_action_identifier() -> &'static str {
    "maekon.debug.notification.open"
}

pub(crate) fn run_debug_notification_cli_command<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    command: DebugNotificationCliCommand,
) -> i32 {
    let payload = match command {
        DebugNotificationCliCommand::Status => {
            let snapshot = crate::desktop_permissions::get_desktop_permission_snapshot(app_handle);
            serde_json::json!({
                "debug_notification": true,
                "command": "status",
                "permissions": snapshot,
            })
        }
        DebugNotificationCliCommand::Request => {
            let started_payload = serde_json::json!({
                "debug_notification": true,
                "command": "request",
                "phase": "started",
            });
            let started_payload = serde_json::to_string(&started_payload).unwrap_or_else(|_| {
                "{\"debug_notification\":true,\"command\":\"request\",\"phase\":\"started\"}"
                    .to_string()
            });
            let marker_exit_code = emit_debug_notification_cli_marker_json(&started_payload);
            if marker_exit_code != 0 {
                return marker_exit_code;
            }

            match crate::desktop_permissions::request_desktop_notification_permission(app_handle) {
                Ok(snapshot) => serde_json::json!({
                    "debug_notification": true,
                    "command": "request",
                    "ok": true,
                    "permissions": snapshot,
                }),
                Err(error) => serde_json::json!({
                    "debug_notification": true,
                    "command": "request",
                    "ok": false,
                    "error": error,
                }),
            }
        }
        DebugNotificationCliCommand::Send => {
            let backend = debug_notification_backend_from(
                std::env::var("MAEKON_DEBUG_NOTIFICATION_BACKEND")
                    .ok()
                    .as_deref(),
            );
            let activation_route = debug_notification_activation_route_from(
                std::env::var("MAEKON_DEBUG_NOTIFICATION_ACTIVATION_ROUTE")
                    .ok()
                    .as_deref(),
            );
            let activation_output_path = debug_notification_cli_activation_output_path_from(
                std::env::var("MAEKON_DEBUG_NOTIFICATION_ACTIVATION_OUTPUT")
                    .ok()
                    .as_deref(),
            );
            let diagnostic_jsonl_path = debug_notification_cli_diagnostic_jsonl_path_from(
                std::env::var("MAEKON_DEBUG_NOTIFICATION_DIAGNOSTIC_JSONL")
                    .ok()
                    .as_deref(),
            );
            let title = std::env::var("MAEKON_DEBUG_NOTIFICATION_TITLE")
                .unwrap_or_else(|_| "Maekon Debug".to_string());
            let body = std::env::var("MAEKON_DEBUG_NOTIFICATION_BODY")
                .unwrap_or_else(|_| "Debug notification body".to_string());

            let (ok, error_message) = match backend {
                DebugNotificationBackend::TauriPlugin => {
                    let notification =
                        tauri_plugin_notification::NotificationExt::notification(app_handle)
                            .builder()
                            .title(&title)
                            .body(&body);
                    match notification.show() {
                        Ok(()) => (true, None),
                        Err(error) => (false, Some(error.to_string())),
                    }
                }
                #[cfg(target_os = "macos")]
                DebugNotificationBackend::MacosUnuser => {
                    match super::notification_macos::show_debug_macos_unuser_notification(
                        app_handle,
                        &title,
                        &body,
                        activation_route.as_deref(),
                        activation_output_path,
                        diagnostic_jsonl_path,
                    ) {
                        Ok(()) => (true, None),
                        Err(error) => (false, Some(error)),
                    }
                }
                #[cfg(not(target_os = "macos"))]
                DebugNotificationBackend::MacosUnuser => (
                    false,
                    Some("macos-unuser backend requires macOS".to_string()),
                ),
            };

            let audit_payload = debug_notification_audit_event_payload(
                "send",
                backend,
                ok,
                &title,
                &body,
                activation_route.as_deref(),
            );
            let audit_exit_code = append_debug_notification_audit_jsonl(&audit_payload);
            if audit_exit_code != 0 {
                return audit_exit_code;
            }

            match error_message {
                None => serde_json::json!({
                    "debug_notification": true,
                    "command": "send",
                    "ok": true,
                    "backend": backend.as_str(),
                }),
                Some(error) => serde_json::json!({
                    "debug_notification": true,
                    "command": "send",
                    "ok": false,
                    "backend": backend.as_str(),
                    "error": error,
                }),
            }
        }
    };

    let payload = serde_json::to_string(&payload).unwrap_or_else(|_| {
        "{\"debug_notification\":true,\"ok\":false,\"error\":\"json serialization failed\"}"
            .to_string()
    });
    let exit_code = emit_debug_notification_cli_json(&payload);
    if exit_code == 0 {
        hold_debug_notification_cli_if_requested();
    }
    exit_code
}
