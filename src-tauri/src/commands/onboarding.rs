use serde::Serialize;
use tauri::{command, State};

use crate::ipc_error::IpcError;
use crate::runtime_state::AppState;

#[derive(Serialize)]
pub struct OnboardingStatus {
    pub completed: bool,
}

#[command]
pub async fn get_onboarding_status(
    state: State<'_, AppState>,
) -> Result<OnboardingStatus, IpcError> {
    let completed = state
        .storage
        .get_meta("onboarding_completed")
        .map(|v| v == "true")
        .unwrap_or(false);
    Ok(OnboardingStatus { completed })
}

#[command]
pub async fn complete_onboarding(state: State<'_, AppState>) -> Result<(), IpcError> {
    state.storage.set_meta("onboarding_completed", "true");
    Ok(())
}

#[command]
pub async fn reset_onboarding(state: State<'_, AppState>) -> Result<(), IpcError> {
    state.storage.delete_meta("onboarding_completed");
    Ok(())
}
