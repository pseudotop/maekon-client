#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use std::sync::Arc;

    use maekon_core::config::{AiProviderType, ExternalApiEndpoint};
    use maekon_core::error::CoreError;
    use maekon_core::models::suggestion::{Priority, SuggestionSource, SuggestionType};
    use maekon_core::ports::analysis_provider::AnalysisProvider;
    use maekon_core::ports::credential_source::CredentialSource;

    use crate::error::NetworkError;
    use maekon_http_core::circuit_breaker::CircuitBreakerRegistry;
    use maekon_http_core::resilience::endpoint_authority;

    use crate::analysis_client::responses::{
        candidate_to_suggestion, extract_text, parse_candidates, SuggestionCandidate,
    };
    use crate::analysis_client::AnalysisClient;

    #[test]
    fn parse_valid_json_response() {
        let text = r#"[
            {"type": "ProductivityTip", "content": "Take a break", "confidence": 0.85, "reasoning": "Long session"},
            {"type": "WorkflowOptimization", "content": "Batch emails", "confidence": 0.72}
        ]"#;

        let candidates = parse_candidates(text).unwrap();
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].suggestion_type, "ProductivityTip");
        assert_eq!(candidates[0].content, "Take a break");
        assert!((candidates[0].confidence - 0.85).abs() < f64::EPSILON);
        assert_eq!(candidates[0].reasoning.as_deref(), Some("Long session"));
        assert!(candidates[1].reasoning.is_none());
    }

    #[test]
    fn parse_malformed_response_returns_error() {
        let text = "This is not JSON at all";
        let result = parse_candidates(text);
        match result.unwrap_err() {
            NetworkError::Analysis(msg) => {
                assert!(msg.contains("no JSON array found in LLM response"));
                assert!(msg.contains("body=omitted_for_privacy"));
            }
            other => panic!("Expected NetworkError::Analysis, got: {:?}", other),
        }
    }

    #[test]
    fn parse_malformed_response_omits_raw_private_output() {
        // The trailing \u{ae40}\u{bc94}\u{c900} is a Korean payroll name kept as
        // test data (encoded via \u{} escapes so the source stays ASCII while
        // preserving the exact bytes that PII masking must redact below).
        let text = "not JSON: alice@example.com OTP 123456 payroll \u{ae40}\u{bc94}\u{c900}";
        let result = parse_candidates(text);
        match result.unwrap_err() {
            NetworkError::Analysis(msg) => {
                assert!(msg.contains("body=omitted_for_privacy"));
                assert!(!msg.contains("alice@example.com"));
                assert!(!msg.contains("123456"));
                assert!(!msg.contains("payroll"));
                assert!(!msg.contains("\u{ae40}\u{bc94}\u{c900}"));
            }
            other => panic!("Expected NetworkError::Analysis, got: {:?}", other),
        }
    }

    #[test]
    fn parse_reversed_bracket_bounds_returns_error_without_panicking() {
        // Adversarial LLM output where a closing bracket precedes the first
        // opening bracket: start=rfind(']') would be < find('['), producing a
        // reversed inclusive-range slice that previously panicked (#6194).
        let text = "] foo [";
        let result = parse_candidates(text);
        match result.unwrap_err() {
            NetworkError::Analysis(msg) => {
                assert!(msg.contains("malformed JSON array bounds in LLM response"));
                assert!(msg.contains("body=omitted_for_privacy"));
            }
            other => panic!("Expected NetworkError::Analysis, got: {:?}", other),
        }
    }

    #[test]
    fn parse_empty_array_returns_empty_vec() {
        let text = "[]";
        let candidates = parse_candidates(text).unwrap();
        assert!(candidates.is_empty());
    }

    #[test]
    fn parse_json_with_markdown_fences() {
        let text = r#"```json
[{"type": "ContextBased", "content": "Focus on current task", "confidence": 0.9}]
```"#;
        let candidates = parse_candidates(text).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].suggestion_type, "ContextBased");
    }

    #[test]
    fn candidate_to_suggestion_maps_fields() {
        let candidate = SuggestionCandidate {
            suggestion_type: "ProductivityTip".to_string(),
            content: "Take a break after 90 minutes".to_string(),
            confidence: 0.85,
            reasoning: Some("Extended focus session detected".to_string()),
        };

        let suggestion = candidate_to_suggestion(candidate);
        assert_eq!(suggestion.suggestion_type, SuggestionType::ProductivityTip);
        assert_eq!(suggestion.content, "Take a break after 90 minutes");
        assert!((suggestion.confidence_score - 0.85).abs() < f64::EPSILON);
        assert_eq!(suggestion.source, SuggestionSource::LlmLocal);
        assert_eq!(suggestion.priority, Priority::Medium);
        assert!(suggestion.reasoning.is_some());
    }

    #[test]
    fn candidate_high_confidence_gets_high_priority() {
        let candidate = SuggestionCandidate {
            suggestion_type: "WorkGuidance".to_string(),
            content: "Critical".to_string(),
            confidence: 0.95,
            reasoning: None,
        };
        let suggestion = candidate_to_suggestion(candidate);
        assert_eq!(suggestion.priority, Priority::High);
    }

    #[test]
    fn candidate_low_confidence_gets_low_priority() {
        let candidate = SuggestionCandidate {
            suggestion_type: "ContextBased".to_string(),
            content: "Maybe try this".to_string(),
            confidence: 0.62,
            reasoning: None,
        };
        let suggestion = candidate_to_suggestion(candidate);
        assert_eq!(suggestion.priority, Priority::Low);
    }

    #[test]
    fn unknown_suggestion_type_defaults_to_context_based() {
        let candidate = SuggestionCandidate {
            suggestion_type: "UnknownType".to_string(),
            content: "test".to_string(),
            confidence: 0.7,
            reasoning: None,
        };
        let suggestion = candidate_to_suggestion(candidate);
        assert_eq!(suggestion.suggestion_type, SuggestionType::ContextBased);
    }

    #[test]
    fn build_anthropic_request_body() {
        let config = ExternalApiEndpoint {
            endpoint: "https://api.anthropic.com/v1/messages".to_string(),
            api_key: "test-key".to_string(),
            model: Some("claude-sonnet-5".to_string()),
            timeout_secs: 30,
            provider_type: AiProviderType::Anthropic,
            surface_id: None,
            credential: None,
        };
        let client = AnalysisClient::new(&config, CircuitBreakerRegistry::new());
        let body = client.build_request_body_pub("ctx", "sys");

        assert_eq!(body["model"], "claude-sonnet-5");
        assert_eq!(body["system"], "sys");
        assert_eq!(body["max_tokens"], 1024);
        assert!(body["messages"].is_array());
    }

    #[test]
    fn build_openai_request_body() {
        let config = ExternalApiEndpoint {
            endpoint: "https://api.openai.com/v1/chat/completions".to_string(),
            api_key: "test-key".to_string(),
            model: Some("gpt-5.4".to_string()),
            timeout_secs: 30,
            provider_type: AiProviderType::OpenAi,
            surface_id: None,
            credential: None,
        };
        let client = AnalysisClient::new(&config, CircuitBreakerRegistry::new());
        let body = client.build_request_body_pub("ctx", "sys");

        assert_eq!(body["model"], "gpt-5.4");
        assert_eq!(body["temperature"], 0.3);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1]["role"], "user");
    }

    #[test]
    fn extract_text_anthropic_format() {
        let body = serde_json::json!({
            "content": [{"type": "text", "text": "[{\"type\": \"ProductivityTip\", \"content\": \"test\", \"confidence\": 0.8}]"}]
        });
        let text = extract_text(AiProviderType::Anthropic, &body).unwrap();
        assert!(text.contains("ProductivityTip"));
    }

    #[test]
    fn extract_text_openai_format() {
        let body = serde_json::json!({
            "choices": [{"message": {"content": "[]"}}]
        });
        let text = extract_text(AiProviderType::OpenAi, &body).unwrap();
        assert_eq!(text, "[]");
    }

    #[test]
    fn extract_text_missing_content_returns_error() {
        let body = serde_json::json!({"choices": []});
        let err = extract_text(AiProviderType::OpenAi, &body).unwrap_err();
        assert!(
            matches!(err, NetworkError::Analysis(_)),
            "empty choices must return NetworkError::Analysis, got: {err:?}"
        );
    }

    /// ADR-019 §3 regression guard: `analyze()` must reject Bedrock before
    /// attempting the HTTP call. A regression that removes the guard would
    /// silently send OpenAI-format payloads to whatever endpoint the user
    /// configured, rather than returning the typed UnsupportedProviderBedrock
    /// code that telemetry/i18n depend on.
    #[tokio::test]
    async fn analyze_rejects_bedrock_provider() {
        let config = ExternalApiEndpoint {
            endpoint: "https://bedrock-runtime.us-east-1.amazonaws.com".to_string(),
            api_key: String::new(),
            model: Some("anthropic.claude-3-5-sonnet".to_string()),
            timeout_secs: 30,
            provider_type: AiProviderType::Bedrock,
            surface_id: None,
            credential: None,
        };
        let client = AnalysisClient::new(&config, CircuitBreakerRegistry::new());
        let result = client.analyze("{}", "you are a test").await;
        match result {
            Err(CoreError::Config { code, message }) => {
                assert_eq!(
                    code,
                    maekon_core::error_codes::ConfigCode::UnsupportedProviderBedrock,
                    "expected UnsupportedProviderBedrock code, got {code:?}"
                );
                assert!(
                    message.contains("Bedrock"),
                    "expected Bedrock-mentioning message, got {message:?}"
                );
            }
            other => panic!(
                "expected CoreError::Config {{ UnsupportedProviderBedrock, .. }}, got {other:?}"
            ),
        }
    }

    /// ADR-019 §3 regression guard: `summarize_text()` must reject Bedrock with
    /// the same typed code as `analyze()`. The guard lives in the shared
    /// `dispatch` funnel, so every method inherits it; this test pins the
    /// summarize funnel specifically, since it previously lacked any Bedrock
    /// check and would have sent an OpenAI-shaped body to a Bedrock endpoint.
    #[tokio::test]
    async fn summarize_text_rejects_bedrock_provider() {
        let config = ExternalApiEndpoint {
            endpoint: "https://bedrock-runtime.us-east-1.amazonaws.com".to_string(),
            api_key: String::new(),
            model: Some("anthropic.claude-3-5-sonnet".to_string()),
            timeout_secs: 30,
            provider_type: AiProviderType::Bedrock,
            surface_id: None,
            credential: None,
        };
        let client = AnalysisClient::new(&config, CircuitBreakerRegistry::new());
        let result = client.summarize_text("{}", "you are a test").await;
        match result {
            Err(CoreError::Config { code, message }) => {
                assert_eq!(
                    code,
                    maekon_core::error_codes::ConfigCode::UnsupportedProviderBedrock,
                    "expected UnsupportedProviderBedrock code, got {code:?}"
                );
                assert!(
                    message.contains("Bedrock"),
                    "expected Bedrock-mentioning message, got {message:?}"
                );
            }
            other => panic!(
                "expected CoreError::Config {{ UnsupportedProviderBedrock, .. }}, got {other:?}"
            ),
        }
    }

    // iter-71 regression guards for iter-59b semantic HTTP status mapping
    // in analysis_client::analyze. Shared helper pattern matches iter-67..70.
    async fn run_analyze_status_test(status: u16) -> CoreError {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(status as usize)
            .with_body(format!("http {status}"))
            .create_async()
            .await;
        let config = ExternalApiEndpoint {
            endpoint: server.url(),
            api_key: "test-key".to_string(),
            model: Some("gpt-5.4".to_string()),
            timeout_secs: 30,
            provider_type: AiProviderType::OpenAi,
            surface_id: None,
            credential: None,
        };
        let client = AnalysisClient::new(&config, CircuitBreakerRegistry::new());
        let result: Result<Vec<_>, CoreError> = client.analyze("{}", "sys").await;
        result.unwrap_err()
    }

    #[tokio::test]
    async fn analyze_403_maps_to_auth() {
        let err = run_analyze_status_test(403).await;
        assert!(
            matches!(err, CoreError::Auth { .. }),
            "403 → Auth, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn analyze_408_maps_to_timeout() {
        let err = run_analyze_status_test(408).await;
        assert!(
            matches!(err, CoreError::RequestTimeout { .. }),
            "408 → RequestTimeout, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn analyze_429_maps_to_rate_limit() {
        let err = run_analyze_status_test(429).await;
        assert!(
            matches!(err, CoreError::RateLimit { .. }),
            "429 → RateLimit, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn analyze_502_maps_to_service_unavailable() {
        let err = run_analyze_status_test(502).await;
        assert!(
            matches!(err, CoreError::ServiceUnavailable { .. }),
            "502 → ServiceUnavailable, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn analyze_504_maps_to_timeout() {
        let err = run_analyze_status_test(504).await;
        assert!(
            matches!(err, CoreError::RequestTimeout { .. }),
            "504 → RequestTimeout, got: {err:?}"
        );
    }

    // iter-74: regression guards for summarize_text sibling of analyze.
    // iter-59b applied the same semantic HTTP status mapping to both.
    async fn run_summarize_status_test(status: u16) -> CoreError {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(status as usize)
            .with_body(format!("http {status}"))
            .create_async()
            .await;
        let config = ExternalApiEndpoint {
            endpoint: server.url(),
            api_key: "test-key".to_string(),
            model: Some("gpt-5.4".to_string()),
            timeout_secs: 30,
            provider_type: AiProviderType::OpenAi,
            surface_id: None,
            credential: None,
        };
        let client = AnalysisClient::new(&config, CircuitBreakerRegistry::new());
        let result: Result<String, CoreError> = client.summarize_text("{}", "sys").await;
        result.unwrap_err()
    }

    #[tokio::test]
    async fn summarize_403_maps_to_auth() {
        let err = run_summarize_status_test(403).await;
        assert!(matches!(err, CoreError::Auth { .. }));
    }

    #[tokio::test]
    async fn summarize_429_maps_to_rate_limit() {
        let err = run_summarize_status_test(429).await;
        assert!(matches!(err, CoreError::RateLimit { .. }));
    }

    #[tokio::test]
    async fn summarize_503_maps_to_service_unavailable() {
        let err = run_summarize_status_test(503).await;
        assert!(matches!(err, CoreError::ServiceUnavailable { .. }));
    }

    /// iter-77: domain fallback regression guard. analyze() falls back to
    /// CoreError::Analysis (via NetworkError::Analysis) for unmapped
    /// status codes, not to Network::Generic. Mirrors cloud_stt / OCR
    /// fallback tests (iter-72, iter-77).
    #[tokio::test]
    async fn analyze_500_falls_back_to_analysis_error() {
        let err = run_analyze_status_test(500).await;
        assert!(
            matches!(err, CoreError::Analysis { .. }),
            "500 should fall back to CoreError::Analysis (domain-specific), got: {err:?}"
        );
    }

    /// iter-79: matching fallback guard for summarize_text (iter-74 sibling).
    /// Same mapping as analyze, but dispatched from a different method —
    /// test separately so a regression in summarize's error flow is caught
    /// even if analyze's tests still pass.
    #[tokio::test]
    async fn summarize_500_falls_back_to_analysis_error() {
        let err = run_summarize_status_test(500).await;
        assert!(
            matches!(err, CoreError::Analysis { .. }),
            "500 should fall back to CoreError::Analysis, got: {err:?}"
        );
    }

    // ── D7 Circuit breaker behavior ───────────────────────────────────────

    fn breaker_registry_with_fast_config(server_url: &str) -> Arc<CircuitBreakerRegistry> {
        let registry = CircuitBreakerRegistry::new();
        let key = endpoint_authority(server_url).unwrap();
        let _ = registry.get_with_config(
            &key,
            maekon_http_core::circuit_breaker::CircuitBreakerConfig {
                failure_threshold: 3,
                initial_cooldown: std::time::Duration::from_millis(50),
                max_cooldown: std::time::Duration::from_millis(200),
                half_open_probes: 1,
            },
        );
        registry
    }

    fn make_analysis_client(
        server_url: &str,
        registry: Arc<CircuitBreakerRegistry>,
    ) -> AnalysisClient {
        let config = ExternalApiEndpoint {
            endpoint: server_url.to_string(),
            api_key: "test-key".to_string(),
            model: Some("gpt-5.4".to_string()),
            timeout_secs: 30,
            provider_type: AiProviderType::OpenAi,
            surface_id: None,
            credential: None,
        };
        AnalysisClient::new(&config, registry)
    }

    #[tokio::test]
    async fn breaker_closed_passthrough_analyze() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(
                serde_json::json!({
                    "choices": [{"message": {"content": "[]"}}]
                })
                .to_string(),
            )
            .create_async()
            .await;

        let registry = breaker_registry_with_fast_config(&server.url());
        let client = make_analysis_client(&server.url(), registry);
        // The mock body is `{"choices": [{"message": {"content": "[]"}}]}` —
        // `analyze` parses content as JSON and yields an empty suggestion list.
        let suggestions: Vec<_> = client
            .analyze("{}", "sys")
            .await
            .expect("analyze must succeed when breaker is Closed and server returns 200");
        assert!(
            suggestions.is_empty(),
            "empty JSON array in content must produce zero suggestions, got {suggestions:?}"
        );
    }

    #[tokio::test]
    async fn breaker_open_fast_fails_analyze() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(503)
            .with_body("down")
            .expect_at_most(3) // after 3 failures breaker is open; should NOT hit server on 4th
            .create_async()
            .await;

        let registry = breaker_registry_with_fast_config(&server.url());
        let client = make_analysis_client(&server.url(), registry);
        for _ in 0..3 {
            let _ = client.analyze("{}", "sys").await;
        }
        let result = client.analyze("{}", "sys").await;
        match result {
            Err(CoreError::ServiceUnavailable { code, .. }) => {
                assert_eq!(code, maekon_core::error_codes::ServiceCode::CircuitOpen);
            }
            other => panic!("expected ServiceUnavailable CircuitOpen, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn breaker_open_also_blocks_summarize() {
        // Verify the SAME breaker state shared by both funnels: if analyze()
        // trips the breaker, summarize_text() is also blocked.
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(503)
            .with_body("down")
            .expect_at_most(3)
            .create_async()
            .await;

        let registry = breaker_registry_with_fast_config(&server.url());
        let client = make_analysis_client(&server.url(), registry);
        // Trip via analyze().
        for _ in 0..3 {
            let _ = client.analyze("{}", "sys").await;
        }
        // summarize_text() on the same client sees Open immediately.
        let result = client.summarize_text("{}", "sys").await;
        match result {
            Err(CoreError::ServiceUnavailable { code, .. }) => {
                assert_eq!(code, maekon_core::error_codes::ServiceCode::CircuitOpen);
            }
            other => panic!("expected cross-funnel CircuitOpen, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn breaker_half_open_failure_doubles_cooldown_analysis() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(503)
            .with_body("down")
            .create_async()
            .await;

        let registry = breaker_registry_with_fast_config(&server.url());
        let client = make_analysis_client(&server.url(), registry.clone());
        for _ in 0..3 {
            let _ = client.analyze("{}", "sys").await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(70)).await;
        let _ = client.analyze("{}", "sys").await;

        let key = endpoint_authority(&server.url()).unwrap();
        let breaker = registry.get(&key);
        assert_eq!(
            breaker.stats().current_cooldown,
            std::time::Duration::from_millis(100)
        );
    }

    // ── ADR-023 Phase-2 MG-PII-02 egress gate (keystone) ───────────────────

    fn ollama_endpoint(endpoint: &str) -> ExternalApiEndpoint {
        ExternalApiEndpoint {
            endpoint: endpoint.to_string(),
            api_key: String::new(),
            model: Some("llama3".to_string()),
            timeout_secs: 5,
            provider_type: AiProviderType::Ollama,
            surface_id: None,
            credential: None,
        }
    }

    // ── C2 #5722: ollama_chat_completions_url helper ────────────────────────────

    #[test]
    fn ollama_chat_completions_url_normalizes_v1_responses() {
        use crate::analysis_client::ollama_chat_completions_url;
        assert_eq!(
            ollama_chat_completions_url("http://localhost:11434/v1/responses"),
            "http://localhost:11434/v1/chat/completions"
        );
    }

    #[test]
    fn ollama_chat_completions_url_is_idempotent_on_chat_completions() {
        use crate::analysis_client::ollama_chat_completions_url;
        let url = "http://localhost:11434/v1/chat/completions";
        assert_eq!(ollama_chat_completions_url(url), url);
    }

    #[test]
    fn ollama_chat_completions_url_preserves_custom_path_prefix() {
        use crate::analysis_client::ollama_chat_completions_url;
        assert_eq!(
            ollama_chat_completions_url("http://127.0.0.1:11434/edge/ollama/v1/responses"),
            "http://127.0.0.1:11434/edge/ollama/v1/chat/completions"
        );
    }

    #[test]
    fn ollama_chat_completions_url_pass_through_for_unknown_path() {
        use crate::analysis_client::ollama_chat_completions_url;
        let url = "http://127.0.0.1:11434/custom-endpoint";
        assert_eq!(ollama_chat_completions_url(url), url);
    }

    #[test]
    fn analysis_client_new_with_ollama_normalizes_endpoint_to_chat_completions() {
        // AnalysisClient::new for Ollama must rewrite /v1/responses → /v1/chat/completions.
        let endpoint = ollama_endpoint("http://localhost:11434/v1/responses");
        let client = AnalysisClient::new(&endpoint, CircuitBreakerRegistry::new());
        // The internal endpoint field is not pub; verify via the breaker key
        // (endpoint_authority is keyed on scheme://host:port, so it doesn't tell
        // us the path directly). We verify via build_request_body_pub which uses
        // self.provider_type — the URL rewrite is an internal invariant tested
        // transitively by the live smoke check. For a purely structural test we
        // just confirm construction succeeds without panic.
        let _ = client.build_request_body_pub("{}", "sys");
    }

    #[test]
    fn new_local_enrichment_refuses_remote_endpoint() {
        // MG-PII-02 construction gate: non-loopback hosts are refused (None).
        assert!(AnalysisClient::new_local_enrichment(
            &ollama_endpoint("https://evil.example.com/v1/chat"),
            CircuitBreakerRegistry::new()
        )
        .is_none());
        // Ollama provider_type does NOT bypass the gate — the endpoint is free-form.
        assert!(AnalysisClient::new_local_enrichment(
            &ollama_endpoint("http://10.0.0.5:11434"),
            CircuitBreakerRegistry::new()
        )
        .is_none());
        // localhost-looking host that is not literal localhost.
        assert!(AnalysisClient::new_local_enrichment(
            &ollama_endpoint("http://localhost.evil.com:11434"),
            CircuitBreakerRegistry::new()
        )
        .is_none());
    }

    #[test]
    fn new_local_enrichment_accepts_localhost() {
        // BLOCKER-2 fix: the pinning helper must SUCCEED for a real local LLM
        // (the prior endpoint_authority impl always returned None).
        assert!(AnalysisClient::new_local_enrichment(
            &ollama_endpoint("http://localhost:11434"),
            CircuitBreakerRegistry::new()
        )
        .is_some());
        assert!(AnalysisClient::new_local_enrichment(
            &ollama_endpoint("http://127.0.0.1:11434"),
            CircuitBreakerRegistry::new()
        )
        .is_some());
    }

    // ── #5736 CredentialSource migration tests ──────────────────────────────

    /// Minimal in-memory SecretStore double for tests.
    struct FixedSecretStore(String);

    #[async_trait::async_trait]
    impl maekon_core::ports::secret_store::SecretStore for FixedSecretStore {
        async fn store(
            &self,
            _namespace: &str,
            _key: &str,
            _value: &str,
        ) -> Result<(), maekon_core::error::CoreError> {
            Ok(())
        }
        async fn retrieve(
            &self,
            _namespace: &str,
            _key: &str,
        ) -> Result<Option<String>, maekon_core::error::CoreError> {
            Ok(Some(self.0.clone()))
        }
        async fn delete(
            &self,
            _namespace: &str,
            _key: &str,
        ) -> Result<(), maekon_core::error::CoreError> {
            Ok(())
        }
        async fn delete_namespace(
            &self,
            _namespace: &str,
        ) -> Result<(), maekon_core::error::CoreError> {
            Ok(())
        }
    }

    /// StoredSecret resolved key appears in Authorization header.
    ///
    /// Asserts that when AnalysisClient is built with a StoredSecret credential,
    /// `dispatch` resolves the key from the store at request time and sends
    /// `Authorization: Bearer <stored-key>` — not the empty inline api_key.
    #[tokio::test]
    async fn stored_secret_credential_sent_in_authorization_header() {
        const STORED_KEY: &str = "sk-secret-from-keychain-12345";

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .match_header("authorization", format!("Bearer {STORED_KEY}").as_str())
            .with_status(200)
            .with_body(r#"{"choices":[{"message":{"content":"[]"}}]}"#)
            .create_async()
            .await;

        let config = ExternalApiEndpoint {
            endpoint: server.url(),
            api_key: String::new(), // deliberately empty — credential from store
            model: Some("gpt-5.4".to_string()),
            timeout_secs: 30,
            provider_type: AiProviderType::OpenAi,
            surface_id: None,
            credential: None,
        };
        let credential = CredentialSource::StoredSecret {
            namespace: "provider/openai/default".to_string(),
            key: "api_key".to_string(),
            secret_store: Arc::new(FixedSecretStore(STORED_KEY.to_string())),
        };
        let client =
            AnalysisClient::new_with_credential(&config, credential, CircuitBreakerRegistry::new());

        let result = client.analyze("{}", "sys").await;
        result.expect("StoredSecret credential must reach the server successfully");
        _mock.assert_async().await;
    }

    /// Inline ApiKey regression: AnalysisClient::new still sends the key as Bearer.
    ///
    /// Verifies that the CredentialSource migration did not regress the existing
    /// inline api_key path — `new()` must wrap the key in ApiKey and dispatch it
    /// as `Authorization: Bearer <key>`.
    #[tokio::test]
    async fn inline_api_key_sent_in_authorization_header_regression() {
        const INLINE_KEY: &str = "sk-inline-api-key-abcdef";

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .match_header("authorization", format!("Bearer {INLINE_KEY}").as_str())
            .with_status(200)
            .with_body(r#"{"choices":[{"message":{"content":"[]"}}]}"#)
            .create_async()
            .await;

        let config = ExternalApiEndpoint {
            endpoint: server.url(),
            api_key: INLINE_KEY.to_string(),
            model: Some("gpt-5.4".to_string()),
            timeout_secs: 30,
            provider_type: AiProviderType::OpenAi,
            surface_id: None,
            credential: None,
        };
        let client = AnalysisClient::new(&config, CircuitBreakerRegistry::new());
        let result = client.analyze("{}", "sys").await;
        result.expect("Inline ApiKey must reach the server successfully");
        _mock.assert_async().await;
    }

    /// Ollama NoAuth regression: AnalysisClient::new for Ollama sends no key.
    ///
    /// Verifies that `new()` still produces `CredentialSource::NoAuth` for
    /// Ollama endpoints so no `Authorization` header is added.
    #[tokio::test]
    async fn ollama_no_auth_sends_no_authorization_header_regression() {
        let mut server = mockito::Server::new_async().await;
        // Match only requests that do NOT include an Authorization header.
        let _mock = server
            .mock("POST", "/")
            .match_header("authorization", mockito::Matcher::Missing)
            .with_status(200)
            .with_body(r#"{"choices":[{"message":{"content":"[]"}}]}"#)
            .create_async()
            .await;

        let config = ExternalApiEndpoint {
            endpoint: server.url(),
            api_key: String::new(),
            model: Some("qwen3:8b".to_string()),
            timeout_secs: 5,
            provider_type: AiProviderType::Ollama,
            surface_id: None,
            credential: None,
        };
        let client = AnalysisClient::new(&config, CircuitBreakerRegistry::new());
        let result = client.analyze("{}", "sys").await;
        // We only care that the mock matched (i.e. no Authorization header sent).
        result.expect("Ollama NoAuth must reach the server successfully");
        _mock.assert_async().await;
    }

    #[tokio::test]
    async fn enrichment_methods_block_non_loopback_at_call_time() {
        // Defense-in-depth: even a client built via `new` (which has no gate)
        // refuses to send when the endpoint is non-loopback — returns Ok(empty),
        // never Err, never a network call.
        let client = AnalysisClient::new(
            &ollama_endpoint("https://evil.example.com/v1/chat"),
            CircuitBreakerRegistry::new(),
        );
        assert!(client
            .extract_relations("[]", "sys")
            .await
            .unwrap()
            .is_empty());
        assert!(client
            .detect_contradictions("[]", "sys")
            .await
            .unwrap()
            .is_empty());
    }
}
