use super::*;

#[test]
fn new_remote_ocr_empty_key_error() {
    let config = ExternalApiEndpoint {
        endpoint: "https://api.example.com".to_string(),
        api_key: "".to_string(),
        model: None,
        timeout_secs: 30,
        provider_type: AiProviderType::Generic,
        surface_id: None,
        credential: None,
    };
    let result = RemoteOcrProvider::new(&config, crate::CircuitBreakerRegistry::new());
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("not configured"));
}

#[test]
fn new_remote_ocr_with_key() {
    let config = ExternalApiEndpoint {
        endpoint: "https://api.example.com".to_string(),
        api_key: "test-api-key-placeholder".to_string(),
        model: None,
        timeout_secs: 30,
        provider_type: AiProviderType::Generic,
        surface_id: None,
        credential: None,
    };
    let result = RemoteOcrProvider::new(&config, crate::CircuitBreakerRegistry::new());
    assert!(result.is_ok());
}

#[test]
fn generic_ocr_uses_spec_shape_and_default_model() {
    let config = ExternalApiEndpoint {
        endpoint: "https://api.example.com".to_string(),
        api_key: "test-api-key-placeholder".to_string(),
        model: None,
        timeout_secs: 30,
        provider_type: AiProviderType::Generic,
        surface_id: None,
        credential: None,
    };

    let provider = RemoteOcrProvider::new(&config, crate::CircuitBreakerRegistry::new())
        .expect("generic OCR provider should build");
    assert_eq!(
        provider.ocr_request_shape().expect("shape should resolve"),
        ProviderRequestShape::OpenAiVisionChatCompletions
    );
    assert_eq!(provider.model.as_deref(), Some("gpt-5-mini"));
}

#[test]
fn ollama_ocr_initializes_without_api_key() {
    let config = ExternalApiEndpoint {
        endpoint: "http://localhost:11434/v1/chat/completions".to_string(),
        api_key: String::new(),
        model: None,
        timeout_secs: 30,
        provider_type: AiProviderType::Ollama,
        surface_id: Some("provider_surface.ollama.local_http".to_string()),
        credential: None,
    };

    let provider = RemoteOcrProvider::new(&config, crate::CircuitBreakerRegistry::new())
        .expect("ollama OCR provider should build");
    assert_eq!(
        provider.ocr_request_shape().expect("shape should resolve"),
        ProviderRequestShape::OpenAiVisionChatCompletions
    );
    assert_eq!(provider.model.as_deref(), Some("qwen3-vl:8b"));
}

#[test]
fn ollama_ocr_rejects_known_text_only_model() {
    let config = ExternalApiEndpoint {
        endpoint: "http://localhost:11434/v1/chat/completions".to_string(),
        api_key: String::new(),
        model: Some("qwen3:8b".to_string()),
        timeout_secs: 30,
        provider_type: AiProviderType::Ollama,
        surface_id: Some("provider_surface.ollama.local_http".to_string()),
        credential: None,
    };

    let result = RemoteOcrProvider::new(&config, crate::CircuitBreakerRegistry::new());
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("OCR-capable"));
}

#[test]
fn local_openai_compatible_ocr_requires_explicit_model_selection() {
    let config = ExternalApiEndpoint {
        endpoint: "http://127.0.0.1:1234/v1/chat/completions".to_string(),
        api_key: String::new(),
        model: None,
        timeout_secs: 30,
        provider_type: AiProviderType::Generic,
        surface_id: Some("provider_surface.generic.local_openai_compatible".to_string()),
        credential: None,
    };

    let result = RemoteOcrProvider::new(&config, crate::CircuitBreakerRegistry::new());
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("requires an explicit model selection"));
}

#[test]
fn local_openai_compatible_ocr_rejects_model_without_structured_output() {
    let config = ExternalApiEndpoint {
        endpoint: "http://127.0.0.1:1234/v1/chat/completions".to_string(),
        api_key: String::new(),
        model: Some("text-embedding-3-small".to_string()),
        timeout_secs: 30,
        provider_type: AiProviderType::Generic,
        surface_id: Some("provider_surface.generic.local_openai_compatible".to_string()),
        credential: None,
    };

    let result = RemoteOcrProvider::new(&config, crate::CircuitBreakerRegistry::new());
    assert!(result.is_err());
    let message = result.unwrap_err().to_string();
    assert!(
        message.contains("structured JSON output")
            || message.contains("OCR-capable")
            || message.contains("not marked as OCR-capable")
    );
}

