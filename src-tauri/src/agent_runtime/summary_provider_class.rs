use maekon_core::config::AppConfig;
use maekon_core::models::ai_summary::AiSummaryProviderClass;

#[cfg(feature = "analysis")]
pub(super) fn classify(config: &AppConfig) -> AiSummaryProviderClass {
    use maekon_core::config::AiAccessMode;

    let access_mode = config.ai_provider.access_mode.normalized_for_ai_surfaces();
    if access_mode == AiAccessMode::ProviderSubscriptionCli {
        return AiSummaryProviderClass::Subprocess;
    }
    match config.ai_provider.llm_api.as_ref() {
        Some(endpoint) if maekon_http_core::outbound::host_is_loopback(&endpoint.endpoint) => {
            AiSummaryProviderClass::Loopback
        }
        Some(_) => AiSummaryProviderClass::ExternalApi,
        None if access_mode == AiAccessMode::LocalModel => AiSummaryProviderClass::Loopback,
        None => AiSummaryProviderClass::Unknown,
    }
}

#[cfg(not(feature = "analysis"))]
// This feature-absent fallback is intentionally the enum default. Replacing
// the expression with `Default::default()` is therefore an equivalent mutant;
// the no-analysis feature matrix is covered by its compile gate instead.
#[mutants::skip]
pub(super) fn classify(_config: &AppConfig) -> AiSummaryProviderClass {
    AiSummaryProviderClass::Unknown
}

#[cfg(all(test, feature = "analysis"))]
mod tests {
    use super::*;
    use maekon_core::config::{AiAccessMode, AiProviderType, ExternalApiEndpoint};

    #[test]
    fn distinguishes_subscription_loopback_and_external() {
        let mut config = AppConfig::default_config();
        assert_eq!(
            classify(&config),
            AiSummaryProviderClass::Unknown,
            "an API/OAuth mode without a configured endpoint must not claim loopback"
        );

        config.ai_provider.access_mode = AiAccessMode::LocalModel;
        assert_eq!(
            classify(&config),
            AiSummaryProviderClass::Loopback,
            "local-model mode without an explicit API endpoint is loopback"
        );

        config.ai_provider.access_mode = AiAccessMode::ProviderSubscriptionCli;
        assert_eq!(classify(&config), AiSummaryProviderClass::Subprocess);

        config.ai_provider.access_mode = AiAccessMode::ProviderApiKey;
        config.ai_provider.llm_api = Some(ExternalApiEndpoint {
            endpoint: "http://127.0.0.1:11434/v1/chat/completions".to_string(),
            api_key: String::new(),
            model: Some("local-test".to_string()),
            timeout_secs: 30,
            provider_type: AiProviderType::Ollama,
            surface_id: None,
            credential: None,
        });
        assert_eq!(classify(&config), AiSummaryProviderClass::Loopback);

        config.ai_provider.llm_api = Some(ExternalApiEndpoint {
            endpoint: "https://example.invalid/v1/chat/completions".to_string(),
            api_key: String::new(),
            model: Some("remote-test".to_string()),
            timeout_secs: 30,
            provider_type: AiProviderType::OpenAi,
            surface_id: None,
            credential: None,
        });
        assert_eq!(classify(&config), AiSummaryProviderClass::ExternalApi);
    }
}
