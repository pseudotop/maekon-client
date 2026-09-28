use super::*;
use maekon_core::models::candidate_assessment::LocalAssessmentChoice;
use maekon_core::models::candidate_decision::DecisionOption;
use maekon_core::models::candidate_decision::DecisionUsage;
use std::sync::atomic::{AtomicUsize, Ordering};

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
fn tags() -> String {
    serde_json::json!({"models":[{"name":"fixture:fixed","model":"fixture:fixed","digest":"a".repeat(64)}]}).to_string()
}

struct Guard {
    checks: AtomicUsize,
    deny_at: usize,
}
impl Default for Guard {
    fn default() -> Self {
        Self {
            checks: AtomicUsize::new(0),
            deny_at: usize::MAX,
        }
    }
}
#[async_trait]
impl CandidateAttemptGuard for Guard {
    async fn checkpoint(&self) -> Result<(), DecisionUnavailable> {
        if self.checks.fetch_add(1, Ordering::SeqCst) >= self.deny_at {
            Err(DecisionUnavailable::Cancelled)
        } else {
            Ok(())
        }
    }
}

#[test]
fn local_candidate_model_rejects_nonlocal_and_ambiguous_endpoints_before_http() {
    for endpoint in [
        "https://example.com",
        "http://192.168.1.3:11434",
        "http://0.0.0.0:11434",
        "http://127.0.0.1:11434/api/chat",
        "http://user@127.0.0.1:11434",
        "http://127.0.0.1:11434?remote=1",
        "http://127.0.0.1:11434#fragment",
        "file:///tmp/model",
        "http://localhost.example.com:11434",
    ] {
        let expected = if endpoint.contains("example.com")
            || endpoint.contains("192.168.")
            || endpoint.contains("0.0.0.0")
        {
            DecisionUnavailable::LocalOnly
        } else {
            DecisionUnavailable::InvalidInput
        };
        match LocalCandidateModelClient::new(endpoint) {
            Err(error) => assert_eq!(error.reason, expected, "{endpoint}"),
            Ok(_) => panic!("unsafe endpoint admitted: {endpoint}"),
        }
    }
    for endpoint in [
        "http://127.0.0.1:11434",
        "http://[::1]:11434",
        "http://localhost:11434",
    ] {
        assert_eq!(
            LocalCandidateModelClient::new(endpoint).unwrap().origin,
            endpoint
        );
    }
}

#[test]
fn local_candidate_model_schema_has_only_closed_ids_and_no_tools() {
    let encoded: serde_json::Value =
        serde_json::from_slice(&encode_request(&text(), "fixture:fixed").unwrap()).unwrap();
    assert_eq!(encoded["stream"], false);
    assert_eq!(encoded["think"], false);
    assert!(encoded.get("tools").is_none());
    assert_eq!(encoded["options"]["num_predict"], 128);
    assert_eq!(
        encoded["format"]["properties"]["selected"]["enum"],
        serde_json::json!(["c0", "none", "delegate"])
    );
    assert_eq!(encoded["format"]["additionalProperties"], false);
}

