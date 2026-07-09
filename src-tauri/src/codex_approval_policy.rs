//! `ApprovalPolicyPort` adapter over the automation `PolicyClient` (E21 #4870).
//!
//! Bridges the architecture-clean approval decider in `maekon-core` to the
//! concrete `maekon_automation::policy::PolicyClient`. Only the binary crate may
//! see BOTH `maekon-core` and `maekon-automation`, so this adapter lives here.
//!
//! Maps a process lookup to a [`PolicyVerdict`] using `get_policy_for_process`
//! (NOT `validate_command`, which is a crypto-token validator — see the decider
//! module doc). The matched policy's [`ConfirmationRequirement`] selects
//! Auto/Confirm/Block, and `allow_network == Some(true)` is the network opt-in.

use std::sync::Arc;

use async_trait::async_trait;
use maekon_automation::policy::PolicyClient;
use maekon_core::config::ConfirmationRequirement;
use maekon_core::ports::codex_approval::{ApprovalPolicyPort, PolicyVerdict};

/// `ApprovalPolicyPort` backed by the shared automation `PolicyClient`.
pub struct PolicyClientApprovalAdapter {
    policy: Arc<PolicyClient>,
}

impl PolicyClientApprovalAdapter {
    pub fn new(policy: Arc<PolicyClient>) -> Self {
        Self { policy }
    }
}

