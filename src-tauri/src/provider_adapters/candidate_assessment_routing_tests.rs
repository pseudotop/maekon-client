use super::*;
use maekon_core::models::candidate_decision::CandidateSnapshot;
use maekon_core::models::context::WindowInfo;
use maekon_core::models::gui::GuiCandidate;
use maekon_core::models::intent::ElementBounds;
use maekon_core::models::ui_scene::{NormalizedBounds, UiSceneElement};

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

use maekon_core::models::candidate_assessment::{
    AssessmentAttempt, AssessmentEvidence, AssessmentOutcome,
};
use maekon_core::models::candidate_decision::{
    CandidateDecision, CandidateDecisionResult, ChoiceEvidence, DecisionAttempt, DecisionStage,
    DecisionUsage,
};
use maekon_core::models::candidate_decision_policy::CandidateCostPolicy;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use uuid::Uuid;

#[test]
fn candidate_routing_keeps_off_local_and_explicit_paid_routes_separate() {
    use CandidateAssessmentRoute as Route;
    use DecisionUnavailable as Denial;
    let modes = [
        AiAccessMode::LocalModel,
        AiAccessMode::ProviderSubscriptionCli,
        AiAccessMode::ProviderApiKey,
        AiAccessMode::ProviderOAuth,
    ];
    let local_model = [
        if cfg!(feature = "analysis") {
            Ok(Route::LocalModel)
        } else {
            Err(Denial::Rejected)
        },
        Err(Denial::LocalOnly),
        Err(Denial::LocalOnly),
        Err(Denial::LocalOnly),
    ];
    let paid_jev = if cfg!(feature = "analysis") {
        [
            Err(Denial::LocalOnly),
            Err(Denial::LocalOnly),
            Ok(Route::JevDirect),
            Err(Denial::LocalOnly),
        ]
    } else {
        [Err(Denial::Rejected); 4]
    };
    let unpaid_jev = if cfg!(feature = "analysis") {
        [
            Err(Denial::LocalOnly),
            Err(Denial::LocalOnly),
            Err(Denial::Rejected),
            Err(Denial::LocalOnly),
        ]
    } else {
        [Err(Denial::Rejected); 4]
    };
    let free = CandidateCostPolicy::NoAdditionalApiCost;
    let paid = CandidateCostPolicy::ExplicitPaidApiAllowed;
    for (provider, cost, expected) in [
        (None, free, [Err(Denial::Off); 4]),
        (None, paid, [Err(Denial::Off); 4]),
        (
            Some(CandidateProvider::LocalRules),
            free,
            [Ok(Route::LocalRules); 4],
        ),
        (
            Some(CandidateProvider::LocalRules),
            paid,
            [Ok(Route::LocalRules); 4],
        ),
        (Some(CandidateProvider::LocalModel), free, local_model),
        (Some(CandidateProvider::LocalModel), paid, local_model),
        (Some(CandidateProvider::JevDirect), free, unpaid_jev),
        (Some(CandidateProvider::JevDirect), paid, paid_jev),
        (
            Some(CandidateProvider::CodexCli),
            free,
            [Err(Denial::Rejected); 4],
        ),
        (
            Some(CandidateProvider::ClaudeCli),
            free,
            [Err(Denial::Rejected); 4],
        ),
        (
            Some(CandidateProvider::JevGateway),
            free,
            [Err(Denial::Rejected); 4],
        ),
        (
            Some(CandidateProvider::CodexCli),
            paid,
            [Err(Denial::Rejected); 4],
        ),
        (
            Some(CandidateProvider::ClaudeCli),
            paid,
            [Err(Denial::Rejected); 4],
        ),
        (
            Some(CandidateProvider::JevGateway),
            paid,
            [Err(Denial::Rejected); 4],
        ),
    ] {
        for (mode, expected) in modes.into_iter().zip(expected) {
            assert_eq!(
                candidate_assessment_route(CandidateDecisionPolicy { provider, cost }, mode),
                expected,
                "{provider:?} {mode:?} {cost:?}"
            );
        }
    }
}