#[test]
fn new_remote_ocr_rejects_retired_model_by_policy() {
    let config = ExternalApiEndpoint {
        endpoint: "https://api.openai.com/v1/chat/completions".to_string(),
        api_key: "test-api-key-placeholder".to_string(),
        model: Some("gpt-3.5-turbo".to_string()),
        timeout_secs: 30,
        provider_type: AiProviderType::OpenAi,
        surface_id: None,
        credential: None,
    };

    let result = RemoteOcrProvider::new(&config, crate::CircuitBreakerRegistry::new());
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("retired as of"));
}

#[test]
fn google_surface_uses_surface_transport_shape() {
    let config = ExternalApiEndpoint {
        endpoint: "https://vision.googleapis.com/v1/images:annotate".to_string(),
        api_key: "test-api-key-placeholder".to_string(),
        model: None,
        timeout_secs: 30,
        provider_type: AiProviderType::Google,
        surface_id: Some("provider_surface.google.direct_api".to_string()),
        credential: None,
    };

    let provider = RemoteOcrProvider::new(&config, crate::CircuitBreakerRegistry::new())
        .expect("google OCR provider should build");
    assert_eq!(
        provider.ocr_request_shape().expect("shape should resolve"),
        ProviderRequestShape::GoogleVisionAnnotate
    );
}

#[test]
fn new_remote_ocr_rejects_known_non_ocr_model() {
    let config = ExternalApiEndpoint {
        endpoint: "https://api.openai.com/v1/chat/completions".to_string(),
        api_key: "test-api-key-placeholder".to_string(),
        model: Some("text-embedding-3-small".to_string()),
        timeout_secs: 30,
        provider_type: AiProviderType::OpenAi,
        surface_id: Some("provider_surface.openai.direct_api".to_string()),
        credential: None,
    };

    let result = RemoteOcrProvider::new(&config, crate::CircuitBreakerRegistry::new());
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("not marked as OCR-capable"));
}

#[test]
fn google_ocr_rejects_explicit_model_selection() {
    let config = ExternalApiEndpoint {
        endpoint: "https://vision.googleapis.com/v1/images:annotate".to_string(),
        api_key: "test-api-key-placeholder".to_string(),
        model: Some("gemini-2.5-flash".to_string()),
        timeout_secs: 30,
        provider_type: AiProviderType::Google,
        surface_id: Some("provider_surface.google.direct_api".to_string()),
        credential: None,
    };

    let result = RemoteOcrProvider::new(&config, crate::CircuitBreakerRegistry::new());
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("does not support configurable model selection"));
}

#[test]
fn parse_claude_vision_response_valid() {
    let response = r#"{
            "content": [
                {
                    "type": "text",
                    "text": "file\nedit\nview\nsave"
                }
            ]
        }"#;
    let results = RemoteOcrProvider::parse_claude_vision_response(response).unwrap();
    assert_eq!(results.len(), 4);
    assert_eq!(results[0].text, "file");
    assert_eq!(results[3].text, "save");
}

#[test]
fn parse_claude_vision_response_empty() {
    let response = r#"{"content": [{"type": "text", "text": ""}]}"#;
    let results = RemoteOcrProvider::parse_claude_vision_response(response).unwrap();
    assert!(results.is_empty());
}

#[test]
fn parse_generic_response_valid() {
    let response = r#"{
            "results": [
                {"text": "save", "x": 100, "y": 200, "width": 60, "height": 25, "confidence": 0.95}
            ]
        }"#;
    let results = RemoteOcrProvider::parse_generic_response(response).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].text, "save");
    assert_eq!(results[0].x, 100);
}

#[test]
fn parse_generic_response_empty() {
    let response = r#"{"results": []}"#;
    let results = RemoteOcrProvider::parse_generic_response(response).unwrap();
    assert!(results.is_empty());
}

#[test]
fn parse_openai_vision_response_json_content() {
    let response = r#"{
            "choices": [{
                "message": {
                    "content": "{\"results\":[{\"text\":\"Save\",\"x\":12,\"y\":20,\"width\":48,\"height\":18,\"confidence\":0.93}]}"
                }
            }]
        }"#;

    let results = RemoteOcrProvider::parse_openai_vision_response(response).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].text, "Save");
    assert_eq!(results[0].x, 12);
    assert!((results[0].confidence - 0.93).abs() < f64::EPSILON);
}

