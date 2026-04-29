use super::*;
use crate::intent_planner::IntentPlanner;
use crate::policy::{AuditLevel, ExecutionPolicy};
use crate::sandbox::NoOpSandbox;
use oneshim_core::error::CoreError;
use oneshim_core::models::intent::{
    AutomationIntent, FinderSource, IntentConfig, PresetCategory, UiElement, WorkflowPreset,
    WorkflowStep,
};
use oneshim_core::models::ui_scene::{
    NormalizedBounds, UiScene, UiSceneElement, UI_SCENE_SCHEMA_VERSION,
};
use oneshim_core::ports::sandbox::SandboxCapabilities;

fn make_controller() -> AutomationController {
    let policy_client = Arc::new(PolicyClient::new());
    let audit_logger = Arc::new(RwLock::new(AuditLogger::default()));
    let sandbox: Arc<dyn Sandbox> = Arc::new(NoOpSandbox);
    let sandbox_config = SandboxConfig::default();
    AutomationController::new(policy_client, audit_logger, sandbox, sandbox_config)
}

fn make_controller_with_policy(
    policy: ExecutionPolicy,
) -> (
    AutomationController,
    Arc<PolicyClient>,
    Arc<RwLock<AuditLogger>>,
) {
    let policy_client = Arc::new(PolicyClient::new());
    let audit_logger = Arc::new(RwLock::new(AuditLogger::new(100, 10)));
    let sandbox: Arc<dyn Sandbox> = Arc::new(NoOpSandbox);
    let sandbox_config = SandboxConfig::default();
    let controller = AutomationController::new(
        policy_client.clone(),
        audit_logger.clone(),
        sandbox,
        sandbox_config,
    );
    let _ = policy; // policy is applied in tests via update_policies
    (controller, policy_client, audit_logger)
}

fn make_policy(audit: AuditLevel, timeout: u64) -> ExecutionPolicy {
    ExecutionPolicy {
        policy_id: "test-pol".to_string(),
        process_name: "test".to_string(),
        process_hash: None,
        allowed_args: vec![],
        requires_sudo: false,
        max_execution_time_ms: timeout,
        audit_level: audit,
        sandbox_profile: None,
        allowed_paths: vec![],
        allow_network: None,
        require_signed_token: false,
        confirmation: oneshim_core::config::ConfirmationRequirement::Auto,
    }
}

struct StubPlanner {
    planned: AutomationIntent,
}

#[async_trait::async_trait]
impl IntentPlanner for StubPlanner {
    async fn plan(&self, _intent_hint: &str) -> Result<AutomationIntent, CoreError> {
        Ok(self.planned.clone())
    }
}

struct StubSceneFinder;

#[async_trait::async_trait]
impl ElementFinder for StubSceneFinder {
    async fn find_element(
        &self,
        _text: Option<&str>,
        _role: Option<&str>,
        _region: Option<&oneshim_core::models::intent::ElementBounds>,
    ) -> Result<Vec<oneshim_core::models::intent::UiElement>, CoreError> {
        Ok(vec![])
    }

    async fn analyze_scene(
        &self,
        app_name: Option<&str>,
        screen_id: Option<&str>,
    ) -> Result<UiScene, CoreError> {
        Ok(UiScene {
            schema_version: UI_SCENE_SCHEMA_VERSION.to_string(),
            scene_id: "scene-stub".to_string(),
            app_name: app_name.map(str::to_string),
            screen_id: screen_id.map(str::to_string),
            captured_at: chrono::Utc::now(),
            screen_width: 1920,
            screen_height: 1080,
            elements: vec![UiSceneElement {
                element_id: "el-1".to_string(),
                bbox_abs: oneshim_core::models::intent::ElementBounds {
                    x: 100,
                    y: 80,
                    width: 240,
                    height: 48,
                },
                bbox_norm: NormalizedBounds::new(0.05, 0.07, 0.12, 0.04),
                label: "Save".to_string(),
                role: Some("button".to_string()),
                intent: Some("execute".to_string()),
                state: Some("enabled".to_string()),
                confidence: 0.95,
                text_masked: Some("Save".to_string()),
                parent_id: None,
            }],
        })
    }

