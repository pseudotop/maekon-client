use chrono::Utc;
use oneshim_api_contracts::automation::{
    ExecuteIntentHintRequest, ExecuteIntentHintResponse, ExecuteSceneActionRequest,
    ExecuteSceneActionResponse, PresetRunResult,
};
use oneshim_core::error::CoreError;
use oneshim_core::models::audit::AuditLevel;
use oneshim_core::models::intent::{builtin_presets, IntentCommand, IntentResult, WorkflowPreset};
use oneshim_core::ports::audit_log::AuditLogPort;

use crate::error::ApiError;
use crate::services::web_contexts::AutomationWebContext;

use super::helpers::{
    build_scene_action_intents, enforce_scene_action_privacy, read_scene_intelligence_config,
    require_config_manager, SceneActionPolicyContext,
};
use super::AUTOMATION_SCENE_ACTION_SCHEMA_VERSION;

#[derive(Clone)]
pub struct AutomationCommandService {
    ctx: AutomationWebContext,
}

impl AutomationCommandService {
    pub fn new(ctx: AutomationWebContext) -> Self {
        Self { ctx }
    }

    pub fn create_preset(&self, preset: WorkflowPreset) -> Result<WorkflowPreset, ApiError> {
        if preset.id.is_empty() || preset.name.is_empty() {
            return Err(ApiError::BadRequest(
                "Preset ID and name are required".into(),
            ));
        }
        if preset.steps.is_empty() {
            return Err(ApiError::BadRequest("At least one step is required".into()));
        }
        self.validate_preset_ai_profile_binding(&preset)?;

        let config_manager = require_config_manager(&self.ctx)?;
        let mut duplicate = false;
        config_manager
            .update_with(|config| {
                if config
                    .automation
                    .custom_presets
                    .iter()
                    .any(|p| p.id == preset.id)
                {
                    duplicate = true;
                    return Ok(());
                }
                let mut new_preset = preset.clone();
                new_preset.builtin = false;
                config.automation.custom_presets.push(new_preset);
                Ok(())
            })
            .map_err(ApiError::from)?;

        if duplicate {
            return Err(ApiError::Conflict(format!(
                "Preset '{}' already exists",
                preset.id,
            )));
        }

        Ok(preset)
    }

    pub fn update_preset(
        &self,
        id: String,
        preset: WorkflowPreset,
    ) -> Result<WorkflowPreset, ApiError> {
        self.validate_preset_ai_profile_binding(&preset)?;
        let config_manager = require_config_manager(&self.ctx)?;
        let mut found = false;
        config_manager
            .update_with(|config| {
                if let Some(existing) = config
                    .automation
                    .custom_presets
                    .iter_mut()
                    .find(|p| p.id == id)
                {
                    existing.name = preset.name.clone();
                    existing.description = preset.description.clone();
                    existing.category = preset.category;
                    existing.steps = preset.steps.clone();
                    existing.platform = preset.platform.clone();
                    existing.ai_profile_id = preset.ai_profile_id.clone();
                    found = true;
                }
                Ok(())
            })
            .map_err(ApiError::from)?;

        if !found {
            return Err(ApiError::NotFound(format!("Preset '{}' not found", id)));
        }

        Ok(preset)
    }

    pub fn delete_preset(&self, id: String) -> Result<serde_json::Value, ApiError> {
        let config_manager = require_config_manager(&self.ctx)?;
        let mut found = false;
        config_manager
            .update_with(|config| {
                let before_len = config.automation.custom_presets.len();
                config.automation.custom_presets.retain(|p| p.id != id);
                found = config.automation.custom_presets.len() < before_len;
                Ok(())
            })
            .map_err(ApiError::from)?;

        if !found {
            return Err(ApiError::NotFound(format!("Preset '{}' not found", id)));
        }

        Ok(serde_json::json!({ "deleted": id }))
    }

