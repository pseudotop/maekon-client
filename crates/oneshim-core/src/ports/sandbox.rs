//! Platform-specific sandbox execution port for running automation actions in isolation.

use async_trait::async_trait;

use crate::config::SandboxConfig;
use crate::error::CoreError;
use crate::models::automation::AutomationAction;

#[derive(Debug, Clone)]
pub struct SandboxCapabilities {
    pub filesystem_isolation: bool,
    pub syscall_filtering: bool,
    pub network_isolation: bool,
    pub resource_limits: bool,
    pub process_isolation: bool,
}

/// # Errors
/// Sandbox emissions split by failure phase (iter-88 re-route):
/// - `CoreError::SandboxInit` (wire: `sandbox.init_failed`) for
///   enforcement setup failures: seccomp filter build/compile/apply,
///   Landlock ruleset creation, Windows Job Object/Restricted Token
///   setup, macOS Seatbelt. All pre-exec phases route here.
/// - `CoreError::SandboxExecution` (wire: `sandbox.execution_failed`)
///   for post-exec runtime failures: child process termination,
///   output parsing errors.
/// - `CoreError::SandboxUnsupported` (wire: `sandbox.unsupported_platform`)
///   when the platform lacks the sandbox feature gate (e.g.,
///   `linux-sandbox` / `windows-sandbox` feature not compiled in).
/// - `CoreError::ExecutionTimeout` (wire: `sandbox.timeout`) when the
///   sandboxed action exceeds its timeout budget.
#[async_trait]
pub trait Sandbox: Send + Sync {
    fn platform(&self) -> &str;

    fn is_available(&self) -> bool;

    async fn execute_sandboxed(
        &self,
        action: &AutomationAction,
        config: &SandboxConfig,
    ) -> Result<(), CoreError>;

    fn capabilities(&self) -> SandboxCapabilities;
}