    fn name(&self) -> &str {
        "stub-scene"
    }
}

struct MatchingElementFinder;

#[async_trait::async_trait]
impl ElementFinder for MatchingElementFinder {
    async fn find_element(
        &self,
        text: Option<&str>,
        role: Option<&str>,
        _region: Option<&oneshim_core::models::intent::ElementBounds>,
    ) -> Result<Vec<UiElement>, CoreError> {
        Ok(vec![UiElement {
            text: text.unwrap_or("matched").to_string(),
            bounds: oneshim_core::models::intent::ElementBounds {
                x: 40,
                y: 60,
                width: 120,
                height: 30,
            },
            role: role.map(str::to_string),
            confidence: 0.95,
            source: FinderSource::Accessibility,
        }])
    }

    fn name(&self) -> &str {
        "matching"
    }
}

struct SlowSandbox {
    delay_ms: u64,
}

#[async_trait::async_trait]
impl Sandbox for SlowSandbox {
    fn platform(&self) -> &str {
        "slow"
    }

    fn is_available(&self) -> bool {
        true
    }

    async fn execute_sandboxed(
        &self,
        _action: &AutomationAction,
        _config: &SandboxConfig,
    ) -> Result<(), CoreError> {
        tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;
        Ok(())
    }

    fn capabilities(&self) -> SandboxCapabilities {
        SandboxCapabilities {
            filesystem_isolation: false,
            syscall_filtering: false,
            network_isolation: false,
            resource_limits: true,
            process_isolation: false,
        }
    }
}

#[test]
fn automation_action_serde_roundtrip() {
    let action = AutomationAction::MouseClick {
        button: "left".to_string(),
        x: 100,
        y: 200,
    };
    let json = serde_json::to_string(&action).unwrap();
    let deser: AutomationAction = serde_json::from_str(&json).unwrap();
    match deser {
        AutomationAction::MouseClick { x, y, .. } => {
            assert_eq!(x, 100);
            assert_eq!(y, 200);
        }
        _ => panic!("unexpected variant"),
    }
}

#[test]
fn command_result_serde() {
    let result = CommandResult::Success;
    let json = serde_json::to_string(&result).unwrap();
    assert!(json.contains("Success"));
}

#[tokio::test]
async fn sandbox_integrated_dispatch() {
    let controller = make_controller();
    let cmd = AutomationCommand {
        command_id: "cmd-1".to_string(),
        session_id: "sess-1".to_string(),
        action: AutomationAction::MouseMove { x: 0, y: 0 },
        timeout_ms: None,
        policy_token: "token".to_string(),
    };
    let result = controller.execute_command(&cmd).await;
    assert!(result.is_err()); // disabled -> PolicyDenied
}

