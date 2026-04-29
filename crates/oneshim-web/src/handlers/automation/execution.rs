use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};

use oneshim_api_contracts::automation::{
    AuditQuery, ExecuteIntentHintRequest, ExecuteSceneActionRequest, PolicyEventQuery,
};
use oneshim_core::models::automation::ExecutionPolicyDto;

use crate::error::ApiError;
use crate::services::automation_service::{AutomationCommandService, AutomationQueryService};
use crate::services::web_contexts::AutomationWebContext;

pub async fn get_contract_versions(
) -> Result<Json<oneshim_api_contracts::automation::AutomationContractsDto>, ApiError> {
    Ok(Json(AutomationQueryService::contract_versions()))
}

pub async fn get_automation_status(
    State(context): State<AutomationWebContext>,
) -> Result<Json<oneshim_api_contracts::automation::AutomationStatusDto>, ApiError> {
    Ok(Json(
        AutomationQueryService::new(context)
            .automation_status()
            .await?,
    ))
}

pub async fn get_audit_logs(
    State(context): State<AutomationWebContext>,
    Query(query): Query<AuditQuery>,
) -> Result<Json<Vec<oneshim_api_contracts::automation::AuditEntryDto>>, ApiError> {
    Ok(Json(
        AutomationQueryService::new(context)
            .audit_logs(query)
            .await?,
    ))
}

pub async fn get_policy_events(
    State(context): State<AutomationWebContext>,
    Query(query): Query<PolicyEventQuery>,
) -> Result<Json<Vec<oneshim_api_contracts::automation::AuditEntryDto>>, ApiError> {
    Ok(Json(
        AutomationQueryService::new(context)
            .policy_events(query)
            .await?,
    ))
}

pub async fn get_policies(
    State(context): State<AutomationWebContext>,
) -> Result<Json<oneshim_api_contracts::automation::PoliciesDto>, ApiError> {
    Ok(Json(AutomationQueryService::new(context).policies()))
}

pub async fn get_automation_stats(
    State(context): State<AutomationWebContext>,
) -> Result<Json<oneshim_api_contracts::automation::AutomationStatsDto>, ApiError> {
    Ok(Json(
        AutomationQueryService::new(context)
            .automation_stats()
            .await,
    ))
}

pub async fn list_presets(
    State(context): State<AutomationWebContext>,
) -> Result<Json<oneshim_api_contracts::automation::PresetListDto>, ApiError> {
    Ok(Json(AutomationQueryService::new(context).list_presets()))
}

pub async fn create_preset(
    State(context): State<AutomationWebContext>,
    Json(preset): Json<oneshim_core::models::intent::WorkflowPreset>,
) -> Result<Json<oneshim_core::models::intent::WorkflowPreset>, ApiError> {
    Ok(Json(
        AutomationCommandService::new(context).create_preset(preset)?,
    ))
}

pub async fn update_preset(
    State(context): State<AutomationWebContext>,
    Path(id): Path<String>,
    Json(preset): Json<oneshim_core::models::intent::WorkflowPreset>,
) -> Result<Json<oneshim_core::models::intent::WorkflowPreset>, ApiError> {
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
) -> Result<Json<oneshim_api_contracts::automation::PresetRunResult>, ApiError> {
    Ok(Json(
        AutomationCommandService::new(context)
            .run_preset(id)
            .await?,
    ))
}

pub async fn execute_intent_hint(
    State(context): State<AutomationWebContext>,
    Json(req): Json<ExecuteIntentHintRequest>,
) -> Result<Json<oneshim_api_contracts::automation::ExecuteIntentHintResponse>, ApiError> {
    Ok(Json(
        AutomationCommandService::new(context)
            .execute_intent_hint(req)
            .await?,
    ))
}

pub async fn execute_scene_action(
    State(context): State<AutomationWebContext>,
    Json(req): Json<ExecuteSceneActionRequest>,
) -> Result<Json<oneshim_api_contracts::automation::ExecuteSceneActionResponse>, ApiError> {
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
fn validate_execution_policy(policy: &ExecutionPolicyDto) -> Result<(), ApiError> {
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

    // I5: Auto confirmation cannot be used with requires_sudo policies
    if policy.confirmation == "Auto" && policy.requires_sudo {
        return Err(ApiError::BadRequest(
            "Auto confirmation cannot be used with requires_sudo policies".into(),
        ));
    }

    Ok(())
}
