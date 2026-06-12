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
    /// Permissive-noop 경로(#4539)에서 액션을 인-프로세스로 실행하는 드라이버.
    /// `None`이면 기존 동작(서브프로세스 생략 + 실행 없이 Success 반환)을 유지한다.
    inline_driver: Option<Arc<dyn InputDriver>>,
}

impl SandboxActionDispatcher {
    /// 인라인 드라이버 없이 생성한다. Permissive-noop 경로에서는 실행을 생략하고
    /// `Success`를 반환하는 기존 동작을 보존한다(레거시 호출처용).
    pub fn new(sandbox: Arc<dyn Sandbox>) -> Self {
        Self {
            sandbox,
            inline_driver: None,
        }
    }

    /// 인라인 InputDriver를 함께 주입해 생성한다. Permissive-noop 경로에서
    /// 서브프로세스를 띄우지 않고 이 드라이버로 액션을 직접 실행한다(#4539).
    pub fn with_inline_driver(sandbox: Arc<dyn Sandbox>, driver: Arc<dyn InputDriver>) -> Self {
        Self {
            sandbox,
            inline_driver: Some(driver),
        }
    }

    /// `AutomationAction`을 6개 variant 매핑으로 인-프로세스 InputDriver에 위임한다.
    /// 워커(`maekon-sandbox-worker`)·`GatedInputDriver`의 매핑과 동일하게 유지한다.
    async fn execute_inline(
        driver: &Arc<dyn InputDriver>,
        action: &AutomationAction,
    ) -> Result<(), CoreError> {
        match action {
            AutomationAction::MouseMove { x, y } => driver.mouse_move(*x, *y).await,
            AutomationAction::MouseClick { button, x, y } => {
                driver.mouse_click(button, *x, *y).await
            }
            AutomationAction::KeyType { text } => driver.type_text(text).await,
            AutomationAction::KeyPress { key } => driver.key_press(key).await,
            AutomationAction::KeyRelease { key } => driver.key_release(key).await,
            AutomationAction::Hotkey { keys } => driver.hotkey(keys).await,
        }
    }
}

#[async_trait]
impl AutomationActionDispatcher for SandboxActionDispatcher {
    async fn dispatch(&self, action: &AutomationAction, config: &SandboxConfig) -> CommandResult {
        // Permissive-noop 경로: 서브프로세스 샌드박스를 띄울 필요가 없다.
        // 단, 액션을 그냥 버리면 거짓 성공이 된다(#4539). 인라인 드라이버가
        // 배선되어 있으면 인-프로세스로 실행하고, 아니면 기존 동작을 보존한다.
        if is_permissive_noop(config) {
            match &self.inline_driver {
                Some(driver) => {
                    tracing::info!(
                        action = ?action,
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
                    // 레거시 호출처: 인라인 드라이버가 없으면 실행 없이 Success 반환
                    // (기존 silent-skip 동작 보존). 샌드박스는 호출하지 않는다.
                    tracing::debug!(
                        action = ?action,
                        profile = ?config.profile,
                        "permissive-noop: no inline driver wired, skipping execution"
                    );
                    return CommandResult::Success;
                }
            }
        }

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

    // --- #4539: Permissive-noop 인라인 실행 경로 회귀 테스트 ---

    use maekon_core::config::SandboxProfile;
    use std::sync::Mutex;

    /// 인-프로세스 호출을 기록하는 InputDriver mock.
    /// (worker `MockInputExecutor` / intent_resolver `MockInputDriver` 패턴 차용)
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

    /// Permissive + 0-제한 (max_memory_bytes==0 && max_cpu_time_ms==0) → permissive-noop 판정.
    fn permissive_noop_config() -> SandboxConfig {
        SandboxConfig {
            profile: SandboxProfile::Permissive,
            max_memory_bytes: 0,
            max_cpu_time_ms: 0,
            ..Default::default()
        }
    }

    /// 인라인 드라이버가 배선되면 permissive-noop 경로에서 액션이 실행되고(거짓 성공 X),
    /// (실패하도록 설정된) 샌드박스는 호출되지 않아야 한다(#4539 핵심 회귀).
    #[tokio::test]
    async fn permissive_noop_with_inline_driver_executes_and_bypasses_sandbox() {
        // 샌드박스는 호출되면 무조건 실패하도록 설정 — 인라인 경로가 샌드박스를
        // 우회함을 증명한다(호출되면 Failed 가 되어 테스트가 깨짐).
        let sandbox = Arc::new(MockSandbox { should_fail: true });
        let driver = Arc::new(RecordingInputDriver::default());
        let dispatcher = SandboxActionDispatcher::with_inline_driver(sandbox, driver.clone());

        let action = AutomationAction::KeyType {
            text: "hello".to_string(),
        };
        let result = dispatcher
            .dispatch(&action, &permissive_noop_config())
            .await;

        // 1) 실제 실행되어 Success
        assert!(
            matches!(result, CommandResult::Success),
            "expected Success, got {result:?}"
        );
        // 2) 드라이버가 호출을 기록 (액션이 버려지지 않음)
        let calls = driver.calls.lock().unwrap();
        assert_eq!(calls.as_slice(), &["type:hello".to_string()]);
        // 3) should_fail 샌드박스가 호출되지 않음 (호출됐다면 Failed 였을 것)
    }

    /// 인라인 드라이버가 없으면(레거시 호출처) permissive-noop 경로는 기존 동작 보존:
    /// 실행 없이 Success 반환, 샌드박스 미호출.
    #[tokio::test]
    async fn permissive_noop_without_inline_driver_preserves_skip() {
        // should_fail=true 인데도 Success 여야 함 — 샌드박스가 호출되지 않음을 증명.
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

    /// 6개 variant 모두 인라인 드라이버로 올바르게 매핑되는지 확인.
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
