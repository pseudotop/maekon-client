use super::*;
use crate::models::candidate_decision::{DecisionStage, DECISION_RUBRIC, JEV_MODEL};
use std::collections::BTreeMap;
use std::time::Duration;

fn binding() -> DecisionBinding {
    let now = Instant::now();
    DecisionBinding {
        digest: "a".repeat(64),
        window_digest: "b".repeat(64),
        observed_at: now,
        deadline: now + Duration::from_secs(5),
    }
}

#[test]
fn assessment_jev_conversion_preserves_all_original_observations() {
    let attempts: Vec<_> = [DecisionStage::Choice, DecisionStage::Suitability]
        .into_iter()
        .enumerate()
        .map(|(index, stage)| DecisionAttempt {
            id: Uuid::new_v4(),
            request_hash: Some(format!("request-{index}")),
            stage,
            attempted: true,
            elapsed_ms: 13 + index as u64,
            usage: (index == 0).then_some(DecisionUsage {
                input_tokens: 35,
                output_tokens: 9,
            }),
            estimated_cost_microusd: (index == 0).then_some(8),
            failure: None,
        })
        .collect();
    let id = Uuid::new_v4();
    let source = Some(Uuid::new_v4());
    let original_binding = binding();
    let distribution = BTreeMap::from([
        ("c0".into(), 0.7),
        ("none".into(), 0.2),
        ("delegate".into(), 0.1),
    ]);
    let result = CandidateAssessmentResult::from_jev(CandidateDecisionResult {
        decision: CandidateDecision::Selected {
            candidate_id: "private-id".into(),
            selected_probability: 0.7,
            suitability: 0.8,
        },
        binding: original_binding.clone(),
        provider: "typesafe",
        rubric_revision: DECISION_RUBRIC,
        sanitized_state_hash: Some("c".repeat(64)),
        requested_model: JEV_MODEL,
        observed_model: Some(JEV_MODEL.into()),
        confidence_kind: ConfidenceKind::ChoiceDistribution,
        choice: Some(ChoiceEvidence {
            selected: "c0".into(),
            probabilities: distribution.clone(),
            confidence: 0.9,
        }),
        attempts: attempts.clone(),
        cache_source: source,
        decision_id: id,
    });
    assert_eq!(
        result.outcome,
        AssessmentOutcome::Selected {
            candidate_id: "private-id".into()
        }
    );
    assert_eq!(result.binding, original_binding);
    assert_eq!(result.decision_id, id);
    assert_eq!(result.cache_source, source);
    assert_eq!(
        result.sanitized_state_hash.as_deref(),
        Some("c".repeat(64).as_str())
    );
    let provenance = result.provenance.unwrap();
    assert_eq!(provenance.provider, CandidateProvider::JevDirect);
    assert_eq!(provenance.transport, AssessmentTransport::DirectHttps);
    assert_eq!(provenance.inference_provider, "typesafe");
    assert_eq!(provenance.requested_model.as_deref(), Some(JEV_MODEL));
    assert_eq!(provenance.observed_model.as_deref(), Some(JEV_MODEL));
    assert_eq!(provenance.model_revision, None);
    assert_eq!(provenance.rubric_revision, DECISION_RUBRIC);
    let AssessmentEvidence::JevDistribution {
        choice,
        confidence_kind,
        selected_probability,
        suitability,
    } = result.evidence
    else {
        panic!("missing Jev evidence");
    };
    let choice = choice.unwrap();
    assert_eq!(choice.selected, "c0");
    assert_eq!(choice.probabilities, distribution);
    assert_eq!(choice.confidence, 0.9);
    assert_eq!(confidence_kind, ConfidenceKind::ChoiceDistribution);
    assert_eq!(selected_probability, Some(0.7));
    assert_eq!(suitability, Some(0.8));
    for (mapped, original) in result.attempts.iter().zip(attempts) {
        let AssessmentAttempt::JevV1(mapped) = mapped else {
            panic!("lost v1 attempt");
        };
        assert_eq!(
            serde_json::to_value(mapped).unwrap(),
            serde_json::to_value(original).unwrap()
        );
    }
}

#[test]
fn assessment_unavailable_and_local_results_have_no_fabricated_provider_or_probability() {
    let result = CandidateAssessmentResult::unavailable(&binding(), DecisionUnavailable::Off);
    assert!(result.provenance.is_none());
    assert!(matches!(result.evidence, AssessmentEvidence::NotObserved));
    assert!(result.attempts.is_empty());
    let local = AssessmentProvenance::local_rules();
    assert_eq!(local.requested_model, None);
    assert_eq!(local.observed_model, None);
    assert_eq!(local.inference_provider, "maekon");
    assert_eq!(local.transport, AssessmentTransport::InProcess);
}

