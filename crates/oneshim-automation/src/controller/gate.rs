use std::sync::Arc;
use std::time::Instant;

use tokio::sync::RwLock;

use crate::action_dispatcher::AutomationActionDispatcher;
use crate::audit::AuditLogger;
use crate::error::AutomationError;
use crate::policy::{AuditLevel, PolicyClient};
use crate::resolver;
use oneshim_core::config::SandboxConfig;
use oneshim_core::models::automation::{AutomationCommand, CommandResult};

pub(super) const GUI_SESSION_POLICY_TOKEN: &str = "gui-session";
pub(super) const INTENT_HINT_POLICY_TOKEN: &str = "intent-hint";
pub(super) const SCENE_ACTION_POLICY_TOKEN: &str = "scene-action";
pub(super) const WORKFLOW_STEP_POLICY_TOKEN: &str = "workflow-step";

#[derive(Clone)]
pub(super) struct CommandExecutionGate {
    policy_client: Arc<PolicyClient>,
    audit_logger: Arc<RwLock<AuditLogger>>,
    action_dispatcher: Arc<dyn AutomationActionDispatcher>,
    base_sandbox_config: SandboxConfig,
}

impl CommandExecutionGate {
    pub(super) fn new(
        policy_client: Arc<PolicyClient>,
        audit_logger: Arc<RwLock<AuditLogger>>,
        action_dispatcher: Arc<dyn AutomationActionDispatcher>,
        base_sandbox_config: SandboxConfig,
    ) -> Self {
        Self {
            policy_client,
            audit_logger,
            action_dispatcher,
            base_sandbox_config,
        }
    }

    pub(super) fn uses_internal_policy_token(policy_token: &str) -> bool {
        matches!(
            policy_token,
            GUI_SESSION_POLICY_TOKEN
                | INTENT_HINT_POLICY_TOKEN
                | SCENE_ACTION_POLICY_TOKEN
                | WORKFLOW_STEP_POLICY_TOKEN
        )
    }

    pub(super) async fn resolve_for_command(
        &self,
        cmd: &AutomationCommand,
    ) -> (SandboxConfig, AuditLevel) {
        if Self::uses_internal_policy_token(&cmd.policy_token) {
            return (
                resolver::default_strict_config(&self.base_sandbox_config),
                AuditLevel::Basic,
            );
        }

        match self
            .policy_client
            .get_policy_for_token(&cmd.policy_token)
            .await
        {
            Some(policy) => {
                let config = resolver::resolve_sandbox_config(&policy, &self.base_sandbox_config);
                (config, policy.audit_level)
            }
            None => {
                let config = resolver::default_strict_config(&self.base_sandbox_config);
                (config, AuditLevel::Basic)
            }
        }
    }

    pub(super) async fn effective_timeout_ms(&self, cmd: &AutomationCommand) -> Option<u64> {
        let (resolved_config, _) = self.resolve_for_command(cmd).await;
        cmd.timeout_ms.or(if resolved_config.max_cpu_time_ms > 0 {
            Some(resolved_config.max_cpu_time_ms)
        } else {
            None
        })
    }

    pub(super) async fn execute(
        &self,
        cmd: &AutomationCommand,
    ) -> Result<CommandResult, AutomationError> {
        if !Self::uses_internal_policy_token(&cmd.policy_token)
            && !self.policy_client.validate_command(cmd).await?
        {
            let mut logger = self.audit_logger.write().await;
            logger.log_denied(
                &cmd.command_id,
                &cmd.session_id,
                &format!("{:?}", cmd.action),
            );
            return Ok(CommandResult::Denied);
        }

        let (resolved_config, audit_level) = self.resolve_for_command(cmd).await;

        {
            let mut logger = self.audit_logger.write().await;
            logger.log_start_if(
                audit_level,
                &cmd.command_id,
                &cmd.session_id,
                &format!("{:?}", cmd.action),
            );
        }

        let timeout_ms = cmd.timeout_ms.or(if resolved_config.max_cpu_time_ms > 0 {
            Some(resolved_config.max_cpu_time_ms)
        } else {
            None
        });

        let start = Instant::now();

        let result = if let Some(timeout) = timeout_ms {
            let duration = std::time::Duration::from_millis(timeout);
            match tokio::time::timeout(
                duration,
                self.action_dispatcher
                    .dispatch(&cmd.action, &resolved_config),
            )
            .await
            {
                Ok(result) => result,
                Err(_elapsed) => {
                    let mut logger = self.audit_logger.write().await;
                    logger.log_timeout(&cmd.command_id, &cmd.session_id, timeout);
                    return Ok(CommandResult::Timeout);
                }
            }
        } else {
            self.action_dispatcher
                .dispatch(&cmd.action, &resolved_config)
                .await
        };

        let elapsed_ms = start.elapsed().as_millis() as u64;

        {
            let mut logger = self.audit_logger.write().await;
            logger.log_complete_with_time(
                audit_level,
                &cmd.command_id,
                &cmd.session_id,
                &format!("{:?}", result),
                elapsed_ms,
            );
        }

        Ok(result)
    }
}
