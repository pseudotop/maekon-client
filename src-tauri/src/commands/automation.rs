// #5703: Automation IPC commands — state wiring fixed so AutomationRuntimeState.controller
// is no longer permanently None. list_automation_presets and run_automation_preset now have
// real implementations mirroring the REST AutomationQueryService / AutomationCommandService
// paths. The 5 other automation IPCs (execute_automation_hint, analyze_automation_scene,
// get_pending_confirmations, confirm_automation_command, check_automation_available) light up
// automatically once the state carries the live controller.

use chrono::{DateTime, Utc};
use maekon_core::config::AppConfig;
use maekon_core::models::intent::{builtin_presets, WorkflowPreset};
use serde::Serialize;
use tauri::command;

use crate::ipc_error::IpcError;
use crate::runtime_state::{AutomationRuntimeState, ConfigRuntimeState};

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

// ── Pure helpers (no tauri::State — fully unit-testable) ──────────────────────

/// Resolve a preset by ID from the combined builtin+custom list.
///
/// Gate order mirrors the REST AutomationCommandService::run_preset path exactly:
///   1. Search builtins first (builtin wins on id collision with custom).
///   2. Search custom_presets if not found in builtins.
///   3. Return not_found if neither source has the id.
///   4. Validate ai_profile_id binding (invalid_arguments if drift detected).
///   5. Reject if automation.enabled == false (service.unavailable).
///
/// Returns the resolved `WorkflowPreset` on success. The caller is responsible
/// for obtaining the controller and calling `run_workflow`. (#5703)
fn resolve_run_preset(config: &AppConfig, preset_id: &str) -> Result<WorkflowPreset, IpcError> {
    // Step 1 + 2: builtin wins on collision (same order as REST list_presets and run_preset).
    let preset = builtin_presets()
        .into_iter()
        .find(|p| p.id == preset_id)
        .or_else(|| {
            config
                .automation
                .custom_presets
                .iter()
                .find(|p| p.id == preset_id)
                .cloned()
        });

    // Step 3: not found.
    let preset = preset.ok_or_else(|| {
        IpcError::new(
            "not_found.resource_missing",
            format!("Preset '{}' not found", preset_id),
        )
    })?;

    // Step 4: ai_profile binding check (mirrors validate_preset_ai_profile_binding in REST).
    if let Some(ai_profile_id) = preset
        .ai_profile_id
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        let exists = config
            .ai_provider
            .saved_profiles
            .iter()
            .any(|p| p.profile_id == ai_profile_id);
        if !exists {
            return Err(IpcError::new(
                "validation.invalid_arguments",
                format!(
                    "Preset '{}' references an unknown AI profile '{}'.",
                    preset.id, ai_profile_id
                ),
            ));
        }
    }

    // Step 5: automation master switch (same gate as REST run_preset).
    if !config.automation.enabled {
        return Err(IpcError::new(
            "service.unavailable",
            "Automation is disabled",
        ));
    }

    Ok(preset)
}

// ── IPC commands ──────────────────────────────────────────────────────────────

/// Check if automation controller is available (distinct from system::get_automation_status).
#[command]
pub async fn check_automation_available(
    state: tauri::State<'_, AutomationRuntimeState>,
) -> Result<AutomationAvailabilityDto, IpcError> {
    Ok(AutomationAvailabilityDto {
        available: state.controller().is_some(),
    })
}

/// List available workflow presets (builtin + custom from config).
///
/// Mirrors REST AutomationQueryService::list_presets. Does NOT require a live
/// controller — the list is sourced from config, same as the REST path. (#5703)
#[command]
pub async fn list_automation_presets(
    config_state: tauri::State<'_, ConfigRuntimeState>,
) -> Result<Vec<PresetDto>, IpcError> {
    let config = config_state.config_manager().get();
    let mut presets = builtin_presets();
    presets.extend(config.automation.custom_presets.clone());

    Ok(presets
        .into_iter()
        .map(|p| PresetDto {
            id: p.id,
            name: p.name,
            description: p.description,
            // PresetCategory Display emits PascalCase, matching the serde wire contract.
            category: p.category.to_string(),
        })
        .collect())
}

