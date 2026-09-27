use super::*;
use maekon_core::models::candidate_decision::DecisionOption;
use std::time::Duration;

fn approval() -> LocalModelApproval {
    LocalModelApproval {
        daemon_reference: "test-only-daemon".into(),
        configuration_revision: "cloud-disabled".into(),
        endpoint_origin: "http://127.0.0.1:11434".into(),
        model: "fixture:fixed".into(),
        model_digest: format!("sha256:{}", "a".repeat(64)),
        expires_at: std::time::Instant::now() + Duration::from_secs(60),
    }
}
fn text() -> DecisionText {
    DecisionText {
        goal: "Save".into(),
        candidates: vec![DecisionOption {
            id: "c0".into(),
            text: "Save".into(),
            role: None,
            intent: None,
            state: None,
        }],
    }
}
fn answer(content: &str) -> serde_json::Value {
    serde_json::json!({
        "model":"fixture:fixed", "done":true,
        "message":{"role":"assistant","content":content},
        "prompt_eval_count":14,"eval_count":3,
    })
}
#[test]
fn local_candidate_response_strict_output_preserves_meaning_and_unknown_usage() {
    for (selection, expected) in [
        ("c0", LocalAssessmentChoice::Selected("c0".into())),
        ("none", LocalAssessmentChoice::None),
        ("delegate", LocalAssessmentChoice::Delegate),
    ] {
        let mut wire = answer(&serde_json::json!({"selected":selection}).to_string());
        let decoded =
            decode_response(&serde_json::to_vec(&wire).unwrap(), &text(), &approval()).unwrap();
        assert_eq!(decoded.0, expected);
        assert_eq!(
            decoded.1,
            Some(DecisionUsage {
                input_tokens: 14,
                output_tokens: 3
            })
        );
        wire.as_object_mut().unwrap().remove("eval_count");
        assert_eq!(
            decode_response(&serde_json::to_vec(&wire).unwrap(), &text(), &approval())
                .unwrap()
                .1,
            None
        );
    }
    for content in [
        r#"{"selected":"c1"}"#,
        r#"{"selected":"private-element-id"}"#,
        r#"{"selected":"c0","selected":"none"}"#,
        r#"{"selected":"c0","tool":"click"}"#,
        r#"{"selected":"c0"} {}"#,
        r#"{selected:"c0"}"#,
        "c0",
        r#"{"selected":NaN}"#,
    ] {
        let error = decode_response(
            &serde_json::to_vec(&answer(content)).unwrap(),
            &text(),
            &approval(),
        )
        .unwrap_err();
        assert_eq!(
            error.reason,
            DecisionUnavailable::InvalidResponse,
            "{content}"
        );
        assert!(error.attempted);
        assert_eq!(
            error.usage,
            Some(DecisionUsage {
                input_tokens: 14,
                output_tokens: 3
            })
        );
    }
}

