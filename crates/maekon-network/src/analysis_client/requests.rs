use maekon_api_contracts::provider_specs::ProviderAuthScheme;
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
/// #10055 AC1: apply the auth header for a CATALOG-RESOLVED scheme.
///
/// Previously this matched on `AiProviderType` with a catch-all `_ => Bearer`,
/// which ignored `surface_id` entirely — so a `x_goog_api_key` surface got
/// `Authorization: Bearer` and the `Generic` surface's `bearer`/`none` split was
/// flattened. Mirrors `ai_llm_client::request`'s scheme arms so the suggestion
/// path and the automation path authenticate identically.
///
/// `AwsSignatureV4` is unreachable here: `dispatch` rejects Bedrock before
/// building a request (ADR-019 §3). It is still handled explicitly rather than
/// via a catch-all, so adding a future scheme is a compile error instead of a
/// silent Bearer.
pub(crate) fn apply_auth_headers(
    mut builder: RequestBuilder,
    auth_scheme: ProviderAuthScheme,
    api_key: &str,
) -> RequestBuilder {
    match auth_scheme {
        ProviderAuthScheme::None => {}
        ProviderAuthScheme::XApiKey => {
            builder = builder
                .header("x-api-key", api_key)
                .header("anthropic-version", crate::ANTHROPIC_API_VERSION);
        }
        ProviderAuthScheme::XGoogApiKey => {
            builder = builder.header("x-goog-api-key", api_key);
        }
        ProviderAuthScheme::Bearer => {
            // Preserve the historical empty-key behaviour: NoAuth credentials
            // resolve to an empty token, and adding `Bearer ` with nothing after
            // it would be worse than sending no header at all.
            if !api_key.is_empty() {
                builder = builder.header("Authorization", format!("Bearer {}", api_key));
            }
        }
        ProviderAuthScheme::AwsSignatureV4 => {}
    }
    builder
}
