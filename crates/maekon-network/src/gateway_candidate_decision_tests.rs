use super::*;
use maekon_core::models::candidate_decision::{digest_bytes, DecisionOption};
use mockito::{Matcher, Server};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

fn text() -> DecisionText {
    DecisionText {
        goal: "Save safely".into(),
        candidates: (0..2)
            .map(|index| DecisionOption {
                id: format!("c{index}"),
                text: format!("Candidate {index}"),
                role: Some("button".into()),
                intent: None,
                state: None,
            })
            .collect(),
    }
}

fn envelope(answer: Value) -> Value {
    json!({
        "model":"typesafe-ai/jev",
        "answers":{"decision":answer},
        "usage":{"input_tokens":11,"output_tokens":3},
        "provider_metadata":{"gateway":{
            "routing":{
                "originalModelId":"typesafe-ai/jev",
                "canonicalSlug":"typesafe-ai/jev",
                "resolvedProvider":"typesafe-ai",
                "finalProvider":"typesafe-ai"
            },
            "cost":"0","marketCost":"0.0001","surchargeCost":"0","gatewayCost":"0",
            "generationId":"gateway-fixture-1"
        }}
    })
}

fn choice() -> Value {
    envelope(json!({
        "type":"choice","choice":"c1","confidence":0.9,
        "probabilities":{"c0":0.1,"c1":0.8,"none":0.05,"delegate":0.05}
    }))
}

fn client(server: &Server) -> GatewayCandidateDecisionClient {
    GatewayCandidateDecisionClient {
        client: build_client(TransportPolicy::AllowLoopbackCleartext).unwrap(),
        endpoint: format!("{}/typesafe/v1/systemone", server.url()),
        deadline: Duration::from_secs(3),
    }
}

#[derive(Default)]
struct Guard {
    calls: AtomicUsize,
    deny: Option<usize>,
    hang: Option<usize>,
}

#[async_trait]
impl CandidateAttemptGuard for Guard {
    async fn checkpoint(&self) -> Result<(), DecisionUnavailable> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if self.hang == Some(call) {
            std::future::pending::<()>().await;
        }
        if self.deny == Some(call) {
            Err(DecisionUnavailable::ConsentOrPolicyDenied)
        } else {
            Ok(())
        }
    }
}

#[tokio::test]
async fn gateway_http_sends_exact_prepared_bytes_and_binds_choice_and_suitability() {
    let mut server = Server::new_async().await;
    let http = client(&server);
    let state = text();
    let (expected_body, expected_hash) = encode_choice(&state).unwrap().into_parts();
    let captured = Arc::new(Mutex::new(Vec::new()));
    let record = captured.clone();
    let mock = server
        .mock("POST", "/typesafe/v1/systemone")
        .match_header("authorization", "Bearer offline-fixture")
        .match_header("content-type", "application/json")
        .match_header("accept-encoding", Matcher::Missing)
        .with_body_from_request(move |request| {
            *record.lock().unwrap() = request.body().unwrap().to_vec();
            choice().to_string().into_bytes()
        })
        .expect(1)
        .create_async()
        .await;
    let response = http
        .choose(&state, "offline-fixture", &Guard::default())
        .await
        .unwrap();
    assert_eq!(response.value.selected, "c1");
    assert_eq!(response.value.confidence, Some(0.9));
    assert_eq!(response.request_hash, expected_hash);
    assert_eq!(*captured.lock().unwrap(), expected_body);
    assert_eq!(
        response.request_hash,
        digest_bytes(&captured.lock().unwrap())
    );
    assert_eq!(response.observation.usage.unwrap().input_tokens, 11);
    mock.assert_async().await;
    drop(mock);
    let (_, expected_hash) = encode_suitability(&state, "c1").unwrap().into_parts();
    let mock = server
        .mock("POST", "/typesafe/v1/systemone")
        .match_body(Matcher::PartialJson(
            json!({"questions":{"decision":{"type":"noul"}}}),
        ))
        .with_body(envelope(json!({"type":"noul","noul":0.83})).to_string())
        .expect(1)
        .create_async()
        .await;
    let response = http
        .assess_selected(&state, "c1", "offline-fixture", &Guard::default())
        .await
        .unwrap();
    assert_eq!(response.value.selected, "c1");
    assert_eq!(response.value.score, 0.83);
    assert_eq!(response.request_hash, expected_hash);
    mock.assert_async().await;
}

