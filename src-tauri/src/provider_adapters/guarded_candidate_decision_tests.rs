use super::*;
use async_trait::async_trait;
use maekon_core::config::{ExternalDataPolicy, PiiFilterLevel, PrivacyConfig};
use maekon_core::consent::{ConsentManager, ConsentPermissions};
use maekon_core::error::CoreError;
use maekon_core::models::candidate_decision::CandidateSnapshot;
use maekon_core::models::context::{ProcessInfo, WindowInfo};
use maekon_core::models::event::ProcessDetail;
use maekon_core::models::gui::GuiCandidate;
use maekon_core::models::intent::ElementBounds;
use maekon_core::models::ui_scene::{NormalizedBounds, UiSceneElement};
use maekon_core::ports::monitor::ProcessMonitor;
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::TempDir;

pub(super) fn approval_at(now: Instant) -> CandidateShadowApproval {
    CandidateShadowApproval {
        approval_reference: "test-only-approval".into(),
        principal_namespace: "test-principal".into(),
        credential_profile: "test".into(),
        credential_revision: "test-key-revision".into(),
        policy_revision: "test-policy".into(),
        price_reference: "test-price".into(),
        input_microusd_per_million: 42_000,
        output_microusd_per_million: 5_000,
        budget_microusd: 100_000,
        max_attempts: 10,
        expires_at: now + Duration::from_secs(300),
    }
}

fn request_at(now: Instant, generation: u64) -> CandidateDecisionRequest {
    CandidateDecisionRequest::new(
        CandidateSnapshot {
            goal: "Save".into(),
            scene_id: "scene".into(),
            frame_id: "frame".into(),
            generation,
            window: WindowInfo {
                title: "Editor".into(),
                app_name: "Editor".into(),
                app_bundle_id: None,
                pid: 1,
                bounds: None,
            },
            candidates: vec![GuiCandidate {
                eligible: true,
                ranking_reason: None,
                element: UiSceneElement {
                    element_id: "private-element-id".into(),
                    label: "Save".into(),
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
                    confidence: 0.8,
                    text_masked: None,
                    parent_id: None,
                },
            }],
        },
        now,
        Duration::from_secs(10),
    )
    .unwrap()
}

#[test]
fn candidate_approval_enforces_each_reference_and_all_inclusive_bounds() {
    let now = Instant::now();
    let mut valid = approval_at(now);
    valid.approval_reference = "x".repeat(256);
    valid.max_attempts = 1000;
    valid.budget_microusd = 1;
    valid.expires_at = now + Duration::from_secs(3600);
    assert_eq!(valid.validate(now), Ok(()));
    for field in 0..6 {
        for invalid in [" ".to_owned(), "x".repeat(257)] {
            let mut value = approval_at(now);
            let fields = [
                &mut value.approval_reference,
                &mut value.principal_namespace,
                &mut value.credential_profile,
                &mut value.credential_revision,
                &mut value.policy_revision,
                &mut value.price_reference,
            ];
            *fields.into_iter().nth(field).unwrap() = invalid;
            assert_eq!(
                value.validate(now),
                Err(DecisionUnavailable::ApprovalMissing)
            );
        }
    }
    for mutate in [
        |a: &mut CandidateShadowApproval| a.budget_microusd = 0,
        |a: &mut CandidateShadowApproval| a.max_attempts = 0,
        |a: &mut CandidateShadowApproval| a.max_attempts = 1001,
        |a: &mut CandidateShadowApproval| a.credential_profile = "../other".into(),
    ] {
        let mut value = approval_at(now);
        mutate(&mut value);
        assert_eq!(
            value.validate(now),
            Err(DecisionUnavailable::ApprovalMissing)
        );
    }
    for expiry in [
        now - Duration::from_nanos(1),
        now,
        now + Duration::from_secs(3600) + Duration::from_nanos(1),
    ] {
        let mut value = approval_at(now);
        value.expires_at = expiry;
        assert_eq!(
            value.validate(now),
            Err(DecisionUnavailable::ApprovalMissing)
        );
    }
    let mut shortest = approval_at(now);
    shortest.max_attempts = 1;
    shortest.expires_at = now + Duration::from_nanos(1);
    assert_eq!(shortest.validate(now), Ok(()));
}

