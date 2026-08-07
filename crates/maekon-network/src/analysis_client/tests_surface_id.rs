#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use maekon_core::config::{AiProviderType, ExternalApiEndpoint};
    use maekon_core::error::CoreError;
    use maekon_core::ports::analysis_provider::AnalysisProvider;

    use maekon_http_core::circuit_breaker::CircuitBreakerRegistry;

    use super::super::AnalysisClient;

    fn endpoint_with_surface(
        endpoint: String,
        provider_type: AiProviderType,
        surface_id: Option<&str>,
        api_key: &str,
    ) -> ExternalApiEndpoint {
        ExternalApiEndpoint {
            endpoint,
            api_key: api_key.to_string(),
            model: Some("test-model".to_string()),
            timeout_secs: 5,
            provider_type,
            surface_id: surface_id.map(str::to_string),
            credential: None,
        }
    }

    /// AC2 — THE headline defect. The catalog's Google LLM surface is
    /// `google_generate_content`, but this client only ever sends a fixed
    /// chat-completions body and parses `choices[]`. It used to send that body
    /// (plus a wrong `Authorization: Bearer`) to `…:generateContent` and fail
    /// with a confusing downstream error. It must now fail closed BEFORE any
    /// request is issued.
    #[tokio::test]
    async fn google_generate_content_surface_fails_closed_without_sending() {
        let mut server = mockito::Server::new_async().await;
        // Any request at all is a failure for this test.
        let mock = server
            .mock("POST", mockito::Matcher::Any)
            .with_status(200)
            .create_async()
            .await;

        let config = endpoint_with_surface(
            server.url(),
            AiProviderType::Google,
            Some("provider_surface.google.direct_api"),
            "AIza-test-key",
        );
        let client = AnalysisClient::new(&config, CircuitBreakerRegistry::new());
        let result = client.analyze("{}", "sys").await;

        match result {
            Err(CoreError::Config { code, message }) => {
                assert_eq!(code, maekon_core::error_codes::ConfigCode::Invalid);
                assert!(
                    message.contains("provider_surface.google.direct_api"),
                    "error must name the offending surface, got {message:?}"
                );
            }
            other => panic!("expected fail-closed CoreError::Config, got {other:?}"),
        }
        mock.expect(0).assert_async().await;
    }

    /// AC3 — the generalized shape guard must NOT swallow Bedrock's specific
    /// typed code that telemetry/i18n depend on.
    #[tokio::test]
    async fn bedrock_still_reports_its_own_typed_code_not_the_shape_guard() {
        let config = endpoint_with_surface(
            "https://bedrock-runtime.us-east-1.amazonaws.com".to_string(),
            AiProviderType::Bedrock,
            None,
            "",
        );
        let client = AnalysisClient::new(&config, CircuitBreakerRegistry::new());
        match client.analyze("{}", "sys").await {
            Err(CoreError::Config { code, .. }) => assert_eq!(
                code,
                maekon_core::error_codes::ConfigCode::UnsupportedProviderBedrock,
                "Bedrock must keep its own code, not the generic Invalid"
            ),
            other => panic!("expected UnsupportedProviderBedrock, got {other:?}"),
        }
    }

    /// AC4 — Ollama's catalog shape is `openai_responses`, which the documented
    /// URL rewrite makes servable. Rejecting it would regress every
    /// wizard-configured Ollama user, so it must still reach the server.
    #[tokio::test]
    async fn ollama_openai_responses_surface_is_still_servable() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", mockito::Matcher::Any)
            .with_status(200)
            .with_body(r#"{"choices":[{"message":{"content":"[]"}}]}"#)
            .create_async()
            .await;

        let config = endpoint_with_surface(
            server.url(),
            AiProviderType::Ollama,
            Some("provider_surface.ollama.local_http"),
            "",
        );
        let client = AnalysisClient::new(&config, CircuitBreakerRegistry::new());
        client
            .analyze("{}", "sys")
            .await
            .expect("Ollama must remain servable on the analysis path");
        mock.assert_async().await;
    }

    /// AC1 — Anthropic resolves to `x_api_key`, so the header pair must be the
    /// Anthropic one and NOT `Authorization: Bearer`.
    #[tokio::test]
    async fn anthropic_surface_sends_x_api_key_not_bearer() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", mockito::Matcher::Any)
            .match_header("x-api-key", "sk-ant-test")
            .match_header("authorization", mockito::Matcher::Missing)
            .with_status(200)
            .with_body(r#"{"content":[{"text":"[]"}]}"#)
            .create_async()
            .await;

        let config = endpoint_with_surface(
            server.url(),
            AiProviderType::Anthropic,
            Some("provider_surface.anthropic.direct_api"),
            "sk-ant-test",
        );
        let client = AnalysisClient::new(&config, CircuitBreakerRegistry::new());
        client
            .analyze("{}", "sys")
            .await
            .expect("anthropic path must succeed");
        mock.assert_async().await;
    }

    /// AC1 — OpenAI resolves to `bearer`; the pre-#10055 catch-all happened to
    /// be right here, so this pins that it stays right after the refactor.
    #[tokio::test]
    async fn openai_surface_still_sends_bearer() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", mockito::Matcher::Any)
            .match_header("authorization", "Bearer sk-openai-test")
            .with_status(200)
            .with_body(r#"{"choices":[{"message":{"content":"[]"}}]}"#)
            .create_async()
            .await;

        let config = endpoint_with_surface(
            server.url(),
            AiProviderType::OpenAi,
            Some("provider_surface.openai.direct_api"),
            "sk-openai-test",
        );
        let client = AnalysisClient::new(&config, CircuitBreakerRegistry::new());
        client
            .analyze("{}", "sys")
            .await
            .expect("openai path must succeed");
        mock.assert_async().await;
    }
}
