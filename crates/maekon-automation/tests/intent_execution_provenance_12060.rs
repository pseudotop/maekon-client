//! Synthetic boundary tests: no provider traffic, screen capture or real input.
#![cfg(test)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use maekon_automation::audit::AuditLogger;
use maekon_automation::controller::AutomationController;
use maekon_automation::input_driver::NoOpElementFinder;
use maekon_automation::intent_planner::LlmIntentPlanner;
use maekon_automation::intent_resolver::{IntentExecutor, IntentResolver};
use maekon_automation::policy::PolicyClient;
use maekon_automation::sandbox::NoOpSandbox;
use maekon_core::config::{ConfirmationRequirement, SandboxConfig};
use maekon_core::error::CoreError;
use maekon_core::models::audit::AuditStatus;
use maekon_core::models::intent::{AutomationIntent, IntentConfig};
use maekon_core::models::intent_execution::*;
use maekon_core::ports::automation::AutomationPort;
use maekon_core::ports::input_driver::InputDriver;
use maekon_core::ports::intent_planner::IntentPlanner;
use maekon_core::ports::llm_provider::{InterpretedAction, LlmProvider, ScreenContext};
use tokio::sync::{Barrier, Notify, RwLock};

const CANARY: &str = "CANARY_TOKEN https://secret.invalid /private/file WINDOW_CANARY";

#[derive(Default)]
struct Driver {
    calls: AtomicUsize,
    fail: bool,
}

#[async_trait]
impl InputDriver for Driver {
    async fn mouse_move(&self, _: i32, _: i32) -> Result<(), CoreError> {
        panic!("unexpected input")
    }
    async fn mouse_click(&self, _: &str, _: i32, _: i32) -> Result<(), CoreError> {
        panic!("unexpected input")
    }
    async fn type_text(&self, _: &str) -> Result<(), CoreError> {
        panic!("unexpected input")
    }
    async fn key_press(&self, _: &str) -> Result<(), CoreError> {
        panic!("unexpected input")
    }
    async fn key_release(&self, _: &str) -> Result<(), CoreError> {
        panic!("unexpected input")
    }
    async fn hotkey(&self, _: &[String]) -> Result<(), CoreError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            Err(network_error())
        } else {
            Ok(())
        }
    }
    fn platform(&self) -> &str {
        "synthetic"
    }
}

fn network_error() -> CoreError {
    CoreError::Network {
        code: maekon_core::error_codes::NetworkCode::Generic,
        message: CANARY.into(),
    }
}

#[derive(Default)]
struct Provider {
    entered: Option<Arc<Barrier>>,
    release_a: Option<Arc<Notify>>,
    release_b: Option<Arc<Notify>>,
}

#[async_trait]
impl LlmProvider for Provider {
    async fn interpret_intent(
        &self,
        _: &ScreenContext,
        hint: &str,
    ) -> Result<InterpretedAction, CoreError> {
        if let Some(entered) = &self.entered {
            entered.wait().await;
            let release = if hint.contains("failure") {
                &self.release_b
            } else {
                &self.release_a
            };
            release
                .as_ref()
                .expect("controlled provider release")
                .notified()
                .await;
        }
        if hint.contains("failure") {
            return Err(network_error());
        }
        Ok(InterpretedAction {
            target_text: None,
            target_role: None,
            action_type: if hint.contains("unsupported") {
                "not-an-action"
            } else {
                "hotkey"
            }
            .into(),
            confidence: if hint.contains("low") { 0.0 } else { 1.0 },
        })
    }
    fn intent_provider_class(&self) -> IntentProviderClass {
        IntentProviderClass::RemoteModel
    }
    fn provider_name(&self) -> &str {
        CANARY
    }
    fn is_external(&self) -> bool {
        false
    }
}

fn controller(
    provider: Arc<dyn LlmProvider>,
    driver: Arc<Driver>,
    capacity: usize,
) -> AutomationController {
    controller_with_audit(
        provider,
        driver,
        Arc::new(RwLock::new(AuditLogger::new(capacity, 10))),
    )
}