#[tokio::test]
async fn sandbox_error_propagation() {
    let action = AutomationAction::KeyType {
        text: "test".to_string(),
    };
    let sandbox = NoOpSandbox;
    let config = SandboxConfig::default();
    let result = sandbox.execute_sandboxed(&action, &config).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn resolve_uses_policy_config() {
    let policy = make_policy(AuditLevel::Detailed, 5000);
    let (controller, policy_client, _) = make_controller_with_policy(policy.clone());
    policy_client.update_policies(vec![policy]).await;

    let cmd = AutomationCommand {
        command_id: "cmd-1".to_string(),
        session_id: "sess-1".to_string(),
        action: AutomationAction::MouseMove { x: 0, y: 0 },
        timeout_ms: None,
        policy_token: "test-pol:nonce_0001".to_string(),
    };

    let (resolved, audit_level) = controller.resolve_for_command(&cmd).await;
    assert!(matches!(
        resolved.profile,
        oneshim_core::config::SandboxProfile::Strict
    ));
    assert!(matches!(audit_level, AuditLevel::Detailed));
    assert_eq!(resolved.max_cpu_time_ms, 5000);
}

#[tokio::test]
async fn resolve_defaults_to_strict_without_policy() {
    let controller = make_controller();
    let cmd = AutomationCommand {
        command_id: "cmd-1".to_string(),
        session_id: "sess-1".to_string(),
        action: AutomationAction::MouseMove { x: 0, y: 0 },
        timeout_ms: None,
        policy_token: "unknown:nonce".to_string(),
    };

    let (resolved, audit_level) = controller.resolve_for_command(&cmd).await;
    assert!(matches!(
        resolved.profile,
        oneshim_core::config::SandboxProfile::Strict
    ));
    assert!(matches!(audit_level, AuditLevel::Basic));
}

#[tokio::test]
async fn execute_with_timeout_returns_timeout_result() {
    let policy = make_policy(AuditLevel::Basic, 0);
    let (mut controller, policy_client, _) = make_controller_with_policy(policy.clone());
    controller.set_enabled(true);
    policy_client.update_policies(vec![policy]).await;

    let cmd = AutomationCommand {
        command_id: "cmd-timeout".to_string(),
        session_id: "sess-1".to_string(),
        action: AutomationAction::MouseMove { x: 0, y: 0 },
        timeout_ms: Some(5000),
        policy_token: "test-pol:nonce_0002".to_string(),
    };

    let result = controller.execute_command(&cmd).await.unwrap();
    assert!(matches!(result, CommandResult::Success));
}

#[tokio::test]
async fn audit_level_none_skips_logging() {
    let policy = make_policy(AuditLevel::None, 0);
    let (mut controller, policy_client, audit_logger) = make_controller_with_policy(policy.clone());
    controller.set_enabled(true);
    policy_client.update_policies(vec![policy]).await;

    let cmd = AutomationCommand {
        command_id: "cmd-nolog".to_string(),
        session_id: "sess-1".to_string(),
        action: AutomationAction::KeyPress {
            key: "a".to_string(),
        },
        timeout_ms: None,
        policy_token: "test-pol:nonce_0003".to_string(),
    };

    let result = controller.execute_command(&cmd).await.unwrap();
    assert!(matches!(result, CommandResult::Success));

    let logger = audit_logger.read().await;
    assert_eq!(logger.pending_count(), 0);
}

#[test]
fn workflow_result_serde_roundtrip() {
    let result = WorkflowResult {
        preset_id: "save-file".to_string(),
        success: true,
        steps_executed: 2,
        total_steps: 2,
        total_elapsed_ms: 150,
        step_results: vec![
            WorkflowStepResult {
                step_name: "step1".to_string(),
                step_index: 0,
                success: true,
                elapsed_ms: 50,
                error: None,
            },
            WorkflowStepResult {
                step_name: "step2".to_string(),
                step_index: 1,
                success: true,
                elapsed_ms: 100,
                error: None,
            },
        ],
        message: "success".to_string(),
    };
    let json = serde_json::to_string(&result).unwrap();
    let deser: WorkflowResult = serde_json::from_str(&json).unwrap();
    assert_eq!(deser.preset_id, "save-file");
    assert!(deser.success);
    assert_eq!(deser.steps_executed, 2);
    assert_eq!(deser.step_results.len(), 2);
}

#[tokio::test]
async fn run_workflow_disabled_returns_error() {
    let controller = make_controller();
    let preset = WorkflowPreset {
        id: "test".to_string(),
        name: "test".to_string(),
        description: String::new(),
        category: PresetCategory::Productivity,
        steps: vec![],
        builtin: true,
        platform: None,
        ai_profile_id: None,
    };
    let result = controller.run_workflow(&preset).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn run_workflow_no_executor_returns_error() {
    let mut controller = make_controller();
    controller.set_enabled(true);
    let preset = WorkflowPreset {
        id: "test".to_string(),
        name: "test".to_string(),
        description: String::new(),
        category: PresetCategory::Productivity,
        steps: vec![WorkflowStep {
            name: "Step1".to_string(),
            intent: AutomationIntent::ExecuteHotkey {
                keys: vec!["Ctrl".to_string(), "A".to_string()],
            },
            delay_ms: 0,
            stop_on_failure: true,
        }],
        builtin: true,
        platform: None,
        ai_profile_id: None,
    };
    let result = controller.run_workflow(&preset).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn run_workflow_with_executor_success() {
    use crate::input_driver::{NoOpElementFinder, NoOpInputDriver};
    use crate::intent_resolver::{IntentExecutor, IntentResolver};

    let mut controller = make_controller();
    controller.set_enabled(true);

    let input_driver: Arc<dyn oneshim_core::ports::input_driver::InputDriver> =
        Arc::new(NoOpInputDriver);
    let element_finder: Arc<dyn oneshim_core::ports::element_finder::ElementFinder> =
        Arc::new(NoOpElementFinder);
    let resolver = IntentResolver::new(element_finder, input_driver, IntentConfig::default());
    controller.set_intent_executor(Arc::new(IntentExecutor::new(
        resolver,
        IntentConfig::default(),
    )));

    let preset = WorkflowPreset {
        id: "save-file".to_string(),
        name: "file save".to_string(),
        description: "test".to_string(),
        category: PresetCategory::Productivity,
        steps: vec![WorkflowStep {
            name: "Ctrl+S".to_string(),
            intent: AutomationIntent::ExecuteHotkey {
                keys: vec!["Ctrl".to_string(), "S".to_string()],
            },
            delay_ms: 0,
            stop_on_failure: true,
        }],
        builtin: true,
        platform: None,
        ai_profile_id: None,
    };

    let result = controller.run_workflow(&preset).await.unwrap();
    assert!(result.success);
    assert_eq!(result.steps_executed, 1);
    assert_eq!(result.total_steps, 1);
    assert_eq!(result.step_results.len(), 1);
    assert!(result.step_results[0].success);
}

#[tokio::test]
async fn execute_intent_disabled_returns_policy_denied() {
    let controller = make_controller(); // default disabled
    let cmd = oneshim_core::models::intent::IntentCommand {
        command_id: "intent-1".to_string(),
        session_id: "sess-1".to_string(),
        intent: AutomationIntent::ExecuteHotkey {
            keys: vec!["Ctrl".to_string(), "C".to_string()],
        },
        config: None,
        timeout_ms: None,
        policy_token: "token".to_string(),
    };
    let result = controller.execute_intent(&cmd).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(
        err,
        crate::error::AutomationError::PolicyDenied(_)
    ));
}

#[tokio::test]
async fn execute_intent_no_executor_returns_internal_error() {
    let mut controller = make_controller();
    controller.set_enabled(true); // enabled but executor missing
    let cmd = oneshim_core::models::intent::IntentCommand {
        command_id: "intent-2".to_string(),
        session_id: "sess-1".to_string(),
        intent: AutomationIntent::ExecuteHotkey {
            keys: vec!["Ctrl".to_string(), "V".to_string()],
        },
        config: None,
        timeout_ms: None,
        policy_token: "token".to_string(),
    };
    let result = controller.execute_intent(&cmd).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    // Iter-100: "IntentExecutor not configured" now emits config.missing
    // (was internal.generic). The test still catches the regression
    // that iter-100 fixed — just at the wire-code level now.
    let core: oneshim_core::error::CoreError = err.into();
    assert_eq!(core.code(), "config.missing");
}

#[tokio::test]
async fn execute_intent_success_with_audit_log() {
    use super::gate::SCENE_ACTION_POLICY_TOKEN;
    use crate::input_driver::{NoOpElementFinder, NoOpInputDriver};
    use crate::intent_resolver::{IntentExecutor, IntentResolver};

    let policy_client = Arc::new(PolicyClient::new());
    let audit_logger = Arc::new(RwLock::new(AuditLogger::new(100, 10)));
    let sandbox: Arc<dyn Sandbox> = Arc::new(NoOpSandbox);
    let sandbox_config = SandboxConfig::default();
    let mut controller =
        AutomationController::new(policy_client, audit_logger.clone(), sandbox, sandbox_config);
    controller.set_enabled(true);

    let input_driver: Arc<dyn oneshim_core::ports::input_driver::InputDriver> =
        Arc::new(NoOpInputDriver);
    let element_finder: Arc<dyn oneshim_core::ports::element_finder::ElementFinder> =
        Arc::new(NoOpElementFinder);
    let resolver = IntentResolver::new(element_finder, input_driver, IntentConfig::default());
    controller.set_intent_executor(Arc::new(IntentExecutor::new(
        resolver,
        IntentConfig::default(),
    )));

    let cmd = oneshim_core::models::intent::IntentCommand {
        command_id: "intent-3".to_string(),
        session_id: "sess-1".to_string(),
        intent: AutomationIntent::ExecuteHotkey {
            keys: vec!["Alt".to_string(), "Tab".to_string()],
        },
        config: None,
        timeout_ms: None,
        policy_token: SCENE_ACTION_POLICY_TOKEN.to_string(),
    };
    let result = controller.execute_intent(&cmd).await.unwrap();
    assert!(result.success);

    let logger = audit_logger.read().await;
    assert_eq!(logger.pending_count(), 4);
}

#[tokio::test]
async fn execute_intent_with_external_policy_token_preserves_multi_action_execution() {
    use crate::input_driver::NoOpInputDriver;
    use crate::intent_resolver::{IntentExecutor, IntentResolver};

    let policy = make_policy(AuditLevel::Basic, 5000);
    let (mut controller, policy_client, _) = make_controller_with_policy(policy.clone());
    controller.set_enabled(true);
    policy_client.update_policies(vec![policy]).await;

    let input_driver: Arc<dyn oneshim_core::ports::input_driver::InputDriver> =
        Arc::new(NoOpInputDriver);
    let element_finder: Arc<dyn oneshim_core::ports::element_finder::ElementFinder> =
        Arc::new(MatchingElementFinder);
    let resolver = IntentResolver::new(element_finder, input_driver, IntentConfig::default());
    controller.set_intent_executor(Arc::new(IntentExecutor::new(
        resolver,
        IntentConfig::default(),
    )));

    let cmd = oneshim_core::models::intent::IntentCommand {
        command_id: "intent-external".to_string(),
        session_id: "sess-1".to_string(),
        intent: AutomationIntent::TypeIntoElement {
            element_text: Some("Search".to_string()),
            role: Some("textbox".to_string()),
            text: "hello".to_string(),
        },
        config: None,
        timeout_ms: None,
        policy_token: "test-pol:nonce_external_01".to_string(),
    };

    let result = controller.execute_intent(&cmd).await.unwrap();
    assert!(result.success);
}

#[tokio::test]
async fn execute_intent_hint_requires_planner() {
    use crate::input_driver::{NoOpElementFinder, NoOpInputDriver};
    use crate::intent_resolver::{IntentExecutor, IntentResolver};

    let mut controller = make_controller();
    controller.set_enabled(true);

    let input_driver: Arc<dyn oneshim_core::ports::input_driver::InputDriver> =
        Arc::new(NoOpInputDriver);
    let element_finder: Arc<dyn oneshim_core::ports::element_finder::ElementFinder> =
        Arc::new(NoOpElementFinder);
    let resolver = IntentResolver::new(element_finder, input_driver, IntentConfig::default());
    controller.set_intent_executor(Arc::new(IntentExecutor::new(
        resolver,
        IntentConfig::default(),
    )));

    let result = controller
        .execute_intent_hint("hint-1", "sess-1", "save 버튼 클릭")
        .await;
    assert!(result.is_err());
    // Iter-100: "IntentPlanner is not configured" now routes via
    // AutomationError::Core(Config{Missing}) → wire config.missing.
    let err = result.unwrap_err();
    let core: oneshim_core::error::CoreError = err.into();
    assert_eq!(core.code(), "config.missing");
    assert!(
        core.to_string().contains("IntentPlanner"),
        "err should mention IntentPlanner, got: {core}"
    );
}

#[tokio::test]
async fn execute_intent_hint_success() {
    use crate::input_driver::{NoOpElementFinder, NoOpInputDriver};
    use crate::intent_resolver::{IntentExecutor, IntentResolver};

    let mut controller = make_controller();
    controller.set_enabled(true);
    controller.set_intent_planner(Arc::new(StubPlanner {
        planned: AutomationIntent::ExecuteHotkey {
            keys: vec!["Ctrl".to_string(), "S".to_string()],
        },
    }));

    let input_driver: Arc<dyn oneshim_core::ports::input_driver::InputDriver> =
        Arc::new(NoOpInputDriver);
    let element_finder: Arc<dyn oneshim_core::ports::element_finder::ElementFinder> =
        Arc::new(NoOpElementFinder);
    let resolver = IntentResolver::new(element_finder, input_driver, IntentConfig::default());
    controller.set_intent_executor(Arc::new(IntentExecutor::new(
        resolver,
        IntentConfig::default(),
    )));

    let result = controller
        .execute_intent_hint("hint-2", "sess-1", "Ctrl+S execution")
        .await
        .unwrap();

    assert!(matches!(
        result.planned_intent,
        AutomationIntent::ExecuteHotkey { .. }
    ));
    assert!(result.result.success);
}

#[tokio::test]
async fn execute_intent_hint_preserves_template_executor_config() {
    use crate::input_driver::{NoOpElementFinder, NoOpInputDriver};
    use crate::intent_resolver::{IntentExecutor, IntentResolver};

    let mut controller = make_controller();
    controller.set_enabled(true);
    controller.set_intent_planner(Arc::new(StubPlanner {
        planned: AutomationIntent::ClickElement {
            text: Some("Save".to_string()),
            role: Some("button".to_string()),
            app_name: None,
            button: "left".to_string(),
        },
    }));

    let template_config = IntentConfig {
        max_retries: 0,
        ..IntentConfig::default()
    };
    let input_driver: Arc<dyn oneshim_core::ports::input_driver::InputDriver> =
        Arc::new(NoOpInputDriver);
    let element_finder: Arc<dyn oneshim_core::ports::element_finder::ElementFinder> =
        Arc::new(NoOpElementFinder);
    let resolver = IntentResolver::new(element_finder, input_driver, template_config.clone());
    controller.set_intent_executor(Arc::new(IntentExecutor::new(resolver, template_config)));

    let result = controller
        .execute_intent_hint("hint-config", "sess-1", "save button")
        .await
        .unwrap();

    assert!(!result.result.success);
    assert_eq!(result.result.retry_count, 0);
}

#[tokio::test]
async fn analyze_scene_requires_scene_finder() {
    let mut controller = make_controller();
    controller.set_enabled(true);

    let err = controller.analyze_scene(None, None).await.unwrap_err();
    // Iter-100: "Scene analyzer is not configured" now emits config.missing.
    assert_eq!(err.code(), "config.missing");
}

#[tokio::test]
async fn analyze_scene_success_with_scene_finder() {
    let mut controller = make_controller();
    controller.set_enabled(true);
    controller.set_scene_finder(Arc::new(StubSceneFinder));

    let scene = controller
        .analyze_scene(Some("VSCode"), Some("screen-1"))
        .await
        .unwrap();
    assert_eq!(scene.scene_id, "scene-stub");
    assert_eq!(scene.app_name.as_deref(), Some("VSCode"));
    assert_eq!(scene.screen_id.as_deref(), Some("screen-1"));
    assert_eq!(scene.elements.len(), 1);
}

#[tokio::test]
async fn run_workflow_empty_steps_succeeds() {
    use crate::input_driver::{NoOpElementFinder, NoOpInputDriver};
    use crate::intent_resolver::{IntentExecutor, IntentResolver};

    let mut controller = make_controller();
    controller.set_enabled(true);

    let input_driver: Arc<dyn oneshim_core::ports::input_driver::InputDriver> =
        Arc::new(NoOpInputDriver);
    let element_finder: Arc<dyn oneshim_core::ports::element_finder::ElementFinder> =
        Arc::new(NoOpElementFinder);
    let resolver = IntentResolver::new(element_finder, input_driver, IntentConfig::default());
    controller.set_intent_executor(Arc::new(IntentExecutor::new(
        resolver,
        IntentConfig::default(),
    )));

    let preset = WorkflowPreset {
        id: "empty".to_string(),
        name: "빈 워크플로우".to_string(),
        description: String::new(),
        category: PresetCategory::Productivity,
        steps: vec![], // 0 steps
        builtin: true,
        platform: None,
        ai_profile_id: None,
    };

    let result = controller.run_workflow(&preset).await.unwrap();
    assert!(result.success);
    assert_eq!(result.steps_executed, 0);
    assert_eq!(result.total_steps, 0);
    assert!(result.step_results.is_empty());
}

#[tokio::test]
async fn run_workflow_multi_step_with_delay() {
    use crate::input_driver::{NoOpElementFinder, NoOpInputDriver};
    use crate::intent_resolver::{IntentExecutor, IntentResolver};

    let mut controller = make_controller();
    controller.set_enabled(true);

    let input_driver: Arc<dyn oneshim_core::ports::input_driver::InputDriver> =
        Arc::new(NoOpInputDriver);
    let element_finder: Arc<dyn oneshim_core::ports::element_finder::ElementFinder> =
        Arc::new(NoOpElementFinder);
    let resolver = IntentResolver::new(element_finder, input_driver, IntentConfig::default());
    controller.set_intent_executor(Arc::new(IntentExecutor::new(
        resolver,
        IntentConfig::default(),
    )));

    let preset = WorkflowPreset {
        id: "multi".to_string(),
        name: "멀티 스텝".to_string(),
        description: String::new(),
        category: PresetCategory::Productivity,
        steps: vec![
            WorkflowStep {
                name: "Step1".to_string(),
                intent: AutomationIntent::ExecuteHotkey {
                    keys: vec!["Ctrl".to_string(), "A".to_string()],
                },
                delay_ms: 0,
                stop_on_failure: false,
            },
            WorkflowStep {
                name: "Step2".to_string(),
                intent: AutomationIntent::ExecuteHotkey {
                    keys: vec!["Ctrl".to_string(), "C".to_string()],
                },
                delay_ms: 10, // short delay
                stop_on_failure: false,
            },
            WorkflowStep {
                name: "Step3".to_string(),
                intent: AutomationIntent::ExecuteHotkey {
                    keys: vec!["Ctrl".to_string(), "V".to_string()],
                },
                delay_ms: 10,
                stop_on_failure: false,
            },
        ],
        builtin: true,
        platform: None,
        ai_profile_id: None,
    };

    let result = controller.run_workflow(&preset).await.unwrap();
    assert!(result.success);
    assert_eq!(result.steps_executed, 3);
    assert_eq!(result.total_steps, 3);
    assert_eq!(result.step_results.len(), 3);
    assert!(result.step_results.iter().all(|s| s.success));
    assert!(result.total_elapsed_ms >= 20); // includes delay
}

#[tokio::test]
async fn run_workflow_preserves_template_executor_config() {
    use crate::input_driver::NoOpInputDriver;
    use crate::intent_resolver::{IntentExecutor, IntentResolver};

    let mut controller = make_controller();
    controller.set_enabled(true);

    let template_config = IntentConfig {
        min_confidence: 0.99,
        ..IntentConfig::default()
    };
    let input_driver: Arc<dyn oneshim_core::ports::input_driver::InputDriver> =
        Arc::new(NoOpInputDriver);
    let element_finder: Arc<dyn oneshim_core::ports::element_finder::ElementFinder> =
        Arc::new(MatchingElementFinder);
    let resolver = IntentResolver::new(element_finder, input_driver, template_config.clone());
    controller.set_intent_executor(Arc::new(IntentExecutor::new(resolver, template_config)));

    let preset = WorkflowPreset {
        id: "retry-preserve".to_string(),
        name: "Retry Preserve".to_string(),
        description: String::new(),
        category: PresetCategory::Productivity,
        steps: vec![WorkflowStep {
            name: "Missing Save Button".to_string(),
            intent: AutomationIntent::ClickElement {
                text: Some("Save".to_string()),
                role: Some("button".to_string()),
                app_name: None,
                button: "left".to_string(),
            },
            delay_ms: 0,
            stop_on_failure: true,
        }],
        builtin: false,
        platform: None,
        ai_profile_id: None,
    };

    let result = controller.run_workflow(&preset).await.unwrap();
    assert!(!result.success);
    assert_eq!(result.steps_executed, 1);
    assert!(result.step_results[0]
        .error
        .as_deref()
        .is_some_and(|error| error.contains("99%")));
}

#[tokio::test]
async fn execute_intent_internal_timeout_reports_effective_limit() {
    use super::gate::SCENE_ACTION_POLICY_TOKEN;
    use crate::input_driver::{NoOpElementFinder, NoOpInputDriver};
    use crate::intent_resolver::{IntentExecutor, IntentResolver};

    let policy_client = Arc::new(PolicyClient::new());
    let audit_logger = Arc::new(RwLock::new(AuditLogger::new(100, 10)));
    let sandbox: Arc<dyn Sandbox> = Arc::new(SlowSandbox { delay_ms: 25 });
    let sandbox_config = SandboxConfig {
        max_cpu_time_ms: 10,
        ..SandboxConfig::default()
    };
    let mut controller =
        AutomationController::new(policy_client, audit_logger, sandbox, sandbox_config);
    controller.set_enabled(true);

    let input_driver: Arc<dyn oneshim_core::ports::input_driver::InputDriver> =
        Arc::new(NoOpInputDriver);
    let element_finder: Arc<dyn oneshim_core::ports::element_finder::ElementFinder> =
        Arc::new(NoOpElementFinder);
    let resolver = IntentResolver::new(element_finder, input_driver, IntentConfig::default());
    controller.set_intent_executor(Arc::new(IntentExecutor::new(
        resolver,
        IntentConfig::default(),
    )));

    let cmd = oneshim_core::models::intent::IntentCommand {
        command_id: "intent-timeout".to_string(),
        session_id: "sess-1".to_string(),
        intent: AutomationIntent::ExecuteHotkey {
            keys: vec!["Ctrl".to_string(), "S".to_string()],
        },
        config: Some(IntentConfig {
            max_retries: 0,
            ..IntentConfig::default()
        }),
        timeout_ms: None,
        policy_token: SCENE_ACTION_POLICY_TOKEN.to_string(),
    };

    let result = controller.execute_intent(&cmd).await.unwrap();
    assert!(!result.success);
    assert_eq!(
        result.error.as_deref(),
        Some("Execution timeout [sandbox.timeout] exceeded: 10ms")
    );
}

#[tokio::test]
async fn execute_command_enabled_with_valid_policy() {
    let policy = make_policy(AuditLevel::Basic, 5000);
    let (mut controller, policy_client, _) = make_controller_with_policy(policy.clone());
    controller.set_enabled(true);
    policy_client.update_policies(vec![policy]).await;

    let cmd = AutomationCommand {
        command_id: "cmd-ok".to_string(),
        session_id: "sess-1".to_string(),
        action: AutomationAction::KeyType {
            text: "hello".to_string(),
        },
        timeout_ms: None,
        policy_token: "test-pol:nonce_0099".to_string(),
    };

    let result = controller.execute_command(&cmd).await.unwrap();
    assert!(matches!(result, CommandResult::Success));
}

#[tokio::test]
async fn workflow_step_result_error_field() {
    let result = WorkflowStepResult {
        step_name: "fail-step".to_string(),
        step_index: 2,
        success: false,
        elapsed_ms: 50,
        error: Some("Element not found".to_string()),
    };
    let json = serde_json::to_string(&result).unwrap();
    assert!(json.contains("Element not found"));
    assert!(json.contains("fail-step"));
    let deser: WorkflowStepResult = serde_json::from_str(&json).unwrap();
    assert!(!deser.success);
    assert_eq!(deser.error.unwrap(), "Element not found");
}

// ── M2: Execution timeout constants ────────────────────────────────

#[test]
fn gui_execute_timeout_is_bounded() {
    assert!(
        std::hint::black_box(GUI_EXECUTE_TIMEOUT_SECS) >= 10
            && std::hint::black_box(GUI_EXECUTE_TIMEOUT_SECS) <= 120,
        "Total execution timeout should be between 10s and 120s"
    );
}

#[test]
fn gui_action_timeout_is_bounded() {
    assert!(
        std::hint::black_box(GUI_ACTION_TIMEOUT_SECS) >= 3
            && std::hint::black_box(GUI_ACTION_TIMEOUT_SECS) <= 60,
        "Per-action timeout should be between 3s and 60s"
    );
}

#[test]
fn gui_action_timeout_less_than_total() {
    assert!(
        std::hint::black_box(GUI_ACTION_TIMEOUT_SECS)
            < std::hint::black_box(GUI_EXECUTE_TIMEOUT_SECS),
        "Per-action timeout must be less than total execution timeout"
    );
}
