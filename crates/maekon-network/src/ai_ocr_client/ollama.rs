use serde::Deserialize;
use serde_json::Value;

use maekon_core::error::CoreError;

#[derive(Debug, Deserialize)]
struct OllamaShowResponse {
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    details: Option<OllamaShowDetails>,
    #[serde(default)]
    projector_info: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct OllamaShowDetails {
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    families: Vec<String>,
}

fn derive_ollama_show_endpoint(endpoint: &str) -> String {
    let trimmed = endpoint.trim().trim_end_matches('/');
    for suffix in [
        "/v1/responses",
        "/v1/chat/completions",
        "/api/tags",
        "/api/show",
    ] {
        if let Some(prefix) = trimmed.strip_suffix(suffix) {
            return format!("{prefix}/api/show");
        }
    }
    format!("{trimmed}/api/show")
}

fn infer_ollama_vision_support(model: &str) -> bool {
    let normalized = model.trim().to_ascii_lowercase();
    [
        "vision",
        "vl",
        "llava",
        "bakllava",
        "moondream",
        "minicpm-v",
        "minicpmv",
        "gemma3",
    ]
    .iter()
    .any(|token| normalized.contains(token))
}

fn parse_ollama_show_supports_ocr(body: &str, model: &str) -> Result<Option<bool>, CoreError> {
    let parsed: OllamaShowResponse =
        serde_json::from_str(body).map_err(|error| CoreError::Network {
            code: maekon_core::error_codes::NetworkCode::Generic,
            message: format!("Failed to parse Ollama model details: {error}"),
        })?;
    let mut capabilities = parsed.capabilities;
    if let Some(details) = parsed.details {
        capabilities.extend(details.capabilities);
        capabilities.extend(details.families);
    }
    if parsed.projector_info.is_some() {
        capabilities.push("projector".to_string());
    }

    if capabilities.is_empty() {
        return Ok(Some(infer_ollama_vision_support(model)));
    }

    let supports_vision = capabilities.iter().any(|entry| {
        let normalized = entry.trim().to_ascii_lowercase();
        normalized.contains("vision")
            || normalized.contains("clip")
            || normalized.contains("projector")
            || normalized.contains("vl")
            || normalized.contains("llava")
    });
    Ok(Some(supports_vision))
}

pub(super) async fn probe_ollama_model_supports_ocr(
    client: &reqwest::Client,
    endpoint: &str,
    model: &str,
) -> Result<Option<bool>, CoreError> {
    let response = client
        .post(derive_ollama_show_endpoint(endpoint))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({ "model": model }))
        .send()
        .await
        .map_err(|error| {
            // Iter-90: split timeout vs generic per canonical pattern.
            if error.is_timeout() {
                CoreError::RequestTimeout {
                    code: maekon_core::error_codes::NetworkCode::Timeout,
                    timeout_ms: 0,
                }
            } else {
                CoreError::Network {
                    code: maekon_core::error_codes::NetworkCode::Generic,
                    message: format!("Ollama model capability probe failed: {error}"),
                }
            }
        })?;
    let status = response.status();
    // #6939: cap the probe response body — the Ollama endpoint may be remote/
    // user-configured, so a hostile/MITM host could stream multi-GB and OOM the
    // agent on this capability-probe path too (sibling of the main OCR read).
    let body = maekon_http_core::outbound::read_text_capped(
        response,
        maekon_http_core::outbound::MAX_AI_RESPONSE_BYTES,
    )
    .await
    .map_err(|e| match e {
        maekon_http_core::outbound::BodyReadError::Transport(error) => CoreError::Network {
            code: maekon_core::error_codes::NetworkCode::Generic,
            message: format!("Failed to read Ollama model capability probe response: {error}"),
        },
        maekon_http_core::outbound::BodyReadError::TooLarge { len, cap } => CoreError::Network {
            code: maekon_core::error_codes::NetworkCode::Generic,
            message: format!(
                "Ollama model capability probe response exceeded cap {cap} bytes (len {len})"
            ),
        },
    })?;
    if !status.is_success() {
        return Err(CoreError::Network {
            code: maekon_core::error_codes::NetworkCode::Generic,
            message: format!("Ollama model capability probe failed ({status}): {body}"),
        });
    }

    parse_ollama_show_supports_ocr(&body, model)
}