#[tokio::test]
async fn local_candidate_model_positive_control_checks_cloud_and_manifest_before_and_after() {
    let mut server = mockito::Server::new_async().await;
    let status = server
        .mock("GET", "/api/status")
        .with_status(200)
        .with_body(r#"{"cloud":{"disabled":true,"source":"env"}}"#)
        .expect(2)
        .create_async()
        .await;
    let models = server
        .mock("GET", "/api/tags")
        .with_status(200)
        .with_body(tags())
        .expect(2)
        .create_async()
        .await;
    let chat = server
        .mock("POST", "/api/chat")
        .match_header("authorization", mockito::Matcher::Missing)
        .with_status(200)
        .with_body(answer(r#"{"selected":"c0"}"#).to_string())
        .expect(1)
        .create_async()
        .await;
    let client = LocalCandidateModelClient::new(&server.url()).unwrap();
    let mut approved = approval();
    approved.endpoint_origin = server.url();
    let guard = Guard::default();
    let result = client.infer(&text(), &approved, &guard).await.unwrap();
    assert_eq!(result.choice, LocalAssessmentChoice::Selected("c0".into()));
    assert_eq!(result.model_digest, approved.model_digest);
    assert!(guard.checks.load(Ordering::SeqCst) >= 15);
    status.assert_async().await;
    models.assert_async().await;
    chat.assert_async().await;
}

#[tokio::test]
async fn local_candidate_model_cloud_unknown_remote_or_missing_manifest_never_infer_or_pull() {
    let mut remote_tags: serde_json::Value = serde_json::from_str(&tags()).unwrap();
    remote_tags["models"][0]["remote_host"] = serde_json::json!("https://cloud.invalid");
    for (status_body, models_body) in [
        (
            r#"{"cloud":{"disabled":false,"source":"config"}}"#.to_owned(),
            tags(),
        ),
        (r#"{}"#.to_owned(), tags()),
        (
            r#"{"cloud":{"disabled":true,"source":"env"}}"#.to_owned(),
            r#"{"models":[]}"#.to_owned(),
        ),
        (
            r#"{"cloud":{"disabled":true,"source":"env"}}"#.to_owned(),
            remote_tags.to_string(),
        ),
        (
            r#"{"cloud":{"disabled":true,"source":"env"}}"#.to_owned(),
            tags().replace(&"a".repeat(64), &"b".repeat(64)),
        ),
    ] {
        let mut server = mockito::Server::new_async().await;
        let _status = server
            .mock("GET", "/api/status")
            .with_status(200)
            .with_body(status_body)
            .create_async()
            .await;
        let _models = server
            .mock("GET", "/api/tags")
            .with_status(200)
            .with_body(models_body)
            .create_async()
            .await;
        let chat = server
            .mock("POST", "/api/chat")
            .expect(0)
            .create_async()
            .await;
        let pull = server
            .mock("POST", "/api/pull")
            .expect(0)
            .create_async()
            .await;
        let client = LocalCandidateModelClient::new(&server.url()).unwrap();
        let mut approved = approval();
        approved.endpoint_origin = server.url();
        let error = client
            .infer(&text(), &approved, &Guard::default())
            .await
            .unwrap_err();
        assert!(!error.attempted);
        chat.assert_async().await;
        pull.assert_async().await;
    }
}

#[tokio::test]
async fn local_candidate_model_redirect_and_revocation_cannot_reach_inference() {
    let mut target = mockito::Server::new_async().await;
    let redirected = target.mock("GET", "/stolen").expect(0).create_async().await;
    let mut server = mockito::Server::new_async().await;
    let _status = server
        .mock("GET", "/api/status")
        .with_status(307)
        .with_header("location", &format!("{}/stolen", target.url()))
        .create_async()
        .await;
    let chat = server
        .mock("POST", "/api/chat")
        .expect(0)
        .create_async()
        .await;
    let client = LocalCandidateModelClient::new(&server.url()).unwrap();
    let mut approved = approval();
    approved.endpoint_origin = server.url();
    assert_eq!(
        client
            .infer(&text(), &approved, &Guard::default())
            .await
            .unwrap_err()
            .reason,
        DecisionUnavailable::Rejected
    );
    redirected.assert_async().await;
    chat.assert_async().await;
    let guard = Guard {
        checks: AtomicUsize::new(0),
        deny_at: 0,
    };
    assert_eq!(
        client
            .infer(&text(), &approved, &guard)
            .await
            .unwrap_err()
            .reason,
        DecisionUnavailable::Cancelled
    );
}

#[tokio::test]
async fn local_candidate_model_approval_is_bound_to_endpoint_and_config_epoch() {
    let mut server = mockito::Server::new_async().await;
    let status = server
        .mock("GET", "/api/status")
        .expect(0)
        .create_async()
        .await;
    let client = LocalCandidateModelClient::new(&server.url()).unwrap();
    assert_eq!(
        client
            .infer(&text(), &approval(), &Guard::default())
            .await
            .unwrap_err()
            .reason,
        DecisionUnavailable::ApprovalMissing
    );
    status.assert_async().await;
}

#[tokio::test]
async fn local_candidate_model_oversized_reply_is_bounded_without_retry() {
    let mut server = mockito::Server::new_async().await;
    let _status = server
        .mock("GET", "/api/status")
        .with_status(200)
        .with_body(r#"{"cloud":{"disabled":true,"source":"env"}}"#)
        .create_async()
        .await;
    let _models = server
        .mock("GET", "/api/tags")
        .with_status(200)
        .with_body(tags())
        .create_async()
        .await;
    let chat = server
        .mock("POST", "/api/chat")
        .with_status(200)
        .with_body("x".repeat(MAX_RESPONSE_BYTES as usize + 1))
        .expect(1)
        .create_async()
        .await;
    let client = LocalCandidateModelClient::new(&server.url()).unwrap();
    let mut approved = approval();
    approved.endpoint_origin = server.url();
    let error = client
        .infer(&text(), &approved, &Guard::default())
        .await
        .unwrap_err();
    assert_eq!(error.reason, DecisionUnavailable::ResponseTooLarge);
    assert!(error.attempted && error.request_hash.is_some());
    chat.assert_async().await;
}

#[tokio::test]
async fn local_candidate_model_revocation_between_metadata_awaits_prevents_inference() {
    for deny_at in 1..=6 {
        let mut server = mockito::Server::new_async().await;
        let _status = server
            .mock("GET", "/api/status")
            .with_status(200)
            .with_body(r#"{"cloud":{"disabled":true,"source":"env"}}"#)
            .create_async()
            .await;
        let _models = server
            .mock("GET", "/api/tags")
            .with_status(200)
            .with_body(tags())
            .create_async()
            .await;
        let chat = server
            .mock("POST", "/api/chat")
            .expect(0)
            .create_async()
            .await;
        let client = LocalCandidateModelClient::new(&server.url()).unwrap();
        let mut approved = approval();
        approved.endpoint_origin = server.url();
        let guard = Guard {
            checks: AtomicUsize::new(0),
            deny_at,
        };
        let error = client.infer(&text(), &approved, &guard).await.unwrap_err();
        assert_eq!(error.reason, DecisionUnavailable::Cancelled);
        assert!(!error.attempted);
        chat.assert_async().await;
    }
}

#[tokio::test]
async fn local_candidate_model_post_inference_drift_preserves_the_original_attempt() {
    let mut server = mockito::Server::new_async().await;
    let _status = server
        .mock("GET", "/api/status")
        .with_status(200)
        .with_body(r#"{"cloud":{"disabled":true,"source":"env"}}"#)
        .expect(2)
        .create_async()
        .await;
    let reads = std::sync::Arc::new(AtomicUsize::new(0));
    let read_count = reads.clone();
    let _models = server
        .mock("GET", "/api/tags")
        .with_status(200)
        .with_chunked_body(move |writer| {
            let body = if read_count.fetch_add(1, Ordering::SeqCst) == 0 {
                tags()
            } else {
                tags().replace(&"a".repeat(64), &"b".repeat(64))
            };
            writer.write_all(body.as_bytes())
        })
        .expect(2)
        .create_async()
        .await;
    let chat = server
        .mock("POST", "/api/chat")
        .with_status(200)
        .with_body(answer(r#"{"selected":"c0"}"#).to_string())
        .expect(1)
        .create_async()
        .await;
    let client = LocalCandidateModelClient::new(&server.url()).unwrap();
    let mut approved = approval();
    approved.endpoint_origin = server.url();
    let error = client
        .infer(&text(), &approved, &Guard::default())
        .await
        .unwrap_err();
    assert_eq!(error.reason, DecisionUnavailable::InvalidResponse);
    assert!(error.attempted);
    assert_eq!(
        error.usage,
        Some(DecisionUsage {
            input_tokens: 14,
            output_tokens: 3
        })
    );
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    assert!(error.request_hash.is_some());
    chat.assert_async().await;
}

#[tokio::test]
async fn local_candidate_model_proxy_environment_is_ignored() {
    const CHILD: &str = "MAEKON_CANDIDATE_PROXY_TEST_CHILD";
    if std::env::var(CHILD).as_deref() != Ok("1") {
        // Use a child process so parallel tests never share mutated proxy env.
        let mut proxy = mockito::Server::new_async().await;
        let hit = proxy
            .mock("GET", mockito::Matcher::Any)
            .expect(0)
            .create_async()
            .await;
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "local_candidate_model::tests::local_candidate_model_proxy_environment_is_ignored",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("HTTP_PROXY", proxy.url())
            .env("http_proxy", proxy.url())
            .env("HTTPS_PROXY", proxy.url())
            .env("https_proxy", proxy.url())
            .env("ALL_PROXY", proxy.url())
            .env("all_proxy", proxy.url())
            .env("NO_PROXY", "")
            .env("no_proxy", "")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        hit.assert_async().await;
        return;
    }
    let mut server = mockito::Server::new_async().await;
    let _status = server
        .mock("GET", "/api/status")
        .with_status(200)
        .with_body(r#"{"cloud":{"disabled":true,"source":"env"}}"#)
        .expect(2)
        .create_async()
        .await;
    let _models = server
        .mock("GET", "/api/tags")
        .with_status(200)
        .with_body(tags())
        .expect(2)
        .create_async()
        .await;
    let _chat = server
        .mock("POST", "/api/chat")
        .with_status(200)
        .with_body(answer(r#"{"selected":"c0"}"#).to_string())
        .expect(1)
        .create_async()
        .await;
    let client = LocalCandidateModelClient::new(&server.url()).unwrap();
    let mut approved = approval();
    approved.endpoint_origin = server.url();
    assert_eq!(
        client
            .infer(&text(), &approved, &Guard::default())
            .await
            .unwrap()
            .choice,
        LocalAssessmentChoice::Selected("c0".into())
    );
}

#[test]
fn local_candidate_model_request_enforces_the_exact_encoded_body_budget() {
    let mut input = text();
    input.goal = "\"".repeat(5_000);
    input.validate().unwrap();
    let initial = encode_request(&input, "fixture:fixed").unwrap();
    let remaining = MAX_PAYLOAD_BYTES + 8192 - initial.len();
    input.goal.push_str(&"x".repeat(remaining));
    input.validate().unwrap();
    assert_eq!(
        encode_request(&input, "fixture:fixed").unwrap().len(),
        MAX_PAYLOAD_BYTES + 8192
    );
    input.goal.push('x');
    input.validate().unwrap();
    let error = encode_request(&input, "fixture:fixed").unwrap_err();
    assert_eq!(error.reason, DecisionUnavailable::InvalidInput);
    assert!(!error.attempted);
}

#[tokio::test]
async fn local_candidate_model_accepts_bare_manifest_digest_and_exact_source_limit() {
    let mut server = mockito::Server::new_async().await;
    let status = server
        .mock("GET", "/api/status")
        .with_status(200)
        .with_body(
            serde_json::json!({"cloud":{"disabled":true,"source":"x".repeat(64)}}).to_string(),
        )
        .expect(2)
        .create_async()
        .await;
    let mut models: serde_json::Value = serde_json::from_str(&tags()).unwrap();
    assert_eq!(models["models"][0]["digest"], "a".repeat(64));
    models["models"].as_array_mut().unwrap().push(
        serde_json::json!({"name":"unrelated:fixed","model":"unrelated:fixed","digest":"b".repeat(64)})
    );
    let models = server
        .mock("GET", "/api/tags")
        .with_status(200)
        .with_body(models.to_string())
        .expect(2)
        .create_async()
        .await;
    let chat = server
        .mock("POST", "/api/chat")
        .with_status(200)
        .with_body(answer(r#"{"selected":"none"}"#).to_string())
        .expect(1)
        .create_async()
        .await;
    let client = LocalCandidateModelClient::new(&server.url()).unwrap();
    let mut approved = approval();
    approved.endpoint_origin = server.url();
    let result = client
        .infer(&text(), &approved, &Guard::default())
        .await
        .unwrap();
    assert_eq!(result.choice, LocalAssessmentChoice::None);
    assert_eq!(result.model_digest, approved.model_digest);
    status.assert_async().await;
    models.assert_async().await;
    chat.assert_async().await;
}

#[tokio::test]
async fn local_candidate_model_rejects_ambiguous_names_and_noncanonical_wire_digests() {
    let valid: serde_json::Value = serde_json::from_str(&tags()).unwrap();
    let mut cases = vec![];
    for digest in [
        approval().model_digest,
        "a".repeat(63),
        "a".repeat(65),
        "A".repeat(64),
        "b".repeat(64),
        format!("{} ", "a".repeat(64)),
    ] {
        let mut wire = valid.clone();
        wire["models"][0]["digest"] = digest.into();
        cases.push((wire, DecisionUnavailable::InvalidResponse));
    }
    for field in ["name", "model"] {
        let mut wire = valid.clone();
        wire["models"][0][field] = "different:fixed".into();
        cases.push((wire, DecisionUnavailable::InvalidResponse));
    }
    for field in ["remote_model", "remote_host"] {
        let mut wire = valid.clone();
        wire["models"][0][field] = "remote".into();
        cases.push((wire, DecisionUnavailable::InvalidResponse));
    }
    let mut duplicate = valid.clone();
    duplicate["models"]
        .as_array_mut()
        .unwrap()
        .push(valid["models"][0].clone());
    cases.push((duplicate, DecisionUnavailable::Rejected));
    for (wire, expected) in cases {
        let mut server = mockito::Server::new_async().await;
        let _status = server
            .mock("GET", "/api/status")
            .with_status(200)
            .with_body(r#"{"cloud":{"disabled":true,"source":"env"}}"#)
            .create_async()
            .await;
        let models = server
            .mock("GET", "/api/tags")
            .with_status(200)
            .with_body(wire.to_string())
            .expect(1)
            .create_async()
            .await;
        let chat = server
            .mock("POST", "/api/chat")
            .expect(0)
            .create_async()
            .await;
        let client = LocalCandidateModelClient::new(&server.url()).unwrap();
        let mut approved = approval();
        approved.endpoint_origin = server.url();
        let error = client
            .infer(&text(), &approved, &Guard::default())
            .await
            .unwrap_err();
        assert_eq!(error.reason, expected);
        assert!(!error.attempted);
        models.assert_async().await;
        chat.assert_async().await;
    }
}

#[tokio::test]
async fn local_candidate_model_rejects_blank_and_oversized_cloud_config_evidence() {
    for source in ["".to_owned(), " \t".to_owned(), "x".repeat(65)] {
        let mut server = mockito::Server::new_async().await;
        let status = server
            .mock("GET", "/api/status")
            .with_status(200)
            .with_body(serde_json::json!({"cloud":{"disabled":true,"source":source}}).to_string())
            .expect(1)
            .create_async()
            .await;
        let chat = server
            .mock("POST", "/api/chat")
            .expect(0)
            .create_async()
            .await;
        let client = LocalCandidateModelClient::new(&server.url()).unwrap();
        let mut approved = approval();
        approved.endpoint_origin = server.url();
        let error = client
            .infer(&text(), &approved, &Guard::default())
            .await
            .unwrap_err();
        assert_eq!(error.reason, DecisionUnavailable::LocalOnly);
        assert!(!error.attempted);
        status.assert_async().await;
        chat.assert_async().await;
    }
}

#[tokio::test]
async fn local_candidate_model_maps_failed_statuses_without_retry_or_success() {
    for (status, expected) in [
        (401, DecisionUnavailable::Unauthorized),
        (403, DecisionUnavailable::Unauthorized),
        (429, DecisionUnavailable::RateLimited),
        (500, DecisionUnavailable::Rejected),
        (201, DecisionUnavailable::Rejected),
    ] {
        let mut server = mockito::Server::new_async().await;
        let hit = server
            .mock("POST", "/api/chat")
            .with_status(status)
            .expect(1)
            .create_async()
            .await;
        let client = LocalCandidateModelClient::new(&server.url()).unwrap();
        let error = client
            .request(
                "/api/chat",
                Some(b"{}"),
                MAX_RESPONSE_BYTES,
                &Guard::default(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.reason, expected);
        assert!(error.attempted);
        assert_eq!(error.usage, None);
        hit.assert_async().await;
    }
}

#[tokio::test]
async fn local_candidate_model_rechecks_post_read_guard_and_bounds_metadata() {
    let mut server = mockito::Server::new_async().await;
    let hit = server
        .mock("GET", "/api/tags")
        .with_status(200)
        .with_body("x".repeat(MAX_METADATA_BYTES as usize + 1))
        .expect(1)
        .create_async()
        .await;
    let client = LocalCandidateModelClient::new(&server.url()).unwrap();
    let error = client
        .request("/api/tags", None, MAX_METADATA_BYTES, &Guard::default())
        .await
        .unwrap_err();
    assert_eq!(error.reason, DecisionUnavailable::ResponseTooLarge);
    assert!(!error.attempted);
    hit.assert_async().await;
    let hit = server
        .mock("POST", "/api/chat")
        .with_status(200)
        .with_body("{}")
        .expect(1)
        .create_async()
        .await;
    let guard = Guard {
        checks: AtomicUsize::new(0),
        deny_at: 2,
    };
    let error = client
        .request("/api/chat", Some(b"{}"), MAX_RESPONSE_BYTES, &guard)
        .await
        .unwrap_err();
    assert_eq!(error.reason, DecisionUnavailable::Cancelled);
    assert!(error.attempted);
    hit.assert_async().await;
}
