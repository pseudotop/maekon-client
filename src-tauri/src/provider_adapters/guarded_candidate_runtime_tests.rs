use super::tests::{approval_at, permissions, preparation_fixture, PreparationFixture};
use super::*;
use maekon_core::error::CoreError;
use maekon_core::models::candidate_decision::{ChoiceEvidence, SuitabilityEvidence};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::Notify;

type AuditHook = (DecisionAuditPhase, Box<dyn FnOnce() + Send>);

#[derive(Default)]
struct Audit {
    records: Mutex<Vec<DecisionAuditRecord>>,
    fail_at: Mutex<Option<DecisionAuditPhase>>,
    after_record: Mutex<Option<AuditHook>>,
}

#[async_trait]
impl CandidateDecisionAuditPort for Audit {
    async fn record(&self, record: DecisionAuditRecord) -> Result<(), CoreError> {
        if self.fail_at.lock().as_ref() == Some(&record.phase) {
            return Err(CoreError::Storage {
                code: maekon_core::error_codes::StorageCode::Failed,
                message: "test-only audit failure".into(),
            });
        }
        let phase = record.phase;
        self.records.lock().push(record);
        tokio::task::yield_now().await;
        let hook = {
            let mut slot = self.after_record.lock();
            if slot
                .as_ref()
                .is_some_and(|(expected, _)| *expected == phase)
            {
                slot.take().map(|(_, hook)| hook)
            } else {
                None
            }
        };
        if let Some(hook) = hook {
            hook();
        }
        Ok(())
    }
}

struct Backend {
    calls: AtomicUsize,
    selected: Mutex<String>,
    fail: AtomicBool,
    hold_choice: AtomicBool,
    entered: Notify,
    release: Notify,
    wire: Mutex<Vec<String>>,
    wrong_suitability: AtomicBool,
    choice_usage: Mutex<DecisionUsage>,
    observed_model: Mutex<String>,
    suitability_score: Mutex<f64>,
}

impl Default for Backend {
    fn default() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            selected: Mutex::new("c0".into()),
            fail: AtomicBool::new(false),
            hold_choice: AtomicBool::new(false),
            entered: Notify::new(),
            release: Notify::new(),
            wire: Mutex::new(vec![]),
            wrong_suitability: AtomicBool::new(false),
            choice_usage: Mutex::new(DecisionUsage {
                input_tokens: 100,
                output_tokens: 0,
            }),
            observed_model: Mutex::new(JEV_MODEL.into()),
            suitability_score: Mutex::new(0.75),
        }
    }
}

#[async_trait]
impl CandidateDecisionBackendPort for Backend {
    async fn choose(
        &self,
        text: &DecisionText,
        api_key: &str,
    ) -> Result<BackendResponse<ChoiceEvidence>, BackendFailure> {
        assert_eq!(api_key, "test-only-key");
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.wire.lock().push(serde_json::to_string(text).unwrap());
        if self.hold_choice.load(Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.notified().await;
        }
        if self.fail.load(Ordering::SeqCst) {
            return Err(BackendFailure {
                request_hash: Some(digest_bytes(b"test-wire")),
                usage: None,
                reason: DecisionUnavailable::Timeout,
                attempted: true,
            });
        }
        let selected = self.selected.lock().clone();
        let mut probabilities = BTreeMap::from([
            ("c0".into(), 0.1),
            ("none".into(), 0.1),
            ("delegate".into(), 0.1),
        ]);
        probabilities.insert(selected.clone(), 0.8);
        Ok(BackendResponse {
            request_hash: digest_bytes(&serde_json::to_vec(text).unwrap()),
            value: ChoiceEvidence {
                selected,
                probabilities,
                confidence: 0.9,
            },
            observed_model: self.observed_model.lock().clone(),
            usage: *self.choice_usage.lock(),
        })
    }