    pub async fn run_preset(&self, id: String) -> Result<PresetRunResult, ApiError> {
        let mut preset = builtin_presets().iter().find(|p| p.id == id).cloned();

        if preset.is_none() {
            if let Some(ref config_manager) = self.ctx.config_manager {
                let config = config_manager.get();
                preset = config
                    .automation
                    .custom_presets
                    .iter()
                    .find(|p| p.id == id)
                    .cloned();
            }
        }

        let Some(preset) = preset else {
            return Err(ApiError::NotFound(format!("Preset '{}' not found", id)));
        };
        self.validate_preset_ai_profile_binding(&preset)?;

        if let Some(ref config_manager) = self.ctx.config_manager {
            let config = config_manager.get();
            if !config.automation.enabled {
                return Err(ApiError::BadRequest("Automation is disabled.".to_string()));
            }
        }

        if let Some(ref controller) = self.ctx.automation_controller {
            match controller.run_workflow(&preset).await {
                Ok(result) => {
                    if !result.success {
                        return Err(ApiError::BadRequest(result.message));
                    }

                    Ok(PresetRunResult {
                        preset_id: result.preset_id,
                        success: true,
                        message: result.message,
                        steps_executed: Some(result.steps_executed),
                        total_steps: Some(result.total_steps),
                        total_elapsed_ms: Some(result.total_elapsed_ms),
                    })
                }
                Err(e) => Err(ApiError::Internal(format!("execution failure: {}", e))),
            }
        } else {
            Err(ApiError::BadRequest(
                "Automation controller is not active.".to_string(),
            ))
        }
    }

    fn validate_preset_ai_profile_binding(&self, preset: &WorkflowPreset) -> Result<(), ApiError> {
        let Some(ai_profile_id) = preset
            .ai_profile_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Ok(());
        };

        let config_manager = require_config_manager(&self.ctx)?;
        let config = config_manager.get();
        let exists = config
            .ai_provider
            .saved_profiles
            .iter()
            .any(|profile| profile.profile_id == ai_profile_id);

