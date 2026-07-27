/// Tauri IPC command for exporting bug report bundles to a user-selected file.
///
/// Uses `tauri-plugin-dialog` for native save-file dialog and writes the
/// JSON bundle to the chosen path.
use crate::ipc_error::IpcError;

#[tauri::command]
pub async fn export_bug_report(
    app: tauri::AppHandle,
    bug_id: String,
    bundle_json: String,
) -> Result<Option<String>, IpcError> {
    use tauri_plugin_dialog::DialogExt;

    // blocking_save_file must run off the async executor
    let dialog = app.dialog().clone();
    let file_name = format!("maekon-report-{bug_id}.json");

    let path = tokio::task::spawn_blocking(move || {
        dialog
            .file()
            .set_file_name(file_name)
            .add_filter("JSON", &["json"])
            .blocking_save_file()
    })
    .await
    .map_err(|e| IpcError::new("internal.generic", format!("dialog task failed: {e}")))?;

    match path {
        Some(file_path) => {
            let p = file_path.as_path().ok_or_else(|| {
                IpcError::new("validation.invalid_arguments", "invalid file path")
            })?;
            tokio::fs::write(p, &bundle_json)
                .await
                .map_err(IpcError::from)?;
            Ok(Some(p.display().to_string()))
        }
        None => Ok(None), // User cancelled the dialog
    }
}

/// Opens the real native save dialog for the bounded Windows UIA proof.
///
/// The hook exists only in debug builds and remains inert unless the private
/// harness supplies the explicit gate. It exercises the same command and
/// `tauri-plugin-dialog` path as the product UI; the harness cancels the dialog,
/// so it cannot write a report or overwrite user data.
#[cfg(debug_assertions)]
pub(crate) fn maybe_spawn_debug_native_dialog_from_env(app: &tauri::AppHandle) {
    let enabled = std::env::var("MAEKON_TEST_NATIVE_DIALOG")
        .ok()
        .is_some_and(|value| {
            matches!(value.trim(), "1")
                || value.eq_ignore_ascii_case("true")
                || value.eq_ignore_ascii_case("yes")
                || value.eq_ignore_ascii_case("on")
        });
    if !enabled {
        return;
    }

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let outcome = export_bug_report(
            app,
            "uia-native-dialog".to_owned(),
            "{\"source\":\"private-windows-uia-proof\"}".to_owned(),
        )
        .await;
        match outcome {
            Ok(None) => tracing::info!(
                target: "maekon_app::native_dialog",
                action = "export_bug_report",
                outcome = "cancelled",
                "native dialog proof completed"
            ),
            Ok(Some(_)) => tracing::warn!(
                target: "maekon_app::native_dialog",
                action = "export_bug_report",
                outcome = "unexpected_save",
                "native dialog proof unexpectedly selected a path"
            ),
            Err(error) => tracing::warn!(
                target: "maekon_app::native_dialog",
                action = "export_bug_report",
                outcome = "error",
                error.code = %error.code,
                "native dialog proof failed"
            ),
        }
    });
}
