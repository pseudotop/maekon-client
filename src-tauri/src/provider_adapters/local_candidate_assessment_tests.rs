use super::*;
use maekon_core::config::{ExternalDataPolicy, PiiFilterLevel, PrivacyConfig};
use maekon_core::consent::{ConsentManager, ConsentPermissions};
use maekon_core::error::CoreError;
use maekon_core::models::candidate_decision::{BackendFailure, CandidateSnapshot};
use maekon_core::models::context::{ProcessInfo, WindowInfo};
use maekon_core::models::event::ProcessDetail;
use maekon_core::models::gui::GuiCandidate;
use maekon_core::models::intent::ElementBounds;
use maekon_core::models::ui_scene::{NormalizedBounds, UiSceneElement};
use maekon_core::ports::monitor::ProcessMonitor;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::Notify;

/// Smallest wall-clock step every platform can represent. Windows `SystemTime`
/// counts 100 ns ticks, so a 1 ns step vanishes there: "just before the
/// deadline" becomes "at the deadline" and a 1 ns regression is no regression
/// (#12745).
const WALL_STEP: Duration = Duration::from_micros(1);

type Hook = Box<dyn FnOnce() + Send>;
struct Monitor {
    calls: AtomicUsize,
    window: Mutex<Option<WindowInfo>>,
    after_read: Mutex<Option<Hook>>,
}
#[async_trait]
impl ProcessMonitor for Monitor {
    async fn get_active_window(&self) -> Result<Option<WindowInfo>, CoreError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let window = self.window.lock().clone();
        tokio::task::yield_now().await;
        if let Some(hook) = self.after_read.lock().take() {
            hook();
        }
        Ok(window)
    }
    async fn get_top_processes(&self, _limit: usize) -> Result<Vec<ProcessInfo>, CoreError> {
        Ok(vec![])
    }
    async fn get_detailed_processes(
        &self,
        _foreground_pid: Option<u32>,
        _top_n: usize,
    ) -> Result<Vec<ProcessDetail>, CoreError> {
        Ok(vec![])
    }
}

#[derive(Default)]
struct Audit {
    records: Mutex<Vec<AssessmentAuditRecord>>,
    fail: Mutex<Option<AssessmentAuditPhase>>,
    hook: Mutex<Option<(AssessmentAuditPhase, Hook)>>,
}
#[async_trait]
impl CandidateAssessmentAuditPort for Audit {
    async fn record_assessment(&self, record: AssessmentAuditRecord) -> Result<(), CoreError> {
        if self.fail.lock().as_ref() == Some(&record.phase) {
            return Err(CoreError::Storage {
                code: maekon_core::error_codes::StorageCode::Failed,
                message: "test-only audit failure".into(),
            });
        }
        let phase = record.phase;
        self.records.lock().push(record);
        tokio::task::yield_now().await;
        let hook = {
            let mut hook = self.hook.lock();
            if hook
                .as_ref()
                .is_some_and(|(expected, _)| *expected == phase)
            {
                hook.take()
            } else {
                None
            }
        };
        if let Some((_, hook)) = hook {
            hook();
        }
        Ok(())
    }
}
struct Model {
    calls: AtomicUsize,
    wire: Mutex<Vec<String>>,
    hold: AtomicBool,
    entered: Notify,
    release: Notify,
    hook: Mutex<Option<Hook>>,
    choice: Mutex<LocalAssessmentChoice>,
    wrong_model: AtomicBool,
}
impl Default for Model {
    fn default() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            wire: Mutex::new(vec![]),
            hold: AtomicBool::new(false),
            entered: Notify::new(),
            release: Notify::new(),
            hook: Mutex::new(None),
            choice: Mutex::new(LocalAssessmentChoice::Selected("c0".into())),
            wrong_model: AtomicBool::new(false),
        }
    }
}
#[async_trait]
impl LocalCandidateModelPort for Model {
    async fn infer(
        &self,
        text: &DecisionText,
        approval: &LocalModelApproval,
        _guard: &dyn CandidateAttemptGuard,
    ) -> Result<LocalAssessmentAnswer, BackendFailure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.wire.lock().push(serde_json::to_string(text).unwrap());
        if self.hold.load(Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.notified().await;
        }
        if let Some(hook) = self.hook.lock().take() {
            hook();
        }
        // Deliberately no final guard in this double: composition must reject
        // late/revoked output even if an adapter misbehaves.
        Ok(LocalAssessmentAnswer {
            choice: self.choice.lock().clone(),
            request_hash: digest_bytes(b"test-wire"),
            observed_model: if self.wrong_model.load(Ordering::SeqCst) {
                "wrong".into()
            } else {
                approval.model.clone()
            },
            model_digest: approval.model_digest.clone(),
            usage: None,
        })
    }
}

