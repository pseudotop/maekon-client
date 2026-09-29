use super::*;
use maekon_core::models::candidate_decision::DecisionOption;
use maekon_core::models::candidate_gateway::{GATEWAY_MAX_TOKENS, GATEWAY_MODEL};
use serde_json::{json, Value};

fn text() -> DecisionText {
    DecisionText {
        goal: "Save".into(),
        candidates: vec![DecisionOption {
            id: "c0".into(),
            text: "Save".into(),
            role: Some("button".into()),
            intent: None,
            state: None,
        }],
    }
}

fn body() -> Value {
    json!({
        "model": GATEWAY_MODEL,
        "answers": {"decision": {
            "type": "choice", "choice": "c0",
            "probabilities": {"c0": 0.8, "none": 0.1, "delegate": 0.1}
        }},
        "usage": {"input_tokens": 100, "output_tokens": 0},
        "provider_metadata": {"gateway": {
            "routing": {
                "originalModelId": GATEWAY_MODEL, "canonicalSlug": GATEWAY_MODEL,
                "resolvedProvider": "typesafe-ai", "finalProvider": "typesafe-ai"
            },
            "cost": "0.0000042", "marketCost": "0.0000042",
            "surchargeCost": "0", "gatewayCost": "0.0000042", "generationId": "gen_test"
        }}
    })
}

fn choice(wire: &Value) -> Result<(GatewayChoice, GatewayObservation), GatewayFailure> {
    decode_choice(&serde_json::to_vec(wire).unwrap(), &text())
}

fn assert_failure(failure: &GatewayFailure, observation: bool) {
    assert_eq!(failure.reason, DecisionUnavailable::InvalidResponse);
    assert!(failure.attempted);
    assert!(failure.request_hash.is_none());
    assert_eq!(failure.observation.is_some(), observation);
    if let Some(observed) = &failure.observation {
        assert_eq!(observed.usage.unwrap().input_tokens, 100);
        assert_eq!(observed.costs.cost.as_deref(), Some("0.0000042"));
        assert_eq!(observed.generation_hash, Some(digest_bytes(b"gen_test")));
    }
}

#[test]
fn gateway_choice_preserves_independent_confidence_and_exact_metadata() {
    let (answer, observed) = choice(&body()).unwrap();
    assert_eq!(answer.selected, "c0");
    assert_eq!(answer.confidence, None);
    assert_eq!(
        answer.probabilities,
        BTreeMap::from([
            ("c0".into(), 0.8),
            ("none".into(), 0.1),
            ("delegate".into(), 0.1)
        ])
    );
    assert_eq!(observed.usage.unwrap().input_tokens, 100);
    assert_eq!(observed.usage.unwrap().output_tokens, 0);
    assert_eq!(observed.costs.cost.as_deref(), Some("0.0000042"));
    assert_eq!(observed.costs.market_cost.as_deref(), Some("0.0000042"));
    assert_eq!(observed.costs.surcharge_cost.as_deref(), Some("0"));
    assert_eq!(observed.costs.gateway_cost.as_deref(), Some("0.0000042"));
    assert_eq!(observed.generation_hash, Some(digest_bytes(b"gen_test")));
    assert_eq!(observed.routing.response_model, GATEWAY_MODEL);
    assert_eq!(observed.routing.original_model_id, GATEWAY_MODEL);
    assert_eq!(observed.routing.canonical_slug, GATEWAY_MODEL);
    assert_eq!(observed.routing.resolved_provider, "typesafe-ai");
    assert_eq!(observed.routing.final_provider, "typesafe-ai");
    for confidence in [0.0, 0.4, 1.0] {
        let mut wire = body();
        wire["answers"]["decision"]["confidence"] = json!(confidence);
        assert_eq!(choice(&wire).unwrap().0.confidence, Some(confidence));
    }
    let mut wire = body();
    wire["answers"]["decision"]["confidence"] = Value::Null;
    assert_eq!(choice(&wire).unwrap().0.confidence, None);
}

#[test]
fn gateway_choice_accepts_abstention_and_tied_maxima_without_inventing_an_order() {
    for selected in [NONE_OPTION, DELEGATE_OPTION] {
        let mut wire = body();
        let probabilities = &mut wire["answers"]["decision"]["probabilities"];
        probabilities["c0"] = json!(0.1);
        probabilities[selected] = json!(0.8);
        wire["answers"]["decision"]["choice"] = json!(selected);
        assert_eq!(choice(&wire).unwrap().0.selected, selected);
    }
    let mut context = text();
    let mut other = context.candidates[0].clone();
    other.id = "c1".into();
    context.candidates.push(other);
    let mut wire = body();
    wire["answers"]["decision"]["probabilities"] =
        json!({"c0": 0.5, "c1": 0.5, "none": 0.0, "delegate": 0.0});
    for selected in ["c0", "c1"] {
        wire["answers"]["decision"]["choice"] = json!(selected);
        assert_eq!(
            decode_choice(wire.to_string().as_bytes(), &context)
                .unwrap()
                .0
                .selected,
            selected
        );
    }
}

