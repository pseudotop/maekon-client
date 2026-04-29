use super::*;
use super::{parsers, request};
use oneshim_core::config::ExternalApiEndpoint;

#[test]
fn system_prompt_not_empty() {
    let prompt = request::system_prompt();
    assert!(!prompt.is_empty());
    assert!(prompt.contains("JSON"));
}

#[test]
fn new_remote_llm_rejects_retired_model_by_policy() {
    let config = ExternalApiEndpoint {
        endpoint: "https://api.openai.com/v1/chat/completions".to_string(),
        api_key: "test-api-key".to_string(),
        model: Some("gpt-3.5-turbo".to_string()),
        timeout_secs: 30,
        provider_type: AiProviderType::OpenAi,
        surface_id: None,
        credential: None,
    };

    let result = RemoteLlmProvider::new(&config, crate::CircuitBreakerRegistry::new());
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("retired as of"));
}

#[test]
fn openai_llm_uses_spec_default_model() {
    let config = ExternalApiEndpoint {
        endpoint: "https://api.openai.com/v1/responses".to_string(),
        api_key: "test-api-key".to_string(),
        model: None,
        timeout_secs: 30,
        provider_type: AiProviderType::OpenAi,
        surface_id: None,
        credential: None,
    };

    let provider = RemoteLlmProvider::new(&config, crate::CircuitBreakerRegistry::new())
        .expect("provider should initialize");
    assert_eq!(provider.model, "gpt-5.4");
    assert_eq!(
        provider.llm_request_shape().expect("shape should resolve"),
        ProviderRequestShape::OpenAiResponses
    );
}

#[test]
fn new_remote_llm_rejects_known_non_llm_model() {
    let config = ExternalApiEndpoint {
        endpoint: "https://api.openai.com/v1/responses".to_string(),
        api_key: "test-api-key".to_string(),
        model: Some("text-embedding-3-small".to_string()),
        timeout_secs: 30,
        provider_type: AiProviderType::OpenAi,
        surface_id: Some("provider_surface.openai.direct_api".to_string()),
        credential: None,
    };

    let result = RemoteLlmProvider::new(&config, crate::CircuitBreakerRegistry::new());
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("not marked as LLM-capable"));
}

#[test]
fn ollama_llm_initializes_without_api_key() {
    let config = ExternalApiEndpoint {
        endpoint: "http://localhost:11434/v1/responses".to_string(),
        api_key: String::new(),
        model: None,
        timeout_secs: 30,
        provider_type: AiProviderType::Ollama,
        surface_id: Some("provider_surface.ollama.local_http".to_string()),
        credential: None,
    };

    let provider = RemoteLlmProvider::new(&config, crate::CircuitBreakerRegistry::new())
        .expect("ollama llm should initialize");
    assert_eq!(provider.model, "qwen3:8b");
    assert_eq!(
        provider.llm_request_shape().expect("shape should resolve"),
        ProviderRequestShape::OpenAiResponses
    );
}

#[test]
fn google_llm_rewrites_endpoint_for_selected_model() {
    let config = ExternalApiEndpoint {
            endpoint: "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:generateContent"
                .to_string(),
            api_key: "goog-api-key".to_string(),
            model: Some("gemini-2.5-pro".to_string()),
            timeout_secs: 30,
            provider_type: AiProviderType::Google,
            surface_id: Some("provider_surface.google.direct_api".to_string()),
            credential: None,
        };

    let provider = RemoteLlmProvider::new(&config, crate::CircuitBreakerRegistry::new())
        .expect("google llm should initialize");
    assert_eq!(
        provider.endpoint,
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-pro:generateContent"
    );
}

#[test]
fn build_user_prompt_basic() {
    let ctx = ScreenContext {
        visible_texts: vec!["file".to_string(), "save".to_string()],
        active_app: "VSCode".to_string(),
        active_window_title: "main.rs".to_string(),
        layout_description: None,
    };
    let prompt = request::build_user_prompt(&ctx, "click the save button");
    assert!(prompt.contains("VSCode"));
    assert!(prompt.contains("file"));
    assert!(prompt.contains("click the save button"));
}

#[test]
fn build_user_prompt_with_layout() {
    let ctx = ScreenContext {
        visible_texts: vec![],
        active_app: "Chrome".to_string(),
        active_window_title: "Google".to_string(),
        layout_description: Some("Search bar is centered at the top".to_string()),
    };
    let prompt = request::build_user_prompt(&ctx, "search");
    assert!(prompt.contains("Layout"));
    assert!(prompt.contains("Search bar is centered at the top"));
}