#[test]
fn parse_openai_vision_response_line_fallback() {
    let response = r#"{
            "choices": [{
                "message": {
                    "content": "File\nEdit\nSave"
                }
            }]
        }"#;

    let results = RemoteOcrProvider::parse_openai_vision_response(response).unwrap();
    assert_eq!(results.len(), 3);
    assert_eq!(results[2].text, "Save");
}

#[test]
fn strategy_openai_request_uses_image_url_and_json_mode() {
    let payload =
        OcrProviderStrategy::OpenAi.build_request_body("ZmFrZS1pbWFnZQ==", "image/png", "gpt");
    assert_eq!(payload["response_format"]["type"], "json_object");
    assert_eq!(payload["messages"][0]["content"][1]["type"], "image_url");
    let url = payload["messages"][0]["content"][1]["image_url"]["url"]
        .as_str()
        .unwrap_or("");
    assert!(url.starts_with("data:image/png;base64,"));
}

#[test]
fn remote_ocr_provider_info() {
    let response = r#"{"content": [{"type": "text", "text": "test\nline2"}]}"#;
    let results = RemoteOcrProvider::parse_claude_vision_response(response).unwrap();
    assert_eq!(results.len(), 2);
    assert!((results[0].confidence - 0.8).abs() < f64::EPSILON);
}

/// ADR-019 §3 core security-fix regression guard: `apply_auth_headers`
/// must return a typed error on `AwsSignatureV4`, not silently fall
/// through (original pre-ADR-019 behavior). This is the foundational
/// contract that changed the function's signature from infallible to
/// `Result<_, CoreError>`. A regression that reverted the signature or
/// removed the error would silently send unauthenticated requests to
/// Bedrock endpoints — a real security bug.
#[test]
fn apply_auth_headers_rejects_aws_sigv4() {
    let client = reqwest::Client::new();
    let builder = client.get("https://bedrock-runtime.us-east-1.amazonaws.com");
    let result = apply_auth_headers(ProviderAuthScheme::AwsSignatureV4, builder, "irrelevant");
    match result {
        Err(CoreError::Config { code, message }) => {
            assert_eq!(
                code,
                oneshim_core::error_codes::ConfigCode::UnsupportedProviderBedrock,
                "expected UnsupportedProviderBedrock code, got {code:?}"
            );
            assert!(
                message.contains("Bedrock"),
                "expected Bedrock-mentioning message, got {message:?}"
            );
        }
        Ok(_) => panic!(
            "apply_auth_headers(AwsSignatureV4, ..) returned Ok — SILENT NO-AUTH FALLTHROUGH REGRESSION"
        ),
        Err(other) => panic!(
            "expected CoreError::Config {{ UnsupportedProviderBedrock, .. }}, got {other:?}"
        ),
    }
}

/// Positive control: non-AwsSignatureV4 schemes must still succeed after
/// the signature change, so we don't accidentally regress valid auth paths.
#[test]
fn apply_auth_headers_succeeds_for_supported_schemes() {
    let client = reqwest::Client::new();
    for scheme in [
        ProviderAuthScheme::None,
        ProviderAuthScheme::Bearer,
        ProviderAuthScheme::XApiKey,
        ProviderAuthScheme::XGoogApiKey,
    ] {
        let builder = client.get("https://api.example.com");
        let result = apply_auth_headers(scheme, builder, "test-key");
        assert!(
            result.is_ok(),
            "apply_auth_headers({scheme:?}, ..) unexpectedly failed: {:?}",
            result.err()
        );
    }
}