        if exists {
            Ok(())
        } else {
            Err(ApiError::BadRequest(format!(
                "Preset '{}' references an unknown AI profile '{}'.",
                preset.id, ai_profile_id
            )))
        }
    }

    pub async fn execute_intent_hint(
        &self,
        req: ExecuteIntentHintRequest,
    ) -> Result<ExecuteIntentHintResponse, ApiError> {
        if req.session_id.trim().is_empty() {
            return Err(ApiError::BadRequest("session_id is required".to_string()));
        }
        if req.intent_hint.trim().is_empty() {
            return Err(ApiError::BadRequest("intent_hint is required".to_string()));
        }

        let Some(ref controller) = self.ctx.automation_controller else {
            return Err(ApiError::BadRequest(
                "Automation controller is not active.".to_string(),
            ));
        };

        let command_id = req
            .command_id
            .filter(|id| !id.trim().is_empty())
            .unwrap_or_else(|| format!("intent-hint-{}", Utc::now().timestamp_millis().abs()));

        match controller
            .execute_intent_hint(&command_id, &req.session_id, &req.intent_hint)
            .await
        {
            Ok(planned) => Ok(ExecuteIntentHintResponse {
                command_id,
                session_id: req.session_id,
                planned_intent: planned.planned_intent,
                result: planned.result,
            }),
            Err(
                CoreError::PolicyDenied { message: msg, .. }
                | CoreError::InvalidArguments { message: msg, .. },
            ) => Err(ApiError::BadRequest(msg)),
            Err(CoreError::ElementNotFound { name: msg, .. }) => Err(ApiError::BadRequest(msg)),
            // Iter-104 cascading fix from iter-100: IntentPlanner/IntentExecutor
            // "not configured" errors switched from Internal.Generic to
            // Config.Missing. Match both variants so the BadRequest routing
            // still fires — a previously silent regression where these
            // config-miss errors would have returned 500 instead of 400.
            Err(
                CoreError::Internal {
                    code: _,
                    message: msg,
                }
                | CoreError::Config {
                    code: _,
                    message: msg,
                },
            ) if msg.contains("IntentPlanner") || msg.contains("IntentExecutor") => {
                Err(ApiError::BadRequest(msg))
            }
            Err(e) => Err(ApiError::Internal(format!(
                "Natural language intent execution failed: {e}"
            ))),
        }
    }

    pub async fn execute_scene_action(
        &self,
        req: ExecuteSceneActionRequest,
    ) -> Result<ExecuteSceneActionResponse, ApiError> {
        let Some(ref controller) = self.ctx.automation_controller else {
            return Err(ApiError::BadRequest(
                "Automation controller is not active.".to_string(),
            ));
        };

        let scene_cfg = read_scene_intelligence_config(&self.ctx);
        if !scene_cfg.enabled {
            return Err(ApiError::BadRequest(
                "Scene intelligence is disabled.".to_string(),
            ));
        }
        if !scene_cfg.allow_action_execution {
            return Err(ApiError::BadRequest(
                "Scene action execution is disabled in settings.".to_string(),
            ));
        }

        let intents = build_scene_action_intents(&req)?;
        let policy_context = match enforce_scene_action_privacy(&self.ctx, &req) {
            Ok(context) => context,
            Err(err) => {
                if let Some(logger) = self.ctx.audit_logger.as_ref() {
                    logger
                        .log_event(
                            "policy.scene_action.blocked",
                            &req.session_id,
                            &format!(
                                "action_type={:?} element_id={} allow_sensitive_input={} error={}",
                                req.action_type,
                                req.element_id,
                                req.allow_sensitive_input.unwrap_or(false),
                                err
                            ),
                        )
                        .await;
                }
                return Err(err);
            }
        };
        let command_id = req
            .command_id
            .as_ref()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| format!("scene-action-{}", Utc::now().timestamp_millis().abs()));
        let started_at = std::time::Instant::now();

        if let Some(logger) = self.ctx.audit_logger.as_ref() {
            log_scene_action_policy_audit(logger.as_ref(), &req, &policy_context, &command_id)
                .await;
        }

        let mut last_result: Option<IntentResult> = None;
        for (index, intent) in intents.iter().enumerate() {
            let staged_command_id = if index == 0 {
                command_id.clone()
            } else {
                format!("{command_id}:stage-{index}")
            };

            let intent_command = IntentCommand {
                command_id: staged_command_id,
                session_id: req.session_id.clone(),
                intent: intent.clone(),
                config: None,
                timeout_ms: None,
                policy_token: "scene-action".to_string(),
            };

            match controller.execute_intent(&intent_command).await {
                Ok(result) => {
                    let failed = !result.success;
                    last_result = Some(result);
                    if failed {
                        break;
                    }
                }
                Err(
                    CoreError::PolicyDenied { message: msg, .. }
                    | CoreError::InvalidArguments { message: msg, .. },
                ) => return Err(ApiError::BadRequest(msg)),
                Err(CoreError::ElementNotFound { name: msg, .. }) => {
                    return Err(ApiError::BadRequest(msg));
                }
                // Iter-104 cascading fix from iter-100 (same as line 245-256
                // above). Post-iter-100, "IntentExecutor/IntentPlanner not
                // configured" emits Config.Missing, not Internal.Generic.
                // Accept both variants on the substring guard to keep the
                // BadRequest routing intact.
                Err(
                    CoreError::Internal {
                        code: _,
                        message: msg,
                    }
                    | CoreError::Config {
                        code: _,
                        message: msg,
                    },
                ) if msg.contains("IntentExecutor") || msg.contains("IntentPlanner") => {
                    return Err(ApiError::BadRequest(msg));
                }
                Err(e) => {
                    return Err(ApiError::Internal(format!(
                        "Scene action execution failed: {e}"
                    )))
                }
            }
        }

        let result = last_result.unwrap_or(IntentResult {
            success: false,
            element: None,
            verification: None,
            retry_count: 0,
            elapsed_ms: 0,
            error: Some("No executable action was produced.".to_string()),
        });
        let elapsed_ms = started_at.elapsed().as_millis() as u64;

        if let Some(logger) = self.ctx.audit_logger.as_ref() {
            log_scene_action_result_audit(
                logger.as_ref(),
                &req,
                &policy_context,
                &command_id,
                &result,
                elapsed_ms,
            )
            .await;
        }

        Ok(ExecuteSceneActionResponse {
            schema_version: AUTOMATION_SCENE_ACTION_SCHEMA_VERSION.to_string(),
            command_id,
            session_id: req.session_id,
            frame_id: req.frame_id,
            scene_id: req.scene_id,
            element_id: req.element_id,
            applied_privacy_policy: format!("{:?}", policy_context.policy),
            scene_action_override_active: policy_context.override_active,
            scene_action_override_expires_at: policy_context
                .override_expires_at
                .map(|value| value.to_rfc3339()),
            executed_intents: intents,
            result,
        })
    }
}