#[test]
fn local_candidate_response_rejects_remote_model_tools_partial_and_duplicate_metadata() {
    for (field, value) in [
        ("model", serde_json::json!("other:latest")),
        ("done", serde_json::json!(false)),
        ("remote_host", serde_json::json!("https://cloud.invalid")),
        ("remote_model", serde_json::json!("cloud-model")),
        ("error", serde_json::json!("provider-canary")),
        ("eval_count", serde_json::json!(65_537)),
        (
            "message",
            serde_json::json!({"role":"assistant","content":r#"{"selected":"c0"}"#,"tool_calls":[{"function":{"name":"click"}}]}),
        ),
    ] {
        let mut wire = answer(r#"{"selected":"c0"}"#);
        wire[field] = value;
        assert_eq!(
            decode_response(&serde_json::to_vec(&wire).unwrap(), &text(), &approval())
                .unwrap_err()
                .reason,
            DecisionUnavailable::InvalidResponse,
            "{field}"
        );
    }
    let duplicate = br#"{"model":"fixture:fixed","model":"other","done":true,"message":{"role":"assistant","content":"{\"selected\":\"c0\"}"}}"#;
    assert_eq!(
        decode_response(duplicate, &text(), &approval())
            .unwrap_err()
            .reason,
        DecisionUnavailable::InvalidResponse
    );
}

#[test]
fn local_candidate_response_enforces_byte_token_and_context_boundaries() {
    let valid = answer(r#"{"selected":"c0"}"#);
    let mut bytes = serde_json::to_vec(&valid).unwrap();
    bytes.resize(MAX_LOCAL_CANDIDATE_RESPONSE_BYTES, b' ');
    let decoded = decode_response(&bytes, &text(), &approval()).unwrap();
    assert_eq!(
        decoded,
        (
            LocalAssessmentChoice::Selected("c0".into()),
            Some(DecisionUsage {
                input_tokens: 14,
                output_tokens: 3,
            })
        )
    );
    bytes.push(b' ');
    let error = decode_response(&bytes, &text(), &approval()).unwrap_err();
    assert_eq!(error.reason, DecisionUnavailable::ResponseTooLarge);
    assert!(error.attempted);
    assert_eq!(error.usage, None);
    for count in [0, MAX_TOKENS] {
        let mut wire = valid.clone();
        wire["prompt_eval_count"] = count.into();
        wire["eval_count"] = count.into();
        assert_eq!(
            decode_response(&serde_json::to_vec(&wire).unwrap(), &text(), &approval())
                .unwrap()
                .1,
            Some(DecisionUsage {
                input_tokens: count,
                output_tokens: count
            })
        );
    }
    for field in ["prompt_eval_count", "eval_count"] {
        let mut wire = valid.clone();
        wire[field] = (MAX_TOKENS + 1).into();
        let error =
            decode_response(&serde_json::to_vec(&wire).unwrap(), &text(), &approval()).unwrap_err();
        assert_eq!(error.reason, DecisionUnavailable::InvalidResponse);
        assert!(error.attempted);
        assert_eq!(error.usage, None);
        wire.as_object_mut().unwrap().remove(field);
        assert_eq!(
            decode_response(&serde_json::to_vec(&wire).unwrap(), &text(), &approval())
                .unwrap()
                .1,
            None
        );
    }
    let mut invalid_text = text();
    invalid_text.candidates[0].id = "private-candidate-id".into();
    assert_eq!(
        decode_response(
            &serde_json::to_vec(&valid).unwrap(),
            &invalid_text,
            &approval()
        )
        .unwrap_err()
        .reason,
        DecisionUnavailable::InvalidInput
    );
}

#[test]
fn local_candidate_response_rejects_non_assistant_output_and_invalid_token_types() {
    let mut wire = answer(r#"{"selected":"c0"}"#);
    wire["message"]["role"] = "user".into();
    let error =
        decode_response(&serde_json::to_vec(&wire).unwrap(), &text(), &approval()).unwrap_err();
    assert_eq!(error.reason, DecisionUnavailable::InvalidResponse);
    assert_eq!(
        error.usage,
        Some(DecisionUsage {
            input_tokens: 14,
            output_tokens: 3
        })
    );
    for field in ["prompt_eval_count", "eval_count"] {
        for invalid in [
            serde_json::json!(-1),
            serde_json::json!("3"),
            serde_json::json!(false),
        ] {
            let mut wire = answer(r#"{"selected":"c0"}"#);
            wire[field] = invalid;
            let error = decode_response(&serde_json::to_vec(&wire).unwrap(), &text(), &approval())
                .unwrap_err();
            assert_eq!(error.reason, DecisionUnavailable::InvalidResponse);
            assert_eq!(error.usage, None);
        }
    }
    let mut wire = answer(r#"{"selected":"delegate"}"#);
    wire["message"]["tool_calls"] = serde_json::json!([]);
    assert_eq!(
        decode_response(&serde_json::to_vec(&wire).unwrap(), &text(), &approval())
            .unwrap()
            .0,
        LocalAssessmentChoice::Delegate
    );
}
