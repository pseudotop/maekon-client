use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};

use maekon_api_contracts::automation::{
    AuditQuery, ExecuteIntentHintRequest, ExecuteSceneActionRequest, PolicyEventQuery,
};
use maekon_core::models::automation::ExecutionPolicyDto;

use crate::error::ApiError;
use crate::services::automation_service::{AutomationCommandService, AutomationQueryService};
use crate::services::web_contexts::AutomationWebContext;

pub async fn get_contract_versions(
) -> Result<Json<maekon_api_contracts::automation::AutomationContractsDto>, ApiError> {
    Ok(Json(AutomationQueryService::contract_versions()))
}

pub async fn get_automation_status(
    State(context): State<AutomationWebContext>,
) -> Result<Json<maekon_api_contracts::automation::AutomationStatusDto>, ApiError> {
    Ok(Json(
        AutomationQueryService::new(context)
            .automation_status()
            .await?,
    ))
}

pub async fn get_audit_logs(
    State(context): State<AutomationWebContext>,
    Query(query): Query<AuditQuery>,
) -> Result<Json<Vec<maekon_api_contracts::automation::AuditEntryDto>>, ApiError> {
    Ok(Json(
        AutomationQueryService::new(context)
            .audit_logs(query)
            .await?,
    ))
}

pub async fn get_policy_events(
    State(context): State<AutomationWebContext>,
    Query(query): Query<PolicyEventQuery>,
) -> Result<Json<Vec<maekon_api_contracts::automation::AuditEntryDto>>, ApiError> {
    Ok(Json(
        AutomationQueryService::new(context)
            .policy_events(query)
            .await?,
    ))
}

pub async fn get_policies(
    State(context): State<AutomationWebContext>,
) -> Result<Json<maekon_api_contracts::automation::PoliciesDto>, ApiError> {
    Ok(Json(AutomationQueryService::new(context).policies()))
}

pub async fn get_automation_stats(
    State(context): State<AutomationWebContext>,
) -> Result<Json<maekon_api_contracts::automation::AutomationStatsDto>, ApiError> {
    Ok(Json(
        AutomationQueryService::new(context)
            .automation_stats()
            .await,
    ))
}

pub async fn list_presets(
    State(context): State<AutomationWebContext>,
) -> Result<Json<maekon_api_contracts::automation::PresetListDto>, ApiError> {
    Ok(Json(AutomationQueryService::new(context).list_presets()))
}

pub async fn create_preset(
    State(context): State<AutomationWebContext>,
    Json(preset): Json<maekon_core::models::intent::WorkflowPreset>,
) -> Result<Json<maekon_core::models::intent::WorkflowPreset>, ApiError> {
    Ok(Json(
        AutomationCommandService::new(context).create_preset(preset)?,
    ))
}

pub async fn update_preset(
    State(context): State<AutomationWebContext>,
    Path(id): Path<String>,
    Json(preset): Json<maekon_core::models::intent::WorkflowPreset>,
) -> Result<Json<maekon_core::models::intent::WorkflowPreset>, ApiError> {
    Ok(Json(
        AutomationCommandService::new(context).update_preset(id, preset)?,
    ))
}

pub async fn delete_preset(
    State(context): State<AutomationWebContext>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(
        AutomationCommandService::new(context).delete_preset(id)?,
    ))
}

pub async fn run_preset(
    State(context): State<AutomationWebContext>,
    Path(id): Path<String>,
) -> Result<Json<maekon_api_contracts::automation::PresetRunResult>, ApiError> {
    Ok(Json(
        AutomationCommandService::new(context)
            .run_preset(id)
            .await?,
    ))
}

pub async fn execute_intent_hint(
    State(context): State<AutomationWebContext>,
    Json(req): Json<ExecuteIntentHintRequest>,
) -> Result<Json<maekon_api_contracts::automation::ExecuteIntentHintResponse>, ApiError> {
    Ok(Json(
        AutomationCommandService::new(context)
            .execute_intent_hint(req)
            .await?,
    ))
}

pub async fn execute_scene_action(
    State(context): State<AutomationWebContext>,
    Json(req): Json<ExecuteSceneActionRequest>,
) -> Result<Json<maekon_api_contracts::automation::ExecuteSceneActionResponse>, ApiError> {
    Ok(Json(
        AutomationCommandService::new(context)
            .execute_scene_action(req)
            .await?,
    ))
}

pub async fn list_execution_policies(
    State(context): State<AutomationWebContext>,
) -> Result<Json<Vec<ExecutionPolicyDto>>, ApiError> {
    let Some(ref controller) = context.automation_controller else {
        return Ok(Json(Vec::new()));
    };
    Ok(Json(controller.list_execution_policies().await?))
}

pub async fn create_execution_policy(
    State(context): State<AutomationWebContext>,
    Json(policy): Json<ExecutionPolicyDto>,
) -> Result<Json<ExecutionPolicyDto>, ApiError> {
    if policy.policy_id.trim().is_empty() || policy.process_name.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "policy_id and process_name are required".into(),
        ));
    }
    validate_execution_policy(&policy)?;
    let Some(ref controller) = context.automation_controller else {
        return Err(ApiError::BadRequest(
            "Automation controller is not active.".into(),
        ));
    };
    Ok(Json(controller.add_execution_policy(policy).await?))
}

