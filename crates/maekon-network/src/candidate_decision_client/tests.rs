use super::*;
use maekon_core::models::candidate_decision::DecisionOption;
use mockito::{Matcher, Server};
use serde_json::{json, Value};

fn text() -> DecisionText {
    DecisionText {
        goal: "Save changes".into(),
        candidates: vec![DecisionOption {
            id: "c0".into(),
            text: "Save".into(),
            role: Some("button".into()),
            intent: None,
            state: Some("enabled".into()),
        }],
    }
}

fn choice() -> Value {
    json!({"model":JEV_MODEL,"answers":{"decision":{"type":"choice","choice":"c0","probabilities":{"c0":0.8,"none":0.1,"delegate":0.1},"confidence":0.9}},"usage":{"input_tokens":100,"output_tokens":0}})
}

fn noul(score: f64) -> Value {
    json!({"model":JEV_MODEL,"answers":{"decision":{"type":"noul","noul":score}},"usage":{"input_tokens":100,"output_tokens":0}})
}

#[test]
fn choice_evidence_preserves_model_usage_and_distinct_confidence() {
    let decoded = decode_choice(&serde_json::to_vec(&choice()).unwrap(), &text()).unwrap();
    assert_eq!(decoded.value.selected, "c0");
    assert_eq!(decoded.value.probabilities["c0"], 0.8);
    assert_eq!(decoded.value.confidence, 0.9);
    assert_eq!(decoded.observed_model, "jev-1.13.0");
    assert_eq!(
        decoded.usage,
        DecisionUsage {
            input_tokens: 100,
            output_tokens: 0
        }
    );
    for selected in ["none", "delegate"] {
        let mut body = choice();
        body["answers"][QUESTION]["choice"] = json!(selected);
        body["answers"][QUESTION]["probabilities"]["c0"] = json!(0.1);
        body["answers"][QUESTION]["probabilities"][selected] = json!(0.8);
        let decoded = decode_choice(&serde_json::to_vec(&body).unwrap(), &text()).unwrap();
        assert_eq!(decoded.value.selected, selected);
        assert_eq!(decoded.value.probabilities[selected], 0.8);
    }
}

#[test]
fn structural_errors_discard_untrusted_usage_and_raw_text() {
    let mut bodies = Vec::new();
    for (field, value) in [
        ("model", json!("private unexpected model")),
        ("usage", json!({"input_tokens":-1,"output_tokens":0})),
        ("usage", json!({"input_tokens":1.1,"output_tokens":0})),
        ("usage", json!({"input_tokens":100})),
        ("usage", json!({"input_tokens":65_537,"output_tokens":0})),
        ("usage", json!({"input_tokens":0,"output_tokens":65_537})),
        ("answers", json!({})),
        ("answers", json!({"outside":{"type":"noul","noul":0.5}})),
    ] {
        let mut body = choice();
        body[field] = value;
        bodies.push(body.to_string());
    }
    let mut body = choice();
    body["answers"]["extra"] = body["answers"][QUESTION].clone();
    bodies.push(body.to_string());
    let mut body = choice();
    body["extra"] = json!("private raw text");
    bodies.push(body.to_string());
    bodies.extend([
        choice()
            .to_string()
            .replace("\"c0\":0.8", "\"c0\":0.8,\"c0\":0.8"),
        choice().to_string().replace(
            "\"input_tokens\":100",
            "\"input_tokens\":100,\"input_tokens\":100",
        ),
        choice()
            .to_string()
            .replace("\"confidence\":0.9", "\"confidence\":NaN"),
        format!("{} {{}}", choice()),
    ]);
    for body in bodies {
        let error = decode_choice(body.as_bytes(), &text()).unwrap_err();
        assert_eq!(error, invalid(None));
        assert!(!format!("{error:?}").contains("private"));
    }
}

#[test]
fn invalid_choice_semantics_preserve_only_validated_usage() {
    let mut bodies = Vec::new();
    for (field, value) in [
        ("choice", json!("outside")),
        ("choice", json!("none")),
        ("confidence", json!(1.1)),
        ("confidence", json!(-0.1)),
        ("probabilities", json!({"c0":0.9,"none":0.1})),
        ("probabilities", json!({"c0":0.8,"none":0.1,"delegate":0.2})),
        ("probabilities", json!({"c0":0.8,"none":0.1,"delegate":0.0})),
        (
            "probabilities",
            json!({"c0":1.1,"none":-0.1,"delegate":0.0}),
        ),
        (
            "probabilities",
            json!({"c0":0.8,"none":0.1,"delegate":0.1,"other":0.0}),
        ),
    ] {
        let mut body = choice();
        body["answers"][QUESTION][field] = value;
        bodies.push(body);
    }
    bodies.push(noul(0.5));
    for body in bodies {
        let error = decode_choice(&serde_json::to_vec(&body).unwrap(), &text()).unwrap_err();
        assert_eq!(
            error,
            invalid(Some(DecisionUsage {
                input_tokens: 100,
                output_tokens: 0
            }))
        );
    }
}

