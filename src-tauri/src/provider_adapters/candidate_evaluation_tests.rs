//! #12423: execute production candidate generation and advisory runtimes on frozen synthetic input.

use super::{
    build_candidate_assessment_runtime, AssessmentRuntimeControl, CandidateAssessmentRuntime,
    ExternalOcrPrivacyGuard, LocalAssessmentApproval,
};
use maekon_core::models::candidate_assessment::{
    AssessmentEvidence, AssessmentOutcome, CandidateAssessmentResult,
};
use maekon_core::models::candidate_decision::digest_bytes;
use serde_json::{json, Value};
use std::io::Write;
use std::sync::atomic::Ordering;
use std::time::Instant;

#[path = "candidate_evaluation_fixtures.rs"]
mod fixtures;
use fixtures::{Candidate, Corpus, Input};

const CORPUS_HASH: &str = "2f9063ae0a968e81d76106e6872247767072ee8386a2ad779cdee77e52327ee6";

fn observation(result: CandidateAssessmentResult, elapsed_ns: u64, audit_added: u64) -> Value {
    let (outcome, selected_id, reason) = match result.outcome {
        AssessmentOutcome::Selected { candidate_id } => ("selected", Some(candidate_id), None),
        AssessmentOutcome::None => ("none", None, None),
        AssessmentOutcome::Delegate => ("delegate", None, None),
        AssessmentOutcome::Unavailable(reason) => ("unavailable", None, Some(reason)),
    };
    let (kind, matches) = match result.evidence {
        AssessmentEvidence::LocalHeuristic { exact_matches } => {
            ("local_heuristic", Some(exact_matches))
        }
        AssessmentEvidence::NotObserved => ("not_observed", None),
        _ => panic!("external model evidence is forbidden in this collector"),
    };
    json!({"status":"observed", "outcome":outcome, "selected_id":selected_id, "reason":reason,
        "confidence":null, "evidence_kind":kind, "exact_matches":matches, "provenance":result.provenance,
        "sanitized_state_hash":result.sanitized_state_hash, "attempts":result.attempts,
        "cache_source":result.cache_source, "decision_id":result.decision_id,
        "elapsed_ns":elapsed_ns, "audit_events_added":audit_added})
}