#[tokio::test]
async fn candidate_routing_unavailable_port_preserves_binding_without_observations() {
    let request = request("Save", &["Save"]);
    for reason in [
        DecisionUnavailable::Off,
        DecisionUnavailable::LocalOnly,
        DecisionUnavailable::Rejected,
        DecisionUnavailable::CredentialUnavailable,
        DecisionUnavailable::Transport,
    ] {
        let provider = UnavailableCandidateAssessment::new(reason);
        let result = provider.assess(&request).await;
        assert_eq!(result.outcome, AssessmentOutcome::Unavailable(reason));
        assert_eq!(&result.binding, request.binding());
        assert!(result.provenance.is_none());
        assert!(matches!(result.evidence, AssessmentEvidence::NotObserved));
        assert!(result.attempts.is_empty());
        assert_eq!(result.cache_source, None);
    }
}

struct Legacy {
    calls: AtomicUsize,
    result: CandidateDecisionResult,
}
#[async_trait]
impl CandidateDecisionPort for Legacy {
    async fn decide(&self, request: &CandidateDecisionRequest) -> CandidateDecisionResult {
        assert_eq!(request.binding(), &self.result.binding);
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.result.clone()
    }
}

#[tokio::test]
async fn candidate_routing_jev_port_forwards_original_attempts_and_evidence_once() {
    let request = request("Save", &["Save"]);
    let mut original =
        CandidateDecisionResult::unavailable(request.binding(), DecisionUnavailable::Off);
    original.decision = CandidateDecision::Selected {
        candidate_id: "private-id-0".into(),
        selected_probability: 0.7,
        suitability: 0.8,
    };
    original.sanitized_state_hash = Some("sanitized".into());
    original.choice = Some(ChoiceEvidence {
        selected: "c0".into(),
        probabilities: BTreeMap::from([
            ("c0".into(), 0.7),
            ("none".into(), 0.2),
            ("delegate".into(), 0.1),
        ]),
        confidence: 0.9,
    });
    original.attempts.push(DecisionAttempt {
        request_hash: Some("wire".into()),
        id: Uuid::new_v4(),
        stage: DecisionStage::Choice,
        attempted: true,
        elapsed_ms: 13,
        usage: Some(DecisionUsage {
            input_tokens: 35,
            output_tokens: 9,
        }),
        estimated_cost_microusd: Some(8),
        failure: None,
    });
    original.cache_source = Some(Uuid::new_v4());
    let legacy = Arc::new(Legacy {
        calls: AtomicUsize::new(0),
        result: original.clone(),
    });
    let provider = JevCandidateAssessment::new(legacy.clone());
    let result = provider.assess(&request).await;
    assert_eq!(legacy.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        result.outcome,
        AssessmentOutcome::Selected {
            candidate_id: "private-id-0".into()
        }
    );
    assert_eq!(result.binding, original.binding);
    assert_eq!(result.decision_id, original.decision_id);
    assert_eq!(result.cache_source, original.cache_source);
    assert_eq!(result.sanitized_state_hash, original.sanitized_state_hash);
    assert_eq!(
        result.provenance.as_ref().unwrap().provider,
        CandidateProvider::JevDirect
    );
    let AssessmentEvidence::JevDistribution {
        choice,
        selected_probability,
        suitability,
        ..
    } = result.evidence
    else {
        panic!("missing original Jev evidence");
    };
    assert_eq!(
        choice.unwrap().probabilities,
        original.choice.unwrap().probabilities
    );
    assert_eq!(selected_probability, Some(0.7));
    assert_eq!(suitability, Some(0.8));
    assert_eq!(result.attempts.len(), 1);
    let AssessmentAttempt::JevV1(attempt) = &result.attempts[0] else {
        panic!("lost legacy attempt");
    };
    assert_eq!(
        serde_json::to_value(attempt).unwrap(),
        serde_json::to_value(&original.attempts[0]).unwrap()
    );
}
