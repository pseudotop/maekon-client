use async_trait::async_trait;
use std::sync::Arc;

use maekon_core::config::SandboxConfig;
use maekon_core::error::CoreError;
use maekon_core::ports::input_driver::InputDriver;
use maekon_core::ports::sandbox::Sandbox;

use crate::controller::{AutomationAction, CommandResult};
use crate::sandbox::is_permissive_noop;

#[async_trait]
pub trait AutomationActionDispatcher: Send + Sync {
    async fn dispatch(&self, action: &AutomationAction, config: &SandboxConfig) -> CommandResult;
}

pub struct SandboxActionDispatcher {
    sandbox: Arc<dyn Sandbox>,
    /// Driver that executes the action in-process on the permissive-noop path (#4539).
    /// When `None`, the legacy behavior is kept (subprocess skipped + Success returned
    /// without executing anything).
    inline_driver: Option<Arc<dyn InputDriver>>,
}

impl SandboxActionDispatcher {
    /// Construct without an inline driver. On the permissive-noop path this preserves
    /// the legacy behavior of skipping execution and returning `Success` (for legacy
    /// call sites).
    pub fn new(sandbox: Arc<dyn Sandbox>) -> Self {
        Self {
            sandbox,
            inline_driver: None,
        }
    }

    /// Construct with an inline InputDriver injected. On the permissive-noop path this
    /// executes the action directly via this driver instead of spawning a subprocess
    /// (#4539).
    pub fn with_inline_driver(sandbox: Arc<dyn Sandbox>, driver: Arc<dyn InputDriver>) -> Self {
        Self {
            sandbox,
            inline_driver: Some(driver),
        }
    }

    /// Delegate an `AutomationAction` to the in-process InputDriver via a 6-variant
    /// mapping. Kept identical to the mapping in the worker (`maekon-sandbox-worker`)
    /// and `GatedInputDriver`.
    async fn execute_inline(
        driver: &Arc<dyn InputDriver>,
        action: &AutomationAction,
    ) -> Result<(), CoreError> {
        // Match the sandbox worker's keystroke-burst cap (oneshim#5982) on the
        // in-process path too (review4 A8): without it the inline driver synthesizes
        // unbounded keystrokes, strictly weaker than the subprocess path this mapping
        // claims to mirror. Shared const so the dispatcher and intent_resolver caps
        // cannot diverge (review4 re-verify).
        const MAX_KEY_TYPE_CHARS: usize = crate::input_driver::MAX_SYNTHESIZED_INPUT_LEN;
        match action {
            AutomationAction::MouseMove { x, y } => driver.mouse_move(*x, *y).await,
            AutomationAction::MouseClick { button, x, y } => {
                driver.mouse_click(button, *x, *y).await
            }
            AutomationAction::KeyType { text } => {
                if text.chars().count() > MAX_KEY_TYPE_CHARS {
                    return Err(CoreError::InvalidArguments {
                        code: maekon_core::error_codes::ValidationCode::InvalidArguments,
                        message: format!("KeyType text exceeds {MAX_KEY_TYPE_CHARS} chars"),
                    });
                }
                driver.type_text(text).await
            }
            AutomationAction::KeyPress { key } => driver.key_press(key).await,
            AutomationAction::KeyRelease { key } => driver.key_release(key).await,
            AutomationAction::Hotkey { keys } => {
                if keys.len() > MAX_KEY_TYPE_CHARS {
                    return Err(CoreError::InvalidArguments {
                        code: maekon_core::error_codes::ValidationCode::InvalidArguments,
                        message: format!("Hotkey exceeds {MAX_KEY_TYPE_CHARS} keys"),
                    });
                }
                driver.hotkey(keys).await
            }
        }
    }
}