fn controller_with_audit(
    provider: Arc<dyn LlmProvider>,
    driver: Arc<Driver>,
    audit: Arc<RwLock<AuditLogger>>,
) -> AutomationController {
    let mut controller = AutomationController::new(
        Arc::new(PolicyClient::new()),
        audit,
        Arc::new(NoOpSandbox),
        SandboxConfig::default(),
    );
    let config = IntentConfig {
        verify_after_action: false,
        max_retries: 0,
        ..Default::default()
    };
    let finder = Arc::new(NoOpElementFinder);
    controller.set_intent_planner(Arc::new(LlmIntentPlanner::new(provider, finder.clone())));
    controller.set_intent_executor(Arc::new(IntentExecutor::new(
        IntentResolver::new(finder, driver.clone(), config.clone()),
        config,
    )));
    controller.set_inline_action_executor(driver);
    controller.set_enabled(true);
    controller
}

fn submission() -> IntentSubmission {
    IntentSubmission {
        submission_id: Some(OpaqueIntentId::new()),
        retry_of: None,
    }
}

fn lookup(
    controller: &AutomationController,
    submission: IntentSubmission,
) -> IntentExecutionProvenance {
    match controller
        .intent_execution_by_submission(submission.submission_id.expect("test submission"))
    {
        IntentExecutionLookup::Found(receipt) => *receipt,
        value => panic!("missing exact observation: {value:?}"),
    }
}

fn assert_private(receipt: &IntentExecutionProvenance) {
    let json = serde_json::to_string(receipt).expect("safe receipt serialization");
    for forbidden in [
        "CANARY",
        "secret.invalid",
        "/private",
        "Ctrl+S",
        "same-command",
        "same-session",
    ] {
        assert!(!json.contains(forbidden), "provenance leaked {forbidden}");
    }
}

#[tokio::test]
async fn error_paths_preserve_attempts_without_executing_actions() {
    for (hint, planning, attempt) in [
        (
            "failure Ctrl+S",
            IntentPlanningOutcome::Error,
            IntentAttemptOutcome::Unavailable,
        ),
        (
            "low Ctrl+S",
            IntentPlanningOutcome::Denied,
            IntentAttemptOutcome::Succeeded,
        ),
        (
            "unsupported Ctrl+S",
            IntentPlanningOutcome::Unsupported,
            IntentAttemptOutcome::Succeeded,
        ),
    ] {
        let driver = Arc::new(Driver::default());
        let controller = controller(Arc::new(Provider::default()), driver.clone(), 100);
        let submission = submission();
        let failure = controller
            .execute_intent_hint_observed("same-command", "same-session", hint, submission)
            .await
            .unwrap_err();
        let receipt = *failure.provenance.unwrap();
        assert_eq!(receipt.planning_outcome, planning);
        assert_eq!(receipt.attempts.len(), 1);
        assert_eq!(receipt.attempts[0].outcome, attempt);
        assert_eq!(receipt.action_outcome, IntentActionOutcome::NotStarted);
        assert_eq!(receipt.observation, IntentObservation::Complete);
        assert_eq!(lookup(&controller, submission), receipt);
        assert_eq!(driver.calls.load(Ordering::SeqCst), 0);
        assert_private(&receipt);
    }
}