#[test]
fn assessment_jev_non_selection_reasons_and_partial_attempts_are_preserved() {
    for (decision, expected) in [
        (CandidateDecision::None, AssessmentOutcome::None),
        (CandidateDecision::Delegate, AssessmentOutcome::Delegate),
        (
            CandidateDecision::Unavailable(DecisionUnavailable::Timeout),
            AssessmentOutcome::Unavailable(DecisionUnavailable::Timeout),
        ),
    ] {
        let mut original =
            CandidateDecisionResult::unavailable(&binding(), DecisionUnavailable::Timeout);
        original.decision = decision;
        let converted = CandidateAssessmentResult::from_jev(original);
        assert_eq!(converted.outcome, expected);
        assert!(matches!(
            converted.evidence,
            AssessmentEvidence::JevDistribution {
                selected_probability: None,
                suitability: None,
                ..
            }
        ));
    }
}

#[test]
fn assessment_local_model_approval_requires_bounded_trusted_identity() {
    let now = Instant::now();
    let valid = LocalModelApproval {
        daemon_reference: "approved-local-daemon".into(),
        configuration_revision: "cloud-disabled-config-v1".into(),
        endpoint_origin: "http://127.0.0.1:11434".into(),
        model: "local-model:fixed".into(),
        model_digest: format!("sha256:{}", "a".repeat(64)),
        expires_at: now + Duration::from_secs(60),
    };
    assert_eq!(valid.validate(now), Ok(()));
    for bad in [
        LocalModelApproval {
            daemon_reference: String::new(),
            ..valid.clone()
        },
        LocalModelApproval {
            configuration_revision: String::new(),
            ..valid.clone()
        },
        LocalModelApproval {
            model_digest: "unknown".into(),
            ..valid.clone()
        },
        LocalModelApproval {
            model: "model\ncanary".into(),
            ..valid.clone()
        },
        LocalModelApproval {
            expires_at: now,
            ..valid.clone()
        },
        LocalModelApproval {
            expires_at: now + Duration::from_secs(3601),
            ..valid
        },
    ] {
        assert_eq!(bad.validate(now), Err(DecisionUnavailable::ApprovalMissing));
    }
}

#[test]
fn assessment_debug_redacts_candidate_identifiers() {
    assert_eq!(
        format!(
            "{:?}",
            AssessmentOutcome::Selected {
                candidate_id: "private-canary".into()
            }
        ),
        "Selected { .. }"
    );
    assert_eq!(format!("{:?}", AssessmentOutcome::None), "None");
    assert_eq!(format!("{:?}", AssessmentOutcome::Delegate), "Delegate");
    assert_eq!(
        format!(
            "{:?}",
            AssessmentOutcome::Unavailable(DecisionUnavailable::Off)
        ),
        "Unavailable(Off)"
    );
}

#[test]
fn assessment_model_approval_validates_exact_boundaries_and_digest_alphabet() {
    let now = Instant::now();
    let valid = LocalModelApproval {
        daemon_reference: "d".repeat(256),
        configuration_revision: "c".repeat(256),
        endpoint_origin: "e".repeat(256),
        model: "m".repeat(256),
        model_digest: format!("sha256:{}", "0123456789abcdef".repeat(4)),
        expires_at: now + Duration::from_secs(3600),
    };
    assert_eq!(valid.validate(now), Ok(()));
    for field in 0..4 {
        for value in [" ".into(), "x".repeat(257)] {
            let mut invalid = valid.clone();
            match field {
                0 => invalid.daemon_reference = value,
                1 => invalid.configuration_revision = value,
                2 => invalid.endpoint_origin = value,
                _ => invalid.model = value,
            }
            assert_eq!(
                invalid.validate(now),
                Err(DecisionUnavailable::ApprovalMissing)
            );
        }
    }
    for digest in [
        format!("sha256:{}", "a".repeat(63)),
        format!("sha256:{}", "a".repeat(65)),
        format!("sha256:{}", "g".repeat(64)),
        format!("sha256:{}", "A".repeat(64)),
        format!("sha512:{}", "a".repeat(64)),
    ] {
        let invalid = LocalModelApproval {
            model_digest: digest,
            ..valid.clone()
        };
        assert_eq!(
            invalid.validate(now),
            Err(DecisionUnavailable::ApprovalMissing)
        );
    }
    let expired = LocalModelApproval {
        expires_at: now - Duration::from_secs(1),
        ..valid
    };
    assert_eq!(
        expired.validate(now),
        Err(DecisionUnavailable::ApprovalMissing)
    );
}