#[test]
fn probability_sum_accepts_both_endpoints_and_rejects_adjacent_outside_values() {
    let lower = 1.0_f64 - 1e-6;
    let upper = 1.0_f64 + 1e-6;
    for sum in [lower, upper] {
        let mut body = choice();
        body["answers"][QUESTION]["probabilities"] =
            json!({"c0":0.75,"none":sum - 0.75,"delegate":0.0});
        let decoded = decode_choice(&serde_json::to_vec(&body).unwrap(), &text()).unwrap();
        assert_eq!(decoded.value.selected, "c0");
        assert_eq!(decoded.value.probabilities.values().sum::<f64>(), sum);
    }
    for sum in [lower.next_down(), upper.next_up()] {
        let mut body = choice();
        body["answers"][QUESTION]["probabilities"] =
            json!({"c0":0.75,"none":sum - 0.75,"delegate":0.0});
        assert_eq!(
            decode_choice(&serde_json::to_vec(&body).unwrap(), &text()).unwrap_err(),
            invalid(Some(DecisionUsage {
                input_tokens: 100,
                output_tokens: 0
            }))
        );
    }
}

#[test]
fn noul_is_a_separate_score_with_inclusive_value_and_usage_bounds() {
    for score in [0.0, 0.7, 1.0] {
        let mut body = noul(score);
        body["usage"] = json!({"input_tokens":65_536,"output_tokens":65_536});
        let decoded =
            decode_suitability(&serde_json::to_vec(&body).unwrap(), &text(), "c0").unwrap();
        assert_eq!(decoded.value.score, score);
        assert_eq!(decoded.value.selected, "c0");
        assert_eq!(decoded.observed_model, JEV_MODEL);
        assert_eq!(
            decoded.usage,
            DecisionUsage {
                input_tokens: 65_536,
                output_tokens: 65_536
            }
        );
    }
    for body in [noul(-0.1), noul(1.1), choice()] {
        assert_eq!(
            decode_suitability(&serde_json::to_vec(&body).unwrap(), &text(), "c0").unwrap_err(),
            invalid(Some(DecisionUsage {
                input_tokens: 100,
                output_tokens: 0
            }))
        );
    }
}

#[test]
fn duplicate_questions_and_extra_noul_fields_are_rejected() {
    let raw = format!("{{\"model\":\"{JEV_MODEL}\",\"answers\":{{\"decision\":{{\"type\":\"noul\",\"noul\":0.5}},\"decision\":{{\"type\":\"noul\",\"noul\":0.9}}}},\"usage\":{{\"input_tokens\":1,\"output_tokens\":0}}}}");
    assert_eq!(
        decode_suitability(raw.as_bytes(), &text(), "c0").unwrap_err(),
        invalid(None)
    );
    let valid = raw.replace(",\"decision\":{\"type\":\"noul\",\"noul\":0.9}", "");
    let decoded = decode_suitability(valid.as_bytes(), &text(), "c0").unwrap();
    assert_eq!(decoded.value.score, 0.5);
    assert_eq!(decoded.value.selected, "c0");
    assert_eq!(decoded.usage.input_tokens, 1);
    let extra = valid.replace("\"noul\":0.5", "\"noul\":0.5,\"confidence\":0.9");
    assert_eq!(
        decode_suitability(extra.as_bytes(), &text(), "c0").unwrap_err(),
        invalid(None)
    );

    let error = serde_json::from_str::<UniqueMap<u64>>("[]")
        .map(|_| ())
        .unwrap_err();
    assert!(error.is_data());
    assert!(error.to_string().contains("an object with unique keys"));
    let error = serde_json::from_str::<UniqueMap<u64>>("{\"a\":1,\"a\":2}")
        .map(|_| ())
        .unwrap_err();
    assert!(error.to_string().contains("duplicate key"));
}