#[tokio::test]
async fn gateway_http_rejects_invalid_input_and_credentials_before_guard_or_send() {
    let mut server = Server::new_async().await;
    let mock = server
        .mock("POST", "/typesafe/v1/systemone")
        .expect(0)
        .create_async()
        .await;
    let http = client(&server);
    let guard = Guard::default();
    let keys = vec![
        "".into(),
        " ".into(),
        "tab\there".into(),
        "line\r\nbreak".into(),
        "한글".into(),
        " leading".into(),
        "trailing ".into(),
        "x".repeat(4097),
    ];
    for key in keys {
        let failure = http.choose(&text(), &key, &guard).await.err().unwrap();
        assert_eq!(failure.reason, DecisionUnavailable::CredentialUnavailable);
        assert!(!failure.attempted);
        assert_eq!(failure.request_hash, None);
        assert!(failure.observation.is_none());
    }
    let mut invalid = text();
    invalid.candidates[0].id = "external".into();
    let failure = http
        .choose(&invalid, "offline-fixture", &guard)
        .await
        .err()
        .unwrap();
    assert_eq!(failure.reason, DecisionUnavailable::InvalidInput);
    assert!(!failure.attempted);
    for selected in ["none", "delegate", "c2", "c1\n"] {
        let failure = http
            .assess_selected(&text(), selected, "offline-fixture", &guard)
            .await
            .err()
            .unwrap();
        assert_eq!(failure.reason, DecisionUnavailable::InvalidInput);
        assert!(!failure.attempted);
    }
    assert_eq!(guard.calls.load(Ordering::SeqCst), 0);
    mock.assert_async().await;
}

#[tokio::test]
async fn gateway_http_maximum_key_is_accepted_and_errors_never_expose_response_text() {
    let mut server = Server::new_async().await;
    let key = "k".repeat(4096);
    let mock = server
        .mock("POST", "/typesafe/v1/systemone")
        .match_header("authorization", format!("Bearer {key}").as_str())
        .with_status(403)
        .with_body("private response text")
        .expect(1)
        .create_async()
        .await;
    let failure = client(&server)
        .choose(&text(), &key, &Guard::default())
        .await
        .err()
        .unwrap();
    assert_eq!(failure.reason, DecisionUnavailable::Unauthorized);
    assert!(failure.attempted);
    assert!(failure.request_hash.is_some());
    let diagnostic = format!("{failure:?}");
    assert!(!diagnostic.contains("private response text"));
    assert!(!diagnostic.contains(&key));
    mock.assert_async().await;
}

#[tokio::test]
async fn gateway_http_statuses_are_classified_without_retries() {
    let mut server = Server::new_async().await;
    for (status, expected) in [
        (400, DecisionUnavailable::Rejected),
        (401, DecisionUnavailable::Unauthorized),
        (403, DecisionUnavailable::Unauthorized),
        (402, DecisionUnavailable::BudgetExceeded),
        (429, DecisionUnavailable::RateLimited),
        (529, DecisionUnavailable::Overloaded),
        (503, DecisionUnavailable::Rejected),
    ] {
        let mock = server
            .mock("POST", "/typesafe/v1/systemone")
            .with_status(status)
            .with_header("retry-after", "0")
            .with_body("private error")
            .expect(1)
            .create_async()
            .await;
        let failure = client(&server)
            .choose(&text(), "offline-fixture", &Guard::default())
            .await
            .err()
            .unwrap();
        assert_eq!(failure.reason, expected);
        assert!(failure.attempted);
        assert_eq!(
            failure.request_hash,
            Some(encode_choice(&text()).unwrap().into_parts().1)
        );
        assert!(failure.observation.is_none());
        mock.assert_async().await;
        drop(mock);
    }
}

#[tokio::test]
async fn gateway_http_does_not_follow_any_redirect() {
    let mut server = Server::new_async().await;
    let target = server
        .mock("POST", "/redirected")
        .expect(0)
        .create_async()
        .await;
    let target_get = server
        .mock("GET", "/redirected")
        .expect(0)
        .create_async()
        .await;
    for status in [301, 302, 303, 307, 308] {
        let mock = server
            .mock("POST", "/typesafe/v1/systemone")
            .with_status(status)
            .with_header("location", &format!("{}/redirected", server.url()))
            .expect(1)
            .create_async()
            .await;
        let failure = client(&server)
            .choose(&text(), "offline-fixture", &Guard::default())
            .await
            .err()
            .unwrap();
        assert_eq!(failure.reason, DecisionUnavailable::Rejected);
        assert!(failure.attempted);
        mock.assert_async().await;
        drop(mock);
    }
    target.assert_async().await;
    target_get.assert_async().await;
}

