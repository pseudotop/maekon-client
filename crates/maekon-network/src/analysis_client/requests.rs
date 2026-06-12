use maekon_core::config::AiProviderType;
use reqwest::RequestBuilder;
use serde_json::Value;

/// Build provider-specific request body for analysis/summarize calls.
pub(crate) fn build_request_body(
    provider_type: AiProviderType,
    model: &str,
    context_json: &str,
    system_prompt: &str,
) -> Value {
    match provider_type {
        AiProviderType::Anthropic => {
            serde_json::json!({
                "model": model,
                "max_tokens": 1024,
                "system": system_prompt,
                "messages": [{"role": "user", "content": context_json}]
            })
        }
        AiProviderType::OpenAi
        | AiProviderType::Google
        | AiProviderType::Ollama
        | AiProviderType::Bedrock
        | AiProviderType::Copilot
        | AiProviderType::Generic => {
            serde_json::json!({
                "model": model,
                "messages": [
                    {"role": "system", "content": system_prompt},
                    {"role": "user", "content": context_json}
                ],
                "max_tokens": 1024,
                "temperature": 0.3
            })
        }
    }
}

/// Apply provider-specific auth headers to a request builder.
pub(crate) fn apply_auth_headers(
    mut builder: RequestBuilder,
    provider_type: AiProviderType,
    api_key: &str,
) -> RequestBuilder {
    match provider_type {
        AiProviderType::Anthropic => {
            builder = builder
                .header("x-api-key", api_key)
                .header("anthropic-version", crate::ANTHROPIC_API_VERSION);
        }
        _ => {
            if !api_key.is_empty() {
                builder = builder.header("Authorization", format!("Bearer {}", api_key));
            }
        }
    }
    builder
}
