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
    let file_name = format!("oneshim-report-{bug_id}.json");

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
