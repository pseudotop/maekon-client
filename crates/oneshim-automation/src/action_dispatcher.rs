use async_trait::async_trait;
use std::sync::Arc;

use oneshim_core::config::SandboxConfig;
use oneshim_core::ports::sandbox::Sandbox;

use crate::controller::{AutomationAction, CommandResult};

#[async_trait]
pub trait AutomationActionDispatcher: Send + Sync {
    async fn dispatch(&self, action: &AutomationAction, config: &SandboxConfig) -> CommandResult;
}

pub struct SandboxActionDispatcher {
    sandbox: Arc<dyn Sandbox>,
}

impl SandboxActionDispatcher {
    pub fn new(sandbox: Arc<dyn Sandbox>) -> Self {
        Self { sandbox }
    }
}

#[async_trait]
impl AutomationActionDispatcher for SandboxActionDispatcher {
    async fn dispatch(&self, action: &AutomationAction, config: &SandboxConfig) -> CommandResult {
        tracing::info!(
            action = ?action,
            sandbox = self.sandbox.platform(),
            profile = ?config.profile,
            "dispatching to sandboxed worker"
        );

        match self.sandbox.execute_sandboxed(action, config).await {
            Ok(()) => CommandResult::Success,
            Err(e) => {
                tracing::error!(error = %e, "sandboxed execution failed");
                CommandResult::Failed(format!("Sandbox execution failed: {}", e))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oneshim_core::error::CoreError;
    use oneshim_core::models::automation::AutomationAction as CoreAction;
    use oneshim_core::ports::sandbox::SandboxCapabilities;

    struct MockSandbox {
        should_fail: bool,
    }

    #[async_trait]
    impl Sandbox for MockSandbox {
        fn platform(&self) -> &str {
            "mock"
        }

        fn is_available(&self) -> bool {
            true
        }

        async fn execute_sandboxed(
            &self,
            _action: &CoreAction,
            _config: &SandboxConfig,
        ) -> Result<(), CoreError> {
            if self.should_fail {
                Err(CoreError::Network {
                    code: oneshim_core::error_codes::NetworkCode::Generic,
                    message: "mock sandbox failure".to_string(),
                })
            } else {
                Ok(())
            }
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

    #[tokio::test]
    async fn dispatch_mouse_move_returns_success() {
        let sandbox = Arc::new(MockSandbox { should_fail: false });
        let dispatcher = SandboxActionDispatcher::new(sandbox);
        let action = AutomationAction::MouseMove { x: 100, y: 200 };
        let config = SandboxConfig::default();
        let result = dispatcher.dispatch(&action, &config).await;
        assert!(matches!(result, CommandResult::Success));
    }

    #[tokio::test]
    async fn dispatch_key_type_returns_success() {
        let sandbox = Arc::new(MockSandbox { should_fail: false });
        let dispatcher = SandboxActionDispatcher::new(sandbox);
        let action = AutomationAction::KeyType {
            text: "hello world".to_string(),
        };
        let config = SandboxConfig::default();
        let result = dispatcher.dispatch(&action, &config).await;
        assert!(matches!(result, CommandResult::Success));
    }

    #[tokio::test]
    async fn dispatch_returns_failed_when_sandbox_errors() {
        let sandbox = Arc::new(MockSandbox { should_fail: true });
        let dispatcher = SandboxActionDispatcher::new(sandbox);
        let action = AutomationAction::KeyPress {
            key: "Enter".to_string(),
        };
        let config = SandboxConfig::default();
        let result = dispatcher.dispatch(&action, &config).await;
        assert!(matches!(result, CommandResult::Failed(_)));
    }

    #[tokio::test]
    async fn dispatch_hotkey_returns_success() {
        let sandbox = Arc::new(MockSandbox { should_fail: false });
        let dispatcher = SandboxActionDispatcher::new(sandbox);
        let action = AutomationAction::Hotkey {
            keys: vec!["ctrl".to_string(), "c".to_string()],
        };
        let config = SandboxConfig::default();
        let result = dispatcher.dispatch(&action, &config).await;
        assert!(matches!(result, CommandResult::Success));
    }
}