#[test]
fn parse_claude_response_valid() {
    let body = r#"{
            "content": [{
                "type": "text",
                "text": "{\"target_text\": \"save\", \"target_role\": \"button\", \"action_type\": \"click\", \"confidence\": 0.92}"
            }]
        }"#;
    let action = parsers::parse_claude_response(body).unwrap();
    assert_eq!(action.target_text.unwrap(), "save");
    assert_eq!(action.action_type, "click");
    assert!((action.confidence - 0.92).abs() < f64::EPSILON);
}

#[test]
fn parse_claude_response_with_markdown() {
    let body = r#"{
            "content": [{
                "type": "text",
                "text": "Analysis result:\n```json\n{\"target_text\": \"Confirm\", \"target_role\": null, \"action_type\": \"click\", \"confidence\": 0.85}\n```"
            }]
        }"#;
    let action = parsers::parse_claude_response(body).unwrap();
    assert_eq!(action.target_text.unwrap(), "Confirm");
    assert_eq!(action.action_type, "click");
}

#[test]
fn parse_openai_response_valid() {
    let body = r#"{
            "choices": [{
                "message": {
                    "content": "{\"target_text\": \"Submit\", \"target_role\": \"button\", \"action_type\": \"click\", \"confidence\": 0.88}"
                }
            }]
        }"#;
    let action = parsers::parse_openai_response(body).unwrap();
    assert_eq!(action.target_text.unwrap(), "Submit");
    assert_eq!(action.target_role.unwrap(), "button");
}

#[test]
fn parse_openai_response_with_content_array() {
    let body = r#"{
            "choices": [{
                "message": {
                    "content": [
                        {
                            "type": "text",
                            "text": "{\"target_text\": \"Apply\", \"target_role\": \"button\", \"action_type\": \"click\", \"confidence\": 0.74}"
                        }
                    ]
                }
            }]
        }"#;

    let action = parsers::parse_openai_response(body).unwrap();
    assert_eq!(action.target_text.unwrap(), "Apply");
    assert_eq!(action.action_type, "click");
}

#[test]
fn parse_openai_response_with_output_text() {
    let body = r#"{
            "output_text": "{\"target_text\": \"Save\", \"target_role\": \"button\", \"action_type\": \"click\", \"confidence\": 0.91}"
        }"#;

    let action = parsers::parse_openai_response(body).unwrap();
    assert_eq!(action.target_text.unwrap(), "Save");
    assert_eq!(action.action_type, "click");
}

#[test]
fn parse_claude_response_invalid_json() {
    let body = r#"{"content": [{"type": "text", "text": "not json at all"}]}"#;
    let result = parsers::parse_claude_response(body);
    assert!(result.is_err());
}

#[test]
fn parse_openai_response_no_choices() {
    let body = r#"{"choices": []}"#;
    let result = parsers::parse_openai_response(body);
    assert!(result.is_err());
}

/// Iter-151 regression guard: parsers that can't extract text from a
/// syntactically-valid LLM response envelope must emit
/// `CoreError::Analysis` / `provider.analysis_failed`, not
/// `Internal.Generic`. The provider responded; the provider misbehaved;
/// telemetry should attribute that to the LLM, not our internals.
#[test]
fn claude_empty_envelope_maps_to_analysis_failed() {
    let body = r#"{"content": []}"#;
    let err = match parsers::parse_claude_response(body) {
        Ok(_) => panic!("empty claude envelope should fail"),
        Err(e) => e,
    };
    assert_eq!(err.code(), "provider.analysis_failed");
}

#[test]
fn openai_empty_envelope_maps_to_analysis_failed() {
    // No "choices" key, no "output_text", no "output" — the extractor
    // has no path to any text. This is the envelope-exhaustion case.
    let body = r#"{}"#;
    let err = match parsers::parse_openai_response(body) {
        Ok(_) => panic!("empty openai envelope should fail"),
        Err(e) => e,
    };
    assert_eq!(err.code(), "provider.analysis_failed");
}

#[test]
fn google_empty_envelope_maps_to_analysis_failed() {
    let body = r#"{"candidates": []}"#;
    let err = match parsers::parse_google_response(body) {
        Ok(_) => panic!("empty google envelope should fail"),
        Err(e) => e,
    };
    assert_eq!(err.code(), "provider.analysis_failed");
}

#[test]
fn build_system_prompt_no_skills() {
    let ctx = SkillContext::default();
    let prompt = request::build_system_prompt(&ctx);
    assert!(prompt.contains("UI automation agent"));
    assert!(!prompt.contains("Available skills"));
}

