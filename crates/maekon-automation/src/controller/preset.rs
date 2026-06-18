use std::sync::atomic::Ordering;
use std::time::Instant;

use crate::controller::gate::{audit_action_label, WORKFLOW_STEP_POLICY_TOKEN};
use crate::controller::intent::audit_intent_label;
use crate::error::AutomationError;
use crate::policy::AuditLevel;
use maekon_core::config::ConfirmationRequirement;
#[cfg(test)]
use maekon_core::config::SandboxConfig;
use maekon_core::models::audit::AuditStatus;
use maekon_core::models::intent::{IntentCommand, WorkflowPreset};

use super::types::{AutomationCommand, CommandResult, WorkflowResult, WorkflowStepResult};
use super::AutomationController;

impl AutomationController {
    pub async fn run_workflow(
        &self,
        preset: &WorkflowPreset,
    ) -> Result<WorkflowResult, AutomationError> {
        self.ensure_enabled()?;

        // Honor the user's confirmation policy for the whole workflow (review4 A11):
        // run_workflow previously ignored it, so a Block/Confirm setting was silently
        // bypassed for custom presets that may carry arbitrary Raw input synthesis.
        match self.confirmation_policy {
            ConfirmationRequirement::Block => return Err(AutomationError::PolicyBlocked),
            ConfirmationRequirement::Confirm => {
                let action_label = format!(
                    "workflow preset: {} ({} steps)",
                    preset.id,
                    preset.steps.len()
                );
                let approved = self
                    .request_confirmation(&preset.id, "workflow-step", &[action_label], "CONFIRM")
                    .await?;
                if !approved {
                    return Err(AutomationError::UserDenied);
                }
            }
            ConfirmationRequirement::Auto => {}
        }

        let total_steps = preset.steps.len();
        let mut step_results = Vec::with_capacity(total_steps);
        let mut all_success = true;
        let workflow_start = Instant::now();

        tracing::info!(
            preset_id = %preset.id,
            total_steps,
            "workflow preset execution started"
        );

        for (idx, step) in preset.steps.iter().enumerate() {
            if idx > 0 && step.delay_ms > 0 {
                // Clamp the (untrusted-config) inter-step delay so a malformed preset
                // cannot hang a step effectively forever (review4 A19).
                const MAX_STEP_DELAY_MS: u64 = 60_000;
                tokio::time::sleep(std::time::Duration::from_millis(
                    step.delay_ms.min(MAX_STEP_DELAY_MS),
                ))
                .await;
            }

            let step_cmd_id = format!("{}:step-{}", preset.id, idx);
            let intent_command = IntentCommand {
                command_id: step_cmd_id.clone(),
                session_id: preset.id.clone(),
                intent: step.intent.clone(),
                config: None,
                timeout_ms: None,
                policy_token: WORKFLOW_STEP_POLICY_TOKEN.to_string(),
            };
            let executor = self.scoped_intent_executor(&intent_command)?;

            {
                let mut logger = self.audit_logger.write().await;
                logger.log_start_if(
                    AuditLevel::Basic,
                    &step_cmd_id,
                    &preset.id,
                    // text_len-redacted (review4 A5): raw step.intent Debug embeds
                    // free-form TypeIntoElement/KeyType text (secrets).
                    &format!(
                        "step[{}] {}: {}",
                        idx,
                        step.name,
                        audit_intent_label(&step.intent)
                    ),
                );
            }

            let step_start = Instant::now();
            let result = executor.execute(&intent_command.intent).await;
            let step_elapsed = step_start.elapsed().as_millis() as u64;

            match result {
                Ok(intent_result) => {
                    {
                        let mut logger = self.audit_logger.write().await;
                        // Record the real step status (review4 A1).
                        let (action_type, status) = if intent_result.success {
                            ("complete", AuditStatus::Completed)
                        } else {
                            ("failed", AuditStatus::Failed)
                        };
                        logger.log_with_status_and_time(
                            AuditLevel::Basic,
                            &step_cmd_id,
                            &preset.id,
                            action_type,
                            status,
                            &format!(
                                "step[{}] success={}, elapsed={}ms",
                                idx, intent_result.success, step_elapsed
                            ),
                            step_elapsed,
                        );
                    }

                    let step_success = intent_result.success;
                    step_results.push(WorkflowStepResult {
                        step_name: step.name.clone(),
                        step_index: idx,
                        success: step_success,
                        elapsed_ms: step_elapsed,
                        error: if step_success {
                            None
                        } else {
                            intent_result.error.clone()
                        },
                    });

                    if !step_success {
                        all_success = false;
                        if step.stop_on_failure {
                            tracing::warn!(
                                step = idx,
                                name = %step.name,
                                "workflow step failed -> stopping"
                            );
                            break;
                        }
                    }
                }
                Err(e) => {
                    {
                        let mut logger = self.audit_logger.write().await;
                        logger.log_with_status_and_time(
                            AuditLevel::Basic,
                            &step_cmd_id,
                            &preset.id,
                            "failed",
                            AuditStatus::Failed,
                            &format!("step[{}] error: {}", idx, e),
                            step_elapsed,
                        );
                    }

                    step_results.push(WorkflowStepResult {
                        step_name: step.name.clone(),
                        step_index: idx,
                        success: false,
                        elapsed_ms: step_elapsed,
                        error: Some(e.to_string()),
                    });

                    all_success = false;
                    if step.stop_on_failure {
                        tracing::warn!(
                            step = idx,
                            name = %step.name,
                            error = %e,
                            "workflow step error -> stopping"
                        );
                        break;
                    }
                }
            }
        }

        let total_elapsed = workflow_start.elapsed().as_millis() as u64;
        let steps_executed = step_results.len();

        let message = if all_success {
            format!(
                "preset '{}' succeeded ({}/{} steps, {}ms)",
                preset.name, steps_executed, total_steps, total_elapsed
            )
        } else {
            format!(
                "preset '{}' partially failed ({}/{} steps, {}ms)",
                preset.name, steps_executed, total_steps, total_elapsed
            )
        };

        tracing::info!(
            preset_id = %preset.id,
            success = all_success,
            steps_executed,
            total_elapsed_ms = total_elapsed,
            "workflow preset execution completed"
        );

        Ok(WorkflowResult {
            preset_id: preset.id.clone(),
            success: all_success,
            steps_executed,
            total_steps,
            total_elapsed_ms: total_elapsed,
            step_results,
            message,
        })
    }

