use std::sync::atomic::Ordering;
use std::time::Instant;

use crate::controller::gate::WORKFLOW_STEP_POLICY_TOKEN;
use crate::error::AutomationError;
use crate::policy::AuditLevel;
#[cfg(test)]
use oneshim_core::config::SandboxConfig;
use oneshim_core::models::intent::{IntentCommand, WorkflowPreset};

use super::types::{AutomationCommand, CommandResult, WorkflowResult, WorkflowStepResult};
use super::AutomationController;

impl AutomationController {
    pub async fn run_workflow(
        &self,
        preset: &WorkflowPreset,
    ) -> Result<WorkflowResult, AutomationError> {
        self.ensure_enabled()?;

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
                tokio::time::sleep(std::time::Duration::from_millis(step.delay_ms)).await;
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
                    &format!("step[{}] {}: {:?}", idx, step.name, step.intent),
                );
            }

            let step_start = Instant::now();
            let result = executor.execute(&intent_command.intent).await;
            let step_elapsed = step_start.elapsed().as_millis() as u64;

            match result {
                Ok(intent_result) => {
                    {
                        let mut logger = self.audit_logger.write().await;
                        logger.log_complete_with_time(
                            AuditLevel::Basic,
                            &step_cmd_id,
                            &preset.id,
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
                        logger.log_complete_with_time(
                            AuditLevel::Basic,
                            &step_cmd_id,
                            &preset.id,
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
                oneshim_core::config::ConfirmationRequirement::Block => {
                    return Ok(CommandResult::Denied);
                }
                oneshim_core::config::ConfirmationRequirement::Confirm => {
                    let approved = self
                        .request_confirmation(
                            &cmd.command_id,
                            &policy.process_name,
                            &policy.allowed_args,
                            &format!("{:?}", policy.audit_level),
                        )
                        .await?;
                    if !approved {
                        return Ok(CommandResult::Denied);
                    }
                }
                oneshim_core::config::ConfirmationRequirement::Auto => {}
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