#[test]
fn build_system_prompt_with_available_skills() {
    let ctx = SkillContext {
        available_skills: vec![
            oneshim_core::models::skill::SkillMeta {
                name: "coding".into(),
                description: "Write code".into(),
            },
            oneshim_core::models::skill::SkillMeta {
                name: "review".into(),
                description: "Review code".into(),
            },
        ],
        active_skill_body: None,
    };
    let prompt = request::build_system_prompt(&ctx);
    assert!(prompt.contains("Available skills:"));
    assert!(prompt.contains("coding: Write code"));
    assert!(prompt.contains("review: Review code"));
    assert!(!prompt.contains("Active Skill"));
}

#[test]
fn build_system_prompt_with_active_skill() {
    let ctx = SkillContext {
        available_skills: vec![],
        active_skill_body: Some("# Do the thing\nStep 1: click.".into()),
    };
    let prompt = request::build_system_prompt(&ctx);
    assert!(prompt.contains("--- Active Skill ---"));
    assert!(prompt.contains("Do the thing"));
    assert!(prompt.contains("--- End Skill ---"));
}

#[test]
fn responses_api_body_format() {
    let config = ExternalApiEndpoint {
        endpoint: "https://chatgpt.com/backend-api/codex".to_string(),
        api_key: "test-key".to_string(),
        model: Some("gpt-5.4".to_string()),
        timeout_secs: 30,
        provider_type: AiProviderType::OpenAi,
        surface_id: None,
        credential: None,
    };
    let provider = RemoteLlmProvider::new(&config, crate::CircuitBreakerRegistry::new()).unwrap();
    let body = provider.build_responses_api_body("system prompt", "user input");

    assert_eq!(body["model"], "gpt-5.4");
    assert_eq!(body["instructions"], "system prompt");
    assert_eq!(body["input"], "user input");
    assert_eq!(body["max_output_tokens"], 512);
    // Responses API should NOT have "messages" field.
    assert!(body.get("messages").is_none());
}

#[test]
fn openai_llm_uses_responses_api_from_spec() {
    let config = ExternalApiEndpoint {
        endpoint: "https://api.openai.com/v1/responses".to_string(),
        api_key: "test-key".to_string(),
        model: Some("gpt-5.4".to_string()),
        timeout_secs: 30,
        provider_type: AiProviderType::OpenAi,
        surface_id: None,
        credential: None,
    };
    let provider = RemoteLlmProvider::new(&config, crate::CircuitBreakerRegistry::new()).unwrap();
    assert!(provider.uses_responses_api());
}

#[test]
fn managed_openai_surface_uses_surface_shape() {
    let config = ExternalApiEndpoint {
        endpoint: "https://chatgpt.com/backend-api/codex".to_string(),
        api_key: "test-key".to_string(),
        model: None,
        timeout_secs: 30,
        provider_type: AiProviderType::OpenAi,
        surface_id: Some("provider_surface.openai.managed_oauth".to_string()),
        credential: None,
    };
    let provider = RemoteLlmProvider::new(&config, crate::CircuitBreakerRegistry::new()).unwrap();
    assert_eq!(provider.model, "gpt-5.4");
    assert_eq!(
        provider.llm_request_shape().expect("shape should resolve"),
        ProviderRequestShape::OpenAiResponses
    );
}

#[test]
fn local_openai_compatible_llm_requires_explicit_model_selection() {
    let config = ExternalApiEndpoint {
        endpoint: "http://127.0.0.1:1234/v1/chat/completions".to_string(),
        api_key: String::new(),
        model: None,
        timeout_secs: 30,
        provider_type: AiProviderType::Generic,
        surface_id: Some("provider_surface.generic.local_openai_compatible".to_string()),
        credential: None,
    };
    let result = RemoteLlmProvider::new(&config, crate::CircuitBreakerRegistry::new());
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("requires an explicit model selection"));
}

// iter-68 regression guards for iter-55b semantic HTTP status mapping
// in ai_llm_client/request::send_and_parse. Shared helper pattern
// mirrors iter-67's remote_embedding_client tests.
#[cfg(test)]
mod http_status_mapping {
    use super::*;
    use oneshim_core::ports::llm_provider::{LlmProvider, ScreenContext};

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
        let provider = RemoteLlmProvider::new(&config, crate::CircuitBreakerRegistry::new())
            .expect("provider init");
        let ctx = ScreenContext {
            visible_texts: vec!["Save".to_string()],
            active_app: "App".to_string(),
            active_window_title: "Window".to_string(),
            layout_description: None,
        };
        provider
            .interpret_intent(&ctx, "click save")
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