#[test]
fn candidate_cost_rounds_up_combined_usage_and_refuses_numeric_overflow() {
    let mut value = approval_at(Instant::now());
    assert_eq!(
        value.cost(DecisionUsage {
            input_tokens: 100,
            output_tokens: 200
        }),
        Some(6)
    );
    assert_eq!(
        value.cost(DecisionUsage {
            input_tokens: 0,
            output_tokens: 0
        }),
        Some(0)
    );
    assert_eq!(
        value.cost(DecisionUsage {
            input_tokens: 1000,
            output_tokens: 0
        }),
        Some(42)
    );
    assert_eq!(
        value.cost(DecisionUsage {
            input_tokens: 0,
            output_tokens: 200
        }),
        Some(1)
    );
    value.input_microusd_per_million = u64::MAX;
    value.output_microusd_per_million = u64::MAX;
    assert_eq!(
        value.cost(DecisionUsage {
            input_tokens: u64::MAX,
            output_tokens: u64::MAX
        }),
        None
    );
    assert_eq!(
        value.cost(DecisionUsage {
            input_tokens: u64::MAX,
            output_tokens: 0
        }),
        None
    );
}

#[test]
fn candidate_control_starts_off_and_approval_is_single_use() {
    let control = CandidateDecisionControl::default();
    assert_eq!(control.state.lock().epoch, 0);
    assert!(control.state.lock().approval.is_none());
    let mut invalid = approval_at(Instant::now());
    invalid.budget_microusd = 0;
    assert_eq!(
        control.approve_shadow(invalid),
        Err(DecisionUnavailable::ApprovalMissing)
    );
    assert_eq!(control.state.lock().epoch, 0);
    assert_eq!(control.approve_shadow(approval_at(Instant::now())), Ok(()));
    assert_eq!(control.state.lock().epoch, 1);
    assert_eq!(
        control
            .state
            .lock()
            .approval
            .as_ref()
            .unwrap()
            .approval_reference,
        "test-only-approval"
    );
    assert_eq!(
        control.approve_shadow(approval_at(Instant::now())),
        Err(DecisionUnavailable::Cancelled)
    );
    assert_eq!(control.state.lock().epoch, 1);
}

#[test]
fn candidate_binding_epoch_detects_aba_and_revocation_reaches_every_clone() {
    let control = CandidateDecisionControl::default();
    let clone = control.clone();
    let now = Instant::now();
    let first = request_at(now, 1);
    let second = request_at(now, 2);
    for (request, epoch) in [(&first, 1), (&second, 2), (&first, 3)] {
        assert_eq!(control.bind(request), Ok(()));
        let state = clone.state.lock();
        assert_eq!(state.epoch, epoch);
        assert_eq!(state.binding.as_ref(), Some(request.binding()));
    }
    clone.revoke();
    assert!(control.state.lock().revoked);
    assert_eq!(control.state.lock().epoch, 4);
    assert_eq!(control.bind(&first), Err(DecisionUnavailable::Cancelled));
    assert_eq!(
        control.approve_shadow(approval_at(now)),
        Err(DecisionUnavailable::Cancelled)
    );
    assert_eq!(control.state.lock().epoch, 4);
}

#[test]
fn candidate_epoch_exhaustion_permanently_revokes_without_accepting_new_authority() {
    for approve in [true, false] {
        let control = CandidateDecisionControl::default();
        control.state.lock().epoch = u64::MAX - 1;
        let now = Instant::now();
        let request = request_at(now, 1);
        assert_eq!(control.bind(&request), Ok(()));
        assert_eq!(control.state.lock().epoch, u64::MAX);
        let result = if approve {
            control.approve_shadow(approval_at(now))
        } else {
            control.bind(&request)
        };
        assert_eq!(result, Err(DecisionUnavailable::Cancelled));
        assert!(control.state.lock().revoked);
        assert_eq!(control.state.lock().epoch, u64::MAX);
        assert!(control.state.lock().approval.is_none());
        control.revoke();
        assert_eq!(control.state.lock().epoch, u64::MAX);
        assert_eq!(control.bind(&request), Err(DecisionUnavailable::Cancelled));
    }
}

