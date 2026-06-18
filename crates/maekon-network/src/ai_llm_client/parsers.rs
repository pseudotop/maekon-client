use serde_json::Value;

use maekon_core::error::CoreError;
use maekon_core::ports::llm_provider::InterpretedAction;

use crate::provider_error_body::provider_parse_error_message;

pub(super) fn parse_claude_response(body: &str) -> Result<InterpretedAction, CoreError> {
    let response: Value = serde_json::from_str(body).map_err(CoreError::from)?;

    let text = response
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|block| block.get("text"))
        .and_then(|t| t.as_str())
        .ok_or_else(|| CoreError::Analysis {
            code: maekon_core::error_codes::ProviderCode::AnalysisFailed,
            message: "No text found in LLM response".to_string(),
        })?;

    parse_action_json(text)
}

pub(super) fn parse_openai_response(body: &str) -> Result<InterpretedAction, CoreError> {
    let response: Value = serde_json::from_str(body).map_err(CoreError::from)?;

    let text = extract_openai_text(&response).ok_or_else(|| CoreError::Analysis {
        code: maekon_core::error_codes::ProviderCode::AnalysisFailed,
        message: "No text found in OpenAI response".to_string(),
    })?;

    parse_action_json(&text)
}

pub(super) fn parse_google_response(body: &str) -> Result<InterpretedAction, CoreError> {
    let response: Value = serde_json::from_str(body).map_err(CoreError::from)?;

    let text = response
        .get("candidates")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|candidate| candidate.get("content"))
        .and_then(|content| content.get("parts"))
        .and_then(|parts| parts.as_array())
        .and_then(|parts| parts.first())
        .and_then(|part| part.get("text"))
        .and_then(|t| t.as_str())
        .ok_or_else(|| CoreError::Analysis {
            code: maekon_core::error_codes::ProviderCode::AnalysisFailed,
            message: "No text found in Google response".to_string(),
        })?;

    parse_action_json(text)
}

fn parse_action_json(text: &str) -> Result<InterpretedAction, CoreError> {
    // Guard end >= start before the inclusive-range slice: adversarial model
    // output with a '}' before the first '{' (e.g. "} foo {") would otherwise
    // produce a reversed range and PANIC. Same panic class as the #6194
    // analysis_client::parse_candidates fix; fall back to the whole text so
    // serde_json surfaces a clean parse error instead of crashing.
    let json_str = match (text.find('{'), text.rfind('}')) {
        (Some(start), Some(end)) if end >= start => &text[start..=end],
        _ => text,
    };

    serde_json::from_str(json_str).map_err(|e| CoreError::Validation {
        code: maekon_core::error_codes::ValidationCode::InvalidField,
        field: "llm_response.action".to_string(),
        message: provider_parse_error_message(
            "LLM API",
            &format!("failed to parse InterpretedAction ({e})"),
        ),
    })
}

fn extract_openai_text(response: &Value) -> Option<String> {
    if let Some(content) = response
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|msg| msg.get("content"))
    {
        if let Some(text) = value_to_text(content) {
            return Some(text);
        }
    }

    if let Some(text) = response.get("output_text").and_then(|value| value.as_str()) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    let mut chunks = Vec::new();
    if let Some(outputs) = response.get("output").and_then(|value| value.as_array()) {
        for output in outputs {
            if let Some(content) = output.get("content").and_then(|value| value.as_array()) {
                for part in content {
                    if let Some(text) = part.get("text").and_then(|value| value.as_str()) {
                        let trimmed = text.trim();
                        if !trimmed.is_empty() {
                            chunks.push(trimmed.to_string());
                        }
                    }
                }
            }
        }
    }

    if chunks.is_empty() {
        None
    } else {
        Some(chunks.join("\n"))
    }
}

fn value_to_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Value::Array(items) => {
            let mut chunks = Vec::new();
            for item in items {
                if let Some(text) = item.get("text").and_then(|value| value.as_str()) {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        chunks.push(trimmed.to_string());
                    }
                }
            }

            if chunks.is_empty() {
                None
            } else {
                Some(chunks.join("\n"))
            }
        }
        _ => None,
    }
}