    /// iter-78: domain fallback. Unmapped statuses stay as CoreError::Network.
    #[tokio::test]
    async fn status_500_falls_back_to_network() {
        let err = run_status_mapping_test(500).await;
        assert!(
            matches!(err, CoreError::Network { .. }),
            "500 should fall back to Network, got: {err:?}"
        );
    }

    // ── D7 Circuit breaker behavior ───────────────────────────────────────

    fn breaker_registry_fast_llm(server_url: &str) -> Arc<crate::CircuitBreakerRegistry> {
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

    fn test_screen_ctx() -> ScreenContext {
        ScreenContext {
            visible_texts: vec!["Save".to_string()],
            active_app: "App".to_string(),
            active_window_title: "Window".to_string(),
            layout_description: None,
        }
    }

    fn make_llm_provider(
        server_url: &str,
        registry: Arc<crate::CircuitBreakerRegistry>,
    ) -> RemoteLlmProvider {
        let config = ExternalApiEndpoint {
            endpoint: server_url.to_string(),
            api_key: "test-key".to_string(),
            model: Some("claude-sonnet-4-20250514".to_string()),
            timeout_secs: 30,
            provider_type: AiProviderType::Anthropic,
            surface_id: None,
            credential: None,
        };
        RemoteLlmProvider::new(&config, registry).expect("provider init")
    }

    #[tokio::test]
    async fn breaker_open_fast_fails_llm() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(503)
            .with_body("down")
            .expect_at_most(3)
            .create_async()
            .await;

        let registry = breaker_registry_fast_llm(&server.url());
        let provider = make_llm_provider(&server.url(), registry);
        for _ in 0..3 {
            let _ = provider
                .interpret_intent(&test_screen_ctx(), "click save")
                .await;
        }
        let result = provider
            .interpret_intent(&test_screen_ctx(), "click save")
            .await;
        match result {
            Err(CoreError::ServiceUnavailable { code, .. }) => {
                assert_eq!(code, oneshim_core::error_codes::ServiceCode::CircuitOpen);
            }
            other => panic!("expected CircuitOpen, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn breaker_half_open_failure_doubles_cooldown_llm() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(503)
            .with_body("down")
            .create_async()
            .await;

        let registry = breaker_registry_fast_llm(&server.url());
        let provider = make_llm_provider(&server.url(), registry.clone());
        for _ in 0..3 {
            let _ = provider
                .interpret_intent(&test_screen_ctx(), "click save")
                .await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(70)).await;
        let _ = provider
            .interpret_intent(&test_screen_ctx(), "click save")
            .await;

        let key = crate::resilience::endpoint_authority(&server.url()).unwrap();
        let breaker = registry.get(&key);
        assert_eq!(
            breaker.stats().current_cooldown,
            std::time::Duration::from_millis(100)
        );
    }

    /// Registry-sharing test (spec §Testing integration test): two adapters
    /// pointing at the same endpoint share one breaker. When the LLM provider
    /// trips, the embedding provider sees Open immediately.
    #[tokio::test]
    async fn shared_registry_trips_across_adapters() {
        use crate::remote_embedding_client::RemoteEmbeddingProvider;
        use oneshim_core::ports::embedding_provider::EmbeddingProvider;

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(503)
            .with_body("down")
            .create_async()
            .await;

        let registry = breaker_registry_fast_llm(&server.url());
        let llm = make_llm_provider(&server.url(), registry.clone());
        let emb = RemoteEmbeddingProvider::new(
            server.url(),
            "test-key".to_string(),
            "text-embedding-3-small".to_string(),
            3,
            30,
            registry.clone(),
        );

        // Trip via the LLM's 3 failures.
        for _ in 0..3 {
            let _ = llm.interpret_intent(&test_screen_ctx(), "click save").await;
        }
        // Embedding client sharing the same endpoint sees Open immediately —
        // no server hit, just the local fast-fail.
        let result = emb.embed("test text").await;
        match result {
            Err(CoreError::ServiceUnavailable { code, .. }) => {
                assert_eq!(
                    code,
                    oneshim_core::error_codes::ServiceCode::CircuitOpen,
                    "shared registry should propagate Open state across adapters"
                );
            }
            other => panic!("expected CircuitOpen via shared registry, got {other:?}"),
        }
    }
}
