use std::env;
use std::time::Duration;

use oneshim_core::config::{AiProviderType, ExternalApiEndpoint};
use oneshim_core::ports::llm_provider::{LlmProvider, ScreenContext};
use oneshim_core::ports::ocr_provider::OcrProvider;
use oneshim_network::ai_llm_client::RemoteLlmProvider;
use oneshim_network::ai_ocr_client::RemoteOcrProvider;
use tokio::time::sleep;

const RUN_SMOKE_ENV: &str = "ONESHIM_RUN_AI_LIVE_SMOKE";
const RUN_OCR_ENV: &str = "ONESHIM_AI_SMOKE_RUN_OCR";

const SAMPLE_PNG_1X1: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0,
    0, 0, 31, 21, 196, 137, 0, 0, 0, 10, 73, 68, 65, 84, 120, 156, 99, 96, 0, 0, 0, 2, 0, 1, 226,
    33, 188, 51, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ai_provider_live_smoke() {
    if !should_run_live_smoke() {
        eprintln!(
            "Skipping AI provider live smoke test because {} is not enabled.",
            RUN_SMOKE_ENV
        );
        return;
    }

    run_llm_smoke().await;

    if parse_bool_env(RUN_OCR_ENV).unwrap_or(false) {
        run_ocr_smoke().await;
    } else {
        eprintln!(
            "Skipping OCR live smoke because {} is disabled.",
            RUN_OCR_ENV
        );
    }
}

async fn run_llm_smoke() {
    let endpoint = build_endpoint(SmokeTarget::Llm);
    let provider =
        RemoteLlmProvider::new(&endpoint, oneshim_network::CircuitBreakerRegistry::new())
            .expect("LLM provider setup failed");

    let screen_context = ScreenContext {
        visible_texts: vec![
            "File".to_string(),
            "Save".to_string(),
            "Cancel".to_string(),
            "Settings".to_string(),
        ],
        active_app: "SmokeApp".to_string(),
        active_window_title: "AI Provider Live Smoke".to_string(),
        layout_description: Some("Save button is in the lower-right area".to_string()),
    };

    let mut last_err = None;
    for attempt in 1..=2 {
        match provider
            .interpret_intent(
                &screen_context,
                "click the save button and ignore cancel button",
            )
            .await
        {
            Ok(action) => {
                assert!(
                    !action.action_type.trim().is_empty(),
                    "LLM returned empty action_type"
                );
                assert!(
                    action.confidence.is_finite() && (0.0..=1.0).contains(&action.confidence),
                    "LLM confidence must be finite and in 0.0..=1.0, got {}",
                    action.confidence
                );
                return;
            }
            Err(err) => {
                last_err = Some(err);
                if attempt < 2 {
                    sleep(Duration::from_secs(2)).await;
                }
            }
        }
    }

    panic!(
        "LLM smoke test failed after retries: {}",
        last_err.expect("last error missing")
    );
}