#[tokio::test]
async fn gateway_http_guard_revocation_is_observed_at_each_await_boundary() {
    for deny in 1..=6 {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/typesafe/v1/systemone")
            .with_body(choice().to_string())
            .expect(usize::from(deny != 1))
            .create_async()
            .await;
        let guard = Guard {
            deny: Some(deny),
            ..Default::default()
        };
        let failure = client(&server)
            .choose(&text(), "offline-fixture", &guard)
            .await
            .err()
            .unwrap();
        assert_eq!(failure.reason, DecisionUnavailable::ConsentOrPolicyDenied);
        assert_eq!(failure.attempted, deny != 1);
        assert_eq!(guard.calls.load(Ordering::SeqCst), deny);
        assert!(failure.request_hash.is_some());
        mock.assert_async().await;
    }
}

#[tokio::test]
async fn gateway_http_total_deadline_includes_hanging_guards_before_and_after_send() {
    for hang in [1, 2] {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/typesafe/v1/systemone")
            .with_body(choice().to_string())
            .expect(usize::from(hang == 2))
            .create_async()
            .await;
        let mut http = client(&server);
        http.deadline = Duration::from_millis(500);
        let guard = Guard {
            hang: Some(hang),
            ..Default::default()
        };
        let failure = http
            .choose(&text(), "offline-fixture", &guard)
            .await
            .err()
            .unwrap();
        assert_eq!(failure.reason, DecisionUnavailable::Timeout);
        assert_eq!(failure.attempted, hang == 2);
        assert_eq!(guard.calls.load(Ordering::SeqCst), hang);
        assert!(failure.request_hash.is_some());
        mock.assert_async().await;
    }
}

#[tokio::test]
async fn gateway_http_total_deadline_bounds_a_stalled_response_without_pool_reuse() {
    // The stalled callback outlives this client's deadline. Its runtime must
    // never be returned to mockito's shared server pool (#12458).
    let mut server = Server::new_with_opts_async(Default::default()).await;
    let mock = server
        .mock("POST", "/typesafe/v1/systemone")
        .with_chunked_body(|writer| {
            std::thread::sleep(Duration::from_millis(250));
            writer.write_all(b"{}")
        })
        .expect(1)
        .create_async()
        .await;
    let mut http = client(&server);
    http.deadline = Duration::from_millis(80);
    let failure = http
        .choose(&text(), "offline-fixture", &Guard::default())
        .await
        .err()
        .unwrap();
    assert_eq!(failure.reason, DecisionUnavailable::Timeout);
    assert!(failure.attempted);
    assert!(failure.request_hash.is_some());
    mock.assert_async().await;
}

#[tokio::test]
async fn gateway_http_caps_declared_and_streamed_response_bytes() {
    let mut server = Server::new_async().await;
    let http = client(&server);
    for size in [32_767, 32_768, 32_769] {
        let mut body = choice().to_string();
        body.push_str(&" ".repeat(size - body.len()));
        let mock = server
            .mock("POST", "/typesafe/v1/systemone")
            .with_body(body)
            .expect(1)
            .create_async()
            .await;
        let result = http
            .choose(&text(), "offline-fixture", &Guard::default())
            .await;
        if size <= 32_768 {
            assert_eq!(result.unwrap().value.selected, "c1");
        } else {
            let failure = result.err().unwrap();
            assert_eq!(failure.reason, DecisionUnavailable::ResponseTooLarge);
            assert!(failure.attempted);
        }
        mock.assert_async().await;
        drop(mock);
    }
    let mock = server
        .mock("POST", "/typesafe/v1/systemone")
        .with_chunked_body(|writer| {
            for _ in 0..9 {
                writer.write_all(&vec![b' '; 4096])?;
            }
            Ok(())
        })
        .expect(1)
        .create_async()
        .await;
    let failure = http
        .choose(&text(), "offline-fixture", &Guard::default())
        .await
        .err()
        .unwrap();
    assert_eq!(failure.reason, DecisionUnavailable::ResponseTooLarge);
    assert!(failure.attempted);
    mock.assert_async().await;
}

