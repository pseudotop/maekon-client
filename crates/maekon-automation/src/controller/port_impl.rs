//! `AutomationPort` trait implementation for `AutomationController`

use async_trait::async_trait;
use tokio::sync::broadcast;

use maekon_core::error::{CoreError, GuiInteractionError};
use maekon_core::models::automation::{
    AutomationCommand, CommandResult, ExecutionPolicyDto, GuiExecutionResult, PendingConfirmation,
    PlannedIntentResult, WorkflowResult,
};
use maekon_core::models::gui::{
    GuiConfirmRequest, GuiCreateSessionRequest, GuiCreateSessionResponse, GuiExecutionRequest,
    GuiExecutionTicket, GuiHighlightRequest, GuiInteractionSession, GuiSessionEvent,
};
use maekon_core::models::intent::{IntentCommand, IntentResult, WorkflowPreset};
use maekon_core::models::ui_scene::UiScene;
use maekon_core::ports::automation::AutomationPort;

use crate::policy::{AuditLevel, ExecutionPolicy};

use super::AutomationController;

/// Convert `ExecutionPolicy` → `ExecutionPolicyDto` for the port boundary.
fn policy_to_dto(p: &ExecutionPolicy) -> ExecutionPolicyDto {
    ExecutionPolicyDto {
        policy_id: p.policy_id.clone(),
        process_name: p.process_name.clone(),
        process_hash: p.process_hash.clone(),
        allowed_args: p.allowed_args.clone(),
        requires_sudo: p.requires_sudo,
        max_execution_time_ms: p.max_execution_time_ms,
        audit_level: format!("{:?}", p.audit_level),
        sandbox_profile: p.sandbox_profile.as_ref().map(|s| format!("{:?}", s)),
        allowed_paths: p.allowed_paths.clone(),
        allow_network: p.allow_network,
        require_signed_token: p.require_signed_token,
        confirmation: p.confirmation.to_string(),
    }
}

/// Convert `ExecutionPolicyDto` → `ExecutionPolicy` for internal use.
fn dto_to_policy(d: &ExecutionPolicyDto) -> ExecutionPolicy {
    let audit_level = match d.audit_level.as_str() {
        "None" => AuditLevel::None,
        "Detailed" => AuditLevel::Detailed,
        // review4 A15/A21: "Full" was missing, so an operator's Full setting
        // silently round-tripped to Basic (and resolver maps Full→Strict but
        // Basic→Standard — a latent sandbox downgrade).
        "Full" => AuditLevel::Full,
        _ => AuditLevel::Basic,
    };
    let sandbox_profile = d.sandbox_profile.as_deref().and_then(|s| match s {
        "Permissive" => Some(maekon_core::config::SandboxProfile::Permissive),
        "Standard" => Some(maekon_core::config::SandboxProfile::Standard),
        "Strict" => Some(maekon_core::config::SandboxProfile::Strict),
        _ => None,
    });
    let confirmation = match d.confirmation.as_str() {
        "AUTO" => maekon_core::config::ConfirmationRequirement::Auto,
        "BLOCK" => maekon_core::config::ConfirmationRequirement::Block,
        _ => maekon_core::config::ConfirmationRequirement::Confirm,
    };
    ExecutionPolicy {
        policy_id: d.policy_id.clone(),
        process_name: d.process_name.clone(),
        process_hash: d.process_hash.clone(),
        allowed_args: d.allowed_args.clone(),
        requires_sudo: d.requires_sudo,
        max_execution_time_ms: d.max_execution_time_ms,
        audit_level,
        sandbox_profile,
        allowed_paths: d.allowed_paths.clone(),
        allow_network: d.allow_network,
        require_signed_token: d.require_signed_token,
        confirmation,
    }
}

fn audit_safe_policy_id(policy_id: &str) -> String {
    const MAX_POLICY_ID_CHARS: usize = 128;
    let is_safe = policy_id.chars().count() <= MAX_POLICY_ID_CHARS
        && policy_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':'));
    if is_safe {
        policy_id.to_string()
    } else {
        "[REDACTED_POLICY_ID]".to_string()
    }
}