#[test]
fn exact_response_byte_limit_is_accepted_and_one_more_byte_is_rejected() {
    let mut bytes = serde_json::to_vec(&choice()).unwrap();
    bytes.resize(MAX_RESPONSE_BYTES, b' ');
    assert_eq!(decode_choice(&bytes, &text()).unwrap().value.selected, "c0");
    bytes.push(b' ');
    assert_eq!(
        decode_choice(&bytes, &text()).unwrap_err(),
        CandidateDecodeFailure {
            reason: DecisionUnavailable::ResponseTooLarge,
            usage: None,
        }
    );
}

fn client(server: &Server) -> JevCandidateDecisionClient {
    JevCandidateDecisionClient {
        client: hardened_client_builder(TransportPolicy::AllowLoopbackCleartext)
            .retry(reqwest::retry::never())
            .timeout(Duration::from_millis(100))
            .build()
            .unwrap(),
        endpoint: server.url(),
    }
}

#[tokio::test]
async fn transport_binds_each_minimal_pinned_request_to_its_actual_bytes() {
    let mut server = Server::new_async().await;
    let c = client(&server);
    let observed = std::sync::Arc::new(std::sync::Mutex::new(None));
    let capture = observed.clone();
    let choice_mock = server.mock("POST", "/")
        .match_header("authorization", "Bearer test-only-key")
        .match_header("content-type", "application/json")
        .match_body(Matcher::Json(json!({"model":JEV_MODEL,"state":text(),"questions":{"decision":{"type":"choice","instructions":RUBRIC,"criteria":{
            "c0":"Candidate c0 in state.candidates best satisfies state.goal.","none":"No supplied candidate satisfies the goal.","delegate":"The evidence is insufficient; request human judgment."}}}})))
        .with_status(200)
        .with_body_from_request(move |request| {
            *capture.lock().unwrap() = Some(digest_bytes(request.body().unwrap()));
            choice().to_string().into_bytes()
        })
        .expect(1).create_async().await;
    let selected = c.choose(&text(), "test-only-key").await.unwrap();
    assert_eq!(selected.value.selected, "c0");
    assert_eq!(selected.value.probabilities["c0"], 0.8);
    assert_eq!(selected.observed_model, JEV_MODEL);
    assert_eq!(selected.usage.input_tokens, 100);
    assert_eq!(Some(selected.request_hash), *observed.lock().unwrap());
    choice_mock.assert_async().await;
    choice_mock.remove_async().await;

    let capture = observed.clone();
    let noul_mock = server.mock("POST", "/")
        .match_header("authorization", "Bearer test-only-key")
        .match_body(Matcher::Json(json!({"model":JEV_MODEL,"state":text(),"questions":{"decision":{"type":"noul",
            "instructions":format!("{RUBRIC} Assess only candidate c0 in state.candidates against state.goal. Do not assess whether some other candidate could satisfy the goal."),
            "criteria":{"true":"This selected candidate satisfies the stated goal.","false":"This selected candidate does not satisfy the stated goal."}}}})))
        .with_status(200)
        .with_body_from_request(move |request| {
            *capture.lock().unwrap() = Some(digest_bytes(request.body().unwrap()));
            noul(0.7).to_string().into_bytes()
        })
        .expect(1).create_async().await;
    let suitability = c
        .assess_selected(&text(), "c0", "test-only-key")
        .await
        .unwrap();
    assert_eq!(suitability.value.selected, "c0");
    assert_eq!(suitability.value.score, 0.7);
    assert_eq!(suitability.observed_model, JEV_MODEL);
    assert_eq!(suitability.usage.input_tokens, 100);
    assert_eq!(Some(suitability.request_hash), *observed.lock().unwrap());
    noul_mock.assert_async().await;
}

#[tokio::test]
async fn http_failures_never_retry_follow_redirects_or_return_private_bodies() {
    let mut server = Server::new_async().await;
    let redirected = server
        .mock("POST", "/redirected")
        .expect(0)
        .create_async()
        .await;
    let c = client(&server);
    for (status, reason) in [
        (401, DecisionUnavailable::Unauthorized),
        (403, DecisionUnavailable::Unauthorized),
        (422, DecisionUnavailable::Rejected),
        (429, DecisionUnavailable::RateLimited),
        (529, DecisionUnavailable::Overloaded),
        (500, DecisionUnavailable::Rejected),
        (302, DecisionUnavailable::Rejected),
        (307, DecisionUnavailable::Rejected),
    ] {
        let m = server
            .mock("POST", "/")
            .with_status(status)
            .with_header("location", &format!("{}/redirected", server.url()))
            .with_body("private provider text and credential")
            .expect(1)
            .create_async()
            .await;
        let error = c.choose(&text(), "test-only-key").await.unwrap_err();
        assert_eq!(error.reason, reason);
        assert!(error.attempted);
        assert_eq!(error.usage, None);
        assert_eq!(error.request_hash.as_ref().map(String::len), Some(64));
        assert!(!format!("{error:?}").contains("private"));
        m.assert_async().await;
        m.remove_async().await;
    }
    redirected.assert_async().await;
}

