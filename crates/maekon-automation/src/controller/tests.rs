use super::*;
use crate::intent_planner::IntentPlanner;
use crate::policy::{AuditLevel, ExecutionPolicy};
use crate::sandbox::NoOpSandbox;
use maekon_core::error::CoreError;
use maekon_core::models::intent::{
    AutomationIntent, FinderSource, IntentConfig, PresetCategory, UiElement, WorkflowPreset,
    WorkflowStep,
};
use maekon_core::models::ui_scene::{
    NormalizedBounds, UiScene, UiSceneElement, UI_SCENE_SCHEMA_VERSION,
};
use maekon_core::ports::sandbox::SandboxCapabilities;

fn make_controller() -> AutomationController {
    let policy_client = Arc::new(PolicyClient::new());
    let audit_logger = Arc::new(RwLock::new(AuditLogger::default()));
    let sandbox: Arc<dyn Sandbox> = Arc::new(NoOpSandbox);
    let sandbox_config = SandboxConfig::default();
    let mut controller =
        AutomationController::new(policy_client, audit_logger, sandbox, sandbox_config);
    wire_noop_inline_action_executor(&mut controller);
    controller
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

fn wire_noop_inline_action_executor(controller: &mut AutomationController) {
    let input_driver: Arc<dyn maekon_core::ports::input_driver::InputDriver> =
        Arc::new(crate::input_driver::NoOpInputDriver);
    controller.set_inline_action_executor(input_driver);
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
        confirmation: maekon_core::config::ConfirmationRequirement::Auto,
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
        _region: Option<&maekon_core::models::intent::ElementBounds>,
    ) -> Result<Vec<maekon_core::models::intent::UiElement>, CoreError> {
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
                bbox_abs: maekon_core::models::intent::ElementBounds {
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
        _region: Option<&maekon_core::models::intent::ElementBounds>,
    ) -> Result<Vec<UiElement>, CoreError> {
        Ok(vec![UiElement {
            text: text.unwrap_or("matched").to_string(),
            bounds: maekon_core::models::intent::ElementBounds {
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

struct HangingSandbox;

#[async_trait::async_trait]
impl Sandbox for HangingSandbox {
    fn platform(&self) -> &str {
        "hanging"
    }

    fn is_available(&self) -> bool {
        true
    }

    async fn execute_sandboxed(
        &self,
        _action: &AutomationAction,
        _config: &SandboxConfig,
    ) -> Result<(), CoreError> {
        std::future::pending::<Result<(), CoreError>>().await
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
        origin: maekon_core::models::automation::CommandOrigin::External,
    };
    let err = controller.execute_command(&cmd).await.unwrap_err();
    assert!(
        matches!(err, AutomationError::PolicyDenied(_)),
        "disabled controller must produce PolicyDenied, got: {err:?}"
    );
}

#[tokio::test]
async fn sandbox_error_propagation() {
    let action = AutomationAction::KeyType {
        text: "test".to_string(),
    };
    let sandbox = NoOpSandbox;
    let config = SandboxConfig::default();
    // NoOpSandbox.execute_sandboxed is a pure no-op: it always returns Ok(()).
    // The contract being checked is that a NoOp sandbox never propagates errors
    // regardless of the action or config supplied — Ok(()) is the full contract.
    // (#5594: Ok-only is justified; no return value to assert beyond the unit type.)
    sandbox
        .execute_sandboxed(&action, &config)
        .await
        .expect("NoOpSandbox::execute_sandboxed must never return an error");
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
        origin: maekon_core::models::automation::CommandOrigin::External,
    };

    let (resolved, audit_level) = controller.resolve_for_command(&cmd).await;
    assert!(matches!(
        resolved.profile,
        maekon_core::config::SandboxProfile::Strict
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
        origin: maekon_core::models::automation::CommandOrigin::External,
    };

    let (resolved, audit_level) = controller.resolve_for_command(&cmd).await;
    assert!(matches!(
        resolved.profile,
        maekon_core::config::SandboxProfile::Strict
    ));
    assert!(matches!(audit_level, AuditLevel::Basic));
}

#[tokio::test]
async fn execute_with_timeout_returns_timeout_result() {
    let policy = make_policy(AuditLevel::Basic, 0);
    let (mut controller, policy_client, _) = make_controller_with_policy(policy.clone());
    controller.set_enabled(true);
    wire_noop_inline_action_executor(&mut controller);
    policy_client.update_policies(vec![policy]).await;

    let cmd = AutomationCommand {
        command_id: "cmd-timeout".to_string(),
        session_id: "sess-1".to_string(),
        action: AutomationAction::MouseMove { x: 0, y: 0 },
        timeout_ms: Some(5000),
        policy_token: "test-pol:nonce_0002".to_string(),
        // #6333 A16: execution-behavior test → trusted in-process command (Internal).
        origin: maekon_core::models::automation::CommandOrigin::Internal,
    };

    let result = controller.execute_command(&cmd).await.unwrap();
    assert!(matches!(result, CommandResult::Success));
}

#[tokio::test]
async fn audit_level_none_skips_logging() {
    let policy = make_policy(AuditLevel::None, 0);
    let (mut controller, policy_client, audit_logger) = make_controller_with_policy(policy.clone());
    controller.set_enabled(true);
    wire_noop_inline_action_executor(&mut controller);
    policy_client.update_policies(vec![policy]).await;

    let cmd = AutomationCommand {
        command_id: "cmd-nolog".to_string(),
        session_id: "sess-1".to_string(),
        action: AutomationAction::KeyPress {
            key: "a".to_string(),
        },
        timeout_ms: None,
        policy_token: "test-pol:nonce_0003".to_string(),
        // #6333 A16: execution-behavior test → trusted in-process command (Internal).
        origin: maekon_core::models::automation::CommandOrigin::Internal,
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
    let err = controller.run_workflow(&preset).await.unwrap_err();
    assert!(
        matches!(err, AutomationError::PolicyDenied(_)),
        "disabled controller run_workflow must produce PolicyDenied, got: {err:?}"
    );
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
    let err = controller.run_workflow(&preset).await.unwrap_err();
    // IntentExecutor not configured → Core(Config { code: Missing })
    let core: maekon_core::error::CoreError = err.into();
    assert_eq!(
        core.code(),
        "config.missing",
        "no-executor run_workflow must produce config.missing wire code, got: {core:?}"
    );
}

#[tokio::test]
async fn run_workflow_with_executor_success() {
    use crate::input_driver::{NoOpElementFinder, NoOpInputDriver};
    use crate::intent_resolver::{IntentExecutor, IntentResolver};

    let mut controller = make_controller();
    controller.set_enabled(true);

    let input_driver: Arc<dyn maekon_core::ports::input_driver::InputDriver> =
        Arc::new(NoOpInputDriver);
    let element_finder: Arc<dyn maekon_core::ports::element_finder::ElementFinder> =
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
    let cmd = maekon_core::models::intent::IntentCommand {
        command_id: "intent-1".to_string(),
        session_id: "sess-1".to_string(),
        intent: AutomationIntent::ExecuteHotkey {
            keys: vec!["Ctrl".to_string(), "C".to_string()],
        },
        config: None,
        timeout_ms: None,
        policy_token: "token".to_string(),
        origin: maekon_core::models::automation::CommandOrigin::External,
    };
    let err = controller.execute_intent(&cmd).await.unwrap_err();
    assert!(
        matches!(err, crate::error::AutomationError::PolicyDenied(_)),
        "disabled controller execute_intent must produce PolicyDenied, got: {err:?}"
    );
}

#[tokio::test]
async fn execute_intent_no_executor_returns_internal_error() {
    let mut controller = make_controller();
    controller.set_enabled(true); // enabled but executor missing
    let cmd = maekon_core::models::intent::IntentCommand {
        command_id: "intent-2".to_string(),
        session_id: "sess-1".to_string(),
        intent: AutomationIntent::ExecuteHotkey {
            keys: vec!["Ctrl".to_string(), "V".to_string()],
        },
        config: None,
        timeout_ms: None,
        policy_token: "token".to_string(),
        origin: maekon_core::models::automation::CommandOrigin::External,
    };
    let err = controller.execute_intent(&cmd).await.unwrap_err();
    // Iter-100: "IntentExecutor not configured" now emits config.missing
    // (was internal.generic). The test still catches the regression
    // that iter-100 fixed — just at the wire-code level now.
    let core: maekon_core::error::CoreError = err.into();
    assert_eq!(
        core.code(),
        "config.missing",
        "no-executor execute_intent must produce config.missing wire code, got: {core:?}"
    );
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
    wire_noop_inline_action_executor(&mut controller);

    let input_driver: Arc<dyn maekon_core::ports::input_driver::InputDriver> =
        Arc::new(NoOpInputDriver);
    let element_finder: Arc<dyn maekon_core::ports::element_finder::ElementFinder> =
        Arc::new(NoOpElementFinder);
    let resolver = IntentResolver::new(element_finder, input_driver, IntentConfig::default());
    controller.set_intent_executor(Arc::new(IntentExecutor::new(
        resolver,
        IntentConfig::default(),
    )));

    let cmd = maekon_core::models::intent::IntentCommand {
        command_id: "intent-3".to_string(),
        session_id: "sess-1".to_string(),
        intent: AutomationIntent::ExecuteHotkey {
            keys: vec!["Alt".to_string(), "Tab".to_string()],
        },
        config: None,
        timeout_ms: None,
        policy_token: SCENE_ACTION_POLICY_TOKEN.to_string(),
        origin: maekon_core::models::automation::CommandOrigin::Internal,
    };
    let result = controller.execute_intent(&cmd).await.unwrap();
    assert!(result.success);

    let logger = audit_logger.read().await;
    assert_eq!(logger.pending_count(), 4);
}

#[tokio::test]
async fn gui_session_key_type_audit_masks_raw_text_payload() {
    use super::gate::GUI_SESSION_POLICY_TOKEN;
    use crate::input_driver::{NoOpElementFinder, NoOpInputDriver};
    use crate::intent_resolver::{IntentExecutor, IntentResolver};

    let policy_client = Arc::new(PolicyClient::new());
    let audit_logger = Arc::new(RwLock::new(AuditLogger::new(100, 10)));
    let sandbox: Arc<dyn Sandbox> = Arc::new(NoOpSandbox);
    let sandbox_config = SandboxConfig::default();
    let mut controller =
        AutomationController::new(policy_client, audit_logger.clone(), sandbox, sandbox_config);
    controller.set_enabled(true);
    wire_noop_inline_action_executor(&mut controller);

    let input_driver: Arc<dyn maekon_core::ports::input_driver::InputDriver> =
        Arc::new(NoOpInputDriver);
    let element_finder: Arc<dyn maekon_core::ports::element_finder::ElementFinder> =
        Arc::new(NoOpElementFinder);
    let resolver = IntentResolver::new(element_finder, input_driver, IntentConfig::default());
    controller.set_intent_executor(Arc::new(IntentExecutor::new(
        resolver,
        IntentConfig::default(),
    )));

    let raw_text = "Secret payroll code 1234";
    let cmd = maekon_core::models::intent::IntentCommand {
        command_id: "gui-session-raw-type".to_string(),
        session_id: "sess-gui-1".to_string(),
        intent: AutomationIntent::Raw(AutomationAction::KeyType {
            text: raw_text.to_string(),
        }),
        config: None,
        timeout_ms: None,
        policy_token: GUI_SESSION_POLICY_TOKEN.to_string(),
        origin: maekon_core::models::automation::CommandOrigin::Internal,
    };

    let result = controller.execute_intent(&cmd).await.unwrap();
    assert!(result.success);

    let logger = audit_logger.read().await;
    let audit_blob = logger
        .recent_entries(20)
        .into_iter()
        .map(|entry| {
            format!(
                "{} {} {:?}",
                entry.command_id, entry.action_type, entry.details
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        !audit_blob.contains(raw_text),
        "GUI session audit must not store raw typed text: {audit_blob}"
    );
    assert!(
        audit_blob.contains("text_len=24"),
        "GUI session audit should keep payload-free length evidence: {audit_blob}"
    );
}

#[tokio::test]
async fn execute_intent_with_external_policy_token_preserves_multi_action_execution() {
    use crate::input_driver::NoOpInputDriver;
    use crate::intent_resolver::{IntentExecutor, IntentResolver};

    let policy = make_policy(AuditLevel::Basic, 5000);
    let (mut controller, policy_client, _) = make_controller_with_policy(policy.clone());
    controller.set_enabled(true);
    policy_client.update_policies(vec![policy]).await;

    let input_driver: Arc<dyn maekon_core::ports::input_driver::InputDriver> =
        Arc::new(NoOpInputDriver);
    let element_finder: Arc<dyn maekon_core::ports::element_finder::ElementFinder> =
        Arc::new(MatchingElementFinder);
    let resolver = IntentResolver::new(element_finder, input_driver, IntentConfig::default());
    controller.set_intent_executor(Arc::new(IntentExecutor::new(
        resolver,
        IntentConfig::default(),
    )));

    let cmd = maekon_core::models::intent::IntentCommand {
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
        origin: maekon_core::models::automation::CommandOrigin::External,
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

    let input_driver: Arc<dyn maekon_core::ports::input_driver::InputDriver> =
        Arc::new(NoOpInputDriver);
    let element_finder: Arc<dyn maekon_core::ports::element_finder::ElementFinder> =
        Arc::new(NoOpElementFinder);
    let resolver = IntentResolver::new(element_finder, input_driver, IntentConfig::default());
    controller.set_intent_executor(Arc::new(IntentExecutor::new(
        resolver,
        IntentConfig::default(),
    )));

    // Intent-hint test input ("click the save button"); the planner is absent so
    // the hint is never consumed — Korean is incidental test data (ASCII-escaped).
    let err = controller
        .execute_intent_hint("hint-1", "sess-1", "save \u{bc84}\u{d2bc} \u{d074}\u{b9ad}")
        .await
        .unwrap_err();
    // Iter-100: "IntentPlanner is not configured" now routes via
    // AutomationError::Core(Config{Missing}) → wire config.missing.
    let core: maekon_core::error::CoreError = err.into();
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

    let input_driver: Arc<dyn maekon_core::ports::input_driver::InputDriver> =
        Arc::new(NoOpInputDriver);
    let element_finder: Arc<dyn maekon_core::ports::element_finder::ElementFinder> =
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
    let input_driver: Arc<dyn maekon_core::ports::input_driver::InputDriver> =
        Arc::new(NoOpInputDriver);
    let element_finder: Arc<dyn maekon_core::ports::element_finder::ElementFinder> =
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

    let input_driver: Arc<dyn maekon_core::ports::input_driver::InputDriver> =
        Arc::new(NoOpInputDriver);
    let element_finder: Arc<dyn maekon_core::ports::element_finder::ElementFinder> =
        Arc::new(NoOpElementFinder);
    let resolver = IntentResolver::new(element_finder, input_driver, IntentConfig::default());
    controller.set_intent_executor(Arc::new(IntentExecutor::new(
        resolver,
        IntentConfig::default(),
    )));

    let preset = WorkflowPreset {
        id: "empty".to_string(),
        // Preset name is incidental test data ("empty workflow"), ASCII-escaped to
        // keep the source ASCII while preserving the exact bytes.
        name: "\u{be48} \u{c6cc}\u{d06c}\u{d50c}\u{b85c}\u{c6b0}".to_string(),
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

    let input_driver: Arc<dyn maekon_core::ports::input_driver::InputDriver> =
        Arc::new(NoOpInputDriver);
    let element_finder: Arc<dyn maekon_core::ports::element_finder::ElementFinder> =
        Arc::new(NoOpElementFinder);
    let resolver = IntentResolver::new(element_finder, input_driver, IntentConfig::default());
    controller.set_intent_executor(Arc::new(IntentExecutor::new(
        resolver,
        IntentConfig::default(),
    )));

    let preset = WorkflowPreset {
        id: "multi".to_string(),
        // Preset name is incidental test data ("multi step"), ASCII-escaped to keep
        // the source ASCII while preserving the exact bytes.
        name: "\u{ba40}\u{d2f0} \u{c2a4}\u{d15d}".to_string(),
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
    let input_driver: Arc<dyn maekon_core::ports::input_driver::InputDriver> =
        Arc::new(NoOpInputDriver);
    let element_finder: Arc<dyn maekon_core::ports::element_finder::ElementFinder> =
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
    let sandbox: Arc<dyn Sandbox> = Arc::new(HangingSandbox);
    let sandbox_config = SandboxConfig {
        enabled: true,
        max_cpu_time_ms: 10,
        ..SandboxConfig::default()
    };
    let mut controller =
        AutomationController::new(policy_client, audit_logger, sandbox, sandbox_config);
    controller.set_enabled(true);

    let input_driver: Arc<dyn maekon_core::ports::input_driver::InputDriver> =
        Arc::new(NoOpInputDriver);
    let element_finder: Arc<dyn maekon_core::ports::element_finder::ElementFinder> =
        Arc::new(NoOpElementFinder);
    let resolver = IntentResolver::new(element_finder, input_driver, IntentConfig::default());
    controller.set_intent_executor(Arc::new(IntentExecutor::new(
        resolver,
        IntentConfig::default(),
    )));

    let cmd = maekon_core::models::intent::IntentCommand {
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
        origin: maekon_core::models::automation::CommandOrigin::Internal,
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
    wire_noop_inline_action_executor(&mut controller);
    policy_client.update_policies(vec![policy]).await;

    let cmd = AutomationCommand {
        command_id: "cmd-ok".to_string(),
        session_id: "sess-1".to_string(),
        action: AutomationAction::KeyType {
            text: "hello".to_string(),
        },
        timeout_ms: None,
        policy_token: "test-pol:nonce_0099".to_string(),
        // #6333 A16: execution-behavior test → trusted in-process command (Internal).
        origin: maekon_core::models::automation::CommandOrigin::Internal,
    };

    let result = controller.execute_command(&cmd).await.unwrap();
    assert!(matches!(result, CommandResult::Success));
}

#[tokio::test]
async fn execute_command_default_disabled_sandbox_without_inline_audits_failed() {
    let policy = make_policy(AuditLevel::Basic, 5000);
    let (mut controller, policy_client, audit_logger) = make_controller_with_policy(policy.clone());
    controller.set_enabled(true);
    policy_client.update_policies(vec![policy]).await;

    let cmd = AutomationCommand {
        command_id: "cmd-disabled-no-inline".to_string(),
        session_id: "sess-disabled".to_string(),
        action: AutomationAction::MouseMove { x: 0, y: 0 },
        timeout_ms: None,
        policy_token: "test-pol:nonce_disabled01".to_string(),
        origin: maekon_core::models::automation::CommandOrigin::Internal,
    };

    let result = controller.execute_command(&cmd).await.unwrap();
    assert!(
        matches!(result, CommandResult::Failed(ref message) if message.contains("disabled")),
        "disabled sandbox without inline executor must fail, got {result:?}"
    );

    let logger = audit_logger.read().await;
    let failed = logger.entries_by_status(&crate::audit::AuditStatus::Failed, 10);
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].command_id, "cmd-disabled-no-inline");
    assert!(
        failed[0]
            .details
            .as_deref()
            .is_some_and(|details| details.contains("disabled")),
        "failed audit should explain the disabled/no-inline path: {:?}",
        failed[0].details
    );
    assert!(
        logger
            .entries_by_status(&crate::audit::AuditStatus::Completed, 10)
            .is_empty(),
        "skipped no-op actions must not be audited as Completed"
    );
}

#[tokio::test]
async fn execute_command_block_policy_audits_denial() {
    // Regression (automation-deny-audit): a ConfirmationRequirement::Block policy
    // must record a denial audit entry, on the same footing as the policy-allow
    // and validate_command-denial paths. Previously the Block branch returned
    // CommandResult::Denied without auditing.
    let mut policy = make_policy(AuditLevel::Basic, 5000);
    policy.confirmation = maekon_core::config::ConfirmationRequirement::Block;
    let (mut controller, policy_client, audit_logger) = make_controller_with_policy(policy.clone());
    controller.set_enabled(true);
    policy_client.update_policies(vec![policy]).await;

    let cmd = AutomationCommand {
        command_id: "cmd-blocked".to_string(),
        session_id: "sess-block".to_string(),
        action: AutomationAction::KeyType {
            text: "hello".to_string(),
        },
        timeout_ms: None,
        policy_token: "test-pol:nonce_block01".to_string(),
        origin: maekon_core::models::automation::CommandOrigin::External,
    };

    let result = controller.execute_command(&cmd).await.unwrap();
    assert!(matches!(result, CommandResult::Denied));

    let logger = audit_logger.read().await;
    let denied = logger.entries_by_status(&crate::audit::AuditStatus::Denied, 10);
    assert_eq!(denied.len(), 1, "Block denial must produce one audit entry");
    assert_eq!(denied[0].command_id, "cmd-blocked");
    assert_eq!(denied[0].session_id, "sess-block");
    assert!(
        denied[0].action_type.starts_with("KeyType"),
        "denial audit should carry the action label, got: {}",
        denied[0].action_type
    );
}

#[tokio::test]
async fn execute_command_user_denied_confirmation_audits_denial() {
    // Regression (automation-deny-audit): a user-denied ConfirmationRequirement::Confirm
    // command must record a denial audit entry, mirroring the Block branch.
    let mut policy = make_policy(AuditLevel::Basic, 5000);
    policy.confirmation = maekon_core::config::ConfirmationRequirement::Confirm;
    let (mut controller, policy_client, audit_logger) = make_controller_with_policy(policy.clone());
    controller.set_enabled(true);
    policy_client.update_policies(vec![policy]).await;

    // Resolve the pending confirmation with `false` (user denied) so the test
    // does not block on the confirmation timeout.
    let pending = controller.pending_confirmations.clone();
    let cmd_id = "cmd-confirm-denied".to_string();
    tokio::spawn(async move {
        tokio::task::yield_now().await;
        let mut map = pending.lock().await;
        if let Some((_, tx)) = map.remove(&cmd_id) {
            let _ = tx.send(false);
        }
    });

    let cmd = AutomationCommand {
        command_id: "cmd-confirm-denied".to_string(),
        session_id: "sess-confirm".to_string(),
        action: AutomationAction::KeyPress {
            key: "a".to_string(),
        },
        timeout_ms: None,
        policy_token: "test-pol:nonce_confirm1".to_string(),
        origin: maekon_core::models::automation::CommandOrigin::External,
    };

    let result = controller.execute_command(&cmd).await.unwrap();
    assert!(matches!(result, CommandResult::Denied));

    let logger = audit_logger.read().await;
    let denied = logger.entries_by_status(&crate::audit::AuditStatus::Denied, 10);
    assert_eq!(
        denied.len(),
        1,
        "user-denied confirmation must produce one audit entry"
    );
    assert_eq!(denied[0].command_id, "cmd-confirm-denied");
    assert_eq!(denied[0].session_id, "sess-confirm");
    assert!(
        denied[0].action_type.starts_with("KeyPress"),
        "denial audit should carry the action label, got: {}",
        denied[0].action_type
    );
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

// ── C3 HITL invariant regression pin ─────────────────────────────────────────
//
// Decision 7 (E35-C3): execute_intent_hint operates under INTENT_HINT_POLICY_TOKEN,
// which is classified as an internal policy token.  This means:
//   (a) uses_internal_policy_token("intent-hint") == true
//   (b) resolve_for_command returns default_strict_config + Basic audit level
//   (c) gate.execute() skips PolicyClient.validate_command (no external approval)
//
// C3 changes only the planner quality (LLM vs rule matcher); it must NOT widen
// execution authority or introduce new confirmation paths.  These tests pin the
// gate semantics so any future change that widens authority fails CI.

#[test]
fn hitl_intent_hint_is_classified_as_internal_policy_token() {
    use super::gate::INTENT_HINT_POLICY_TOKEN;
    // (a) Static classification: "intent-hint" must be an internal token.
    // This prevents any refactor that accidentally promotes it to an external,
    // validate_command-gated token.
    assert!(
        gate::CommandExecutionGate::uses_internal_policy_token(INTENT_HINT_POLICY_TOKEN),
        "INTENT_HINT_POLICY_TOKEN must be classified as internal \
         so that PolicyClient.validate_command is bypassed"
    );
}

#[test]
fn hitl_intent_hint_does_not_widen_authority_vs_other_internal_tokens() {
    use super::gate::{
        GUI_SESSION_POLICY_TOKEN, INTENT_HINT_POLICY_TOKEN, SCENE_ACTION_POLICY_TOKEN,
        WORKFLOW_STEP_POLICY_TOKEN,
    };
    // (a) All four well-known internal tokens must be recognised as internal.
    // If INTENT_HINT_POLICY_TOKEN were ever dropped from the match, the gate
    // would fall through to validate_command, changing HITL posture.
    for token in [
        INTENT_HINT_POLICY_TOKEN,
        GUI_SESSION_POLICY_TOKEN,
        SCENE_ACTION_POLICY_TOKEN,
        WORKFLOW_STEP_POLICY_TOKEN,
    ] {
        assert!(
            gate::CommandExecutionGate::uses_internal_policy_token(token),
            "token {token:?} must remain an internal policy token"
        );
    }
    // (b) External-looking tokens must NOT be classified as internal.
    for external_token in ["", "external:nonce", "test-pol:nonce_0001"] {
        assert!(
            !gate::CommandExecutionGate::uses_internal_policy_token(external_token),
            "token {external_token:?} must NOT be classified as internal"
        );
    }
}

#[tokio::test]
async fn hitl_intent_hint_resolves_default_strict_config_and_basic_audit() {
    use super::gate::INTENT_HINT_POLICY_TOKEN;
    // (b) resolve_for_command must return default_strict_config + AuditLevel::Basic
    // for INTENT_HINT_POLICY_TOKEN — identical to all other internal tokens.
    // This pins that C3 does not accidentally introduce a wider sandbox profile.
    let controller = make_controller();
    let cmd = AutomationCommand {
        command_id: "hitl-pin".to_string(),
        session_id: "sess-hitl".to_string(),
        action: AutomationAction::MouseMove { x: 0, y: 0 },
        timeout_ms: None,
        policy_token: INTENT_HINT_POLICY_TOKEN.to_string(),
        origin: maekon_core::models::automation::CommandOrigin::Internal,
    };

    let (resolved, audit_level) = controller.resolve_for_command(&cmd).await;

    // Internal tokens always resolve to Strict profile (default_strict_config).
    assert!(
        matches!(
            resolved.profile,
            maekon_core::config::SandboxProfile::Strict
        ),
        "INTENT_HINT_POLICY_TOKEN must resolve to Strict sandbox profile"
    );
    // Internal tokens always use Basic audit level.
    assert!(
        matches!(audit_level, AuditLevel::Basic),
        "INTENT_HINT_POLICY_TOKEN must use Basic audit level, not Detailed"
    );
}

// ── confirmation_policy knob gate tests ──────────────────────────────────────
//
// #5734: AutomationConfig.confirmation_policy must gate execute_intent_hint.
//   Auto  → immediate execution (D2-② default; C3 pins remain unaffected)
//   Block → PolicyBlocked before execution
//   Confirm → UserDenied when denied (fail-closed, mirrors preset semantics)

#[tokio::test]
async fn confirmation_policy_auto_executes_immediately() {
    use crate::input_driver::{NoOpElementFinder, NoOpInputDriver};
    use crate::intent_resolver::{IntentExecutor, IntentResolver};
    use maekon_core::config::ConfirmationRequirement;

    let mut controller = make_controller();
    controller.set_enabled(true);
    // Auto is the default; set explicitly to document intent.
    controller.set_confirmation_policy(ConfirmationRequirement::Auto);
    controller.set_intent_planner(Arc::new(StubPlanner {
        planned: AutomationIntent::ExecuteHotkey {
            keys: vec!["Ctrl".to_string(), "S".to_string()],
        },
    }));

    let input_driver: Arc<dyn maekon_core::ports::input_driver::InputDriver> =
        Arc::new(NoOpInputDriver);
    let element_finder: Arc<dyn maekon_core::ports::element_finder::ElementFinder> =
        Arc::new(NoOpElementFinder);
    let resolver = IntentResolver::new(element_finder, input_driver, IntentConfig::default());
    controller.set_intent_executor(Arc::new(IntentExecutor::new(
        resolver,
        IntentConfig::default(),
    )));

    // Auto: must succeed without any confirmation callback wired.
    let result = controller
        .execute_intent_hint("policy-auto", "sess-1", "save")
        .await
        .expect("Auto policy must execute immediately without a confirmation callback");
    assert!(result.result.success);
}

#[tokio::test]
async fn confirmation_policy_block_denies_before_execution() {
    use crate::input_driver::{NoOpElementFinder, NoOpInputDriver};
    use crate::intent_resolver::{IntentExecutor, IntentResolver};
    use maekon_core::config::ConfirmationRequirement;

    let mut controller = make_controller();
    controller.set_enabled(true);
    controller.set_confirmation_policy(ConfirmationRequirement::Block);
    controller.set_intent_planner(Arc::new(StubPlanner {
        planned: AutomationIntent::ExecuteHotkey {
            keys: vec!["Ctrl".to_string(), "S".to_string()],
        },
    }));

    let input_driver: Arc<dyn maekon_core::ports::input_driver::InputDriver> =
        Arc::new(NoOpInputDriver);
    let element_finder: Arc<dyn maekon_core::ports::element_finder::ElementFinder> =
        Arc::new(NoOpElementFinder);
    let resolver = IntentResolver::new(element_finder, input_driver, IntentConfig::default());
    controller.set_intent_executor(Arc::new(IntentExecutor::new(
        resolver,
        IntentConfig::default(),
    )));

    let err = controller
        .execute_intent_hint("policy-block", "sess-1", "save")
        .await
        .expect_err("Block policy must return a PolicyBlocked error");
    assert!(
        matches!(err, crate::error::AutomationError::PolicyBlocked),
        "expected PolicyBlocked, got: {err:?}"
    );
}

#[tokio::test]
async fn confirmation_policy_confirm_denies_when_rejected() {
    use crate::input_driver::{NoOpElementFinder, NoOpInputDriver};
    use crate::intent_resolver::{IntentExecutor, IntentResolver};
    use maekon_core::config::ConfirmationRequirement;

    let mut controller = make_controller();
    controller.set_enabled(true);
    controller.set_confirmation_policy(ConfirmationRequirement::Confirm);
    controller.set_intent_planner(Arc::new(StubPlanner {
        planned: AutomationIntent::ExecuteHotkey {
            keys: vec!["Ctrl".to_string(), "S".to_string()],
        },
    }));

    let input_driver: Arc<dyn maekon_core::ports::input_driver::InputDriver> =
        Arc::new(NoOpInputDriver);
    let element_finder: Arc<dyn maekon_core::ports::element_finder::ElementFinder> =
        Arc::new(NoOpElementFinder);
    let resolver = IntentResolver::new(element_finder, input_driver, IntentConfig::default());
    controller.set_intent_executor(Arc::new(IntentExecutor::new(
        resolver,
        IntentConfig::default(),
    )));

    // Simulate user clicking "Deny" via the pending-confirmation map.
    // Spawn a task that resolves the oneshot with false so the test does not wait 30 s.
    let pending = controller.pending_confirmations.clone();
    let cmd_id = "policy-confirm-deny".to_string();
    tokio::spawn(async move {
        // Yield to let execute_intent_hint insert the pending entry first.
        tokio::task::yield_now().await;
        let mut map = pending.lock().await;
        if let Some((_, tx)) = map.remove(&cmd_id) {
            let _ = tx.send(false);
        }
    });

    let err = controller
        .execute_intent_hint("policy-confirm-deny", "sess-1", "save")
        .await
        .expect_err("Confirm policy with denial must return UserDenied");
    assert!(
        matches!(err, crate::error::AutomationError::UserDenied),
        "expected UserDenied after denial, got: {err:?}"
    );
}

#[tokio::test]
async fn confirmation_policy_confirm_executes_when_approved() {
    use crate::input_driver::{NoOpElementFinder, NoOpInputDriver};
    use crate::intent_resolver::{IntentExecutor, IntentResolver};
    use maekon_core::config::ConfirmationRequirement;

    let mut controller = make_controller();
    controller.set_enabled(true);
    controller.set_confirmation_policy(ConfirmationRequirement::Confirm);
    controller.set_intent_planner(Arc::new(StubPlanner {
        planned: AutomationIntent::ExecuteHotkey {
            keys: vec!["Ctrl".to_string(), "S".to_string()],
        },
    }));

    let input_driver: Arc<dyn maekon_core::ports::input_driver::InputDriver> =
        Arc::new(NoOpInputDriver);
    let element_finder: Arc<dyn maekon_core::ports::element_finder::ElementFinder> =
        Arc::new(NoOpElementFinder);
    let resolver = IntentResolver::new(element_finder, input_driver, IntentConfig::default());
    controller.set_intent_executor(Arc::new(IntentExecutor::new(
        resolver,
        IntentConfig::default(),
    )));

    // Simulate user clicking "Approve".
    let pending = controller.pending_confirmations.clone();
    let cmd_id = "policy-confirm-approve".to_string();
    tokio::spawn(async move {
        tokio::task::yield_now().await;
        let mut map = pending.lock().await;
        if let Some((_, tx)) = map.remove(&cmd_id) {
            let _ = tx.send(true);
        }
    });

    let result = controller
        .execute_intent_hint("policy-confirm-approve", "sess-1", "save")
        .await
        .expect("Confirm policy with approval must execute successfully");
    assert!(result.result.success);
}