#[test]
fn gateway_choice_checks_every_probability_boundary_and_exact_option_set() {
    for probabilities in [
        json!({"c0": 1.0, "none": 0.0, "delegate": 0.0}),
        json!({"c0": 0.8 - 0.0000005, "none": 0.1, "delegate": 0.1}),
        json!({"c0": 0.8 + 0.0000005, "none": 0.1, "delegate": 0.1}),
    ] {
        let mut wire = body();
        wire["answers"]["decision"]["probabilities"] = probabilities;
        choice(&wire).unwrap();
    }
    for probabilities in [
        json!({"c0": 0.8 - 0.000002, "none": 0.1, "delegate": 0.1}),
        json!({"c0": 0.8 + 0.000002, "none": 0.1, "delegate": 0.1}),
        json!({"c0": 1.1, "none": -0.1, "delegate": 0.0}),
        json!({"c0": 0.8, "none": 0.2}),
        json!({"c0": 0.8, "none": 0.1, "delegate": 0.1, "outside": 0.0}),
        json!({"c0": 0.0, "none": 0.0, "delegate": 0.0}),
        json!({"c0": 0.1, "none": 0.8, "delegate": 0.1}),
        json!({"c0": "0.8", "none": 0.1, "delegate": 0.1}),
    ] {
        let mut wire = body();
        wire["answers"]["decision"]["probabilities"] = probabilities;
        assert_failure(&choice(&wire).unwrap_err(), true);
    }
    for selected in ["none", "outside", "", "private-payload"] {
        let mut wire = body();
        wire["answers"]["decision"]["choice"] = json!(selected);
        let failure = choice(&wire).unwrap_err();
        assert_failure(&failure, true);
        assert!(!format!("{failure:?}").contains("private-payload"));
    }
    for confidence in [json!(-0.00001), json!(1.00001), json!("0.8"), json!(true)] {
        let mut wire = body();
        wire["answers"]["decision"]["confidence"] = confidence;
        assert_failure(&choice(&wire).unwrap_err(), true);
    }
}

#[test]
fn gateway_invalid_answers_keep_valid_usage_and_cost_in_the_failure() {
    for answers in [
        Value::Null,
        json!({}),
        json!([]),
        json!({"other": {"type": "noul", "noul": 0.8}}),
        json!({"decision": {"type": "noul", "noul": 0.8}}),
        json!({"decision": {"type": "score", "score": 1}}),
        json!({"decision": {"type": "choice", "choice": "c0"}}),
        json!({"decision": {"type": "choice", "choice": "c0", "probabilities": {"c0": 1.0}, "extra": "private-payload"}}),
    ] {
        let mut wire = body();
        wire["answers"] = answers;
        let failure = choice(&wire).unwrap_err();
        assert_failure(&failure, true);
        assert!(!format!("{failure:?}").contains("private-payload"));
    }
    let mut wire = body();
    wire.as_object_mut().unwrap().remove("answers");
    assert_failure(&choice(&wire).unwrap_err(), true);
    let mut wire = body();
    wire["answers"]["extra"] = json!({"type": "noul", "noul": 0.5});
    assert_failure(&choice(&wire).unwrap_err(), true);
}