fn request(goal: &str, labels: &[&str]) -> CandidateDecisionRequest {
    CandidateDecisionRequest::new(
        CandidateSnapshot {
            goal: goal.into(),
            scene_id: "scene".into(),
            frame_id: "frame".into(),
            generation: 1,
            window: WindowInfo {
                title: "Editor".into(),
                app_name: "Editor".into(),
                app_bundle_id: None,
                pid: 1,
                bounds: None,
            },
            candidates: labels
                .iter()
                .enumerate()
                .map(|(index, label)| GuiCandidate {
                    eligible: true,
                    ranking_reason: None,
                    element: UiSceneElement {
                        element_id: format!("private-id-{index}"),
                        label: (*label).into(),
                        bbox_abs: ElementBounds {
                            x: 1,
                            y: 2,
                            width: 3,
                            height: 4,
                        },
                        bbox_norm: NormalizedBounds::new(0.1, 0.2, 0.3, 0.4),
                        role: Some("button".into()),
                        intent: None,
                        state: Some("enabled".into()),
                        confidence: if index == 0 { 0.99 } else { 0.2 },
                        text_masked: None,
                        parent_id: None,
                    },
                })
                .collect(),
        },
        Instant::now(),
        Duration::from_secs(10),
    )
    .unwrap()
}

fn permissions() -> ConsentPermissions {
    ConsentPermissions {
        full_text_extraction: true,
        ocr_processing: true,
        ..ConsentPermissions::default()
    }
}
fn approval(provider: CandidateProvider, max_attempts: u32) -> LocalAssessmentApproval {
    LocalAssessmentApproval {
        approval_reference: "test-only-approval".into(),
        principal_namespace: "test-principal".into(),
        policy_revision: "policy-1".into(),
        configuration_revision: "config-1".into(),
        trigger: CandidateTrigger::UserRequest,
        max_attempts,
        expires_at: Instant::now() + Duration::from_secs(60),
        model: (provider == CandidateProvider::LocalModel).then(|| LocalModelApproval {
            daemon_reference: "trusted-test-daemon".into(),
            configuration_revision: "cloud-disabled".into(),
            endpoint_origin: "http://127.0.0.1:11434".into(),
            model: "fixture:fixed".into(),
            model_digest: format!("sha256:{}", "a".repeat(64)),
            expires_at: Instant::now() + Duration::from_secs(60),
        }),
    }
}
struct Fixture {
    runtime: LocalCandidateAssessment,
    control: LocalAssessmentControl,
    consent: Arc<ConsentManager>,
    monitor: Arc<Monitor>,
    model: Arc<Model>,
    audit: Arc<Audit>,
    request: CandidateDecisionRequest,
    _dir: tempfile::TempDir,
}
fn fixture(provider: CandidateProvider, max_attempts: u32) -> Fixture {
    let request = request("Save", &["Save"]);
    let dir = tempfile::TempDir::new().unwrap();
    let consent = Arc::new(ConsentManager::new(dir.path().join("consent.json")));
    consent.grant_consent(permissions(), 30).unwrap();
    let monitor = Arc::new(Monitor {
        calls: AtomicUsize::new(0),
        window: Mutex::new(Some(request.snapshot().window.clone())),
        after_read: Mutex::new(None),
    });
    let privacy = ExternalOcrPrivacyGuard::new(
        consent.clone(),
        PiiFilterLevel::Strict,
        ExternalDataPolicy::PiiFilterStrict,
        PrivacyConfig::default(),
        monitor.clone(),
        None,
    );
    let model = Arc::new(Model::default());
    let audit = Arc::new(Audit::default());
    let policy = CandidateDecisionPolicy {
        provider: Some(provider),
        ..CandidateDecisionPolicy::default()
    };
    let (runtime, control) = LocalCandidateAssessment::new(
        policy,
        AiAccessMode::LocalModel,
        privacy,
        audit.clone(),
        Some(model.clone()),
    );
    control.bind(&request).unwrap();
    control.approve(approval(provider, max_attempts)).unwrap();
    Fixture {
        runtime,
        control,
        consent,
        monitor,
        model,
        audit,
        request,
        _dir: dir,
    }
}
fn unavailable(result: &CandidateAssessmentResult, expected: DecisionUnavailable) {
    assert_eq!(
        result.outcome,
        AssessmentOutcome::Unavailable(expected),
        "{result:?}"
    );
}
fn selected(result: &CandidateAssessmentResult, id: &str) {
    assert_eq!(
        result.outcome,
        AssessmentOutcome::Selected {
            candidate_id: id.into()
        },
        "{result:?}"
    );
}

