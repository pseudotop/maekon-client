use async_trait::async_trait;

use maekon_core::config::SandboxConfig;
use maekon_core::error::CoreError;
use maekon_core::models::automation::AutomationAction;
use maekon_core::ports::sandbox::{Sandbox, SandboxCapabilities};

pub struct NoOpSandbox;

#[async_trait]
impl Sandbox for NoOpSandbox {
    fn platform(&self) -> &str {
        "noop"
    }

    fn is_available(&self) -> bool {
        true
    }

    async fn execute_sandboxed(
        &self,
        action: &AutomationAction,
        _config: &SandboxConfig,
    ) -> Result<(), CoreError> {
        // #5980: redact KeyType text so typed PII never reaches the log sink.
        tracing::debug!(action = %super::redact_action(action), "NoOp sandbox: execution");
        Ok(())
    }

    fn capabilities(&self) -> SandboxCapabilities {
        SandboxCapabilities {
            filesystem_isolation: false,
            syscall_filtering: false,
            network_isolation: false,
            resource_limits: false,
            process_isolation: false,
        }
    }
}

/// Sandbox that refuses every execution.
///
/// Returned by `create_native_sandbox` when the user has *enabled* the sandbox
/// (`SandboxConfig.enabled == true`) but the build/platform cannot actually
/// enforce isolation — e.g. the platform sandbox feature was compiled out, or
/// the OS is one we have no adapter for (review4 A2/A6/A7). Substituting a
/// `NoOpSandbox` there was a silent fail-open: actions ran fully uncontained
/// yet reported success. This type fails *closed* instead, so an operator who
/// opted into sandboxing never has commands silently executed without it.
///
/// (The explicit opt-out — `enabled == false` — still maps to `NoOpSandbox`;
/// that is the documented "no sandboxing" path and is unaffected.)
pub struct FailClosedSandbox;

#[async_trait]
impl Sandbox for FailClosedSandbox {
    fn platform(&self) -> &str {
        "fail-closed"
    }

    fn is_available(&self) -> bool {
        false
    }

    async fn execute_sandboxed(
        &self,
        action: &AutomationAction,
        _config: &SandboxConfig,
    ) -> Result<(), CoreError> {
        tracing::error!(
            action = %super::redact_action(action),
            "sandbox enabled but no enforcement is available on this build/platform — refusing to execute uncontained"
        );
        Err(CoreError::SandboxUnsupported {
            code: maekon_core::error_codes::SandboxCode::UnsupportedPlatform,
            message: "sandbox is enabled but this build/platform cannot enforce isolation; refusing to run the action uncontained (build with the platform sandbox feature, or disable the sandbox explicitly)".to_string(),
        })
    }

    fn capabilities(&self) -> SandboxCapabilities {
        SandboxCapabilities {
            filesystem_isolation: false,
            syscall_filtering: false,
            network_isolation: false,
            resource_limits: false,
            process_isolation: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn noop_execute_succeeds() {
        let sandbox = NoOpSandbox;
        let action = AutomationAction::MouseMove { x: 100, y: 200 };
        let config = SandboxConfig::default();
        // NoOpSandbox.execute_sandboxed is a direct Ok(()) — no subprocess, no state.
        // Ok(()) is the full contract; the unit return type has no value to inspect
        // beyond success. (#5594: Ok-only is justified for this pure no-op driver.)
        sandbox
            .execute_sandboxed(&action, &config)
            .await
            .expect("NoOpSandbox::execute_sandboxed must always return Ok(())");
    }

    #[test]
    fn noop_capabilities_all_false() {
        let sandbox = NoOpSandbox;
        let caps = sandbox.capabilities();
        assert!(!caps.filesystem_isolation);
        assert!(!caps.syscall_filtering);
        assert!(!caps.network_isolation);
        assert!(!caps.resource_limits);
        assert!(!caps.process_isolation);
    }

    #[test]
    fn noop_is_available() {
        let sandbox = NoOpSandbox;
        assert!(sandbox.is_available());
        assert_eq!(sandbox.platform(), "noop");
    }

    #[tokio::test]
    async fn fail_closed_refuses_execution() {
        let sandbox = FailClosedSandbox;
        let action = AutomationAction::MouseMove { x: 1, y: 2 };
        let config = SandboxConfig::default();
        let err = sandbox
            .execute_sandboxed(&action, &config)
            .await
            .expect_err("FailClosedSandbox must refuse execution");
        assert!(matches!(err, CoreError::SandboxUnsupported { .. }));
    }

    #[test]
    fn fail_closed_reports_unavailable_and_no_caps() {
        let sandbox = FailClosedSandbox;
        assert!(!sandbox.is_available());
        assert_eq!(sandbox.platform(), "fail-closed");
        let caps = sandbox.capabilities();
        assert!(!caps.filesystem_isolation);
        assert!(!caps.syscall_filtering);
        assert!(!caps.network_isolation);
        assert!(!caps.resource_limits);
        assert!(!caps.process_isolation);
    }
}