#[tokio::test]
async fn transport_preserves_validated_usage_only_for_semantic_decode_failures() {
    let mut server = Server::new_async().await;
    let c = client(&server);
    for (body, usage) in [
        ("private malformed response".to_owned(), None),
        (
            noul(0.7).to_string(),
            Some(DecisionUsage {
                input_tokens: 100,
                output_tokens: 0,
            }),
        ),
    ] {
        let observed = std::sync::Arc::new(std::sync::Mutex::new(None));
        let capture = observed.clone();
        let m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body_from_request(move |request| {
                *capture.lock().unwrap() = Some(digest_bytes(request.body().unwrap()));
                body.clone().into_bytes()
            })
            .expect(1)
            .create_async()
            .await;
        let error = c.choose(&text(), "test-only-key").await.unwrap_err();
        assert_eq!(error.reason, DecisionUnavailable::InvalidResponse);
        assert!(error.attempted);
        assert_eq!(error.usage, usage);
        assert_eq!(error.request_hash, *observed.lock().unwrap());
        assert!(!format!("{error:?}").contains("private"));
        m.assert_async().await;
        m.remove_async().await;
    }
    let m = server
        .mock("POST", "/")
        .with_status(200)
        .with_body(choice().to_string())
        .expect(1)
        .create_async()
        .await;
    let error = c
        .assess_selected(&text(), "c0", "test-only-key")
        .await
        .unwrap_err();
    assert_eq!(error.reason, DecisionUnavailable::InvalidResponse);
    assert!(error.attempted);
    assert_eq!(
        error.usage,
        Some(DecisionUsage {
            input_tokens: 100,
            output_tokens: 0
        })
    );
    assert_eq!(error.request_hash.as_ref().map(String::len), Some(64));
    m.assert_async().await;
}

#[tokio::test]
async fn invalid_input_credentials_and_nonselected_noul_send_nothing() {
    let mut server = Server::new_async().await;
    let m = server.mock("POST", "/").expect(0).create_async().await;
    let c = client(&server);
    let mut bad = text();
    bad.goal.clear();
    assert_eq!(
        c.choose(&bad, "test-only-key").await.unwrap_err(),
        failure(DecisionUnavailable::InvalidInput, false)
    );
    assert_eq!(
        c.assess_selected(&bad, "c0", "test-only-key")
            .await
            .unwrap_err(),
        failure(DecisionUnavailable::InvalidInput, false)
    );
    for key in ["".to_owned(), "bad\nkey".to_owned(), "k".repeat(4097)] {
        assert_eq!(
            c.choose(&text(), &key).await.unwrap_err(),
            failure(DecisionUnavailable::CredentialUnavailable, false)
        );
    }
    for selected in ["none", "delegate", "c1", ""] {
        assert_eq!(
            c.assess_selected(&text(), selected, "test-only-key")
                .await
                .unwrap_err(),
            failure(DecisionUnavailable::InvalidInput, false)
        );
    }
    m.assert_async().await;
}