#[tokio::test]
async fn local_assessment_rules_match_goal_without_order_or_recognition_confidence_ties() {
    let mut f = fixture(CandidateProvider::LocalRules, 10);
    for (goal, labels, expected) in [
        ("Save", vec!["Cancel", "Save"], Some("private-id-1")),
        ("save", vec![" Save ", "Cancel"], Some("private-id-0")),
        ("저장", vec!["취소", "저장"], Some("private-id-1")),
        ("Save", vec!["Save", "Save"], None),
        ("Save", vec!["Cancel", "Close"], None),
        ("Do not save", vec!["Save"], None),
    ] {
        f.request = request(goal, &labels);
        f.control.bind(&f.request).unwrap();
        let result = f.runtime.assess(&f.request).await;
        if let Some(id) = expected {
            selected(&result, id);
        } else {
            assert_eq!(result.outcome, AssessmentOutcome::Delegate);
        }
        assert!(matches!(
            result.evidence,
            AssessmentEvidence::LocalHeuristic { .. }
        ));
        assert_eq!(
            result.provenance.unwrap().provider,
            CandidateProvider::LocalRules
        );
    }
    assert_eq!(f.model.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn local_assessment_redaction_collision_delegates_and_model_wire_is_minimal() {
    let mut f = fixture(CandidateProvider::LocalRules, 10);
    for (goal, label) in [
        ("alice@example.com", "bob@example.com"),
        ("alice@example.com", "[EMAIL]"),
        ("[EMAIL]", "alice@example.com"),
    ] {
        f.request = request(goal, &[label]);
        f.control.bind(&f.request).unwrap();
        let result = f.runtime.assess(&f.request).await;
        assert_eq!(result.outcome, AssessmentOutcome::Delegate);
        assert!(matches!(
            result.evidence,
            AssessmentEvidence::LocalHeuristic { exact_matches: 0 }
        ));
    }
    let mut f = fixture(CandidateProvider::LocalModel, 10);
    f.request = request("Select alice@example.com", &["alice@example.com"]);
    f.control.bind(&f.request).unwrap();
    let result = f.runtime.assess(&f.request).await;
    selected(&result, "private-id-0");
    let wire = f.model.wire.lock()[0].clone();
    assert!(!wire.contains("alice@example.com") && !wire.contains("private-id-0"));
    assert!(wire.contains("[EMAIL]"));
    assert!(matches!(result.evidence, AssessmentEvidence::ModelReported));
    let AssessmentAttempt::Local(attempt) = &result.attempts[0] else {
        panic!("wrong attempt");
    };
    assert_eq!(attempt.usage, None);
}

#[tokio::test]
async fn local_assessment_cache_does_not_renew_deadline_and_aba_requires_new_work() {
    let f = fixture(CandidateProvider::LocalRules, 10);
    let first = f.runtime.assess(&f.request).await;
    selected(&first, "private-id-0");
    let cached = f.runtime.assess(&f.request).await;
    selected(&cached, "private-id-0");
    assert_eq!(cached.cache_source, Some(first.decision_id));
    assert!(cached.attempts.is_empty());
    assert_eq!(cached.binding.deadline, first.binding.deadline);
    let other = request("Cancel", &["Cancel"]);
    f.control.bind(&other).unwrap();
    f.control.bind(&f.request).unwrap();
    let new_result = f.runtime.assess(&f.request).await;
    assert_eq!(new_result.cache_source, None);
    assert_eq!(new_result.attempts.len(), 1);
    f.control.revoke();
    unavailable(
        &f.runtime.assess(&f.request).await,
        DecisionUnavailable::Cancelled,
    );
    assert_eq!(
        f.control
            .approve(approval(CandidateProvider::LocalRules, 10)),
        Err(DecisionUnavailable::Cancelled)
    );
}

#[tokio::test]
async fn local_assessment_consent_and_surface_denials_apply_to_rules_and_cache() {
    for case in 0..5 {
        let f = fixture(CandidateProvider::LocalRules, 10);
        selected(&f.runtime.assess(&f.request).await, "private-id-0");
        match case {
            0 => f.consent.revoke_consent().unwrap(),
            1 => f
                .consent
                .grant_consent(
                    ConsentPermissions {
                        full_text_extraction: false,
                        ..permissions()
                    },
                    30,
                )
                .unwrap(),
            2 => f
                .consent
                .grant_consent(
                    ConsentPermissions {
                        ocr_processing: false,
                        ..permissions()
                    },
                    30,
                )
                .unwrap(),
            3 => f.monitor.window.lock().as_mut().unwrap().app_name = "1Password".into(),
            _ => *f.monitor.window.lock() = None,
        }
        unavailable(
            &f.runtime.assess(&f.request).await,
            DecisionUnavailable::ConsentOrPolicyDenied,
        );
        assert_eq!(f.model.calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn local_assessment_rechecks_window_and_revocation_after_async_observation() {
    let f = fixture(CandidateProvider::LocalRules, 10);
    let control = f.control.clone();
    *f.monitor.after_read.lock() = Some(Box::new(move || control.revoke()));
    unavailable(
        &f.runtime.assess(&f.request).await,
        DecisionUnavailable::Cancelled,
    );
    assert!(f.audit.records.lock().is_empty());
    let f = fixture(CandidateProvider::LocalRules, 10);
    f.monitor.window.lock().as_mut().unwrap().title = "Other".into();
    unavailable(
        &f.runtime.assess(&f.request).await,
        DecisionUnavailable::Stale,
    );
}

#[tokio::test]
async fn local_assessment_audit_failure_never_publishes_or_refunds_an_attempt() {
    for phase in [
        AssessmentAuditPhase::BeforeAttempt,
        AssessmentAuditPhase::AfterAttempt,
    ] {
        let f = fixture(CandidateProvider::LocalModel, 1);
        *f.audit.fail.lock() = Some(phase);
        unavailable(
            &f.runtime.assess(&f.request).await,
            DecisionUnavailable::AuditUnavailable,
        );
        assert_eq!(
            f.model.calls.load(Ordering::SeqCst),
            usize::from(phase == AssessmentAuditPhase::AfterAttempt)
        );
        assert_eq!(f.control.state.lock().remaining_attempts, 0);
        assert!(f.control.state.lock().cache.is_none());
    }
}

#[tokio::test]
async fn local_assessment_revocation_and_rebind_during_audit_or_model_drop_late_result() {
    for phase in [
        AssessmentAuditPhase::BeforeAttempt,
        AssessmentAuditPhase::AfterAttempt,
        AssessmentAuditPhase::CacheHit,
    ] {
        let f = fixture(CandidateProvider::LocalRules, 10);
        if phase == AssessmentAuditPhase::CacheHit {
            selected(&f.runtime.assess(&f.request).await, "private-id-0");
        }
        let control = f.control.clone();
        *f.audit.hook.lock() = Some((phase, Box::new(move || control.revoke())));
        unavailable(
            &f.runtime.assess(&f.request).await,
            DecisionUnavailable::Cancelled,
        );
        assert!(f.control.state.lock().cache.is_none());
    }
    for revoke in [true, false] {
        let f = fixture(CandidateProvider::LocalModel, 10);
        let control = f.control.clone();
        let changed = request("Other", &["Other"]);
        *f.model.hook.lock() = Some(Box::new(move || {
            if revoke {
                control.revoke();
            } else {
                control.bind(&changed).unwrap();
            }
        }));
        let result = f.runtime.assess(&f.request).await;
        unavailable(
            &result,
            if revoke {
                DecisionUnavailable::Cancelled
            } else {
                DecisionUnavailable::Stale
            },
        );
        assert_eq!(result.attempts.len(), 1);
        assert_eq!(f.audit.records.lock().len(), 2);
    }
}

#[tokio::test]
async fn local_assessment_atomic_quota_admits_one_concurrent_provider_attempt() {
    let f = fixture(CandidateProvider::LocalModel, 1);
    let (a, b) = tokio::join!(f.runtime.assess(&f.request), f.runtime.assess(&f.request));
    assert_eq!(f.model.calls.load(Ordering::SeqCst), 1);
    let outcomes = [a.outcome, b.outcome];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, AssessmentOutcome::Selected { .. }))
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome
                == AssessmentOutcome::Unavailable(DecisionUnavailable::BudgetExceeded))
            .count(),
        1
    );
}

#[tokio::test]
async fn local_assessment_timeout_preserves_pending_attempt_and_never_caches_model_output() {
    let mut f = fixture(CandidateProvider::LocalModel, 1);
    f.request = CandidateDecisionRequest::new(
        f.request.snapshot().clone(),
        Instant::now(),
        Duration::from_millis(80),
    )
    .unwrap();
    f.control.bind(&f.request).unwrap();
    f.model.hold.store(true, Ordering::SeqCst);
    let result = tokio::time::timeout(Duration::from_secs(1), f.runtime.assess(&f.request))
        .await
        .unwrap();
    unavailable(&result, DecisionUnavailable::Timeout);
    let AssessmentAttempt::Local(attempt) = &result.attempts[0] else {
        panic!("missing attempt");
    };
    assert!(attempt.attempted);
    assert_eq!(attempt.usage, None);
    assert_eq!(attempt.failure, Some(DecisionUnavailable::Timeout));
    assert!(f.control.state.lock().cache.is_none());
    assert_eq!(f.control.state.lock().remaining_attempts, 0);
}

#[tokio::test]
async fn local_assessment_unknown_ids_and_model_drift_are_rejected_without_substitution() {
    for wrong_model in [false, true] {
        let f = fixture(CandidateProvider::LocalModel, 10);
        if wrong_model {
            f.model.wrong_model.store(true, Ordering::SeqCst);
        } else {
            *f.model.choice.lock() = LocalAssessmentChoice::Selected("removed-id".into());
        }
        unavailable(
            &f.runtime.assess(&f.request).await,
            DecisionUnavailable::InvalidResponse,
        );
        assert!(f.control.state.lock().cache.is_none());
    }
    let f = fixture(CandidateProvider::LocalModel, 10);
    for (choice, expected) in [
        (LocalAssessmentChoice::None, AssessmentOutcome::None),
        (LocalAssessmentChoice::Delegate, AssessmentOutcome::Delegate),
    ] {
        *f.model.choice.lock() = choice;
        assert_eq!(f.runtime.assess(&f.request).await.outcome, expected);
    }
    assert_eq!(f.model.calls.load(Ordering::SeqCst), 2);
}

#[test]
fn local_assessment_final_publication_lock_rejects_stale_epoch_even_after_earlier_check() {
    let f = fixture(CandidateProvider::LocalRules, 10);
    let epoch = f.control.state.lock().epoch;
    let revision = f.runtime.privacy.candidate_consent_revision().unwrap();
    let context = Evaluation {
        runtime: &f.runtime,
        request: &f.request,
        epoch,
        revision: &revision,
    };
    context.check_sync().unwrap();
    f.control.bind(&request("Other", &["Other"])).unwrap();
    f.control.bind(&f.request).unwrap();
    let result =
        CandidateAssessmentResult::unavailable(f.request.binding(), DecisionUnavailable::Off);
    assert_eq!(
        f.runtime.publish(&context, "namespace", &result, true),
        Err(DecisionUnavailable::Stale)
    );
    assert!(f.control.state.lock().cache.is_none());
}

#[test]
fn local_assessment_state_checks_exact_deadline_and_epoch_exhaustion() {
    let f = fixture(CandidateProvider::LocalRules, 10);
    let epoch = f.control.state.lock().epoch;
    assert_eq!(
        f.control
            .state
            .lock()
            .check(&f.request, epoch, f.request.binding().deadline - WALL_STEP),
        Ok(())
    );
    assert_eq!(
        f.control
            .state
            .lock()
            .check(&f.request, epoch, f.request.binding().deadline),
        Err(DecisionUnavailable::Expired)
    );
    f.control.state.lock().epoch = u64::MAX;
    assert_eq!(
        f.control.bind(&f.request),
        Err(DecisionUnavailable::Cancelled)
    );
    assert!(f.control.state.lock().revoked);
}

#[tokio::test]
async fn local_assessment_reconsent_cannot_resurrect_the_original_approval() {
    let f = fixture(CandidateProvider::LocalRules, 10);
    selected(&f.runtime.assess(&f.request).await, "private-id-0");
    f.consent
        .grant_consent(
            ConsentPermissions {
                microphone: true,
                ..permissions()
            },
            30,
        )
        .unwrap();
    unavailable(
        &f.runtime.assess(&f.request).await,
        DecisionUnavailable::ConsentOrPolicyDenied,
    );
    f.consent.grant_consent(permissions(), 30).unwrap();
    unavailable(
        &f.runtime.assess(&f.request).await,
        DecisionUnavailable::Cancelled,
    );
    assert!(f.control.state.lock().cache.is_none());
}

#[test]
fn local_assessment_wall_deadline_survives_paused_monotonic_clock_and_aba() {
    let f = fixture(CandidateProvider::LocalRules, 10);
    let original_deadline = f
        .control
        .state
        .lock()
        .clock
        .wall_at(f.request.binding().deadline)
        .unwrap();
    let now = Instant::now();
    let epoch = f.control.state.lock().epoch;
    assert_eq!(
        f.control
            .state
            .lock()
            .check_at(&f.request, epoch, now, original_deadline - WALL_STEP),
        Ok(())
    );
    f.control.bind(&request("Other", &["Other"])).unwrap();
    f.control.bind(&f.request).unwrap();
    let epoch = f.control.state.lock().epoch;
    assert_eq!(
        f.control
            .state
            .lock()
            .clock
            .wall_at(f.request.binding().deadline),
        Some(original_deadline)
    );
    assert_eq!(
        f.control
            .state
            .lock()
            .check_at(&f.request, epoch, now, original_deadline),
        Err(DecisionUnavailable::Expired)
    );
    assert!(f.control.state.lock().revoked);
    assert_eq!(
        f.control.bind(&f.request),
        Err(DecisionUnavailable::Cancelled)
    );
}

#[test]
fn local_assessment_clock_regression_and_shorter_approval_expire_permanently() {
    for case in 0..3 {
        let f = fixture(CandidateProvider::LocalModel, 10);
        let mut state = f.control.state.lock();
        let now = Instant::now();
        let wall = state.clock.wall_at(now).unwrap();
        let epoch = state.epoch;
        assert_eq!(state.check_at(&f.request, epoch, now, wall), Ok(()));
        let bound = if case == 0 {
            wall - WALL_STEP
        } else {
            let short = now + Duration::from_secs(1);
            let approval = state.approval.as_mut().unwrap();
            if case == 1 {
                approval.expires_at = short;
            } else {
                approval.model.as_mut().unwrap().expires_at = short;
            }
            state.clock.wall_at(short).unwrap()
        };
        assert_eq!(
            state.check_at(&f.request, epoch, now, bound),
            Err(DecisionUnavailable::Expired)
        );
        assert!(state.revoked);
        assert_eq!(
            state.check_at(&f.request, epoch, now, wall),
            Err(DecisionUnavailable::Cancelled)
        );
    }
}

#[tokio::test]
async fn local_assessment_audit_records_keep_provider_attempt_and_cache_provenance() {
    for provider in [CandidateProvider::LocalRules, CandidateProvider::LocalModel] {
        let f = fixture(provider, 2);
        let first = f.runtime.assess(&f.request).await;
        selected(&first, "private-id-0");
        let AssessmentAttempt::Local(attempt) = &first.attempts[0] else {
            panic!("local assessment must preserve its observed attempt");
        };
        let second = f.runtime.assess(&f.request).await;
        selected(&second, "private-id-0");
        let records = f.audit.records.lock();
        let phases: Vec<_> = records.iter().map(|record| record.phase).collect();
        assert_eq!(
            phases,
            if provider == CandidateProvider::LocalRules {
                vec![
                    AssessmentAuditPhase::BeforeAttempt,
                    AssessmentAuditPhase::AfterAttempt,
                    AssessmentAuditPhase::CacheHit,
                ]
            } else {
                vec![
                    AssessmentAuditPhase::BeforeAttempt,
                    AssessmentAuditPhase::AfterAttempt,
                    AssessmentAuditPhase::BeforeAttempt,
                    AssessmentAuditPhase::AfterAttempt,
                ]
            }
        );
        for record in records.iter() {
            assert_eq!(record.provider, provider);
            assert_eq!(
                record.transport,
                if provider == CandidateProvider::LocalRules {
                    AssessmentTransport::InProcess
                } else {
                    AssessmentTransport::LoopbackHttp
                }
            );
            assert_eq!(record.schema_revision, ASSESSMENT_SCHEMA);
            assert_eq!(record.namespace_hash, records[0].namespace_hash);
            assert!(!record.namespace_hash.contains("Save"));
        }
        for record in &records[..2] {
            assert_eq!(record.decision_id, first.decision_id);
            assert_eq!(record.attempt_id, Some(attempt.id));
            assert_eq!(record.attempt.as_ref().unwrap().id, attempt.id);
            assert_eq!(record.cache_source, None);
        }
        assert!(!records[0].attempt.as_ref().unwrap().attempted);
        assert!(records[1].attempt.as_ref().unwrap().attempted);
        assert_eq!(records[1].attempt.as_ref().unwrap().failure, None);
        if provider == CandidateProvider::LocalRules {
            let cache = &records[2];
            assert_eq!(cache.decision_id, second.decision_id);
            assert_ne!(cache.decision_id, first.decision_id);
            assert_eq!(cache.cache_source, Some(first.decision_id));
            assert_eq!(cache.attempt_id, None);
            assert!(cache.attempt.is_none());
        }
    }
}

#[tokio::test]
async fn local_assessment_checkpoint_denies_new_observation_after_authority_changes() {
    for case in 0..3 {
        let f = fixture(CandidateProvider::LocalRules, 10);
        let lease = f.control.lease(&f.request).unwrap();
        let revision = f.runtime.privacy.candidate_consent_revision().unwrap();
        let guard = Evaluation {
            runtime: &f.runtime,
            request: &f.request,
            epoch: lease.epoch,
            revision: &revision,
        };
        assert_eq!(guard.checkpoint().await, Ok(()));
        assert_eq!(f.monitor.calls.load(Ordering::SeqCst), 1);
        let expected = match case {
            0 => {
                f.control.revoke();
                DecisionUnavailable::Cancelled
            }
            1 => {
                f.control.bind(&f.request).unwrap();
                DecisionUnavailable::Stale
            }
            _ => {
                f.consent
                    .grant_consent(
                        ConsentPermissions {
                            ocr_processing: false,
                            ..permissions()
                        },
                        30,
                    )
                    .unwrap();
                DecisionUnavailable::ConsentOrPolicyDenied
            }
        };
        // A transport checkpoint must reject before requesting more window data.
        assert_eq!(guard.checkpoint().await, Err(expected));
        assert_eq!(f.monitor.calls.load(Ordering::SeqCst), 1);
        assert_eq!(f.model.calls.load(Ordering::SeqCst), 0);
        assert!(f.audit.records.lock().is_empty());
    }
}

#[tokio::test]
async fn local_assessment_checkpoint_does_not_grant_transport_after_async_rebind_or_revoke() {
    for rebind in [false, true] {
        let f = fixture(CandidateProvider::LocalRules, 10);
        let lease = f.control.lease(&f.request).unwrap();
        let revision = f.runtime.privacy.candidate_consent_revision().unwrap();
        let guard = Evaluation {
            runtime: &f.runtime,
            request: &f.request,
            epoch: lease.epoch,
            revision: &revision,
        };
        assert_eq!(guard.checkpoint().await, Ok(()));
        let control = f.control.clone();
        let rebound = f.request.clone();
        *f.monitor.after_read.lock() = Some(Box::new(move || {
            if rebind {
                control.bind(&rebound).unwrap();
            } else {
                control.revoke();
            }
        }));
        let expected = if rebind {
            DecisionUnavailable::Stale
        } else {
            DecisionUnavailable::Cancelled
        };
        // No later publication guard can substitute for this transport boundary.
        assert_eq!(guard.checkpoint().await, Err(expected));
        assert_eq!(f.monitor.calls.load(Ordering::SeqCst), 2);
        assert_eq!(f.model.calls.load(Ordering::SeqCst), 0);
        assert!(f.audit.records.lock().is_empty());
    }
}

#[path = "local_candidate_assessment_factory_tests.rs"]
mod factory_tests;
