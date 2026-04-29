use oneshim_core::error::CoreError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AutomationError {
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error("policy denied: {0}")]
    PolicyDenied(String),
    #[error("sandbox init failed: {0}")]
    SandboxInit(String),
    #[error("sandbox execution failed: {0}")]
    SandboxExecution(String),
    #[error("sandbox enforcement failed: {0}")]
    SandboxEnforcement(String),
    #[error("execution timeout after {timeout_ms}ms")]
    ExecutionTimeout { timeout_ms: u64 },
    #[error("element not found: {0}")]
    ElementNotFound(String),
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),
    #[error("internal error: {0}")]
    Internal(String),
    /// User denied the confirmation prompt.
    #[error("user denied automation command")]
    UserDenied,
    /// Policy blocks this command from executing.
    #[error("policy blocks execution of this command")]
    PolicyBlocked,
}

impl From<AutomationError> for CoreError {
    fn from(err: AutomationError) -> Self {
        match err {
            AutomationError::Core(e) => e,
            AutomationError::PolicyDenied(msg) => CoreError::PolicyDenied {
                code: oneshim_core::error_codes::PolicyCode::Denied,
                message: msg,
            },
            AutomationError::SandboxInit(msg) => CoreError::SandboxInit {
                code: oneshim_core::error_codes::SandboxCode::InitFailed,
                message: msg,
            },
            AutomationError::SandboxExecution(msg) => CoreError::SandboxExecution {
                code: oneshim_core::error_codes::SandboxCode::ExecutionFailed,
                message: msg,
            },
            // All SandboxEnforcement emission sites (13 in sandbox/{windows,linux}.rs)
            // are init-phase: Job Object creation, seccomp filter build/compile/apply
            // (pre-exec), Landlock ruleset setup. Runtime enforcement traps (SIGSYS
            // etc.) kill the child and surface as exit-status failure, not through
            // this variant. Map to SandboxInit accordingly.
            AutomationError::SandboxEnforcement(msg) => CoreError::SandboxInit {
                code: oneshim_core::error_codes::SandboxCode::InitFailed,
                message: msg,
            },
            AutomationError::ExecutionTimeout { timeout_ms } => CoreError::ExecutionTimeout {
                code: oneshim_core::error_codes::SandboxCode::Timeout,
                timeout_ms,
            },
            AutomationError::ElementNotFound(msg) => CoreError::ElementNotFound {
                code: oneshim_core::error_codes::UiCode::ElementMissing,
                name: msg,
            },
            AutomationError::InvalidArguments(msg) => CoreError::InvalidArguments {
                code: oneshim_core::error_codes::ValidationCode::InvalidArguments,
                message: msg,
            },
            AutomationError::Internal(msg) => CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: msg,
            },
            AutomationError::UserDenied => CoreError::PolicyDenied {
                code: oneshim_core::error_codes::PolicyCode::Denied,
                message: "user denied automation command".to_string(),
            },
            AutomationError::PolicyBlocked => CoreError::PolicyDenied {
                code: oneshim_core::error_codes::PolicyCode::Denied,
                message: "policy blocks execution of this command".to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression guard: SandboxEnforcement emissions are init-phase (Job Object
    /// creation, seccomp/Landlock config) and must map to SandboxInit with
    /// SandboxCode::InitFailed — not SandboxExecution. Caught in iter-87 drift
    /// audit; pre-fix mapping was SandboxExecution { ExecutionFailed }.
    #[test]
    fn sandbox_enforcement_maps_to_sandbox_init() {
        let err = AutomationError::SandboxEnforcement("Job Object creation failed".into());
        let core: CoreError = err.into();
        match core {
            CoreError::SandboxInit { code, message } => {
                assert_eq!(code.as_str(), "sandbox.init_failed");
                assert!(message.contains("Job Object"));
            }
            other => panic!("expected SandboxInit, got {other:?}"),
        }
    }

    /// Regression guard: PolicyBlocked maps to the single canonical
    /// PolicyCode::Denied wire code along with all other policy-denial variants
    /// (PolicyDenied, UserDenied). Prevents future dispatcher drift that would
    /// re-introduce a dead ProcessDenied wire code.
    #[test]
    fn all_policy_denial_variants_share_single_wire_code() {
        let code_str = "policy.denied";
        let cases: Vec<AutomationError> = vec![
            AutomationError::PolicyDenied("explicit".into()),
            AutomationError::UserDenied,
            AutomationError::PolicyBlocked,
        ];
        for case in cases {
            let core: CoreError = case.into();
            assert_eq!(core.code(), code_str, "variant drifted from {code_str}");
        }
    }

    #[test]
    fn sandbox_init_variant_preserves_init_failed_code() {
        let err = AutomationError::SandboxInit("explicit init failure".into());
        let core: CoreError = err.into();
        assert_eq!(core.code(), "sandbox.init_failed");
    }

    #[test]
    fn sandbox_execution_variant_preserves_execution_failed_code() {
        let err = AutomationError::SandboxExecution("explicit exec failure".into());
        let core: CoreError = err.into();
        assert_eq!(core.code(), "sandbox.execution_failed");
    }
}