/// Logs policy-enforcement audit events before scene action execution.
///
/// Records: override applied/issue, sensitive-input opt-in, and detailed start entry.
async fn log_scene_action_policy_audit(
    logger: &dyn AuditLogPort,
    req: &ExecuteSceneActionRequest,
    policy_context: &SceneActionPolicyContext,
    command_id: &str,
) {
    if policy_context.override_active {
        logger
            .log_event(
                "policy.scene_action_override.applied",
                &req.session_id,
                &format!(
                    "action_type={:?} element_id={} approved_by={:?} expires_at={:?}",
                    req.action_type,
                    req.element_id,
                    policy_context.override_approved_by.as_deref(),
                    policy_context
                        .override_expires_at
                        .as_ref()
                        .map(|value| value.to_rfc3339()),
                ),
            )
            .await;
    } else if policy_context.override_enabled || policy_context.override_issue.is_some() {
        logger
            .log_event(
                "policy.scene_action_override.issue",
                &req.session_id,
                &format!(
                    "action_type={:?} element_id={} issue={:?} enabled={} expires_at={:?}",
                    req.action_type,
                    req.element_id,
                    policy_context.override_issue.as_deref(),
                    policy_context.override_enabled,
                    policy_context
                        .override_expires_at
                        .as_ref()
                        .map(|value| value.to_rfc3339()),
                ),
            )
            .await;
    }
    if req.allow_sensitive_input.unwrap_or(false) {
        logger
            .log_event(
                "policy.scene_action.allow_sensitive_input",
                &req.session_id,
                &format!(
                    "action_type={:?} element_id={} policy={:?} override_active={}",
                    req.action_type,
                    req.element_id,
                    policy_context.policy,
                    policy_context.override_active,
                ),
            )
            .await;
    }
    logger.log_start_if(
        AuditLevel::Detailed,
        command_id,
        &req.session_id,
        &format!(
            "scene_action frame_id={:?} scene_id={:?} element_id={} action_type={:?} policy={:?} pii_level={:?} override_enabled={} override_active={} override_reason={:?} override_approved_by={:?} override_expires_at={:?} override_issue={:?}",
            req.frame_id,
            req.scene_id,
            req.element_id,
            req.action_type,
            policy_context.policy,
            policy_context.pii_filter_level,
            policy_context.override_enabled,
            policy_context.override_active,
            policy_context.override_reason.as_deref(),
            policy_context.override_approved_by.as_deref(),
            policy_context
                .override_expires_at
                .as_ref()
                .map(|value| value.to_rfc3339()),
            policy_context.override_issue.as_deref()
        ),
    ).await;
}