#[tokio::test]
async fn gateway_http_preserves_valid_observations_when_decoding_fails() {
    let mut server = Server::new_async().await;
    let mock = server
        .mock("POST", "/typesafe/v1/systemone")
        .with_body(
            envelope(json!({"type":"choice","choice":"outside","probabilities":{}})).to_string(),
        )
        .expect(1)
        .create_async()
        .await;
    let failure = client(&server)
        .choose(&text(), "offline-fixture", &Guard::default())
        .await
        .err()
        .unwrap();
    assert_eq!(failure.reason, DecisionUnavailable::InvalidResponse);
    assert!(failure.attempted);
    assert_eq!(
        failure.request_hash,
        Some(encode_choice(&text()).unwrap().into_parts().1)
    );
    let observation = failure.observation.unwrap();
    assert_eq!(observation.usage.unwrap().output_tokens, 3);
    assert_eq!(observation.costs.cost.as_deref(), Some("0"));
    assert_eq!(
        observation.generation_hash,
        Some(digest_bytes(b"gateway-fixture-1"))
    );
    mock.assert_async().await;
}

#[tokio::test]
async fn gateway_http_production_client_is_pinned_and_rejects_plaintext() {
    let mut server = Server::new_async().await;
    let mock = server
        .mock("POST", "/typesafe/v1/systemone")
        .expect(0)
        .create_async()
        .await;
    let mut http = GatewayCandidateDecisionClient::new().unwrap();
    assert_eq!(
        http.endpoint,
        "https://ai-gateway.vercel.sh/typesafe/v1/systemone"
    );
    assert_eq!(http.deadline, Duration::from_secs(8));
    // Tests can access the private field; production has no override API.
    http.endpoint = format!("{}/typesafe/v1/systemone", server.url());
    let failure = http
        .choose(&text(), "offline-fixture", &Guard::default())
        .await
        .err()
        .unwrap();
    assert_eq!(failure.reason, DecisionUnavailable::Transport);
    assert!(failure.attempted);
    mock.assert_async().await;
}

#[tokio::test]
async fn gateway_http_rejects_oversized_headers_before_waiting_for_any_body() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            let mut buffer = [0; 4096];
            let count = stream.read(&mut buffer).await.unwrap();
            assert_ne!(count, 0);
            request.extend_from_slice(&buffer[..count]);
            if request.windows(4).any(|part| part == b"\r\n\r\n") {
                break;
            }
            assert!(request.len() < 8192);
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 32769\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        std::future::pending::<()>().await;
    });
    let http = GatewayCandidateDecisionClient {
        client: build_client(TransportPolicy::AllowLoopbackCleartext).unwrap(),
        endpoint: format!("http://{address}/typesafe/v1/systemone"),
        deadline: Duration::from_millis(500),
    };
    let result = http
        .choose(&text(), "offline-fixture", &Guard::default())
        .await;
    server.abort();
    let _ = server.await;
    let failure = result.err().unwrap();
    assert_eq!(failure.reason, DecisionUnavailable::ResponseTooLarge);
    assert!(failure.attempted);
}

#[tokio::test]
async fn gateway_http_maps_the_http_clients_own_timeout_without_raw_errors() {
    let mut server = Server::new_with_opts_async(Default::default()).await;
    let mock = server
        .mock("POST", "/typesafe/v1/systemone")
        .with_chunked_body(|writer| {
            std::thread::sleep(Duration::from_millis(250));
            writer.write_all(choice().to_string().as_bytes())
        })
        .expect(1)
        .create_async()
        .await;
    let mut http = client(&server);
    http.client = hardened_client_builder(TransportPolicy::AllowLoopbackCleartext)
        .no_proxy()
        .retry(reqwest::retry::never())
        .timeout(Duration::from_millis(80))
        .build()
        .unwrap();
    // The valid response arrives before the outer three-second deadline.
    // Only the shorter HTTP timeout can produce this error.
    let failure = http
        .choose(&text(), "offline-fixture", &Guard::default())
        .await
        .err()
        .unwrap();
    assert_eq!(failure.reason, DecisionUnavailable::Timeout);
    assert!(failure.attempted);
    assert!(failure.request_hash.is_some());
    assert!(failure.observation.is_none());
    mock.assert_async().await;
}

#[tokio::test]
async fn gateway_http_does_not_decompress_an_unrequested_encoded_response() {
    use std::io::Write;
    let mut server = Server::new_async().await;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(choice().to_string().as_bytes()).unwrap();
    let body = encoder.finish().unwrap();
    let mock = server
        .mock("POST", "/typesafe/v1/systemone")
        .with_header("content-encoding", "gzip")
        .with_body(body)
        .expect(1)
        .create_async()
        .await;
    let failure = client(&server)
        .choose(&text(), "offline-fixture", &Guard::default())
        .await
        .err()
        .unwrap();
    assert_eq!(failure.reason, DecisionUnavailable::InvalidResponse);
    assert!(failure.attempted);
    assert!(failure.observation.is_none());
    mock.assert_async().await;
}