#[async_trait]
impl ApprovalPolicyPort for PolicyClientApprovalAdapter {
    async fn verdict_for(
        &self,
        process_name: &str,
        args: &[String],
    ) -> Result<PolicyVerdict, String> {
        let Some(policy) = self.policy.get_policy_for_process(process_name).await else {
            return Ok(PolicyVerdict::NoMatch);
        };

        // The args must satisfy the policy's allowed-args allowlist; a violation
        // is treated as a Block (the decider declines) rather than NoMatch, so a
        // matched-but-out-of-scope command is never silently escalated as if no
        // policy existed.
        if !PolicyClient::validate_args(&policy, args) {
            return Ok(PolicyVerdict::Block {
                policy_id: policy.policy_id,
            });
        }

        let allow_network = policy.allow_network == Some(true);
        Ok(match policy.confirmation {
            ConfirmationRequirement::Auto => PolicyVerdict::Auto {
                policy_id: policy.policy_id,
                allow_network,
            },
            ConfirmationRequirement::Confirm => PolicyVerdict::Confirm {
                policy_id: policy.policy_id,
            },
            ConfirmationRequirement::Block => PolicyVerdict::Block {
                policy_id: policy.policy_id,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_automation::policy::{AuditLevel, ExecutionPolicy};

    fn policy(
        process: &str,
        confirmation: ConfirmationRequirement,
        allow_network: Option<bool>,
    ) -> ExecutionPolicy {
        ExecutionPolicy {
            policy_id: format!("pol-{process}"),
            process_name: process.to_string(),
            process_hash: None,
            allowed_args: vec![],
            requires_sudo: false,
            max_execution_time_ms: 5000,
            audit_level: AuditLevel::Basic,
            sandbox_profile: None,
            allowed_paths: vec![],
            allow_network,
            require_signed_token: false,
            confirmation,
        }
    }

    #[tokio::test]
    async fn no_policy_match_yields_no_match() {
        let client = Arc::new(PolicyClient::new());
        let adapter = PolicyClientApprovalAdapter::new(client);
        let verdict = adapter.verdict_for("git", &[]).await.unwrap();
        assert_eq!(verdict, PolicyVerdict::NoMatch);
    }

    #[tokio::test]
    async fn auto_policy_maps_to_auto_with_network_opt_in() {
        let client = Arc::new(PolicyClient::new());
        client
            .add_policy(policy("git", ConfirmationRequirement::Auto, Some(true)))
            .await;
        let adapter = PolicyClientApprovalAdapter::new(client);
        let verdict = adapter.verdict_for("git", &[]).await.unwrap();
        assert_eq!(
            verdict,
            PolicyVerdict::Auto {
                policy_id: "pol-git".to_string(),
                allow_network: true,
            }
        );
    }

    #[tokio::test]
    async fn confirm_default_maps_to_confirm() {
        let client = Arc::new(PolicyClient::new());
        client
            .add_policy(policy("git", ConfirmationRequirement::Confirm, None))
            .await;
        let adapter = PolicyClientApprovalAdapter::new(client);
        let verdict = adapter.verdict_for("git", &[]).await.unwrap();
        assert_eq!(
            verdict,
            PolicyVerdict::Confirm {
                policy_id: "pol-git".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn block_policy_maps_to_block() {
        let client = Arc::new(PolicyClient::new());
        client
            .add_policy(policy("git", ConfirmationRequirement::Block, None))
            .await;
        let adapter = PolicyClientApprovalAdapter::new(client);
        let verdict = adapter.verdict_for("git", &[]).await.unwrap();
        assert_eq!(
            verdict,
            PolicyVerdict::Block {
                policy_id: "pol-git".to_string(),
            }
        );
    }

    /// #7932 Part B: a within-session CRUD add via the automation controller's
    /// port method must be IMMEDIATELY visible to the Codex approval decider's
    /// `verdict_for`, because the composition root wires BOTH over the SAME
    /// `Arc<PolicyClient>` (Port Instance Sharing). Before this fix the two held
    /// separate instances that converged only across restart.
    #[tokio::test]
    async fn add_via_shared_controller_handle_is_visible_to_decider_verdict() {
        use maekon_automation::audit::AuditLogger;
        use maekon_automation::controller::AutomationController;
        use maekon_automation::sandbox::NoOpSandbox;
        use maekon_core::models::automation::ExecutionPolicyDto;
        use maekon_core::ports::automation::AutomationPort;
        use maekon_core::ports::sandbox::Sandbox;
        use tokio::sync::RwLock;

        // ONE shared Arc<PolicyClient> — the single instance the composition root
        // injects into both the controller and the decider.
        let shared = Arc::new(PolicyClient::new());

        // Decider side: approval adapter over the shared instance. With no policy
        // yet, it escalates (NoMatch).
        let decider = PolicyClientApprovalAdapter::new(shared.clone());
        assert_eq!(
            decider.verdict_for("git", &[]).await.unwrap(),
            PolicyVerdict::NoMatch,
            "no grant yet — the decider must escalate"
        );

        // Controller side: a real AutomationController over the SAME shared handle.
        let audit_logger = Arc::new(RwLock::new(AuditLogger::new(100, 10)));
        let sandbox: Arc<dyn Sandbox> = Arc::new(NoOpSandbox);
        let controller = AutomationController::new(
            shared.clone(),
            audit_logger,
            sandbox,
            maekon_core::config::SandboxConfig::default(),
        );

        // A within-session CRUD add via the controller's REAL port method...
        let dto = ExecutionPolicyDto {
            policy_id: "pol-shared".to_string(),
            process_name: "git".to_string(),
            process_hash: None,
            allowed_args: vec![],
            requires_sudo: false,
            max_execution_time_ms: 5000,
            audit_level: "Basic".to_string(),
            sandbox_profile: None,
            allowed_paths: vec![],
            allow_network: Some(true),
            require_signed_token: false,
            confirmation: "AUTO".to_string(),
        };
        controller
            .add_execution_policy(dto)
            .await
            .expect("add_execution_policy must succeed");

        // ...is IMMEDIATELY visible to the decider's verdict_for — no restart —
        // because both sides hold the same Arc<PolicyClient>.
        assert_eq!(
            decider.verdict_for("git", &[]).await.unwrap(),
            PolicyVerdict::Auto {
                policy_id: "pol-shared".to_string(),
                allow_network: true,
            },
            "a controller-side add must be visible to the decider within the same session"
        );
    }

    #[tokio::test]
    async fn args_violating_allowlist_blocks() {
        let client = Arc::new(PolicyClient::new());
        let mut p = policy("git", ConfirmationRequirement::Auto, None);
        p.allowed_args = vec!["status".to_string()];
        client.add_policy(p).await;
        let adapter = PolicyClientApprovalAdapter::new(client);
        // "push" is not in the allowlist → Block (decline), not Auto.
        let verdict = adapter
            .verdict_for("git", &["push".to_string()])
            .await
            .unwrap();
        assert_eq!(
            verdict,
            PolicyVerdict::Block {
                policy_id: "pol-git".to_string(),
            }
        );
    }
}
