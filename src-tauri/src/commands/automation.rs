// #5703: Automation IPC commands — state wiring fixed so AutomationRuntimeState.controller
// is no longer permanently None. The confirmation-flow command below lights up
// automatically once the state carries the live controller.
//
// #7600: check_automation_available, list_automation_presets, run_automation_preset,
// execute_automation_hint, and analyze_automation_scene were removed as dead IPC
// duplicates — the React frontend drives these via the embedded HTTP API
// (POST/GET /automation/*) instead. See docs/architecture for the truth-in-UI
// reconciliation.
//
// #7683 F2: get_pending_confirmations was removed as a residual dead IPC —
// the overlay HITL confirm modal now receives pending confirmations exclusively
// via the pushed `automation:confirm-request` Tauri event (see
// automation_controller_builder.rs), never by polling this query. There is no
// catch-up query anywhere in useOverlayEvents.ts, so the command had zero callers.

use tauri::command;

use crate::ipc_error::IpcError;
use crate::runtime_state::{AppState, AutomationRuntimeState, SuggestionRuntimeState};

/// Canonical "Automation not available" error — surfaced when commands
/// require a live automation controller but the feature is disabled or
/// unwired. service.unavailable lets the frontend surface a graceful
/// feature-disabled message.
fn automation_not_available() -> IpcError {
    IpcError::new("service.unavailable", "Automation not available")
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

/// Run the automation action bound to a suggestion (T4.1 #7917, ADR-027).
///
/// This is the overlay's ONLY automation entry — deliberately scoped to bound
/// suggestions (contrast the #7600/#7683 dead-IPC removal above, which routed
/// generic automation through the embedded HTTP API). The command NEVER accepts
/// a preset id from the client: it re-derives the binding server-side from the
/// suggestion's own `(type, source)` (refusing non-RuleBased / network / LLM
/// origins) and routes execution through the existing `run_workflow` gate chain.
/// See `commands::suggestions::action` for the flow.
#[command]
pub async fn run_suggestion_action(
    suggestion_state: tauri::State<'_, SuggestionRuntimeState>,
    automation_state: tauri::State<'_, AutomationRuntimeState>,
    app_state: tauri::State<'_, AppState>,
    suggestion_id: String,
) -> Result<(), IpcError> {
    let controller = automation_state
        .controller()
        .ok_or_else(automation_not_available)?;

    crate::commands::suggestions::action::run_suggestion_action_inner(
        &suggestion_state,
        &app_state.storage,
        &app_state.config.automation,
        &controller,
        &suggestion_id,
    )
    .await
}

// ── Unit tests (pure functions only — no tauri::State) ────────────────────────

#[cfg(test)]
mod tests {
    use maekon_core::models::intent::{builtin_presets, PresetCategory};

    #[test]
    fn preset_category_is_pascal_case() {
        // PresetCategory::Display must emit PascalCase to match the serde wire contract
        // (serde rename_all = "PascalCase" on the enum). Still exercised by the REST
        // AutomationQueryService::list_presets path in maekon-web.
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