    async fn assess_selected(
        &self,
        text: &DecisionText,
        selected: &str,
        api_key: &str,
    ) -> Result<BackendResponse<SuitabilityEvidence>, BackendFailure> {
        assert_eq!(api_key, "test-only-key");
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.wire.lock().push(serde_json::to_string(text).unwrap());
        Ok(BackendResponse {
            request_hash: digest_bytes(&serde_json::to_vec(text).unwrap()),
            value: SuitabilityEvidence {
                selected: if self.wrong_suitability.load(Ordering::SeqCst) {
                    "c1"
                } else {
                    selected
                }
                .into(),
                score: *self.suitability_score.lock(),
            },
            observed_model: JEV_MODEL.into(),
            usage: DecisionUsage {
                input_tokens: 110,
                output_tokens: 0,
            },
        })
    }
}

struct Fixture {
    base: PreparationFixture,
    guard: GuardedCandidateDecision,
    backend: Arc<Backend>,
    audit: Arc<Audit>,
}

fn fixture() -> Fixture {
    let mut base = preparation_fixture();
    let backend = Arc::new(Backend::default());
    let audit = Arc::new(Audit::default());
    let (mut guard, control) = GuardedCandidateDecision::new(
        backend.clone(),
        audit.clone(),
        base.secrets.clone(),
        base.preparer.privacy.clone(),
        AiAccessMode::ProviderApiKey,
    );
    guard.preparer.now = base.preparer.now.clone();
    control.bind(&base.request).unwrap();
    base.control = control;
    base.preparer.state = base.control.state.clone();
    Fixture {
        base,
        guard,
        backend,
        audit,
    }
}

fn approve(f: &Fixture) {
    f.base
        .control
        .approve_shadow(approval_at(*f.base.clock.lock()))
        .unwrap();
}

fn unavailable(result: &CandidateDecisionResult, reason: DecisionUnavailable) {
    assert!(
        matches!(&result.decision, CandidateDecision::Unavailable(actual) if *actual == reason),
        "{result:?}"
    );
}

fn selected(result: &CandidateDecisionResult) {
    let CandidateDecision::Selected {
        candidate_id,
        selected_probability,
        suitability,
    } = &result.decision
    else {
        panic!("expected a selected candidate, got {result:?}");
    };
    assert_eq!(candidate_id, "private-element-id");
    assert_eq!(*selected_probability, 0.8);
    assert_eq!(*suitability, 0.75);
}

fn reservation(f: &Fixture) -> u64 {
    approval_at(*f.base.clock.lock())
        .cost(DecisionUsage {
            input_tokens: 65_536,
            output_tokens: 65_536,
        })
        .unwrap()
}

#[tokio::test]
async fn candidate_runtime_pre_send_denials_never_reach_backend() {
    for (case, reason) in [
        (0, DecisionUnavailable::Off),
        (1, DecisionUnavailable::LocalOnly),
        (2, DecisionUnavailable::ConsentOrPolicyDenied),
        (3, DecisionUnavailable::AuditUnavailable),
        (4, DecisionUnavailable::BudgetExceeded),
        (5, DecisionUnavailable::CredentialUnavailable),
    ] {
        let mut f = fixture();
        let mut approval = approval_at(*f.base.clock.lock());
        match case {
            0 => {}
            1 => f.guard.preparer.mode = AiAccessMode::LocalModel,
            2 => f.base.consent.revoke_consent().unwrap(),
            3 => *f.audit.fail_at.lock() = Some(DecisionAuditPhase::BeforeSend),
            4 => approval.budget_microusd = 1,
            5 => *f.base.secrets.value.lock() = None,
            _ => unreachable!(),
        }
        if case != 0 {
            f.base.control.approve_shadow(approval).unwrap();
        }
        unavailable(&f.guard.decide(&f.base.request).await, reason);
        assert_eq!(f.backend.calls.load(Ordering::SeqCst), 0, "case {case}");
    }
}