/// Run a workflow preset by ID.
///
/// Mirrors REST AutomationCommandService::run_preset gate order:
///   resolve (builtin-first) → ai_profile check → automation.enabled → controller.run_workflow
/// (#5703)
#[command]
pub async fn run_automation_preset(
    state: tauri::State<'_, AutomationRuntimeState>,
    config_state: tauri::State<'_, ConfigRuntimeState>,
    preset_id: String,
) -> Result<PresetRunResultDto, IpcError> {
    let config = config_state.config_manager().get();
    let preset = resolve_run_preset(&config, &preset_id)?;

    // Controller is required for actual execution (automation.enabled is already checked above).
    let controller = state.controller().ok_or_else(automation_not_available)?;

    match controller.run_workflow(&preset).await {
        Ok(result) => Ok(PresetRunResultDto {
            success: result.success,
            message: result.message,
            steps_executed: result.steps_executed,
            total_steps: result.total_steps,
        }),
        Err(e) => Err(IpcError::from(e)),
    }
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

// ── Unit tests (pure functions only — no tauri::State) ────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::config::AppConfig;
    use maekon_core::models::intent::{
        builtin_presets, AutomationIntent, PresetCategory, WorkflowPreset, WorkflowStep,
    };

    // Helper: build a minimal AppConfig with automation.enabled=true.
    fn enabled_config() -> AppConfig {
        let mut cfg = AppConfig::default_config();
        cfg.automation.enabled = true;
        cfg
    }

    // Helper: build a custom preset with given id, no ai_profile binding.
    fn custom_preset(id: &str) -> WorkflowPreset {
        WorkflowPreset {
            id: id.to_string(),
            name: format!("Custom {id}"),
            description: "test".to_string(),
            category: PresetCategory::Custom,
            steps: vec![WorkflowStep {
                name: "noop".to_string(),
                intent: AutomationIntent::ExecuteHotkey {
                    keys: vec!["Ctrl".to_string(), "Z".to_string()],
                },
                delay_ms: 0,
                stop_on_failure: false,
            }],
            builtin: false,
            platform: None,
            ai_profile_id: None,
        }
    }

    #[test]
    fn resolve_builtin_save_file() {
        // 'save-file' is a guaranteed builtin on all platforms.
        let cfg = enabled_config();
        let preset = resolve_run_preset(&cfg, "save-file").unwrap();
        assert_eq!(preset.id, "save-file");
        assert!(preset.builtin);
    }

    #[test]
    fn resolve_custom_preset() {
        let mut cfg = enabled_config();
        cfg.automation
            .custom_presets
            .push(custom_preset("my-custom"));
        let preset = resolve_run_preset(&cfg, "my-custom").unwrap();
        assert_eq!(preset.id, "my-custom");
        assert!(!preset.builtin);
    }

    #[test]
    fn builtin_beats_custom_on_same_id() {
        // Mirrors REST run_preset which searches builtins first.
        let mut cfg = enabled_config();
        let mut collider = custom_preset("save-file");
        collider.name = "impostor".to_string();
        cfg.automation.custom_presets.push(collider);
        let preset = resolve_run_preset(&cfg, "save-file").unwrap();
        // Should resolve to the real builtin, not the custom impostor.
        assert!(preset.builtin, "builtin must win on id collision");
        assert_ne!(preset.name, "impostor");
    }

    #[test]
    fn unknown_id_returns_not_found() {
        let cfg = enabled_config();
        let err = resolve_run_preset(&cfg, "does-not-exist").unwrap_err();
        assert_eq!(err.code, "not_found.resource_missing");
        assert!(err.message.contains("does-not-exist"));
    }

    #[test]
    fn disabled_automation_returns_unavailable() {
        let mut cfg = AppConfig::default_config();
        cfg.automation.enabled = false; // master switch off
        let err = resolve_run_preset(&cfg, "save-file").unwrap_err();
        assert_eq!(err.code, "service.unavailable");
        assert!(err.message.contains("disabled"));
    }

    #[test]
    fn ai_profile_drift_returns_invalid_arguments() {
        let mut cfg = enabled_config();
        // Custom preset references a profile id that does not exist in saved_profiles.
        let mut p = custom_preset("drifted");
        p.ai_profile_id = Some("missing-profile-id".to_string());
        cfg.automation.custom_presets.push(p);
        let err = resolve_run_preset(&cfg, "drifted").unwrap_err();
        assert_eq!(err.code, "validation.invalid_arguments");
        assert!(err.message.contains("missing-profile-id"));
    }

    #[test]
    fn blank_ai_profile_id_is_ok() {
        // An empty or whitespace-only ai_profile_id is treated as "unset" — no validation error.
        let mut cfg = enabled_config();
        let mut p = custom_preset("blank-profile");
        p.ai_profile_id = Some("   ".to_string());
        cfg.automation.custom_presets.push(p);
        resolve_run_preset(&cfg, "blank-profile").expect("blank ai_profile_id should be accepted");
    }

    #[test]
    fn preset_dto_category_is_pascal_case() {
        // PresetCategory::Display must emit PascalCase to match the serde wire contract
        // (serde rename_all = "PascalCase" on the enum).
        assert_eq!(PresetCategory::Productivity.to_string(), "Productivity");
        assert_eq!(PresetCategory::AppManagement.to_string(), "AppManagement");
        assert_eq!(PresetCategory::Workflow.to_string(), "Workflow");
        assert_eq!(PresetCategory::Custom.to_string(), "Custom");
    }

    #[test]
    fn builtin_presets_count_at_least_seven() {
        // builtin_presets() returns >=7 on any platform (save-file, undo, select-all-copy,
        // find-replace, switch-next-app, close-window, minimize-all).
        // Additional workflow presets (morning-routine, meeting-prep, …) push the total
        // higher — do not hardcode an exact count that would vary by platform.
        let presets = builtin_presets();
        assert!(
            presets.len() >= 7,
            "expected at least 7 builtins, got {}",
            presets.len()
        );
        // All builtins must have the builtin flag set.
        for p in &presets {
            assert!(p.builtin, "preset '{}' is missing builtin=true", p.id);
        }
    }
}
