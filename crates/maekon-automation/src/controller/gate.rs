use std::sync::Arc;
use std::time::Instant;

use tokio::sync::RwLock;

use crate::action_dispatcher::AutomationActionDispatcher;
use crate::audit::AuditLogger;
use crate::error::AutomationError;
use crate::policy::{AuditLevel, PolicyClient};
use crate::resolver;
use maekon_core::config::SandboxConfig;
use maekon_core::models::audit::AuditStatus;
use maekon_core::models::automation::{
    AutomationAction, AutomationCommand, CommandOrigin, CommandResult,
};

pub(super) const GUI_SESSION_POLICY_TOKEN: &str = "gui-session";
pub(super) const INTENT_HINT_POLICY_TOKEN: &str = "intent-hint";
pub(super) const SCENE_ACTION_POLICY_TOKEN: &str = "scene-action";
pub(super) const WORKFLOW_STEP_POLICY_TOKEN: &str = "workflow-step";

// POLICY_TOKEN_VALIDATION_RESERVED_FOR_EXTERNAL_PRODUCERS:
// The signed `PolicyClient::validate_command` path is intentionally reserved for
// a future external AutomationCommand producer. Current production flows mint
// only in-process sentinel tokens and are gated by AutomationConfig confirmation,
// scene-action privacy policy, GUI execution tickets, and sandbox enforcement.
// Any new production caller that wants direct `execute_command` must first wire
// a signed-token issuance plan, signing-secret lifecycle, and command-scope tests.

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

    /// #6333 A20 / E42.14: a command may use the internal-token path ONLY when
    /// it was minted in-process (`Internal`) AND carries one of the four sentinel
    /// tokens. A deserialized/external command carrying a sentinel string is
    /// rejected here because deserialization always yields `CommandOrigin::External`.
    ///
    /// See `POLICY_TOKEN_VALIDATION_RESERVED_FOR_EXTERNAL_PRODUCERS` above: this
    /// split is deliberate until a production external policy-token issuer exists.
    pub(super) fn is_trusted_internal(origin: CommandOrigin, policy_token: &str) -> bool {
        origin == CommandOrigin::Internal && Self::uses_internal_policy_token(policy_token)
    }

    pub(super) async fn resolve_for_command(
        &self,
        cmd: &AutomationCommand,
    ) -> (SandboxConfig, AuditLevel) {
        if Self::is_trusted_internal(cmd.origin, &cmd.policy_token) {
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
        if !Self::is_trusted_internal(cmd.origin, &cmd.policy_token)
            && !self.policy_client.validate_command(cmd).await?
        {
            let mut logger = self.audit_logger.write().await;
            logger.log_denied(
                &cmd.command_id,
                &cmd.session_id,
                &audit_action_label(&cmd.action),
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
                &audit_action_label(&cmd.action),
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
            // Record the REAL outcome status, not a hardcoded Completed (review4 A1).
            // Failed dispatches — including sandbox containment failures — must be
            // durably audited as Failed so entries_by_status/stats and the hash
            // chain do not misreport them as Completed.
            let (action_type, status) = match &result {
                CommandResult::Success => ("complete", AuditStatus::Completed),
                CommandResult::Failed(_) => ("failed", AuditStatus::Failed),
                CommandResult::Denied => ("denied", AuditStatus::Denied),
                CommandResult::Timeout => ("timeout", AuditStatus::Timeout),
            };
            logger.log_with_status_and_time(
                audit_level,
                &cmd.command_id,
                &cmd.session_id,
                action_type,
                status,
                &format!("{result:?}"),
                elapsed_ms,
            );
        }

        Ok(result)
    }
}

pub(super) fn audit_action_label(action: &AutomationAction) -> String {
    match action {
        AutomationAction::MouseMove { x, y } => format!("MouseMove {{ x={x}, y={y} }}"),
        AutomationAction::MouseClick { button, x, y } => {
            format!("MouseClick {{ button={button}, x={x}, y={y} }}")
        }
        AutomationAction::KeyType { text } => {
            format!("KeyType {{ text_len={} }}", text.chars().count())
        }
        AutomationAction::KeyPress { key } => format!("KeyPress {{ key={key} }}"),
        AutomationAction::KeyRelease { key } => format!("KeyRelease {{ key={key} }}"),
        AutomationAction::Hotkey { keys } => format!("Hotkey {{ keys={} }}", keys.join("+")),
    }
}

#[cfg(test)]
mod a20_tests {
    use super::CommandExecutionGate;
    use maekon_core::models::automation::CommandOrigin;

    #[test]
    fn is_trusted_internal_requires_both_internal_origin_and_sentinel_token() {
        // #6333 A20: bypass eligibility = Internal origin AND a sentinel token.
        // internal token + Internal origin → trusted
        assert!(CommandExecutionGate::is_trusted_internal(
            CommandOrigin::Internal,
            "gui-session"
        ));
        // internal token string but External origin (e.g. deserialized) → NOT trusted
        assert!(!CommandExecutionGate::is_trusted_internal(
            CommandOrigin::External,
            "gui-session"
        ));
        // external token + Internal origin → not a sentinel, so not internal anyway
        assert!(!CommandExecutionGate::is_trusted_internal(
            CommandOrigin::Internal,
            "test-pol:nonce"
        ));
    }
}
