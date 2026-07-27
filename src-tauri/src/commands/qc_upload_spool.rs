//! Fail-closed IPC bridge for the debug-only CJ-05-05 upload-spool fixture.
//!
//! The commands remain harmless in release/no-analysis builds: status returns
//! `null`, and mutation returns `service.unavailable`. The implementation is
//! compiled only for debug builds with the network adapter feature.

use crate::ipc_error::IpcError;
use crate::runtime_state::AppState;

#[tauri::command]
pub async fn get_qc_upload_spool_status(
    state: tauri::State<'_, AppState>,
) -> Result<Option<serde_json::Value>, IpcError> {
    #[cfg(all(debug_assertions, feature = "analysis"))]
    {
        let data_dir = maekon_core::config_manager::ConfigManager::data_dir().map_err(|error| {
            IpcError::new(
                "internal.generic",
                format!("resolve isolated QC upload-spool data directory: {error}"),
            )
        })?;
        return crate::qc_upload_spool::status_from_env(&state.storage, &data_dir)
            .await
            .map_err(IpcError::from)?
            .map(serde_json::to_value)
            .transpose()
            .map_err(|error| {
                IpcError::new(
                    "internal.generic",
                    format!("serialize isolated QC upload-spool status: {error}"),
                )
            });
    }

    #[cfg(not(all(debug_assertions, feature = "analysis")))]
    {
        let _ = state;
        Ok(None)
    }
}

#[tauri::command]
pub async fn run_qc_upload_spool_step(
    state: tauri::State<'_, AppState>,
) -> Result<serde_json::Value, IpcError> {
    #[cfg(all(debug_assertions, feature = "analysis"))]
    {
        let data_dir = maekon_core::config_manager::ConfigManager::data_dir().map_err(|error| {
            IpcError::new(
                "internal.generic",
                format!("resolve isolated QC upload-spool data directory: {error}"),
            )
        })?;
        let status = crate::qc_upload_spool::run_step_from_env(&state.storage, &data_dir)
            .await
            .map_err(IpcError::from)?;
        serde_json::to_value(status).map_err(|error| {
            IpcError::new(
                "internal.generic",
                format!("serialize isolated QC upload-spool status: {error}"),
            )
        })
    }

    #[cfg(not(all(debug_assertions, feature = "analysis")))]
    {
        let _ = state;
        Err(IpcError::new(
            "service.unavailable",
            "QC upload-spool fixture is unavailable",
        ))
    }
}