#[test]
fn gateway_duplicate_keys_are_rejected_including_escaped_and_null_keys() {
    let original = body().to_string();
    for (needle, replacement, observed) in [
        (r#""c0":0.8"#, r#""c0":0.8,"c0":0.8"#, true),
        (r#""c0":0.8"#, r#""c0":0.8,"\u0063\u0030":0.8"#, true),
        (r#""choice":"c0""#, r#""choice":"c0","choice":"c0""#, true),
        (
            r#""type":"choice""#,
            r#""type":"choice","type":"choice""#,
            true,
        ),
        (
            r#""choice":"c0""#,
            r#""confidence":null,"confidence":null,"choice":"c0""#,
            true,
        ),
        (
            r#""input_tokens":100"#,
            r#""input_tokens":100,"input_tokens":100"#,
            false,
        ),
        (r#""cost":"0.0000042""#, r#""cost":null,"cost":null"#, false),
        (
            r#""generationId":"gen_test""#,
            r#""generationId":null,"generationId":null"#,
            false,
        ),
        (
            r#""resolvedProvider":"typesafe-ai""#,
            r#""resolvedProvider":"typesafe-ai","resolvedProvider":"typesafe-ai""#,
            false,
        ),
        (
            r#""usage":{"input_tokens":100,"output_tokens":0}"#,
            r#""usage":null,"usage":null"#,
            false,
        ),
    ] {
        assert!(original.contains(needle), "fixture has no {needle}");
        let wire = original.replacen(needle, replacement, 1);
        assert_failure(
            &decode_choice(wire.as_bytes(), &text()).unwrap_err(),
            observed,
        );
    }
    let answer = body()["answers"]["decision"].to_string();
    let mut wire = body();
    wire["answers"] = Value::Null;
    let wire = wire.to_string().replace(
        r#""answers":null"#,
        &format!(r#""answers":{{"decision":{answer},"decision":{answer}}}"#),
    );
    assert_failure(&decode_choice(wire.as_bytes(), &text()).unwrap_err(), true);
}

#[test]
fn gateway_unknown_envelope_fields_and_each_routing_identity_are_rejected() {
    for path in [
        "",
        "/usage",
        "/provider_metadata",
        "/provider_metadata/gateway",
        "/provider_metadata/gateway/routing",
    ] {
        let mut wire = body();
        wire.pointer_mut(path).unwrap()["extra"] = json!("private-payload");
        assert_failure(&choice(&wire).unwrap_err(), false);
    }
    for path in [
        "/model",
        "/provider_metadata/gateway/routing/originalModelId",
        "/provider_metadata/gateway/routing/canonicalSlug",
        "/provider_metadata/gateway/routing/resolvedProvider",
        "/provider_metadata/gateway/routing/finalProvider",
    ] {
        let mut wire = body();
        *wire.pointer_mut(path).unwrap() = json!("unexpected-provider-or-model");
        assert_failure(&choice(&wire).unwrap_err(), false);
    }
}

#[test]
fn gateway_usage_and_every_cost_field_are_typed_and_bounded() {
    for field in ["input_tokens", "output_tokens"] {
        for value in [0, GATEWAY_MAX_TOKENS] {
            let mut wire = body();
            wire["usage"][field] = json!(value);
            let usage = choice(&wire).unwrap().1.usage.unwrap();
            assert_eq!(
                if field == "input_tokens" {
                    usage.input_tokens
                } else {
                    usage.output_tokens
                },
                value
            );
        }
        for value in [
            json!(GATEWAY_MAX_TOKENS + 1),
            json!(u64::MAX),
            json!(-1),
            json!(1.5),
            json!("0"),
            Value::Null,
        ] {
            let mut wire = body();
            wire["usage"][field] = value;
            assert_failure(&choice(&wire).unwrap_err(), false);
        }
    }
    for field in ["cost", "marketCost", "surchargeCost", "gatewayCost"] {
        for value in [
            json!(0),
            json!("-0"),
            json!("1e-9"),
            json!("0.0000000000001"),
            json!("18446745"),
            json!("01"),
        ] {
            let mut wire = body();
            wire["provider_metadata"]["gateway"][field] = value;
            assert_failure(&choice(&wire).unwrap_err(), false);
        }
        let mut wire = body();
        wire["provider_metadata"]["gateway"][field] = json!("0.000000000001");
        choice(&wire).unwrap();
    }
}

#[test]
fn gateway_missing_observations_remain_unknown_and_generation_is_only_hashed() {
    for missing in [false, true] {
        let mut wire = body();
        if missing {
            wire.as_object_mut().unwrap().remove("usage");
        } else {
            wire["usage"] = Value::Null;
        }
        for field in [
            "cost",
            "marketCost",
            "surchargeCost",
            "gatewayCost",
            "generationId",
        ] {
            if missing {
                wire["provider_metadata"]["gateway"]
                    .as_object_mut()
                    .unwrap()
                    .remove(field);
            } else {
                wire["provider_metadata"]["gateway"][field] = Value::Null;
            }
        }
        let observation = choice(&wire).unwrap().1;
        assert!(observation.usage.is_none());
        assert!(observation.costs.cost.is_none());
        assert!(observation.costs.market_cost.is_none());
        assert!(observation.costs.surcharge_cost.is_none());
        assert!(observation.costs.gateway_cost.is_none());
        assert!(observation.generation_hash.is_none());
    }
    for id in ["a".repeat(255), "a".repeat(256), "gEN_0123-:._".into()] {
        let mut wire = body();
        wire["provider_metadata"]["gateway"]["generationId"] = json!(id);
        assert_eq!(
            choice(&wire).unwrap().1.generation_hash,
            Some(digest_bytes(id.as_bytes()))
        );
    }
    for id in [
        "a".repeat(257),
        String::new(),
        "한".into(),
        "a/b".into(),
        "a b".into(),
        "a\n".into(),
        "a\u{85}".into(),
    ] {
        let mut wire = body();
        wire["provider_metadata"]["gateway"]["generationId"] = json!(id);
        assert_failure(&choice(&wire).unwrap_err(), false);
    }
}

#[test]
fn gateway_response_byte_limit_and_complete_json_are_enforced() {
    let original = body().to_string();
    for length in [
        MAX_GATEWAY_CANDIDATE_RESPONSE_BYTES - 1,
        MAX_GATEWAY_CANDIDATE_RESPONSE_BYTES,
    ] {
        let mut bytes = original.as_bytes().to_vec();
        bytes.resize(length, b' ');
        decode_choice(&bytes, &text()).unwrap();
    }
    let mut too_large = original.as_bytes().to_vec();
    too_large.resize(MAX_GATEWAY_CANDIDATE_RESPONSE_BYTES + 1, b' ');
    let failure = decode_choice(&too_large, &text()).unwrap_err();
    assert_eq!(failure.reason, DecisionUnavailable::ResponseTooLarge);
    assert!(failure.attempted);
    assert!(failure.observation.is_none());
    for wire in [
        String::new(),
        "{}".into(),
        "null".into(),
        format!("{original} {{}}"),
        original[..original.len() - 1].into(),
        format!("```json\n{original}\n```"),
        format!("private-payload {original}"),
    ] {
        let failure = decode_choice(wire.as_bytes(), &text()).unwrap_err();
        assert_failure(&failure, false);
        assert!(!format!("{failure:?}").contains("private-payload"));
    }
    assert_failure(
        &decode_choice(&[0xff, b'{', b'}'], &text()).unwrap_err(),
        false,
    );
    for number in ["NaN", "Infinity", "1e999"] {
        let wire = original.replace(r#""c0":0.8"#, &format!(r#""c0":{number}"#));
        let failure = decode_choice(wire.as_bytes(), &text()).unwrap_err();
        assert_eq!(failure.reason, DecisionUnavailable::InvalidResponse);
        assert!(failure.attempted);
    }
    let deep = format!("{}0{}", "[".repeat(200), "]".repeat(200));
    let mut wire = body();
    wire["answers"] = Value::Null;
    let wire = wire
        .to_string()
        .replace(r#""answers":null"#, &format!(r#""answers":{deep}"#));
    assert_eq!(
        decode_choice(wire.as_bytes(), &text()).unwrap_err().reason,
        DecisionUnavailable::InvalidResponse
    );
}

#[test]
fn gateway_suitability_binds_one_candidate_and_preserves_score_boundaries() {
    for score in [0.0, 0.65, 1.0] {
        let mut wire = body();
        wire["answers"]["decision"] = json!({"type": "noul", "noul": score});
        let (answer, observed) =
            decode_suitability(wire.to_string().as_bytes(), &text(), "c0").unwrap();
        assert_eq!(answer.selected, "c0");
        assert_eq!(answer.score, score);
        assert_eq!(observed.usage.unwrap().input_tokens, 100);
        for selected in ["outside", "none", "delegate", ""] {
            let failure =
                decode_suitability(wire.to_string().as_bytes(), &text(), selected).unwrap_err();
            assert_eq!(failure.reason, DecisionUnavailable::InvalidInput);
            assert!(failure.attempted);
        }
    }
    for answer in [
        json!({"type": "noul", "noul": -0.00001}),
        json!({"type": "noul", "noul": 1.00001}),
        json!({"type": "noul", "noul": true}),
        json!({"type": "noul", "noul": 0.8, "extra": null}),
        body()["answers"]["decision"].clone(),
    ] {
        let mut wire = body();
        wire["answers"]["decision"] = answer;
        assert_failure(
            &decode_suitability(wire.to_string().as_bytes(), &text(), "c0").unwrap_err(),
            true,
        );
    }
}

#[test]
fn gateway_suitability_preserves_a_nonfirst_selected_candidate() {
    let mut context = text();
    let mut second = context.candidates[0].clone();
    second.id = "c1".into();
    context.candidates.push(second);
    let mut wire = body();
    wire["answers"]["decision"] = json!({"type": "noul", "noul": 0.75});
    let (answer, _) = decode_suitability(wire.to_string().as_bytes(), &context, "c1").unwrap();
    assert_eq!(answer.selected, "c1");
    assert_eq!(answer.score, 0.75);
    wire["answers"]["decision"]["selected"] = json!("c0");
    assert_failure(
        &decode_suitability(wire.to_string().as_bytes(), &context, "c1").unwrap_err(),
        true,
    );
}

#[test]
fn gateway_decoding_never_marks_an_attempted_response_as_unsent() {
    let wire = body().to_string();
    let mut invalid_context = text();
    invalid_context.candidates.clear();
    for failure in [
        decode_choice(wire.as_bytes(), &invalid_context).unwrap_err(),
        decode_suitability(wire.as_bytes(), &invalid_context, "c0").unwrap_err(),
    ] {
        assert_eq!(failure.reason, DecisionUnavailable::InvalidInput);
        assert!(failure.attempted);
        assert!(failure.request_hash.is_none());
    }
}