#[tokio::test]
async fn transport_accepts_exact_limits_and_rejects_one_extra_byte() {
    let mut server = Server::new_async().await;
    let c = client(&server);
    let mut body = choice().to_string();
    body.push_str(&" ".repeat(MAX_RESPONSE_BYTES - body.len()));
    let m = server
        .mock("POST", "/")
        .with_status(200)
        .with_body(body)
        .expect(1)
        .create_async()
        .await;
    assert_eq!(
        c.choose(&text(), &"k".repeat(4096))
            .await
            .unwrap()
            .value
            .selected,
        "c0"
    );
    m.assert_async().await;
    m.remove_async().await;
    let m = server
        .mock("POST", "/")
        .with_status(200)
        .with_chunked_body(|writer| writer.write_all(&vec![b' '; MAX_RESPONSE_BYTES + 1]))
        .expect(1)
        .create_async()
        .await;
    let error = c.choose(&text(), "test-only-key").await.unwrap_err();
    assert_eq!(error.reason, DecisionUnavailable::ResponseTooLarge);
    assert!(error.attempted);
    assert_eq!(error.usage, None);
    assert_eq!(error.request_hash.as_ref().map(String::len), Some(64));
    m.assert_async().await;
    m.remove_async().await;

    // Exercise the final serialized envelope cap, independently of current rubric length.
    let state = text();
    let envelope = |instructions| Request {
        model: JEV_MODEL,
        state: &state,
        questions: BTreeMap::from([(
            QUESTION,
            Question::Choice {
                instructions,
                criteria: BTreeMap::new(),
            },
        )]),
    };
    let empty_size = serde_json::to_vec(&envelope("")).unwrap().len();
    let instructions = "x".repeat(MAX_PAYLOAD_BYTES + 8192 - empty_size);
    let m = server
        .mock("POST", "/")
        .with_status(200)
        .with_body("bounded")
        .expect(1)
        .create_async()
        .await;
    let (bytes, hash) = c
        .request(
            &state,
            Question::Choice {
                instructions: &instructions,
                criteria: BTreeMap::new(),
            },
            "test-only-key",
        )
        .await
        .unwrap();
    assert_eq!(bytes, b"bounded");
    assert_eq!(
        hash,
        digest_bytes(&serde_json::to_vec(&envelope(&instructions)).unwrap())
    );
    let error = c
        .request(
            &state,
            Question::Choice {
                instructions: &format!("{instructions}x"),
                criteria: BTreeMap::new(),
            },
            "test-only-key",
        )
        .await
        .unwrap_err();
    assert_eq!(error, failure(DecisionUnavailable::InvalidInput, false));
    m.assert_async().await;
}

#[tokio::test]
async fn response_body_timeout_keeps_one_attempt_and_unknown_usage() {
    // #12458: The stalled response outlives the client deadline. Keep its server
    // outside the shared pool so the next test cannot reuse a blocked runtime.
    let mut server = Server::new_with_opts_async(Default::default()).await;
    let m = server
        .mock("POST", "/")
        .with_status(200)
        .with_chunked_body(|writer| {
            writer.write_all(b"{")?;
            std::thread::sleep(Duration::from_millis(250));
            writer.write_all(b"}")
        })
        .expect(1)
        .create_async()
        .await;
    let error = client(&server)
        .choose(&text(), "test-only-key")
        .await
        .unwrap_err();
    assert_eq!(error.reason, DecisionUnavailable::Timeout);
    assert!(error.attempted);
    assert_eq!(error.usage, None);
    assert_eq!(error.request_hash.as_ref().map(String::len), Some(64));
    m.assert_async().await;
}

#[tokio::test]
async fn production_client_is_pinned_and_refuses_cleartext_before_network_io() {
    let mut c = JevCandidateDecisionClient::new().unwrap();
    assert_eq!(c.endpoint, "https://api.typesafe.ai/v1/systemone");
    let mut server = Server::new_async().await;
    let m = server.mock("POST", "/").expect(0).create_async().await;
    // Only this private test can replace the endpoint; production exposes no setter.
    c.endpoint = server.url();
    let error = c.choose(&text(), "test-only-key").await.unwrap_err();
    assert_eq!(error.reason, DecisionUnavailable::Transport);
    assert!(error.attempted);
    assert_eq!(error.usage, None);
    assert_eq!(error.request_hash.as_ref().map(String::len), Some(64));
    m.assert_async().await;
}

#[test]
fn malformed_local_alias_set_is_not_a_valid_decoding_context() {
    let mut state = text();
    state.candidates[0].id = "none".into();
    let error = decode_choice(&serde_json::to_vec(&choice()).unwrap(), &state).unwrap_err();
    assert_eq!(
        error,
        CandidateDecodeFailure {
            reason: DecisionUnavailable::InvalidInput,
            usage: None
        }
    );
}

#[test]
fn suitability_requires_a_candidate_alias_in_a_valid_local_context() {
    let bytes = serde_json::to_vec(&noul(0.5)).unwrap();
    for selected in ["none", "delegate", "c1", ""] {
        assert_eq!(
            decode_suitability(&bytes, &text(), selected).unwrap_err(),
            CandidateDecodeFailure {
                reason: DecisionUnavailable::InvalidInput,
                usage: None,
            }
        );
    }
    let mut state = text();
    state.goal.clear();
    assert_eq!(
        decode_suitability(&bytes, &state, "c0").unwrap_err(),
        CandidateDecodeFailure {
            reason: DecisionUnavailable::InvalidInput,
            usage: None,
        }
    );
}