#[tokio::test]
async fn successful_planning_never_overrides_block_or_explicit_denial() {
    for policy in [
        ConfirmationRequirement::Block,
        ConfirmationRequirement::Confirm,
    ] {
        let driver = Arc::new(Driver::default());
        let mut controller = controller(Arc::new(Provider::default()), driver.clone(), 100);
        controller.set_confirmation_policy(policy);
        let (send, mut receive) = tokio::sync::mpsc::unbounded_channel();
        let controller = Arc::new(controller.with_confirmation_callback(Arc::new(
            move |pending| {
                send.send(pending).unwrap();
            },
        )));
        let task = tokio::spawn({
            let controller = controller.clone();
            async move {
                controller
                    .execute_intent_hint_observed(
                        "same-command",
                        "same-session",
                        "Ctrl+S",
                        submission(),
                    )
                    .await
            }
        });
        if policy == ConfirmationRequirement::Confirm {
            let pending = tokio::time::timeout(Duration::from_secs(2), receive.recv())
                .await
                .unwrap()
                .unwrap();
            controller
                .submit_confirmation(&pending.command_id, &pending.nonce, false)
                .await
                .unwrap();
        }
        let receipt = task.await.unwrap().unwrap_err().provenance.unwrap();
        assert_eq!(receipt.planning_outcome, IntentPlanningOutcome::Llm);
        assert_eq!(receipt.attempts[0].outcome, IntentAttemptOutcome::Succeeded);
        assert_eq!(receipt.action_outcome, IntentActionOutcome::Denied);
        assert_eq!(driver.calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn audit_records_preserve_exact_identity_and_actual_action_status() {
    for (denied, fail, status) in [
        (false, false, AuditStatus::Completed),
        (true, false, AuditStatus::Denied),
        (false, true, AuditStatus::Failed),
    ] {
        let audit = Arc::new(RwLock::new(AuditLogger::new(100, 10)));
        let driver = Arc::new(Driver {
            fail,
            ..Default::default()
        });
        let mut controller =
            controller_with_audit(Arc::new(Provider::default()), driver.clone(), audit.clone());
        if denied {
            controller.set_confirmation_policy(ConfirmationRequirement::Block);
        }
        let receipt = match controller
            .execute_intent_hint_observed("same-command", "same-session", "Ctrl+S", submission())
            .await
        {
            Ok(result) => result.provenance.unwrap(),
            Err(failure) => *failure.provenance.unwrap(),
        };
        let entries = audit
            .read()
            .await
            .entries_by_action_prefix("intent.execution.observation", 10);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, status);
        assert_eq!(entries[0].command_id, receipt.execution_id.to_string());
        assert_eq!(entries[0].session_id, receipt.execution_id.to_string());
        let recorded: IntentExecutionProvenance =
            serde_json::from_str(entries[0].details.as_deref().unwrap()).unwrap();
        assert_eq!(recorded.execution_id, receipt.execution_id);
        assert_eq!(recorded.action_outcome, receipt.action_outcome);
        assert_eq!(recorded.observation, IntentObservation::Partial);
        assert_eq!(recorded.storage, IntentObservationStorage::ProcessMemory);
        assert_private(&recorded);
        assert_eq!(driver.calls.load(Ordering::SeqCst), usize::from(!denied));
    }
}

#[tokio::test]
async fn actual_parallel_entry_and_reverse_completion_preserve_a_b_a_identity() {
    let entered = Arc::new(Barrier::new(3));
    let release_a = Arc::new(Notify::new());
    let release_b = Arc::new(Notify::new());
    let driver = Arc::new(Driver::default());
    let controller = Arc::new(controller(
        Arc::new(Provider {
            entered: Some(entered.clone()),
            release_a: Some(release_a.clone()),
            release_b: Some(release_b.clone()),
        }),
        driver.clone(),
        100,
    ));
    let a = submission();
    let b = submission();
    let spawn = |hint: &'static str, submission| {
        let controller = controller.clone();
        tokio::spawn(async move {
            controller
                .execute_intent_hint_observed("same-command", "same-session", hint, submission)
                .await
        })
    };
    let task_a = spawn("Ctrl+S", a);
    let task_b = spawn("failure Ctrl+S", b);
    // This positive control fails if planning is globally serialized.
    tokio::time::timeout(Duration::from_secs(2), entered.wait())
        .await
        .unwrap();
    assert_eq!(
        lookup(&controller, a).observation,
        IntentObservation::Partial
    );
    release_b.notify_one();
    let receipt_b = *task_b.await.unwrap().unwrap_err().provenance.unwrap();
    assert!(!task_a.is_finished());
    release_a.notify_one();
    let receipt_a = task_a.await.unwrap().unwrap().provenance.unwrap();
    assert_ne!(receipt_a.execution_id, receipt_b.execution_id);
    assert_eq!(receipt_a.action_outcome, IntentActionOutcome::Completed);
    assert_eq!(receipt_b.action_outcome, IntentActionOutcome::NotStarted);
    // These exact lookups also recover either result when its HTTP response is lost.
    assert_eq!(lookup(&controller, a), receipt_a);
    assert_eq!(lookup(&controller, b), receipt_b);
    assert_eq!(lookup(&controller, a), receipt_a);
    assert_eq!(driver.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn interrupted_planning_stays_unknown_without_replay_or_false_cancellation() {
    let entered = Arc::new(Barrier::new(2));
    let driver = Arc::new(Driver::default());
    let controller = Arc::new(controller(
        Arc::new(Provider {
            entered: Some(entered.clone()),
            release_a: Some(Arc::new(Notify::new())),
            release_b: None,
        }),
        driver.clone(),
        100,
    ));
    let submission = submission();
    let task = tokio::spawn({
        let controller = controller.clone();
        async move {
            controller
                .execute_intent_hint_observed("same-command", "same-session", "Ctrl+S", submission)
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(2), entered.wait())
        .await
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let receipt = lookup(&controller, submission);
    assert_eq!(receipt.observation, IntentObservation::Partial);
    assert_eq!(receipt.action_outcome, IntentActionOutcome::Unknown);
    assert_eq!(receipt.attempts[0].outcome, IntentAttemptOutcome::Unknown);
    assert_eq!(driver.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn audit_loss_and_executor_failure_do_not_become_successful_observations() {
    for (capacity, fail) in [(1, false), (100, true)] {
        let driver = Arc::new(Driver {
            fail,
            ..Default::default()
        });
        let controller = controller(Arc::new(Provider::default()), driver.clone(), capacity);
        let result = controller
            .execute_intent_hint_observed("same-command", "same-session", "Ctrl+S", submission())
            .await;
        let receipt = match result {
            Ok(value) => value.provenance.unwrap(),
            Err(value) => *value.provenance.unwrap(),
        };
        assert_eq!(driver.calls.load(Ordering::SeqCst), 1);
        if fail {
            assert_eq!(receipt.action_outcome, IntentActionOutcome::Failed);
            assert_private(&receipt);
        } else {
            assert_eq!(receipt.action_outcome, IntentActionOutcome::Completed);
            assert_eq!(receipt.observation, IntentObservation::Partial);
        }
        assert_eq!(receipt.storage, IntentObservationStorage::ProcessMemory);
    }
}

#[tokio::test]
async fn manual_retry_gets_new_identity_and_duplicate_submission_is_ambiguous() {
    let driver = Arc::new(Driver::default());
    let controller = controller(Arc::new(Provider::default()), driver.clone(), 100);
    let first = submission();
    let a = controller
        .execute_intent_hint_observed("same-command", "same-session", "Ctrl+S", first)
        .await
        .unwrap()
        .provenance
        .unwrap();
    let retry = IntentSubmission {
        retry_of: Some(a.execution_id),
        ..submission()
    };
    let b = controller
        .execute_intent_hint_observed("same-command", "same-session", "Ctrl+S", retry)
        .await
        .unwrap()
        .provenance
        .unwrap();
    assert_ne!(a.execution_id, b.execution_id);
    assert_eq!(b.retry_of, Some(a.execution_id));
    assert_eq!(lookup(&controller, first), a);
    controller
        .execute_intent_hint_observed("same-command", "same-session", "Ctrl+S", first)
        .await
        .unwrap();
    assert!(matches!(
        controller.intent_execution_by_submission(first.submission_id.unwrap()),
        IntentExecutionLookup::Unknown(IntentLookupUnknownReason::Ambiguous)
    ));
    assert!(matches!(
        controller.intent_execution_by_submission(OpaqueIntentId::new()),
        IntentExecutionLookup::Unknown(IntentLookupUnknownReason::Missing)
    ));
    // Correlation is not an idempotency token; only explicit calls execute.
    assert_eq!(driver.calls.load(Ordering::SeqCst), 3);
}

struct LegacyPlanner;
#[async_trait]
impl IntentPlanner for LegacyPlanner {
    async fn plan(&self, _: &str) -> Result<AutomationIntent, CoreError> {
        Ok(AutomationIntent::ExecuteHotkey {
            keys: vec!["Ctrl".into(), "S".into()],
        })
    }
}

#[tokio::test]
async fn legacy_planner_keeps_success_but_never_invents_provider_evidence() {
    let driver = Arc::new(Driver::default());
    let mut controller = controller(Arc::new(Provider::default()), driver.clone(), 100);
    controller.set_intent_planner(Arc::new(LegacyPlanner));
    let result = controller
        .execute_intent_hint_observed("same-command", "same-session", "Ctrl+S", submission())
        .await
        .unwrap();
    assert!(result.result.result.success);
    let receipt = result.provenance.unwrap();
    assert_eq!(receipt.observation, IntentObservation::LegacyUnknown);
    assert!(receipt.attempts.is_empty());
    assert_eq!(receipt.planning_outcome, IntentPlanningOutcome::Unknown);
    assert_eq!(driver.calls.load(Ordering::SeqCst), 1);
}
