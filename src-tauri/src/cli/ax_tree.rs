use super::{emit_debug_ax_tree_cli_json, DebugAxTreeCliCommand};

pub(crate) fn run_debug_ax_tree_cli_command(command: DebugAxTreeCliCommand) -> i32 {
    #[cfg(target_os = "macos")]
    {
        use maekon_core::ports::accessibility::AccessibilityExtractor;

        let DebugAxTreeCliCommand::Extract {
            app_name,
            max_depth,
            max_elements,
        } = command;
        let extractor = maekon_vision::accessibility::MacOsNativeAccessibility::new();
        let permission_granted = extractor.has_permission();

        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                let payload = serde_json::json!({
                    "debug_ax_tree": true,
                    "command": "extract",
                    "ok": false,
                    "requested_app_name": app_name,
                    "permission_granted": permission_granted,
                    "error_code": "internal.generic",
                    "error_message": format!("tokio runtime init failed: {error}"),
                });
                let payload = serde_json::to_string(&payload).unwrap_or_else(|_| {
                    "{\"debug_ax_tree\":true,\"ok\":false,\"error_message\":\"json serialization failed\"}"
                        .to_string()
                });
                return emit_debug_ax_tree_cli_json(&payload);
            }
        };

        let result = runtime.block_on(extractor.extract_application_elements(
            &app_name,
            max_depth,
            max_elements,
            maekon_core::config::PiiFilterLevel::Standard,
            false,
        ));

        let payload = match result {
            Ok(elements) => serde_json::json!({
                "debug_ax_tree": true,
                "command": "extract",
                "ok": true,
                "requested_app_name": app_name,
                "permission_granted": permission_granted,
                "max_depth": max_depth,
                "max_elements": max_elements,
                "element_count": elements.len(),
                "elements": elements,
            }),
            Err(error) => serde_json::json!({
                "debug_ax_tree": true,
                "command": "extract",
                "ok": false,
                "requested_app_name": app_name,
                "permission_granted": permission_granted,
                "max_depth": max_depth,
                "max_elements": max_elements,
                "element_count": 0,
                "elements": [],
                "error_code": error.code(),
                "error_message": error.to_string(),
            }),
        };

        let payload = serde_json::to_string(&payload).unwrap_or_else(|_| {
            "{\"debug_ax_tree\":true,\"ok\":false,\"error_message\":\"json serialization failed\"}"
                .to_string()
        });
        emit_debug_ax_tree_cli_json(&payload)
    }

    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("debug-ax-tree is only supported on macOS");
        let _ = command;
        2
    }
}
