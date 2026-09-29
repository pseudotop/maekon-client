//! Validated policy shared by subprocess constructors and native command writers.

use maekon_api_contracts::provider_specs::SubprocessInvocationMode;
use maekon_core::error::CoreError;
use maekon_core::error_codes::PolicyCode;
use maekon_core::models::ai_session::SessionConfig;
use tokio::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CodexApprovalPolicy {
    OnRequest,
    Never,
}

impl CodexApprovalPolicy {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::OnRequest => "on-request",
            Self::Never => "never",
        }
    }
}

/// Only admitted requests reach a session. None preserves the CLI's approval
/// default; on-request forwards configuration, without promising an exec UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct InvocationPolicy {
    pub(crate) approval: Option<CodexApprovalPolicy>,
}

impl InvocationPolicy {
    pub(crate) const CODEX_SANDBOX: &'static str = "read-only";

    // #12094: a full guard-removal mutant must compile under unused=deny.
    pub(crate) fn require_tools_enabled(_config: &SessionConfig) -> Result<(), CoreError> {
        if !_config.tools_enabled {
            // #12094: built-in tool flags do not prove inherited MCP, plugins,
            // hooks and discovery are disabled. Never silently weaken false.
            return Err(Self::denied(
                "This subprocess adapter cannot guarantee tools_enabled=false",
            ));
        }
        Ok(())
    }

    pub(crate) fn resolve(
        config: &SessionConfig,
        mode: SubprocessInvocationMode,
    ) -> Result<Self, CoreError> {
        Self::require_tools_enabled(config)?;
        let approval = match (mode, config.sandbox_policy.as_deref()) {
            (
                SubprocessInvocationMode::CodexExecJson | SubprocessInvocationMode::CodexAppServer,
                None | Some(Self::CODEX_SANDBOX),
            ) => match config.approval_policy.as_deref() {
                None => None,
                Some("on-request") => Some(CodexApprovalPolicy::OnRequest),
                Some("never") => Some(CodexApprovalPolicy::Never),
                _ => return Err(Self::denied("Unsupported Codex approval policy")),
            },
            // All admitted Codex requests retain the catalog's exec read-only
            // ceiling and the same explicit sandbox on app-server thread/start.
            (
                SubprocessInvocationMode::CodexExecJson | SubprocessInvocationMode::CodexAppServer,
                _,
            ) => {
                return Err(Self::denied("Codex sessions require a read-only sandbox"));
            }
            (_, None) if config.approval_policy.is_none() => None,
            _ => {
                return Err(Self::denied(
                    "This subprocess adapter does not support explicit sandbox or approval policies",
                ));
            }
        };
        Ok(Self { approval })
    }

    pub(crate) fn append_command_flags(self, command: &mut Command) {
        if let Some(approval) = self.approval {
            // exec supports TOML config overrides; root --ask-for-approval is
            // not an exec argument. Keep the quoted TOML string in one argv item.
            command
                .arg("-c")
                .arg(format!("approval_policy=\"{}\"", approval.as_str()));
        }
    }

    fn denied(message: &str) -> CoreError {
        CoreError::PolicyDenied {
            code: PolicyCode::Denied,
            message: message.to_string(),
        }
    }
}