#[tokio::test]
async fn candidate_runtime_selected_cache_and_metadata_audit_have_separate_attempts() {
    let f = fixture();
    let mut approval = approval_at(*f.base.clock.lock());
    approval.max_attempts = 2;
    f.base.control.approve_shadow(approval).unwrap();
    let first = f.guard.decide(&f.base.request).await;
    selected(&first);
    assert_eq!(first.attempts.len(), 2);
    assert_ne!(first.attempts[0].id, first.attempts[1].id);
    assert_eq!(first.attempts[0].stage, DecisionStage::Choice);
    assert_eq!(first.attempts[1].stage, DecisionStage::Suitability);
    assert_eq!(first.attempts[0].estimated_cost_microusd, Some(5));
    assert_eq!(first.attempts[1].estimated_cost_microusd, Some(5));
    assert_eq!(f.base.control.state.lock().remaining_budget, 99_990);
    assert_eq!(f.base.control.state.lock().remaining_attempts, 0);
    for wire in f.backend.wire.lock().iter() {
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(wire).unwrap(),
            serde_json::json!({
                "goal":"Select [EMAIL]", "candidates":[{"id":"c0", "text":"[EMAIL]",
                    "role":"[EMAIL]", "intent":"[EMAIL]", "state":"[EMAIL]"}]
            })
        );
    }
    let cached = f.guard.decide(&f.base.request).await;
    selected(&cached);
    assert_ne!(cached.decision_id, first.decision_id);
    assert_eq!(cached.cache_source, Some(first.decision_id));
    assert_eq!(cached.attempts.len(), 0);
    assert_eq!(f.backend.calls.load(Ordering::SeqCst), 2);
    {
        let records = f.audit.records.lock();
        assert_eq!(records.len(), 5);
        assert_eq!(records[0].phase, DecisionAuditPhase::BeforeSend);
        assert_eq!(records[1].phase, DecisionAuditPhase::AfterSend);
        assert_eq!(records[4].phase, DecisionAuditPhase::CacheHit);
        assert_eq!(records[4].cache_source, Some(first.decision_id));
        assert_eq!(records[4].decision_id, cached.decision_id);
        assert_eq!(
            records[4].context.namespace_hash,
            records[0].context.namespace_hash
        );
        assert_eq!(records[0].context.namespace_hash.len(), 64);
        assert_eq!(records[0].context.requested_model, JEV_MODEL);
        assert_eq!(records[0].context.schema_revision, DECISION_SCHEMA);
        assert_eq!(records[0].context.rubric_revision, DECISION_RUBRIC);
        let serialized = serde_json::to_string(&*records).unwrap();
        for forbidden in ["alice@example.com", "private-element-id", "test-only-key"] {
            assert!(!serialized.contains(forbidden), "{forbidden}");
        }
    }
    f.base.control.bind(&f.base.request).unwrap();
    unavailable(
        &f.guard.decide(&f.base.request).await,
        DecisionUnavailable::BudgetExceeded,
    );
    assert_eq!(f.backend.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn candidate_runtime_none_delegate_and_unknown_cost_are_distinct() {
    for selected in ["none", "delegate"] {
        let f = fixture();
        approve(&f);
        *f.backend.selected.lock() = selected.into();
        let result = f.guard.decide(&f.base.request).await;
        assert!(matches!(
            (&result.decision, selected),
            (CandidateDecision::None, "none") | (CandidateDecision::Delegate, "delegate")
        ));
        assert_eq!(result.attempts.len(), 1);
        assert_eq!(f.backend.calls.load(Ordering::SeqCst), 1);
    }
    let f = fixture();
    approve(&f);
    f.backend.fail.store(true, Ordering::SeqCst);
    let result = f.guard.decide(&f.base.request).await;
    unavailable(&result, DecisionUnavailable::Timeout);
    assert_eq!(result.attempts.len(), 1);
    assert_eq!(result.attempts[0].usage, None);
    assert_eq!(result.attempts[0].estimated_cost_microusd, None);
    assert!(result.attempts[0].attempted);
    assert_eq!(
        f.base.control.state.lock().remaining_budget,
        100_000 - reservation(&f)
    );
    assert_eq!(f.audit.records.lock().len(), 2);
}

#[tokio::test]
async fn candidate_runtime_post_http_changes_discard_response_before_selected_noul() {
    for (change, expected) in [
        (0, DecisionUnavailable::Cancelled),
        (1, DecisionUnavailable::Stale),
        (2, DecisionUnavailable::ConsentOrPolicyDenied),
        (3, DecisionUnavailable::ConsentOrPolicyDenied),
        (4, DecisionUnavailable::Expired),
        (5, DecisionUnavailable::Stale),
        (6, DecisionUnavailable::CredentialUnavailable),
    ] {
        let f = fixture();
        approve(&f);
        f.backend.hold_choice.store(true, Ordering::SeqCst);
        let mutation = async {
            f.backend.entered.notified().await;
            match change {
                0 => f.base.control.revoke(),
                1 => {
                    let mut snapshot = f.base.request.snapshot().clone();
                    snapshot.generation += 1;
                    let other = CandidateDecisionRequest::new(
                        snapshot,
                        f.base.request.binding().observed_at,
                        Duration::from_secs(10),
                    )
                    .unwrap();
                    f.base.control.bind(&other).unwrap();
                    f.base.control.bind(&f.base.request).unwrap();
                }
                2 => f.base.consent.revoke_consent().unwrap(),
                3 => {
                    f.base.consent.grant_consent(permissions(), 30).unwrap();
                }
                4 => *f.base.clock.lock() = f.base.request.binding().deadline,
                5 => f.base.monitor.window.lock().as_mut().unwrap().pid += 1,
                6 => *f.base.secrets.value.lock() = Some("rotated-test-key".into()),
                _ => unreachable!(),
            }
            f.backend.release.notify_one();
        };
        let (result, ()) = tokio::join!(f.guard.decide(&f.base.request), mutation);
        unavailable(&result, expected);
        assert_eq!(f.backend.calls.load(Ordering::SeqCst), 1, "change {change}");
        assert_eq!(result.attempts.len(), 1);
        assert_eq!(f.audit.records.lock().len(), 2);
    }
}

#[tokio::test]
async fn candidate_runtime_checks_after_durable_intent_and_refunds_only_durable_usage() {
    for rotate in [false, true] {
        let f = fixture();
        approve(&f);
        let control = f.base.control.clone();
        let secrets = f.base.secrets.clone();
        *f.audit.after_record.lock() = Some((
            DecisionAuditPhase::BeforeSend,
            Box::new(move || {
                if rotate {
                    *secrets.value.lock() = Some("rotated-test-key".into());
                } else {
                    control.revoke();
                }
            }),
        ));
        unavailable(
            &f.guard.decide(&f.base.request).await,
            if rotate {
                DecisionUnavailable::CredentialUnavailable
            } else {
                DecisionUnavailable::Cancelled
            },
        );
        assert_eq!(f.backend.calls.load(Ordering::SeqCst), 0);
        assert_eq!(f.audit.records.lock().len(), 1);
        assert_eq!(
            f.base.control.state.lock().remaining_budget,
            100_000 - reservation(&f)
        );
    }
    let f = fixture();
    approve(&f);
    *f.audit.fail_at.lock() = Some(DecisionAuditPhase::AfterSend);
    let result = f.guard.decide(&f.base.request).await;
    unavailable(&result, DecisionUnavailable::AuditUnavailable);
    assert_eq!(result.attempts[0].estimated_cost_microusd, Some(5));
    assert_eq!(
        f.base.control.state.lock().remaining_budget,
        100_000 - reservation(&f)
    );
    assert_eq!(f.audit.records.lock().len(), 1);
    assert_eq!(f.backend.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn candidate_runtime_cache_requires_live_credential_audit_and_original_expiry() {
    for (case, reason) in [
        (0, DecisionUnavailable::CredentialUnavailable),
        (1, DecisionUnavailable::AuditUnavailable),
        (2, DecisionUnavailable::Expired),
        (3, DecisionUnavailable::Cancelled),
        (4, DecisionUnavailable::CredentialUnavailable),
    ] {
        let f = fixture();
        approve(&f);
        let first = f.guard.decide(&f.base.request).await;
        assert!(matches!(first.decision, CandidateDecision::Selected { .. }));
        match case {
            0 => *f.base.secrets.value.lock() = None,
            1 => *f.audit.fail_at.lock() = Some(DecisionAuditPhase::CacheHit),
            2 => *f.base.clock.lock() = f.base.request.binding().deadline,
            3 => {
                let c = f.base.control.clone();
                *f.audit.after_record.lock() =
                    Some((DecisionAuditPhase::CacheHit, Box::new(move || c.revoke())));
            }
            4 => {
                let s = f.base.secrets.clone();
                *f.audit.after_record.lock() = Some((
                    DecisionAuditPhase::CacheHit,
                    Box::new(move || *s.value.lock() = Some("rotated-test-key".into())),
                ));
            }
            _ => unreachable!(),
        }
        unavailable(&f.guard.decide(&f.base.request).await, reason);
        assert_eq!(f.backend.calls.load(Ordering::SeqCst), 2);
    }
}

#[tokio::test]
async fn candidate_runtime_attempt_limit_and_suitability_alias_cannot_be_bypassed() {
    let f = fixture();
    let mut approval = approval_at(*f.base.clock.lock());
    approval.max_attempts = 1;
    f.base.control.approve_shadow(approval).unwrap();
    unavailable(
        &f.guard.decide(&f.base.request).await,
        DecisionUnavailable::BudgetExceeded,
    );
    assert_eq!(f.backend.calls.load(Ordering::SeqCst), 1);
    let f = fixture();
    approve(&f);
    f.backend.wrong_suitability.store(true, Ordering::SeqCst);
    unavailable(
        &f.guard.decide(&f.base.request).await,
        DecisionUnavailable::InvalidResponse,
    );
    assert_eq!(f.backend.calls.load(Ordering::SeqCst), 2);
    assert!(f.base.control.state.lock().cache.is_none());
}

#[tokio::test]
async fn candidate_runtime_budget_accepts_exact_reservation_and_rejects_one_less() {
    for exact in [false, true] {
        let f = fixture();
        let reserved = reservation(&f);
        let mut approval = approval_at(*f.base.clock.lock());
        approval.budget_microusd = if exact { reserved } else { reserved - 1 };
        f.base.control.approve_shadow(approval).unwrap();
        *f.backend.selected.lock() = "none".into();
        let result = f.guard.decide(&f.base.request).await;
        if exact {
            assert!(matches!(result.decision, CandidateDecision::None));
            assert_eq!(f.backend.calls.load(Ordering::SeqCst), 1);
            assert_eq!(f.base.control.state.lock().remaining_budget, reserved - 5);
            assert_eq!(f.base.control.state.lock().remaining_attempts, 9);
        } else {
            unavailable(&result, DecisionUnavailable::BudgetExceeded);
            assert_eq!(f.backend.calls.load(Ordering::SeqCst), 0);
            assert_eq!(f.base.control.state.lock().remaining_budget, reserved - 1);
            assert_eq!(f.base.control.state.lock().remaining_attempts, 10);
        }
    }
}

#[tokio::test]
async fn candidate_runtime_factory_starts_off_without_reading_credentials_or_window() {
    let base = preparation_fixture();
    let storage = Arc::new(maekon_storage::sqlite::SqliteStorage::open_in_memory(7).unwrap());
    let (port, control) = super::super::build_candidate_decision_runtime(
        base.preparer.privacy.clone(),
        storage,
        base.secrets.clone(),
        AiAccessMode::ProviderApiKey,
    )
    .unwrap();
    unavailable(&port.decide(&base.request).await, DecisionUnavailable::Off);
    assert_eq!(base.secrets.reads.load(Ordering::SeqCst), 0);
    assert_eq!(base.monitor.reads.load(Ordering::SeqCst), 0);
    assert!(control.state.lock().approval.is_none());
}

#[tokio::test]
async fn candidate_runtime_cancellation_and_deadline_keep_unresolved_intent_reserved() {
    for deadline in [false, true] {
        let mut f = fixture();
        approve(&f);
        if deadline {
            f.base.request = CandidateDecisionRequest::new(
                f.base.request.snapshot().clone(),
                Instant::now(),
                Duration::from_secs(1),
            )
            .unwrap();
            *f.base.clock.lock() = f.base.request.binding().observed_at;
            f.base.control.bind(&f.base.request).unwrap();
        }
        f.backend.hold_choice.store(true, Ordering::SeqCst);
        let mut evaluation = Box::pin(f.guard.decide(&f.base.request));
        tokio::select! {
            _ = &mut evaluation => panic!("held request unexpectedly returned"),
            () = f.backend.entered.notified() => {},
        }
        if deadline {
            unavailable(&evaluation.await, DecisionUnavailable::Expired);
        } else {
            drop(evaluation);
        }
        assert_eq!(f.backend.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            f.base.control.state.lock().remaining_budget,
            100_000 - reservation(&f)
        );
        let records = f.audit.records.lock();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].phase, DecisionAuditPhase::BeforeSend);
        assert!(records[0].attempt_id.is_some());
        assert!(records[0].attempt.is_none());
    }
}

#[test]
fn candidate_runtime_cache_namespace_binds_every_authority_component() {
    let f = fixture();
    let approval = approval_at(*f.base.clock.lock());
    let text = f.guard.preparer.sanitize(&f.base.request).unwrap();
    let key = |request: &CandidateDecisionRequest,
               epoch,
               revision: &str,
               text: &DecisionText,
               approval: &CandidateShadowApproval| {
        GuardedCandidateDecision::cache_key(request, epoch, revision, text, approval).unwrap()
    };
    let baseline = key(&f.base.request, 1, "revision-a", &text, &approval);
    assert_eq!(baseline.len(), 64);
    assert_ne!(
        baseline,
        key(&f.base.request, 2, "revision-a", &text, &approval)
    );
    assert_ne!(
        baseline,
        key(&f.base.request, 1, "revision-b", &text, &approval)
    );
    let mut changed_text = text.clone();
    changed_text.goal = "Other".into();
    assert_ne!(
        baseline,
        key(&f.base.request, 1, "revision-a", &changed_text, &approval)
    );
    let mut snapshot = f.base.request.snapshot().clone();
    snapshot.candidates[0].element.bbox_abs.x += 1;
    let changed = CandidateDecisionRequest::new(
        snapshot,
        f.base.request.binding().observed_at,
        Duration::from_secs(10),
    )
    .unwrap();
    assert_ne!(baseline, key(&changed, 1, "revision-a", &text, &approval));
    for field in 0..6 {
        let mut other = approval.clone();
        let fields = [
            &mut other.principal_namespace,
            &mut other.credential_profile,
            &mut other.credential_revision,
            &mut other.policy_revision,
            &mut other.price_reference,
            &mut other.approval_reference,
        ];
        fields.into_iter().nth(field).unwrap().push_str("-changed");
        assert_ne!(
            baseline,
            key(&f.base.request, 1, "revision-a", &text, &other),
            "field {field}"
        );
    }
}

#[tokio::test]
async fn candidate_runtime_model_and_reported_cost_are_independent_guards() {
    for (tokens, model, valid) in [
        (65_535, JEV_MODEL, true),
        (65_536, JEV_MODEL, true),
        (65_537, JEV_MODEL, false),
        (65_535, "wrong-model", false),
    ] {
        let f = fixture();
        let mut approval = approval_at(*f.base.clock.lock());
        approval.input_microusd_per_million = 1_000_000;
        approval.output_microusd_per_million = 1_000_000;
        approval.budget_microusd = 1_000_000;
        f.base.control.approve_shadow(approval).unwrap();
        *f.backend.selected.lock() = "none".into();
        *f.backend.choice_usage.lock() = DecisionUsage {
            input_tokens: tokens,
            output_tokens: 65_536,
        };
        *f.backend.observed_model.lock() = model.into();
        let result = f.guard.decide(&f.base.request).await;
        if valid {
            assert!(matches!(result.decision, CandidateDecision::None));
            assert_eq!(result.observed_model.as_deref(), Some(JEV_MODEL));
        } else {
            unavailable(&result, DecisionUnavailable::InvalidResponse);
            assert!(f.base.control.state.lock().cache.is_none());
        }
        assert_eq!(f.backend.calls.load(Ordering::SeqCst), 1);
        assert_eq!(f.audit.records.lock().len(), 2);
        assert_eq!(
            result.attempts[0].estimated_cost_microusd,
            Some(tokens + 65_536)
        );
        assert_eq!(
            f.base.control.state.lock().remaining_budget,
            if valid {
                1_000_000 - tokens - 65_536
            } else {
                868_928
            }
        );
    }
}

#[tokio::test]
async fn candidate_runtime_suitability_checks_finite_value_and_closed_range() {
    for (score, valid) in [
        (0.0, true),
        (1.0, true),
        (-f64::EPSILON, false),
        (1.0 + f64::EPSILON, false),
        (f64::NAN, false),
        (f64::INFINITY, false),
        (f64::NEG_INFINITY, false),
    ] {
        let f = fixture();
        approve(&f);
        *f.backend.suitability_score.lock() = score;
        let result = f.guard.decide(&f.base.request).await;
        if valid {
            let CandidateDecision::Selected {
                candidate_id,
                suitability,
                ..
            } = result.decision
            else {
                panic!("valid suitability rejected")
            };
            assert_eq!(candidate_id, "private-element-id");
            assert_eq!(suitability, score);
        } else {
            unavailable(&result, DecisionUnavailable::InvalidResponse);
            assert!(f.base.control.state.lock().cache.is_none());
        }
        assert_eq!(f.backend.calls.load(Ordering::SeqCst), 2);
        assert_eq!(result.attempts.len(), 2);
    }
}

#[test]
fn candidate_runtime_cache_publication_rechecks_each_state_under_lock() {
    let f = fixture();
    let mut result =
        CandidateDecisionResult::unavailable(f.base.request.binding(), DecisionUnavailable::Off);
    result.decision = CandidateDecision::None;
    for (epoch, revoked) in [(6, false), (5, true), (6, true)] {
        let mut state = State {
            epoch: 5,
            ..State::default()
        };
        state
            .cache_completed(5, "original".into(), &result)
            .unwrap();
        state.epoch = epoch;
        state.revoked = revoked;
        assert_eq!(
            state.cache_completed(5, "replacement".into(), &result),
            Err(DecisionUnavailable::Stale)
        );
        let (key, cached) = state.cache.unwrap();
        assert_eq!(key, "original");
        assert_eq!(cached.decision_id, result.decision_id);
    }
    let mut state = State {
        epoch: 5,
        ..State::default()
    };
    state.cache_completed(5, "valid".into(), &result).unwrap();
    let (key, cached) = state.cache.unwrap();
    assert_eq!(key, "valid");
    assert_eq!(cached.decision_id, result.decision_id);
}