type AfterRead = Box<dyn FnOnce() + Send>;

pub(super) struct Monitor {
    pub(super) window: Mutex<Option<WindowInfo>>,
    pub(super) reads: AtomicUsize,
    pub(super) after_read: Mutex<Option<AfterRead>>,
}

#[async_trait]
impl ProcessMonitor for Monitor {
    async fn get_active_window(&self) -> Result<Option<WindowInfo>, CoreError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let window = self.window.lock().clone();
        tokio::task::yield_now().await;
        let hook = self.after_read.lock().take();
        if let Some(hook) = hook {
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

pub(super) struct Secrets {
    pub(super) value: Mutex<Option<String>>,
    pub(super) reads: AtomicUsize,
    pub(super) after_read: Mutex<Option<AfterRead>>,
}

#[async_trait]
impl SecretStore for Secrets {
    async fn retrieve(&self, namespace: &str, key: &str) -> Result<Option<String>, CoreError> {
        assert_eq!((namespace, key), ("provider/typesafe/test", "api_key"));
        self.reads.fetch_add(1, Ordering::SeqCst);
        let value = self.value.lock().clone();
        tokio::task::yield_now().await;
        let hook = self.after_read.lock().take();
        if let Some(hook) = hook {
            hook();
        }
        Ok(value)
    }
    async fn store(&self, _namespace: &str, _key: &str, _value: &str) -> Result<(), CoreError> {
        unreachable!()
    }
    async fn delete(&self, _namespace: &str, _key: &str) -> Result<(), CoreError> {
        unreachable!()
    }
    async fn delete_namespace(&self, _namespace: &str) -> Result<(), CoreError> {
        unreachable!()
    }
}

pub(super) struct PreparationFixture {
    pub(super) preparer: CandidateDecisionPreparer,
    pub(super) control: CandidateDecisionControl,
    pub(super) consent: Arc<ConsentManager>,
    pub(super) monitor: Arc<Monitor>,
    pub(super) secrets: Arc<Secrets>,
    pub(super) clock: Arc<Mutex<Instant>>,
    pub(super) request: CandidateDecisionRequest,
    pub(super) _dir: TempDir,
}

pub(super) fn permissions() -> ConsentPermissions {
    ConsentPermissions {
        full_text_extraction: true,
        ..ConsentPermissions::default()
    }
}

pub(super) fn preparation_fixture() -> PreparationFixture {
    let now = Instant::now();
    let mut snapshot = request_at(now, 1).snapshot().clone();
    snapshot.goal = "Select alice@example.com".into();
    let element = &mut snapshot.candidates[0].element;
    element.label = "alice@example.com".into();
    element.role = Some("alice@example.com".into());
    element.intent = Some("alice@example.com".into());
    element.state = Some("alice@example.com".into());
    let request = CandidateDecisionRequest::new(snapshot, now, Duration::from_secs(10)).unwrap();
    let dir = TempDir::new().unwrap();
    let consent = Arc::new(ConsentManager::new(dir.path().join("consent.json")));
    consent.grant_consent(permissions(), 30).unwrap();
    let monitor = Arc::new(Monitor {
        window: Mutex::new(Some(request.snapshot().window.clone())),
        reads: AtomicUsize::new(0),
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
    let secrets = Arc::new(Secrets {
        value: Mutex::new(Some("test-only-key".into())),
        reads: AtomicUsize::new(0),
        after_read: Mutex::new(None),
    });
    let control = CandidateDecisionControl::default();
    control.bind(&request).unwrap();
    control.approve_shadow(approval_at(now)).unwrap();
    let mut preparer = CandidateDecisionPreparer::new(
        privacy,
        secrets.clone(),
        AiAccessMode::ProviderApiKey,
        &control,
    );
    let clock = Arc::new(Mutex::new(now));
    let reader = clock.clone();
    preparer.now = Arc::new(move || *reader.lock());
    PreparationFixture {
        preparer,
        control,
        consent,
        monitor,
        secrets,
        clock,
        request,
        _dir: dir,
    }
}

#[tokio::test]
async fn candidate_preparation_masks_every_field_and_returns_only_minimal_typed_data() {
    let f = preparation_fixture();
    let prepared = f
        .preparer
        .prepare(&f.request)
        .await
        .map_err(|e| format!("{e:?}"))
        .unwrap();
    assert_eq!(prepared.epoch, 2);
    assert_eq!(
        prepared.authority_revision,
        f.preparer.privacy.candidate_consent_revision().unwrap()
    );
    assert_eq!(prepared.authority_revision.len(), 64);
    assert_eq!(
        serde_json::to_value(prepared.text).unwrap(),
        serde_json::json!({
            "goal": "Select [EMAIL]",
            "candidates": [{"id": "c0", "text": "[EMAIL]", "role": "[EMAIL]",
                            "intent": "[EMAIL]", "state": "[EMAIL]"}]
        })
    );
    assert_eq!(f.secrets.reads.load(Ordering::SeqCst), 1);
    assert_eq!(f.monitor.reads.load(Ordering::SeqCst), 3);
    assert_eq!(
        f.request.snapshot().candidates[0].element.label,
        "alice@example.com"
    );
}

#[tokio::test]
async fn candidate_preparation_rejects_local_authority_failures_before_observation_or_secret() {
    for (case, expected) in [
        (0, DecisionUnavailable::Off),
        (1, DecisionUnavailable::LocalOnly),
        (2, DecisionUnavailable::Cancelled),
        (3, DecisionUnavailable::Stale),
        (4, DecisionUnavailable::Expired),
        (5, DecisionUnavailable::Expired),
        (6, DecisionUnavailable::ConsentOrPolicyDenied),
        (7, DecisionUnavailable::ConsentOrPolicyDenied),
        (8, DecisionUnavailable::ConsentOrPolicyDenied),
        (9, DecisionUnavailable::Expired),
    ] {
        let mut f = preparation_fixture();
        match case {
            0 => f.control.state.lock().approval = None,
            1 => f.preparer.mode = AiAccessMode::LocalModel,
            2 => f.control.revoke(),
            3 => f.control.bind(&request_at(*f.clock.lock(), 2)).unwrap(),
            4 => *f.clock.lock() = f.request.binding().observed_at - Duration::from_nanos(1),
            5 => *f.clock.lock() = f.request.binding().deadline,
            6 => {
                f.consent
                    .grant_consent(ConsentPermissions::default(), 30)
                    .unwrap();
            }
            7 => f.consent.revoke_consent().unwrap(),
            8 => f.consent.erasing().store(true, Ordering::Release),
            9 => f.control.state.lock().approval.as_mut().unwrap().expires_at = *f.clock.lock(),
            _ => unreachable!(),
        }
        assert_eq!(
            f.preparer.prepare(&f.request).await.map(|_| ()),
            Err(expected),
            "case {case}"
        );
        assert_eq!(f.secrets.reads.load(Ordering::SeqCst), 0, "case {case}");
        assert_eq!(f.monitor.reads.load(Ordering::SeqCst), 0, "case {case}");
    }
    let f = preparation_fixture();
    *f.clock.lock() = f.request.binding().deadline - Duration::from_nanos(1);
    assert_eq!(f.preparer.prepare(&f.request).await.map(|_| ()), Ok(()));
}

#[tokio::test]
async fn candidate_preparation_rejects_missing_or_first_mismatched_window_even_if_restored() {
    for missing in [false, true] {
        let f = preparation_fixture();
        if missing {
            *f.monitor.window.lock() = None;
        } else {
            f.monitor.window.lock().as_mut().unwrap().pid = 2;
            let monitor = f.monitor.clone();
            let original = f.request.snapshot().window.clone();
            *f.monitor.after_read.lock() =
                Some(Box::new(move || *monitor.window.lock() = Some(original)));
        }
        assert_eq!(
            f.preparer.prepare(&f.request).await.map(|_| ()),
            Err(if missing {
                DecisionUnavailable::ConsentOrPolicyDenied
            } else {
                DecisionUnavailable::Stale
            })
        );
        assert_eq!(f.monitor.reads.load(Ordering::SeqCst), 1);
        assert_eq!(f.secrets.reads.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn candidate_preparation_detects_revocation_and_snapshot_or_consent_aba_across_await() {
    for (case, expected) in [
        (0, DecisionUnavailable::Cancelled),
        (1, DecisionUnavailable::Stale),
        (2, DecisionUnavailable::ConsentOrPolicyDenied),
    ] {
        let f = preparation_fixture();
        let control = f.control.clone();
        let original = f.request.clone();
        let second = request_at(*f.clock.lock(), 2);
        let consent = f.consent.clone();
        *f.monitor.after_read.lock() = Some(Box::new(move || match case {
            0 => control.revoke(),
            1 => {
                control.bind(&second).unwrap();
                control.bind(&original).unwrap();
            }
            2 => {
                consent
                    .grant_consent(ConsentPermissions::default(), 30)
                    .unwrap();
                consent.grant_consent(permissions(), 30).unwrap();
            }
            _ => unreachable!(),
        }));
        assert_eq!(
            f.preparer.prepare(&f.request).await.map(|_| ()),
            Err(expected)
        );
        assert_eq!(f.secrets.reads.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn candidate_preparation_rejects_missing_oversized_and_rotated_credentials() {
    for value in [None, Some(String::new()), Some("x".repeat(4097))] {
        let f = preparation_fixture();
        *f.secrets.value.lock() = value;
        assert_eq!(
            f.preparer.prepare(&f.request).await.map(|_| ()),
            Err(DecisionUnavailable::CredentialUnavailable)
        );
        assert_eq!(f.secrets.reads.load(Ordering::SeqCst), 1);
        assert!(f.control.state.lock().credential_value.is_none());
    }
    let f = preparation_fixture();
    *f.secrets.value.lock() = Some("x".repeat(4096));
    assert_eq!(f.preparer.prepare(&f.request).await.map(|_| ()), Ok(()));
    *f.secrets.value.lock() = Some("rotated-test-key".into());
    assert_eq!(
        f.preparer.prepare(&f.request).await.map(|_| ()),
        Err(DecisionUnavailable::CredentialUnavailable)
    );
    assert!(f.control.state.lock().revoked);
    assert!(f.control.state.lock().credential_value.is_none());
    *f.secrets.value.lock() = Some("x".repeat(4096));
    assert_eq!(
        f.preparer.prepare(&f.request).await.map(|_| ()),
        Err(DecisionUnavailable::Cancelled)
    );
    assert_eq!(f.secrets.reads.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn candidate_preparation_rechecks_after_secret_await_and_cannot_repopulate_revoked_secret() {
    for (case, expected) in [
        (0, DecisionUnavailable::Cancelled),
        (1, DecisionUnavailable::Stale),
        (2, DecisionUnavailable::ConsentOrPolicyDenied),
        (3, DecisionUnavailable::Expired),
    ] {
        let f = preparation_fixture();
        let control = f.control.clone();
        let monitor = f.monitor.clone();
        let consent = f.consent.clone();
        let clock = f.clock.clone();
        let deadline = f.request.binding().deadline;
        *f.secrets.after_read.lock() = Some(Box::new(move || match case {
            0 => control.revoke(),
            1 => monitor.window.lock().as_mut().unwrap().pid = 2,
            2 => consent.revoke_consent().unwrap(),
            3 => *clock.lock() = deadline,
            _ => unreachable!(),
        }));
        assert_eq!(
            f.preparer.prepare(&f.request).await.map(|_| ()),
            Err(expected)
        );
        assert_eq!(f.secrets.reads.load(Ordering::SeqCst), 1);
        if case == 0 {
            assert!(f.control.state.lock().credential_value.is_none());
        }
    }
}