    #[cfg(test)]
    pub(super) async fn resolve_for_command(
        &self,
        cmd: &AutomationCommand,
    ) -> (SandboxConfig, AuditLevel) {
        self.command_execution_gate().resolve_for_command(cmd).await
    }

    pub async fn execute_command(
        &self,
        cmd: &AutomationCommand,
    ) -> Result<CommandResult, AutomationError> {
        self.ensure_enabled()?;

        // Check confirmation requirement from the policy before execution.
        if let Some(policy) = self
            .policy_client
            .get_policy_for_token(&cmd.policy_token)
            .await
        {
            match policy.confirmation {
                maekon_core::config::ConfirmationRequirement::Block => {
                    // Audit the policy-level Block denial so it is observable on the
                    // same footing as the policy-allow path (gate.rs records allows
                    // and validate_command denials). Without this, Block denials were
                    // silently dropped from the audit trail.
                    let mut logger = self.audit_logger.write().await;
                    logger.log_denied(
                        &cmd.command_id,
                        &cmd.session_id,
                        &audit_action_label(&cmd.action),
                    );
                    return Ok(CommandResult::Denied);
                }
                maekon_core::config::ConfirmationRequirement::Confirm => {
                    let approved = self
                        .request_confirmation(
                            &cmd.command_id,
                            &policy.process_name,
                            &policy.allowed_args,
                            &format!("{:?}", policy.audit_level),
                        )
                        .await?;
                    if !approved {
                        // Audit the user-denied confirmation, mirroring the Block
                        // branch above and the gate.rs denial path.
                        let mut logger = self.audit_logger.write().await;
                        logger.log_denied(
                            &cmd.command_id,
                            &cmd.session_id,
                            &audit_action_label(&cmd.action),
                        );
                        return Ok(CommandResult::Denied);
                    }
                }
                maekon_core::config::ConfirmationRequirement::Auto => {}
            }
        }

        let result = self.command_execution_gate().execute(cmd).await;
        if let Some(ref flag) = self.last_command_ok {
            match &result {
                Ok(CommandResult::Success) => flag.store(true, Ordering::Relaxed),
                Ok(_) | Err(_) => flag.store(false, Ordering::Relaxed),
            }
        }
        result
    }
}