fn policy_change_audit_details(operation: &str, policy: &ExecutionPolicyDto) -> String {
    format!(
        "operation={operation} policy_id={} process_name_chars={} allowed_args_count={} allowed_paths_count={} requires_sudo={} max_execution_time_ms={} audit_level={} sandbox_profile={} allow_network={} require_signed_token={} confirmation={}",
        audit_safe_policy_id(&policy.policy_id),
        policy.process_name.chars().count(),
        policy.allowed_args.len(),
        policy.allowed_paths.len(),
        policy.requires_sudo,
        policy.max_execution_time_ms,
        policy.audit_level,
        policy.sandbox_profile.as_deref().unwrap_or("None"),
        policy.allow_network.unwrap_or(false),
        policy.require_signed_token,
        policy.confirmation,
    )
}

fn policy_delete_audit_details(policy_id: &str, removed: bool) -> String {
    format!(
        "operation=delete policy_id={} removed={removed}",
        audit_safe_policy_id(policy_id)
    )
}

#[async_trait]
impl AutomationPort for AutomationController {
    async fn execute_command(&self, cmd: &AutomationCommand) -> Result<CommandResult, CoreError> {
        self.execute_command(cmd).await.map_err(Into::into)
    }

    async fn execute_intent(&self, cmd: &IntentCommand) -> Result<IntentResult, CoreError> {
        self.execute_intent(cmd).await.map_err(Into::into)
    }

    async fn execute_intent_hint(
        &self,
        command_id: &str,
        session_id: &str,
        intent_hint: &str,
    ) -> Result<PlannedIntentResult, CoreError> {
        self.execute_intent_hint(command_id, session_id, intent_hint)
            .await
            .map_err(Into::into)
    }

    async fn run_workflow(&self, preset: &WorkflowPreset) -> Result<WorkflowResult, CoreError> {
        self.run_workflow(preset).await.map_err(Into::into)
    }

    async fn analyze_scene(
        &self,
        app_name: Option<&str>,
        screen_id: Option<&str>,
    ) -> Result<UiScene, CoreError> {
        self.analyze_scene(app_name, screen_id).await
    }

    async fn analyze_scene_from_image(
        &self,
        image_data: Vec<u8>,
        image_format: String,
        app_name: Option<&str>,
        screen_id: Option<&str>,
    ) -> Result<UiScene, CoreError> {
        self.analyze_scene_from_image(image_data, image_format, app_name, screen_id)
            .await
    }

    async fn gui_create_session(
        &self,
        req: GuiCreateSessionRequest,
    ) -> Result<GuiCreateSessionResponse, GuiInteractionError> {
        self.gui_create_session(req).await
    }

    async fn gui_get_session(
        &self,
        session_id: &str,
        capability_token: &str,
    ) -> Result<GuiInteractionSession, GuiInteractionError> {
        self.gui_get_session(session_id, capability_token).await
    }

    async fn gui_highlight_session(
        &self,
        session_id: &str,
        capability_token: &str,
        req: GuiHighlightRequest,
    ) -> Result<GuiInteractionSession, GuiInteractionError> {
        self.gui_highlight_session(session_id, capability_token, req)
            .await
    }

    async fn gui_confirm_candidate(
        &self,
        session_id: &str,
        capability_token: &str,
        req: GuiConfirmRequest,
    ) -> Result<GuiExecutionTicket, GuiInteractionError> {
        self.gui_confirm_candidate(session_id, capability_token, req)
            .await
    }

    async fn gui_execute(
        &self,
        session_id: &str,
        capability_token: &str,
        req: GuiExecutionRequest,
    ) -> Result<GuiExecutionResult, GuiInteractionError> {
        self.gui_execute(session_id, capability_token, req).await
    }

    async fn gui_cancel_session(
        &self,
        session_id: &str,
        capability_token: &str,
    ) -> Result<GuiInteractionSession, GuiInteractionError> {
        self.gui_cancel_session(session_id, capability_token).await
    }

    async fn gui_subscribe_events(
        &self,
        session_id: &str,
        capability_token: &str,
    ) -> Result<broadcast::Receiver<GuiSessionEvent>, GuiInteractionError> {
        self.gui_subscribe_events(session_id, capability_token)
            .await
    }

    async fn list_pending_confirmations(&self) -> Result<Vec<PendingConfirmation>, CoreError> {
        let map = self.pending_confirmations.lock().await;
        Ok(map.values().map(|(c, _)| c.clone()).collect())
    }

