use serde_json::Value;

use oneshim_core::error::CoreError;
use oneshim_core::ports::llm_provider::InterpretedAction;

pub(super) fn parse_claude_response(body: &str) -> Result<InterpretedAction, CoreError> {
    let response: Value = serde_json::from_str(body).map_err(CoreError::from)?;

    let text = response
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|block| block.get("text"))
        .and_then(|t| t.as_str())
        .ok_or_else(|| CoreError::Analysis {
            code: oneshim_core::error_codes::ProviderCode::AnalysisFailed,
            message: "No text found in LLM response".to_string(),
        })?;

    parse_action_json(text)
}

pub(super) fn parse_openai_response(body: &str) -> Result<InterpretedAction, CoreError> {
    let response: Value = serde_json::from_str(body).map_err(CoreError::from)?;

    let text = extract_openai_text(&response).ok_or_else(|| CoreError::Analysis {
        code: oneshim_core::error_codes::ProviderCode::AnalysisFailed,
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
            code: oneshim_core::error_codes::ProviderCode::AnalysisFailed,
            message: "No text found in Google response".to_string(),
        })?;

    parse_action_json(text)
}

fn parse_action_json(text: &str) -> Result<InterpretedAction, CoreError> {
    let json_str = if let Some(start) = text.find('{') {
        if let Some(end) = text.rfind('}') {
            &text[start..=end]
        } else {
            text
        }
    } else {
        text
    };

    serde_json::from_str(json_str).map_err(|_e| CoreError::Validation {
        code: oneshim_core::error_codes::ValidationCode::InvalidField,
        field: "llm_response.action".to_string(),
        message: format!(
            "Failed to parse InterpretedAction from LLM response (raw: {})",
            json_str.chars().take(200).collect::<String>()
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