/// Logs execution-result audit event after scene action completes.
async fn log_scene_action_result_audit(
    logger: &dyn AuditLogPort,
    req: &ExecuteSceneActionRequest,
    policy_context: &SceneActionPolicyContext,
    command_id: &str,
    result: &IntentResult,
    elapsed_ms: u64,
) {
    logger.log_complete_with_time(
        AuditLevel::Detailed,
        command_id,
        &req.session_id,
        &format!(
            "scene_action_result success={} frame_id={:?} scene_id={:?} element_id={} policy={:?} override_active={} error={:?}",
            result.success,
            req.frame_id,
            req.scene_id,
            req.element_id,
            policy_context.policy,
            policy_context.override_active,
            result.error
        ),
        elapsed_ms,
    ).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::web_contexts::AutomationWebContext;
    use crate::storage_port::WebStorage;
    use oneshim_core::config::{AiProviderProfileConfig, SavedAiProviderProfile};
    use oneshim_core::config_manager::ConfigManager;
    use oneshim_core::models::intent::{AutomationIntent, PresetCategory, WorkflowStep};
    use oneshim_storage::sqlite::SqliteStorage;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn test_context(config_manager: ConfigManager) -> AutomationWebContext {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("in-memory sqlite"))
            as Arc<dyn WebStorage>;
        AutomationWebContext {
            storage,
            frames_dir: None,
            config_manager: Some(config_manager),
            audit_logger: None,
            automation_controller: None,
            ai_runtime_status: None,
        }
    }

    fn test_preset(ai_profile_id: Option<&str>) -> WorkflowPreset {
        WorkflowPreset {
            id: "preset-1".to_string(),
            name: "Preset 1".to_string(),
            description: "Test preset".to_string(),
            category: PresetCategory::Custom,
            steps: vec![WorkflowStep {
                name: "Save".to_string(),
                intent: AutomationIntent::ExecuteHotkey {
                    keys: vec!["Cmd".to_string(), "S".to_string()],
                },
                delay_ms: 0,
                stop_on_failure: true,
            }],
            builtin: false,
            platform: None,
            ai_profile_id: ai_profile_id.map(str::to_string),
        }
    }

    fn saved_ai_profile(profile_id: &str) -> SavedAiProviderProfile {
        SavedAiProviderProfile {
            profile_id: profile_id.to_string(),
            name: "Anthropic Prod".to_string(),
            ai_provider: AiProviderProfileConfig::default(),
            updated_at: None,
        }
    }

    #[test]
    fn create_preset_rejects_unknown_ai_profile_binding() {
        let temp_dir = TempDir::new().expect("temp dir");
        let config_manager =
            ConfigManager::with_path(temp_dir.path().join("config.json")).expect("config manager");
        let service = AutomationCommandService::new(test_context(config_manager));

        let err = service
            .create_preset(test_preset(Some("missing-profile")))
            .expect_err("unknown preset profile should fail");

        match err {
            ApiError::BadRequest(message) => assert!(message.contains("unknown AI profile")),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn create_preset_persists_known_ai_profile_binding() {
        let temp_dir = TempDir::new().expect("temp dir");
        let config_manager =
            ConfigManager::with_path(temp_dir.path().join("config.json")).expect("config manager");
        config_manager
            .update_with(|config| {
                config.ai_provider.saved_profiles = vec![saved_ai_profile("anthropic-prod")];
                Ok(())
            })
            .expect("save config");

        let service = AutomationCommandService::new(test_context(config_manager.clone()));
        let preset = service
            .create_preset(test_preset(Some("anthropic-prod")))
            .expect("known preset profile should save");

        assert_eq!(preset.ai_profile_id.as_deref(), Some("anthropic-prod"));
        let saved = config_manager.get();
        assert_eq!(saved.automation.custom_presets.len(), 1);
        assert_eq!(
            saved.automation.custom_presets[0].ai_profile_id.as_deref(),
            Some("anthropic-prod")
        );
    }

    #[tokio::test]
    async fn run_preset_rejects_drifted_ai_profile_binding() {
        let temp_dir = TempDir::new().expect("temp dir");
        let config_manager =
            ConfigManager::with_path(temp_dir.path().join("config.json")).expect("config manager");
        config_manager
            .update_with(|config| {
                config.automation.enabled = true;
                config.automation.custom_presets = vec![test_preset(Some("anthropic-prod"))];
                config.ai_provider.saved_profiles = Vec::new();
                Ok(())
            })
            .expect("save config");

        let service = AutomationCommandService::new(test_context(config_manager));
        let err = service
            .run_preset("preset-1".to_string())
            .await
            .expect_err("drifted profile binding should fail");

        match err {
            ApiError::BadRequest(message) => assert!(message.contains("unknown AI profile")),
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