/// ADR-019 §3 regression guard: OcrProviderStrategy::try_from must reject
/// BedrockConverse shape at the strategy-dispatch boundary. Unlike the
/// other OCR paths that fail on catalog lookup (Bedrock absent), this one
/// takes the shape as direct input, so it's the only defense if someone
/// constructs the shape manually (e.g., unit-test fixture, re-introduction
/// without SigV4 work).
#[test]
fn ocr_strategy_try_from_rejects_bedrock_converse() {
    let result = OcrProviderStrategy::try_from(ProviderRequestShape::BedrockConverse);
    match result {
        Err(CoreError::Config { code, message }) => {
            assert_eq!(
                code,
                oneshim_core::error_codes::ConfigCode::UnsupportedProviderBedrock,
                "expected UnsupportedProviderBedrock code, got {code:?}"
            );
            assert!(
                message.contains("Bedrock"),
                "expected Bedrock-mentioning message, got {message:?}"
            );
        }
        Ok(strategy) => panic!(
            "expected Err for BedrockConverse but got Ok({strategy:?}) — a regression would enable OCR dispatch to an unsupported provider"
        ),
        Err(other) => panic!(
            "expected CoreError::Config {{ UnsupportedProviderBedrock, .. }}, got {other:?}"
        ),
    }
}

/// Positive control: every non-Bedrock shape must round-trip to a
/// valid strategy variant, so the dispatch table stays exhaustive after
/// the BedrockConverse Err arm.
#[test]
fn ocr_strategy_try_from_accepts_supported_shapes() {
    for shape in [
        ProviderRequestShape::AnthropicMessages,
        ProviderRequestShape::AnthropicVisionMessages,
        ProviderRequestShape::OpenAiChatCompletions,
        ProviderRequestShape::OpenAiVisionChatCompletions,
        ProviderRequestShape::OpenAiResponses,
        ProviderRequestShape::GoogleGenerateContent,
        ProviderRequestShape::GoogleVisionAnnotate,
    ] {
        let result = OcrProviderStrategy::try_from(shape);
        assert!(
            result.is_ok(),
            "OcrProviderStrategy::try_from({shape:?}) unexpectedly failed: {:?}",
            result.err()
        );
    }
}

// iter-69 regression guards for iter-59a semantic HTTP status mapping
// in ai_ocr_client::extract_elements. Shared helper pattern matches
// iter-67/68.
#[cfg(test)]
mod http_status_mapping {
    use super::*;
    use oneshim_core::ports::ocr_provider::OcrProvider;