async fn run_ocr_smoke() {
    let endpoint = build_endpoint(SmokeTarget::Ocr);
    let provider =
        RemoteOcrProvider::new(&endpoint, oneshim_network::CircuitBreakerRegistry::new())
            .expect("OCR provider setup failed");

    let mut last_err = None;
    for attempt in 1..=2 {
        match provider.extract_elements(SAMPLE_PNG_1X1, "png").await {
            Ok(results) => {
                assert!(
                    results.len() <= 2048,
                    "OCR result count too large: {}",
                    results.len()
                );
                for item in results.iter().take(16) {
                    assert!(item.confidence.is_finite(), "OCR confidence must be finite");
                    assert!(
                        (0.0..=1.0).contains(&item.confidence),
                        "OCR confidence must be in 0.0..=1.0, got {}",
                        item.confidence
                    );
                }
                return;
            }
            Err(err) => {
                last_err = Some(err);
                if attempt < 2 {
                    sleep(Duration::from_secs(2)).await;
                }
            }
        }
    }

    panic!(
        "OCR smoke test failed after retries: {}",
        last_err.expect("last error missing")
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SmokeTarget {
    Llm,
    Ocr,
}

impl SmokeTarget {
    fn env_prefix(self) -> &'static str {
        match self {
            Self::Llm => "ONESHIM_AI_SMOKE_LLM",
            Self::Ocr => "ONESHIM_AI_SMOKE_OCR",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProviderCapability {
    ocr_can_inherit_llm_endpoint: bool,
    ocr_can_inherit_llm_model: bool,
}

impl ProviderCapability {
    fn for_provider(provider_type: AiProviderType) -> Self {
        match provider_type {
            AiProviderType::Google => Self {
                ocr_can_inherit_llm_endpoint: false,
                ocr_can_inherit_llm_model: false,
            },
            AiProviderType::Anthropic
            | AiProviderType::OpenAi
            | AiProviderType::Ollama
            | AiProviderType::Bedrock
            | AiProviderType::Copilot
            | AiProviderType::Generic => Self {
                ocr_can_inherit_llm_endpoint: true,
                ocr_can_inherit_llm_model: true,
            },
        }
    }
}

fn build_endpoint(target: SmokeTarget) -> ExternalApiEndpoint {
    match target {
        SmokeTarget::Llm => build_llm_endpoint(),
        SmokeTarget::Ocr => build_ocr_endpoint(),
    }
}

fn build_llm_endpoint() -> ExternalApiEndpoint {
    let provider_type = resolve_provider_type(SmokeTarget::Llm);
    ExternalApiEndpoint {
        endpoint: required_primary_value(SmokeTarget::Llm, "ENDPOINT"),
        api_key: required_api_key_value(SmokeTarget::Llm),
        model: optional_primary_value(SmokeTarget::Llm, "MODEL"),
        timeout_secs: resolve_timeout_secs(SmokeTarget::Llm),
        provider_type,
        surface_id: None,
        credential: None,
    }
}

fn build_ocr_endpoint() -> ExternalApiEndpoint {
    let provider_type = resolve_provider_type(SmokeTarget::Ocr);
    let capability = ProviderCapability::for_provider(provider_type);

    let endpoint = if let Some(endpoint) = optional_primary_value(SmokeTarget::Ocr, "ENDPOINT") {
        endpoint
    } else if capability.ocr_can_inherit_llm_endpoint {
        required_primary_value(SmokeTarget::Llm, "ENDPOINT")
    } else {
        panic!(
            "Missing required env: {}. OCR provider type `{}` does not support endpoint inheritance from {}.",
            env_key(SmokeTarget::Ocr, "ENDPOINT"),
            provider_label(provider_type),
            env_key(SmokeTarget::Llm, "ENDPOINT"),
        );
    };

    let api_key = if let Some(api_key) = optional_api_key_value(SmokeTarget::Ocr) {
        api_key
    } else {
        required_api_key_value(SmokeTarget::Llm)
    };

    let model = if let Some(model) = optional_primary_value(SmokeTarget::Ocr, "MODEL") {
        Some(model)
    } else if capability.ocr_can_inherit_llm_model {
        optional_primary_value(SmokeTarget::Llm, "MODEL")
    } else {
        None
    };

    ExternalApiEndpoint {
        endpoint,
        api_key,
        model,
        timeout_secs: resolve_timeout_secs(SmokeTarget::Ocr),
        provider_type,
        surface_id: None,
        credential: None,
    }
}

fn resolve_provider_type(target: SmokeTarget) -> AiProviderType {
    let primary_key = env_key(target, "PROVIDER_TYPE");
    if let Some(raw) = optional_env(primary_key.as_str()) {
        return parse_provider_type(&raw);
    }

    if target == SmokeTarget::Ocr {
        let llm_key = env_key(SmokeTarget::Llm, "PROVIDER_TYPE");
        if let Some(raw) = optional_env(llm_key.as_str()) {
            return parse_provider_type(&raw);
        }
    }

    AiProviderType::Generic
}

fn resolve_timeout_secs(target: SmokeTarget) -> u64 {
    if let Some(value) = optional_timeout_secs(target) {
        return value;
    }

    if target == SmokeTarget::Ocr {
        if let Some(value) = optional_timeout_secs(SmokeTarget::Llm) {
            return value;
        }
    }

    45
}

fn optional_timeout_secs(target: SmokeTarget) -> Option<u64> {
    let key = env_key(target, "TIMEOUT_SECS");
    env::var(&key)
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|value| *value > 0)
}

fn required_primary_value(target: SmokeTarget, suffix: &str) -> String {
    let key = env_key(target, suffix);
    required_env(key.as_str())
}

fn optional_primary_value(target: SmokeTarget, suffix: &str) -> Option<String> {
    let key = env_key(target, suffix);
    optional_env(key.as_str())
}

fn env_key(target: SmokeTarget, suffix: &str) -> String {
    format!("{}_{}", target.env_prefix(), suffix)
}

fn provider_label(provider_type: AiProviderType) -> &'static str {
    match provider_type {
        AiProviderType::Anthropic => "anthropic",
        AiProviderType::OpenAi => "openai",
        AiProviderType::Google => "google",
        AiProviderType::Ollama => "ollama",
        AiProviderType::Bedrock => "bedrock",
        AiProviderType::Copilot => "copilot",
        AiProviderType::Generic => "generic",
    }
}

fn parse_provider_type(raw: &str) -> AiProviderType {
    match raw.trim().to_ascii_lowercase().as_str() {
        "anthropic" => AiProviderType::Anthropic,
        "openai" | "open_ai" | "open-ai" => AiProviderType::OpenAi,
        "google" | "gemini" => AiProviderType::Google,
        "ollama" => AiProviderType::Ollama,
        "bedrock" | "aws-bedrock" | "amazon-bedrock" => AiProviderType::Bedrock,
        "copilot" | "github-copilot" | "gh-copilot" => AiProviderType::Copilot,
        "generic" => AiProviderType::Generic,
        other => panic!("Unsupported provider type: {other}"),
    }
}

fn should_run_live_smoke() -> bool {
    parse_bool_env(RUN_SMOKE_ENV).unwrap_or(false)
}

fn parse_bool_env(key: &str) -> Option<bool> {
    env::var(key).ok().map(|raw| {
        matches!(
            raw.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn required_env(key: &str) -> String {
    let value = env::var(key).unwrap_or_else(|_| panic!("Missing required env: {key}"));
    if value.trim().is_empty() {
        panic!("Required env is empty: {key}");
    }
    value
}

fn optional_env(key: &str) -> Option<String> {
    env::var(key).ok().filter(|value| !value.trim().is_empty())
}

fn optional_api_key_value(target: SmokeTarget) -> Option<String> {
    let api_key_key = env_key(target, "API_KEY");
    if let Some(value) = optional_env(api_key_key.as_str()) {
        return Some(value);
    }

    let legacy_key = env_key(target, "KEY");
    if let Some(value) = optional_env(legacy_key.as_str()) {
        eprintln!(
            "Using deprecated env {}. Prefer {}.",
            legacy_key, api_key_key
        );
        return Some(value);
    }

    None
}

fn required_api_key_value(target: SmokeTarget) -> String {
    let api_key_key = env_key(target, "API_KEY");
    let legacy_key = env_key(target, "KEY");

    if let Some(value) = optional_api_key_value(target) {
        return value;
    }

    panic!("Missing required env: {} or {}", api_key_key, legacy_key);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn google_capability_disables_ocr_endpoint_and_model_inheritance() {
        let capability = ProviderCapability::for_provider(AiProviderType::Google);
        assert!(!capability.ocr_can_inherit_llm_endpoint);
        assert!(!capability.ocr_can_inherit_llm_model);
    }

    #[test]
    fn non_google_capability_allows_ocr_endpoint_and_model_inheritance() {
        for provider_type in [
            AiProviderType::Anthropic,
            AiProviderType::OpenAi,
            AiProviderType::Generic,
        ] {
            let capability = ProviderCapability::for_provider(provider_type);
            assert!(capability.ocr_can_inherit_llm_endpoint);
            assert!(capability.ocr_can_inherit_llm_model);
        }
    }
}