#[async_trait]
impl AutomationActionDispatcher for SandboxActionDispatcher {
    async fn dispatch(&self, action: &AutomationAction, config: &SandboxConfig) -> CommandResult {
        // Permissive-noop path: there is no need to spawn a subprocess sandbox.
        // But silently dropping the action would be a false success (#4539). If an
        // inline driver is wired, execute in-process; otherwise preserve the legacy
        // behavior.
        if is_permissive_noop(config) {
            match &self.inline_driver {
                Some(driver) => {
                    tracing::info!(
                        action = %crate::sandbox::redact_action(action),
                        profile = ?config.profile,
                        driver = driver.platform(),
                        "permissive-noop: executing action via inline input driver"
                    );
                    return match Self::execute_inline(driver, action).await {
                        Ok(()) => CommandResult::Success,
                        Err(e) => {
                            tracing::error!(error = %e, "inline action execution failed");
                            CommandResult::Failed(format!("Inline execution failed: {}", e))
                        }
                    };
                }
                None => {
                    // Legacy call site: with no inline driver, return Success without
                    // executing (preserves the legacy silent-skip behavior). The sandbox
                    // is not invoked.
                    tracing::debug!(
                        action = %crate::sandbox::redact_action(action),
                        profile = ?config.profile,
                        "permissive-noop: no inline driver wired, skipping execution"
                    );
                    return CommandResult::Success;
                }
            }
        }

        tracing::info!(
            action = %crate::sandbox::redact_action(action),
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
    use maekon_core::error::CoreError;
    use maekon_core::models::automation::AutomationAction as CoreAction;
    use maekon_core::ports::sandbox::SandboxCapabilities;

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
                    code: maekon_core::error_codes::NetworkCode::Generic,
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

    // --- #4539: regression tests for the permissive-noop inline execution path ---

    use maekon_core::config::SandboxProfile;
    use std::sync::Mutex;

    /// InputDriver mock that records in-process calls.
    /// (borrows the worker `MockInputExecutor` / intent_resolver `MockInputDriver` pattern)
    #[derive(Default)]
    struct RecordingInputDriver {
        calls: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl InputDriver for RecordingInputDriver {
        async fn mouse_move(&self, x: i32, y: i32) -> Result<(), CoreError> {
            self.calls.lock().unwrap().push(format!("move:{x},{y}"));
            Ok(())
        }
        async fn mouse_click(&self, button: &str, x: i32, y: i32) -> Result<(), CoreError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("click:{button},{x},{y}"));
            Ok(())
        }
        async fn type_text(&self, text: &str) -> Result<(), CoreError> {
            self.calls.lock().unwrap().push(format!("type:{text}"));
            Ok(())
        }
        async fn key_press(&self, key: &str) -> Result<(), CoreError> {
            self.calls.lock().unwrap().push(format!("press:{key}"));
            Ok(())
        }
        async fn key_release(&self, key: &str) -> Result<(), CoreError> {
            self.calls.lock().unwrap().push(format!("release:{key}"));
            Ok(())
        }
        async fn hotkey(&self, keys: &[String]) -> Result<(), CoreError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("hotkey:{}", keys.join("+")));
            Ok(())
        }
        fn platform(&self) -> &str {
            "recording"
        }
    }

    /// Permissive + zero limits (max_memory_bytes==0 && max_cpu_time_ms==0) → classified
    /// as permissive-noop.
    fn permissive_noop_config() -> SandboxConfig {
        SandboxConfig {
            profile: SandboxProfile::Permissive,
            max_memory_bytes: 0,
            max_cpu_time_ms: 0,
            ..Default::default()
        }
    }

    /// When an inline driver is wired, the permissive-noop path must execute the action
    /// (no false success), and the sandbox (configured to fail) must not be invoked
    /// (the core #4539 regression).
    #[tokio::test]
    async fn permissive_noop_with_inline_driver_executes_and_bypasses_sandbox() {
        // Configure the sandbox to always fail when invoked — this proves the inline
        // path bypasses the sandbox (if invoked it would return Failed and break the test).
        let sandbox = Arc::new(MockSandbox { should_fail: true });
        let driver = Arc::new(RecordingInputDriver::default());
        let dispatcher = SandboxActionDispatcher::with_inline_driver(sandbox, driver.clone());

        let action = AutomationAction::KeyType {
            text: "hello".to_string(),
        };
        let result = dispatcher
            .dispatch(&action, &permissive_noop_config())
            .await;

        // 1) Actually executed → Success
        assert!(
            matches!(result, CommandResult::Success),
            "expected Success, got {result:?}"
        );
        // 2) The driver recorded the call (the action was not dropped)
        let calls = driver.calls.lock().unwrap();
        assert_eq!(calls.as_slice(), &["type:hello".to_string()]);
        // 3) The should_fail sandbox was not invoked (it would have returned Failed)
    }

    /// With no inline driver (legacy call site) the permissive-noop path preserves the
    /// legacy behavior: returns Success without executing, sandbox not invoked.
    #[tokio::test]
    async fn permissive_noop_without_inline_driver_preserves_skip() {
        // Must be Success even though should_fail=true — proves the sandbox is not invoked.
        let sandbox = Arc::new(MockSandbox { should_fail: true });
        let dispatcher = SandboxActionDispatcher::new(sandbox);

        let action = AutomationAction::KeyType {
            text: "hello".to_string(),
        };
        let result = dispatcher
            .dispatch(&action, &permissive_noop_config())
            .await;

        assert!(
            matches!(result, CommandResult::Success),
            "expected Success (skip preserved), got {result:?}"
        );
    }

    /// Verify all 6 variants map correctly through the inline driver.
    #[tokio::test]
    async fn permissive_noop_inline_maps_all_six_variants() {
        let sandbox = Arc::new(MockSandbox { should_fail: true });
        let driver = Arc::new(RecordingInputDriver::default());
        let dispatcher = SandboxActionDispatcher::with_inline_driver(sandbox, driver.clone());
        let config = permissive_noop_config();

        let actions = vec![
            AutomationAction::MouseMove { x: 1, y: 2 },
            AutomationAction::MouseClick {
                button: "left".to_string(),
                x: 3,
                y: 4,
            },
            AutomationAction::KeyType {
                text: "ab".to_string(),
            },
            AutomationAction::KeyPress {
                key: "Enter".to_string(),
            },
            AutomationAction::KeyRelease {
                key: "Enter".to_string(),
            },
            AutomationAction::Hotkey {
                keys: vec!["ctrl".to_string(), "c".to_string()],
            },
        ];
        for action in &actions {
            assert!(matches!(
                dispatcher.dispatch(action, &config).await,
                CommandResult::Success
            ));
        }

        let calls = driver.calls.lock().unwrap();
        assert_eq!(
            calls.as_slice(),
            &[
                "move:1,2".to_string(),
                "click:left,3,4".to_string(),
                "type:ab".to_string(),
                "press:Enter".to_string(),
                "release:Enter".to_string(),
                "hotkey:ctrl+c".to_string(),
            ]
        );
    }
}