    async fn submit_confirmation(
        &self,
        command_id: &str,
        nonce: &str,
        approved: bool,
    ) -> Result<(), CoreError> {
        let mut map = self.pending_confirmations.lock().await;
        if let Some((confirmation, sender)) = map.remove(command_id) {
            // Verify the nonce matches to prevent unauthorised approval from
            // arbitrary scripts running inside the WebView. Constant-time compare
            // (review4 A23) to match the capability-token check's timing-safety
            // posture (gui_interaction/service.rs); ct_eq folds a length mismatch
            // into a non-leaking false.
            use subtle::ConstantTimeEq;
            if !bool::from(confirmation.nonce.as_bytes().ct_eq(nonce.as_bytes())) {
                // Re-insert so a legitimate caller can still respond.
                map.insert(command_id.to_string(), (confirmation, sender));
                return Err(CoreError::PermissionDenied {
                    code: maekon_core::error_codes::PermissionCode::PermissionDenied,
                    message: format!(
                        "confirm automation command '{}': nonce mismatch",
                        command_id
                    ),
                });
            }

            // Send the user's decision through the oneshot channel.
            // If the receiver has been dropped, that is not an error — the
            // command may have timed out already.
            let _ = sender.send(approved);
            Ok(())
        } else {
            Err(CoreError::NotFound {
                code: maekon_core::error_codes::NotFoundCode::ResourceMissing,
                resource_type: "PendingConfirmation".to_string(),
                id: command_id.to_string(),
            })
        }
    }

    async fn list_execution_policies(&self) -> Result<Vec<ExecutionPolicyDto>, CoreError> {
        Ok(self
            .policy_client
            .list_policies()
            .await
            .iter()
            .map(policy_to_dto)
            .collect())
    }

    async fn add_execution_policy(
        &self,
        policy: ExecutionPolicyDto,
    ) -> Result<ExecutionPolicyDto, CoreError> {
        let existed = self
            .policy_client
            .list_policies()
            .await
            .iter()
            .any(|existing| existing.policy_id == policy.policy_id);
        let operation = if existed { "update" } else { "create" };
        let internal = dto_to_policy(&policy);
        self.policy_client.add_policy(internal).await;
        {
            let mut logger = self.audit_logger.write().await;
            logger.log_event(
                if existed {
                    "automation.policy.update"
                } else {
                    "automation.policy.create"
                },
                "automation-policy",
                &policy_change_audit_details(operation, &policy),
            );
        }
        Ok(policy)
    }