    async fn run_status_mapping_test(status: u16) -> CoreError {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(status as usize)
            .with_body(format!(r#"{{"error": "http {status}"}}"#))
            .create_async()
            .await;

        let config = ExternalApiEndpoint {
            endpoint: server.url(),
            api_key: "test-key".to_string(),
            model: Some("claude-sonnet-4-20250514".to_string()),
            timeout_secs: 30,
            provider_type: AiProviderType::Anthropic,
            surface_id: None,
            credential: None,
        };
        let provider = RemoteOcrProvider::new(&config, crate::CircuitBreakerRegistry::new())
            .expect("provider init");
        // Minimal valid PNG (1x1 transparent pixel) for request body.
        let tiny_png = vec![
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9c, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ];
        provider
            .extract_elements(&tiny_png, "png")
            .await
            .unwrap_err()
    }

    #[tokio::test]
    async fn status_403_maps_to_auth() {
        let err = run_status_mapping_test(403).await;
        assert!(
            matches!(err, CoreError::Auth { .. }),
            "403 → Auth, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn status_408_maps_to_timeout() {
        let err = run_status_mapping_test(408).await;
        assert!(
            matches!(err, CoreError::RequestTimeout { .. }),
            "408 → RequestTimeout, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn status_429_maps_to_rate_limit() {
        let err = run_status_mapping_test(429).await;
        assert!(
            matches!(err, CoreError::RateLimit { .. }),
            "429 → RateLimit, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn status_502_maps_to_service_unavailable() {
        let err = run_status_mapping_test(502).await;
        assert!(
            matches!(err, CoreError::ServiceUnavailable { .. }),
            "502 → ServiceUnavailable, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn status_504_maps_to_timeout() {
        let err = run_status_mapping_test(504).await;
        assert!(
            matches!(err, CoreError::RequestTimeout { .. }),
            "504 → RequestTimeout, got: {err:?}"
        );
    }

    /// iter-77: domain fallback regression guard. OCR-specific errors
    /// (500, 418, etc.) should fall back to CoreError::OcrError, not to
    /// Network::Generic. Complements iter-72's cloud_stt fallback test.
    #[tokio::test]
    async fn status_500_falls_back_to_ocr_error() {
        let err = run_status_mapping_test(500).await;
        assert!(
            matches!(err, CoreError::OcrError { .. }),
            "500 should fall back to CoreError::OcrError (domain-specific), got: {err:?}"
        );
    }

    // ── D7 Circuit breaker behavior ───────────────────────────────────────

    fn tiny_png() -> Vec<u8> {
        vec![
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9c, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ]
    }

    fn breaker_registry_with_fast_config_ocr(
        server_url: &str,
    ) -> std::sync::Arc<crate::CircuitBreakerRegistry> {
        let registry = crate::CircuitBreakerRegistry::new();
        let key = crate::resilience::endpoint_authority(server_url).unwrap();
        let _ = registry.get_with_config(
            &key,
            crate::circuit_breaker::CircuitBreakerConfig {
                failure_threshold: 3,
                initial_cooldown: std::time::Duration::from_millis(50),
                max_cooldown: std::time::Duration::from_millis(200),
                half_open_probes: 1,
            },
        );
        registry
    }

    fn make_ocr_provider(
        server_url: &str,
        registry: std::sync::Arc<crate::CircuitBreakerRegistry>,
    ) -> RemoteOcrProvider {
        let config = ExternalApiEndpoint {
            endpoint: server_url.to_string(),
            api_key: "test-key".to_string(),
            model: Some("claude-sonnet-4-20250514".to_string()),
            timeout_secs: 30,
            provider_type: AiProviderType::Anthropic,
            surface_id: None,
            credential: None,
        };
        RemoteOcrProvider::new(&config, registry).expect("provider init")
    }

    #[tokio::test]
    async fn breaker_closed_passthrough_ocr() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(
                serde_json::json!({
                    "content": [{"type": "text", "text": "[]"}]
                })
                .to_string(),
            )
            .create_async()
            .await;

        let registry = breaker_registry_with_fast_config_ocr(&server.url());
        let provider = make_ocr_provider(&server.url(), registry);
        let _ = provider.extract_elements(&tiny_png(), "png").await;
        // Closed → server was hit; even if parsing fails, the breaker stayed closed.
    }

    #[tokio::test]
    async fn breaker_open_fast_fails_ocr() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(503)
            .with_body("down")
            .expect_at_most(3)
            .create_async()
            .await;

        let registry = breaker_registry_with_fast_config_ocr(&server.url());
        let provider = make_ocr_provider(&server.url(), registry);
        for _ in 0..3 {
            let _ = provider.extract_elements(&tiny_png(), "png").await;
        }
        let result = provider.extract_elements(&tiny_png(), "png").await;
        match result {
            Err(CoreError::ServiceUnavailable { code, .. }) => {
                assert_eq!(code, oneshim_core::error_codes::ServiceCode::CircuitOpen);
            }
            other => panic!("expected CircuitOpen, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn breaker_half_open_failure_doubles_cooldown_ocr() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(503)
            .with_body("down")
            .create_async()
            .await;

        let registry = breaker_registry_with_fast_config_ocr(&server.url());
        let provider = make_ocr_provider(&server.url(), registry.clone());
        for _ in 0..3 {
            let _ = provider.extract_elements(&tiny_png(), "png").await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(70)).await;
        let _ = provider.extract_elements(&tiny_png(), "png").await;

        let key = crate::resilience::endpoint_authority(&server.url()).unwrap();
        let breaker = registry.get(&key);
        assert_eq!(
            breaker.stats().current_cooldown,
            std::time::Duration::from_millis(100)
        );
    }

    #[tokio::test]
    async fn breaker_not_affected_by_caller_bug_4xx() {
        // 400 Bad Request is a caller bug (bad payload) — breaker should stay Closed.
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(400)
            .with_body("bad request")
            .create_async()
            .await;

        let registry = breaker_registry_with_fast_config_ocr(&server.url());
        let provider = make_ocr_provider(&server.url(), registry.clone());
        // 5 consecutive 400s — should NOT trip (Neutral classification).
        for _ in 0..5 {
            let _ = provider.extract_elements(&tiny_png(), "png").await;
        }
        let key = crate::resilience::endpoint_authority(&server.url()).unwrap();
        let breaker = registry.get(&key);
        assert!(
            matches!(
                breaker.check(),
                crate::circuit_breaker::CircuitState::Closed
            ),
            "400s should NOT trip the shared breaker (caller bug, not endpoint health)"
        );
    }
}