pub async fn update_execution_policy(
    State(context): State<AutomationWebContext>,
    Path(id): Path<String>,
    Json(mut policy): Json<ExecutionPolicyDto>,
) -> Result<Json<ExecutionPolicyDto>, ApiError> {
    policy.policy_id = id;
    validate_execution_policy(&policy)?;
    let Some(ref controller) = context.automation_controller else {
        return Err(ApiError::BadRequest(
            "Automation controller is not active.".into(),
        ));
    };
    Ok(Json(controller.add_execution_policy(policy).await?))
}

pub async fn delete_execution_policy(
    State(context): State<AutomationWebContext>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let Some(ref controller) = context.automation_controller else {
        return Err(ApiError::BadRequest(
            "Automation controller is not active.".into(),
        ));
    };
    if controller.remove_execution_policy(&id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound("policy not found".into()))
    }
}

/// Shared validation for create and update execution policy handlers.
pub(crate) fn validate_execution_policy(policy: &ExecutionPolicyDto) -> Result<(), ApiError> {
    // I6: Input length and range validation
    if policy.policy_id.len() > 256 {
        return Err(ApiError::BadRequest(
            "policy_id exceeds 256 characters".into(),
        ));
    }
    if policy.process_name.len() > 256 {
        return Err(ApiError::BadRequest(
            "process_name exceeds 256 characters".into(),
        ));
    }
    if policy.max_execution_time_ms > 0 && policy.max_execution_time_ms < 100 {
        return Err(ApiError::BadRequest(
            "max_execution_time_ms must be >= 100 or 0 (unlimited)".into(),
        ));
    }
    if policy.max_execution_time_ms > 3_600_000 {
        return Err(ApiError::BadRequest(
            "max_execution_time_ms exceeds 1 hour".into(),
        ));
    }

    // I5: Auto confirmation cannot be used with requires_sudo policies.
    // The canonical token is SCREAMING_SNAKE_CASE "AUTO" (ConfirmationRequirement::Display).
    // PR #4073 migrated ConfirmationRequirement to SCREAMING_SNAKE_CASE; this guard
    // was left on the old PascalCase "Auto" token and was therefore permanently dead.
    if policy.confirmation == "AUTO" && policy.requires_sudo {
        return Err(ApiError::BadRequest(
            "Auto confirmation cannot be used with requires_sudo policies".into(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_policy(confirmation: &str, requires_sudo: bool) -> ExecutionPolicyDto {
        ExecutionPolicyDto {
            policy_id: "p1".to_string(),
            process_name: "proc".to_string(),
            process_hash: None,
            allowed_args: vec![],
            requires_sudo,
            max_execution_time_ms: 0,
            audit_level: String::new(),
            sandbox_profile: None,
            allowed_paths: vec![],
            allow_network: None,
            require_signed_token: false,
            confirmation: confirmation.to_string(),
        }
    }

    /// F-RC-C35-01: guard must fire on canonical SCREAMING_SNAKE_CASE token "AUTO".
    #[test]
    fn rejects_auto_with_requires_sudo() {
        let policy = make_policy("AUTO", true);
        assert!(
            matches!(
                validate_execution_policy(&policy).unwrap_err(),
                crate::error::ApiError::BadRequest(_)
            ),
            "AUTO + requires_sudo must be rejected with ApiError::BadRequest"
        );
    }

    /// Guard must NOT fire on the old PascalCase "Auto" — that token is not canonical
    /// and would indicate a caller using a stale representation.
    #[test]
    fn old_pascal_case_auto_does_not_bypass_guard() {
        // "Auto" is not the canonical token; it should not match the guard and
        // therefore the policy passes validation (the guard is case-sensitive by design).
        let policy = make_policy("Auto", true);
        // validate_execution_policy does not know "Auto" == AUTO; it should pass
        // through to let dto_to_policy treat it as the Confirm fallback.
        // Security note: the guard is intentionally case-sensitive; "Auto"+requires_sudo
        // must NOT be rejected — only the canonical "AUTO" token triggers the I5 block.
        validate_execution_policy(&policy).expect(
            "PascalCase 'Auto' is not the canonical token and must fall through to Confirm fallback",
        );
    }

    /// CONFIRM + requires_sudo is a valid combination.
    #[test]
    fn allows_confirm_with_requires_sudo() {
        let policy = make_policy("CONFIRM", true);
        // Pin: CONFIRM is an explicit confirmation mode; sudo is compatible with it.
        // A regression that blocks this would break legitimate privileged automation. (#5594)
        validate_execution_policy(&policy).expect("CONFIRM + requires_sudo must be allowed");
    }

    /// AUTO without requires_sudo is a valid combination.
    #[test]
    fn allows_auto_without_requires_sudo() {
        let policy = make_policy("AUTO", false);
        // Pin: AUTO without requires_sudo is the normal unattended automation case.
        // The I5 guard only fires on AUTO+requires_sudo, not AUTO alone. (#5594)
        validate_execution_policy(&policy).expect("AUTO without requires_sudo must be allowed");
    }

    /// BLOCK + requires_sudo is a valid combination.
    #[test]
    fn allows_block_with_requires_sudo() {
        let policy = make_policy("BLOCK", true);
        // Pin: BLOCK always denies execution regardless of sudo; the combination is
        // valid because the policy never reaches the execution gate. (#5594)
        validate_execution_policy(&policy).expect("BLOCK + requires_sudo must be allowed");
    }
}
