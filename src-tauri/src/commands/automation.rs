use chrono::{DateTime, Utc};
use serde::Serialize;
use tauri::command;

use crate::ipc_error::IpcError;
use crate::runtime_state::AutomationRuntimeState;

/// Canonical "Automation not available" error — surfaced when commands
/// require a live automation controller but the feature is disabled or
/// unwired. service.unavailable lets the frontend surface a graceful
/// feature-disabled message.
fn automation_not_available() -> IpcError {
    IpcError::new("service.unavailable", "Automation not available")
}

#[derive(Serialize)]
pub struct AutomationAvailabilityDto {
    pub available: bool,
}

#[derive(Serialize)]
pub struct PresetDto {
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: String,
}

#[derive(Serialize, Default)]
pub struct PresetRunResultDto {
    pub success: bool,
    pub message: String,
    pub steps_executed: usize,
    pub total_steps: usize,
}

/// Check if automation controller is available (distinct from system::get_automation_status).
#[command]
pub async fn check_automation_available(
    state: tauri::State<'_, AutomationRuntimeState>,
) -> Result<AutomationAvailabilityDto, IpcError> {
    Ok(AutomationAvailabilityDto {
        available: state.controller().is_some(),
    })
}

/// List available workflow presets.
#[command]
pub async fn list_automation_presets(
    _state: tauri::State<'_, AutomationRuntimeState>,
) -> Result<Vec<PresetDto>, IpcError> {
    // Presets are loaded from config at startup. For now return empty
    // until the controller exposes a preset listing method.
    // The web dashboard uses REST API which has full preset CRUD.
    Ok(vec![])
}

/// Run a workflow preset by ID.
#[command]
pub async fn run_automation_preset(
    state: tauri::State<'_, AutomationRuntimeState>,
    preset_id: String,
) -> Result<PresetRunResultDto, IpcError> {
    let _controller = state.controller().ok_or_else(automation_not_available)?;

    // Preset lookup requires access to config or storage.
    // For now, return an error indicating the preset wasn't found.
    // Full implementation will query presets from the web API layer.
    Err(IpcError::new(
        "not_found.resource_missing",
        format!(
            "Preset '{}' not found via IPC. Use the web dashboard for preset management.",
            preset_id
        ),
    ))
}

/// Execute an automation action from a natural language hint.
#[command]
pub async fn execute_automation_hint(
    state: tauri::State<'_, AutomationRuntimeState>,
    hint: String,
) -> Result<String, IpcError> {
    let controller = state.controller().ok_or_else(automation_not_available)?;

    let command_id = uuid::Uuid::new_v4().to_string();
    let session_id = uuid::Uuid::new_v4().to_string();

    let result = controller
        .execute_intent_hint(&command_id, &session_id, &hint)
        .await
        .map_err(IpcError::from)?;

    serde_json::to_string(&result).map_err(IpcError::from)
}

/// Analyze the current screen for automation targets.
#[command]
pub async fn analyze_automation_scene(
    state: tauri::State<'_, AutomationRuntimeState>,
) -> Result<String, IpcError> {
    let controller = state.controller().ok_or_else(automation_not_available)?;

    let scene = controller
        .analyze_scene(None, None)
        .await
        .map_err(IpcError::from)?;

    serde_json::to_string(&scene).map_err(IpcError::from)
}

// ── Confirmation flow ──

#[derive(Serialize)]
pub struct PendingConfirmationDto {
    pub command_id: String,
    pub nonce: String,
    pub process_name: String,
    pub args: Vec<String>,
    pub audit_level: String,
    pub requested_at: DateTime<Utc>,
}

/// List pending automation confirmations awaiting user response.
#[command]
pub async fn get_pending_confirmations(
    state: tauri::State<'_, AutomationRuntimeState>,
) -> Result<Vec<PendingConfirmationDto>, IpcError> {
    let controller = state.controller().ok_or_else(automation_not_available)?;

    let confirmations = controller
        .list_pending_confirmations()
        .await
        .map_err(IpcError::from)?;

    Ok(confirmations
        .into_iter()
        .map(|c| PendingConfirmationDto {
            command_id: c.command_id,
            nonce: c.nonce,
            process_name: c.process_name,
            args: c.args,
            audit_level: c.audit_level,
            requested_at: c.requested_at,
        })
        .collect())
}

/// Submit user's confirmation decision for a pending automation command.
/// The `nonce` must match the value from the original confirmation request.
#[command]
pub async fn confirm_automation_command(
    state: tauri::State<'_, AutomationRuntimeState>,
    command_id: String,
    nonce: String,
    approved: bool,
) -> Result<(), IpcError> {
    let controller = state.controller().ok_or_else(automation_not_available)?;

    controller
        .submit_confirmation(&command_id, &nonce, approved)
        .await
        .map_err(IpcError::from)
}