    async fn remove_execution_policy(&self, policy_id: &str) -> Result<bool, CoreError> {
        let removed = self.policy_client.remove_policy(policy_id).await;
        {
            let mut logger = self.audit_logger.write().await;
            logger.log_event(
                "automation.policy.delete",
                "automation-policy",
                &policy_delete_audit_details(policy_id, removed),
            );
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod roundtrip_tests {
    use super::*;
    use crate::audit::AuditLogger;
    use crate::policy::AuditLevel;
    use crate::policy::PolicyClient;
    use crate::sandbox::NoOpSandbox;
    use maekon_core::config::SandboxProfile;
    use maekon_core::models::automation::ExecutionPolicyDto;
    use maekon_core::ports::automation::AutomationPort;
    use maekon_core::ports::sandbox::Sandbox;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    fn make_dto(sandbox: &str, audit: &str) -> ExecutionPolicyDto {
        ExecutionPolicyDto {
            policy_id: "test-policy".to_string(),
            process_name: "test".to_string(),
            process_hash: None,
            allowed_args: vec![],
            requires_sudo: false,
            max_execution_time_ms: 5000,
            audit_level: audit.to_string(),
            sandbox_profile: Some(sandbox.to_string()),
            allowed_paths: vec![],
            allow_network: Some(false),
            require_signed_token: false,
            confirmation: "CONFIRM".to_string(),
        }
    }

    fn make_controller_for_port_tests() -> (AutomationController, Arc<RwLock<AuditLogger>>) {
        let policy_client = Arc::new(PolicyClient::new());
        let audit_logger = Arc::new(RwLock::new(AuditLogger::new(100, 10)));
        let sandbox: Arc<dyn Sandbox> = Arc::new(NoOpSandbox);
        let sandbox_config = maekon_core::config::SandboxConfig::default();
        (
            AutomationController::new(policy_client, audit_logger.clone(), sandbox, sandbox_config),
            audit_logger,
        )
    }

    /// F-RC-C35-03: SandboxProfile Display and serde(rename_all = "PascalCase") roundtrip.
    /// policy_to_dto uses {:?} (PascalCase), dto_to_policy matches PascalCase literals.
    /// Display now also emits PascalCase — all three paths are consistent.
    #[test]
    fn sandbox_profile_round_trip() {
        for (variant, token) in [
            (SandboxProfile::Permissive, "Permissive"),
            (SandboxProfile::Standard, "Standard"),
            (SandboxProfile::Strict, "Strict"),
        ] {
            // Display must produce PascalCase (aligned with dto_to_policy matcher)
            assert_eq!(
                format!("{}", variant),
                token,
                "Display mismatch for {:?}",
                variant
            );
            // Debug must also produce PascalCase (used by policy_to_dto)
            assert_eq!(
                format!("{:?}", variant),
                token,
                "Debug mismatch for {:?}",
                variant
            );
            // serde JSON must produce PascalCase
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(
                json,
                format!("\"{}\"", token),
                "serde mismatch for {:?}",
                variant
            );
            // dto -> policy -> dto round-trip
            let dto = make_dto(token, "Basic");
            let policy = dto_to_policy(&dto);
            assert_eq!(policy.sandbox_profile, Some(variant));
        }
    }

    #[test]
    fn audit_level_round_trip() {
        for (variant, token) in [
            (AuditLevel::None, "None"),
            (AuditLevel::Basic, "Basic"),
            (AuditLevel::Detailed, "Detailed"),
        ] {
            // Debug (used by policy_to_dto) must produce PascalCase
            assert_eq!(
                format!("{:?}", variant),
                token,
                "Debug mismatch for {:?}",
                variant
            );
            // serde JSON must produce PascalCase
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(
                json,
                format!("\"{}\"", token),
                "serde mismatch for {:?}",
                variant
            );
            // dto -> policy round-trip
            let dto = make_dto("Standard", token);
            let policy = dto_to_policy(&dto);
            assert_eq!(policy.audit_level, variant);
        }
    }

    #[tokio::test]
    async fn policy_crud_emits_bounded_audit_events() {
        let (controller, audit_logger) = make_controller_for_port_tests();
        let mut policy = make_dto("Strict", "Detailed");
        policy.policy_id = "policy-1".to_string();
        policy.process_name = "secret-process-name".to_string();
        policy.allowed_args = vec!["--token=sk-secret-value".to_string()];
        policy.allowed_paths = vec!["/Users/alice/Secrets".to_string()];

        controller
            .add_execution_policy(policy.clone())
            .await
            .expect("create policy must succeed");
        policy.max_execution_time_ms = 9000;
        controller
            .add_execution_policy(policy.clone())
            .await
            .expect("update policy must succeed");
        assert!(controller
            .remove_execution_policy("policy-1")
            .await
            .expect("delete policy must return"));

        let logger = audit_logger.read().await;
        let entries = logger.entries_by_action_prefix("automation.policy.", 10);
        let action_types: Vec<_> = entries
            .iter()
            .map(|entry| entry.action_type.as_str())
            .collect();
        assert!(action_types.contains(&"automation.policy.create"));
        assert!(action_types.contains(&"automation.policy.update"));
        assert!(action_types.contains(&"automation.policy.delete"));

        let details = entries
            .iter()
            .filter_map(|entry| entry.details.as_deref())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(details.contains("allowed_args_count=1"));
        assert!(details.contains("allowed_paths_count=1"));
        assert!(!details.contains("sk-secret-value"));
        assert!(!details.contains("/Users/alice/Secrets"));
        assert!(!details.contains("secret-process-name"));
    }
}

#[cfg(test)]
mod round_trip_tests {
    use super::*;
    use crate::policy::{AuditLevel, ExecutionPolicy};

    fn make_policy(confirmation: maekon_core::config::ConfirmationRequirement) -> ExecutionPolicy {
        ExecutionPolicy {
            policy_id: "rt-test".to_string(),
            process_name: "test-proc".to_string(),
            process_hash: None,
            allowed_args: vec![],
            requires_sudo: false,
            max_execution_time_ms: 5000,
            audit_level: AuditLevel::Basic,
            sandbox_profile: None,
            allowed_paths: vec![],
            allow_network: None,
            require_signed_token: false,
            confirmation,
        }
    }

    #[test]
    fn confirmation_round_trip_all_variants() {
        for variant in [
            maekon_core::config::ConfirmationRequirement::Auto,
            maekon_core::config::ConfirmationRequirement::Confirm,
            maekon_core::config::ConfirmationRequirement::Block,
        ] {
            let policy = make_policy(variant);
            let dto = policy_to_dto(&policy);
            let restored = dto_to_policy(&dto);
            assert_eq!(
                restored.confirmation, variant,
                "round-trip failed for variant {:?}: dto confirmation string was {:?}",
                variant, dto.confirmation
            );
        }
    }
}
