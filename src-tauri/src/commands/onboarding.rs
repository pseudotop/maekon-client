use serde::Serialize;
use tauri::{command, State};

use crate::ipc_error::IpcError;
use crate::runtime_state::AppState;

#[derive(Serialize)]
pub struct OnboardingStatus {
    pub completed: bool,
}

/// F-RR-06: get_meta acquires a parking_lot read lock (blocking).
/// Move it off the tokio reactor thread via spawn_blocking.
#[command]
pub async fn get_onboarding_status(
    state: State<'_, AppState>,
) -> Result<OnboardingStatus, IpcError> {
    let storage = state.storage.clone();
    let completed = tokio::task::spawn_blocking(move || {
        storage
            .get_meta("onboarding_completed")
            .map(|v| v == "true")
            .unwrap_or(false)
    })
    .await
    .map_err(|join_err| {
        IpcError::new(
            "internal.generic",
            format!("get_onboarding_status task join failed: {join_err}"),
        )
    })?;
    Ok(OnboardingStatus { completed })
}

/// F-RR-06: set_meta acquires a parking_lot write lock (blocking).
/// Move it off the tokio reactor thread via spawn_blocking.
#[command]
pub async fn complete_onboarding(state: State<'_, AppState>) -> Result<(), IpcError> {
    let storage = state.storage.clone();
    tokio::task::spawn_blocking(move || {
        storage.set_meta("onboarding_completed", "true");
    })
    .await
    .map_err(|join_err| {
        IpcError::new(
            "internal.generic",
            format!("complete_onboarding task join failed: {join_err}"),
        )
    })
}

/// F-RR-06: delete_meta acquires a parking_lot write lock (blocking).
/// Move it off the tokio reactor thread via spawn_blocking.
#[command]
pub async fn reset_onboarding(state: State<'_, AppState>) -> Result<(), IpcError> {
    let storage = state.storage.clone();
    tokio::task::spawn_blocking(move || {
        storage.delete_meta("onboarding_completed");
    })
    .await
    .map_err(|join_err| {
        IpcError::new(
            "internal.generic",
            format!("reset_onboarding task join failed: {join_err}"),
        )
    })
}