fn elapsed(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

#[tokio::test]
#[ignore = "opt-in collection requires MAEKON_GUI_EVAL_CORPUS and MAEKON_GUI_EVAL_OUTPUT"]
async fn jev_gui_offline_runtime_collector() {
    // Explicit runtime input keeps exported client test builds independent of internal docs.
    let input = std::env::var_os("MAEKON_GUI_EVAL_CORPUS").expect("explicit corpus path required");
    let corpus_bytes = std::fs::read(input).unwrap();
    assert_eq!(digest_bytes(&corpus_bytes), CORPUS_HASH);
    let corpus: Corpus = serde_json::from_slice(&corpus_bytes).unwrap();
    assert_eq!(corpus.samples.len(), 60);
    let output = std::env::var_os("MAEKON_GUI_EVAL_OUTPUT").expect("explicit output path required");
    let repetitions = 3;
    let mut rows = Vec::new();
    for sample in &corpus.samples {
        for limited in [false, true] {
            let config = if limited {
                "cutoff_0_9_cap_2"
            } else {
                "production_default"
            };
            for repetition in 0..repetitions {
                let started = Instant::now();
                let generated = fixtures::generate(&sample.input, limited).await;
                let generation_elapsed = elapsed(started);
                let (candidates, error) = match generated {
                    Ok(value) => (value, None),
                    Err(code) => (vec![], Some(code)),
                };
                let ids: Vec<_> = candidates
                    .iter()
                    .map(|c| c.element.element_id.clone())
                    .collect();
                let fixture =
                    (!candidates.is_empty()).then(|| fixtures::runtime(sample, candidates));
                for phase in ["cold", "repeat"] {
                    let (rules, off) = if let Some(f) = &fixture {
                        let before = f.storage.verify_audit_chain().verified_count;
                        let started = Instant::now();
                        let result = f.rules.provider.assess(&f.request).await;
                        let took = elapsed(started);
                        let chain = f.storage.verify_audit_chain();
                        assert!(chain.ok);
                        if sample.permission_state != "granted" || sample.freshness != "current" {
                            assert!(matches!(result.outcome, AssessmentOutcome::Unavailable(_)));
                            assert!(result.attempts.is_empty());
                        }
                        let rules = observation(result, took, chain.verified_count - before);
                        let before_off = chain.verified_count;
                        let started = Instant::now();
                        let result = f.off.provider.assess(&f.request).await;
                        let off_elapsed = elapsed(started);
                        let off_chain = f.storage.verify_audit_chain();
                        assert!(off_chain.ok);
                        assert_eq!(off_chain.verified_count, before_off);
                        assert!(matches!(result.outcome, AssessmentOutcome::Unavailable(_)));
                        assert!(result.attempts.is_empty());
                        assert_eq!(f.secrets.0.load(Ordering::SeqCst), 0);
                        (
                            rules,
                            observation(result, off_elapsed, off_chain.verified_count - before_off),
                        )
                    } else {
                        let missing = json!({"status":"not_run", "reason":"candidate_generation_unavailable"});
                        (missing.clone(), missing)
                    };
                    rows.push(json!({"sample_id":sample.sample_id, "family_id":sample.family_id,
                        "input_sha256":sample.input_sha256, "configuration":config, "repetition":repetition,
                        "phase":phase, "shortlist_ids":ids, "generation_error":error,
                        "generation_elapsed_ns":if phase == "cold" { Some(generation_elapsed) } else { None },
                        "local_rules":rules, "off":off}));
                }
            }
        }
    }
    let artifact = json!({"schema_version":"maekon.gui-runtime-observations.v1",
        "corpus_bytes_sha256":CORPUS_HASH, "repetitions":repetitions,
        "input_source":"synthetic_fixture_ports", "external_inference_calls":0,
        "secret_queries":0, "os_actions":0, "rows":rows});
    let bytes = serde_json::to_vec_pretty(&artifact).unwrap();
    assert!(bytes.len() <= 8 * 1024 * 1024);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .unwrap();
    file.write_all(&bytes).unwrap();
    file.sync_all().unwrap();
}

#[tokio::test]
async fn candidate_evaluation_observes_production_cutoff_stable_ties_and_truncation() {
    let input = Input {
        goal: "x".into(),
        candidates: (0..25)
            .map(|i| Candidate {
                id: format!("candidate-{i}"),
                label: "x".into(),
                recognition_confidence: if i == 0 { 0.49 } else { 0.9 },
            })
            .collect(),
    };
    let selected = fixtures::generate(&input, false).await.unwrap();
    assert_eq!(selected.len(), 20);
    assert_eq!(selected.first().unwrap().element.element_id, "candidate-1");
    assert_eq!(selected.last().unwrap().element.element_id, "candidate-20");
    let limited = fixtures::generate(&input, true).await.unwrap();
    assert_eq!(limited.len(), 2);
    assert_eq!(limited[1].element.element_id, "candidate-2");
    let mut reversed = input;
    reversed.candidates.reverse();
    assert_eq!(
        fixtures::generate(&reversed, true).await.unwrap()[0]
            .element
            .element_id,
        "candidate-24"
    );
    for candidate in &mut reversed.candidates {
        candidate.recognition_confidence = 0.49;
    }
    assert_eq!(
        fixtures::generate(&reversed, false).await.unwrap_err(),
        "gui.bad_request"
    );
}

#[tokio::test]
async fn candidate_evaluation_positive_choice_cache_and_ambiguous_control() {
    let mut sample = fixtures::Sample {
        sample_id: "positive-control".into(),
        family_id: "positive-control".into(),
        input_sha256: String::new(),
        permission_state: "granted".into(),
        freshness: "current".into(),
        input: Input {
            goal: "Save".into(),
            candidates: vec![Candidate {
                id: "save".into(),
                label: "Save".into(),
                recognition_confidence: 0.95,
            }],
        },
    };
    let candidates = fixtures::generate(&sample.input, false).await.unwrap();
    let f = fixtures::runtime(&sample, candidates);
    let first = f.rules.provider.assess(&f.request).await;
    assert!(
        matches!(first.outcome, AssessmentOutcome::Selected { ref candidate_id } if candidate_id == "save")
    );
    assert!(matches!(
        first.evidence,
        AssessmentEvidence::LocalHeuristic { exact_matches: 1 }
    ));
    assert_eq!(first.attempts.len(), 1);
    assert!(first.cache_source.is_none());
    let cached = f.rules.provider.assess(&f.request).await;
    assert_eq!(cached.outcome, first.outcome);
    assert_eq!(cached.cache_source, Some(first.decision_id));
    assert!(cached.attempts.is_empty());
    assert_ne!(cached.decision_id, first.decision_id);
    assert!(f.storage.verify_audit_chain().ok);
    assert_eq!(f.secrets.0.load(Ordering::SeqCst), 0);

    sample.input.candidates.push(Candidate {
        id: "save-duplicate".into(),
        label: "Save".into(),
        recognition_confidence: 0.95,
    });
    let candidates = fixtures::generate(&sample.input, false).await.unwrap();
    let ambiguous = fixtures::runtime(&sample, candidates);
    let result = ambiguous.rules.provider.assess(&ambiguous.request).await;
    assert_eq!(result.outcome, AssessmentOutcome::Delegate);
    assert!(matches!(
        result.evidence,
        AssessmentEvidence::LocalHeuristic { exact_matches: 2 }
    ));
}
