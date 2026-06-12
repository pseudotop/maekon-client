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
            // arbitrary scripts running inside the WebView.
            if confirmation.nonce != nonce {
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
        let internal = dto_to_policy(&policy);
        self.policy_client.add_policy(internal).await;
        Ok(policy)
    }

    async fn remove_execution_policy(&self, policy_id: &str) -> Result<bool, CoreError> {
        Ok(self.policy_client.remove_policy(policy_id).await)
    }
}

#[cfg(test)]
mod roundtrip_tests {
    use super::*;
    use crate::policy::AuditLevel;
    use maekon_core::config::SandboxProfile;
    use maekon_core::models::automation::ExecutionPolicyDto;

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
