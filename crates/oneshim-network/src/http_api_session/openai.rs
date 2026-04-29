//! OpenAI Chat Completions and Responses API serialization and SSE parsing.

use oneshim_api_contracts::provider_specs::{self, SurfaceCapabilityKind};
use oneshim_core::models::ai_session::{
    ChatMessage, ChatRole, ContentBlock, OutboundMessage, TokenUsage, ToolDefinition,
};

use super::content::empty_tool_schema;

/// Serialize content blocks to OpenAI Chat Completions API format.
///
/// OpenAI uses `{"type": "image_url", "image_url": {"url": "data:...;base64,..."}}` for images.
pub(super) fn serialize_openai_content(blocks: &[ContentBlock]) -> Vec<serde_json::Value> {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(serde_json::json!({"type": "text", "text": text})),
            ContentBlock::Image { media_type, data } => Some(serde_json::json!({
                "type": "image_url",
                "image_url": {"url": format!("data:{media_type};base64,{data}")}
            })),
            _ => None,
        })
        .collect()
}

/// Serialize content blocks to OpenAI Responses API format.
pub(super) fn serialize_openai_responses_content(
    blocks: &[ContentBlock],
) -> Vec<serde_json::Value> {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => {
                Some(serde_json::json!({"type": "input_text", "text": text}))
            }
            ContentBlock::Image { media_type, data } => Some(serde_json::json!({
                "type": "input_image",
                "image_url": format!("data:{media_type};base64,{data}")
            })),
            ContentBlock::File { data, filename, .. } => {
                let mut input_file = serde_json::json!({
                    "type": "input_file",
                    "file_data": data,
                });
                if let Some(filename) = filename {
                    input_file["filename"] = serde_json::json!(filename);
                }
                Some(input_file)
            }
            _ => None,
        })
        .collect()
}

/// Serialize tool definitions to OpenAI Chat Completions API format.
pub(super) fn build_openai_tools(tools: &[ToolDefinition]) -> Vec<serde_json::Value> {
    tools
        .iter()
        .map(|t| {
            let schema = t.input_schema.clone().unwrap_or_else(empty_tool_schema);
            serde_json::json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": schema}})
        })
        .collect()
}

/// Serialize tool definitions to OpenAI Responses API format.
pub(super) fn build_openai_responses_tools(tools: &[ToolDefinition]) -> Vec<serde_json::Value> {
    tools
        .iter()
        .map(|t| {
            let schema = t.input_schema.clone().unwrap_or_else(empty_tool_schema);
            serde_json::json!({"type": "function", "name": t.name, "description": t.description, "parameters": schema})
        })
        .collect()
}

/// Normalize a response_format value for the OpenAI Responses API.
pub(super) fn normalize_openai_responses_format(
    response_format: &serde_json::Value,
) -> serde_json::Value {
    if response_format.get("type").and_then(|value| value.as_str()) == Some("json_schema") {
        if let Some(json_schema) = response_format.get("json_schema") {
            let mut format = json_schema.clone();
            if format.get("type").is_none() {
                format["type"] = serde_json::json!("json_schema");
            }
            return format;
        }
    }

    if let Some(schema) = response_format.get("schema") {
        return serde_json::json!({
            "type": "json_schema",
            "name": "response",
            "schema": schema,
        });
    }

    response_format.clone()
}

/// Build the OpenAI Chat Completions API request body.
///
/// System prompt is included as the first message with role "system".
pub(super) fn build_openai_chat_request_body(
    model: &str,
    max_output_tokens: u32,
    thinking: Option<&serde_json::Value>,
    messages: &[ChatMessage],
    response_format: Option<&serde_json::Value>,
    tools: Option<&[ToolDefinition]>,
) -> serde_json::Value {
    let api_messages: Vec<serde_json::Value> = messages
        .iter()
        .map(|m| {
            let content = if let Some(ref blocks) = m.content_blocks {
                serde_json::Value::Array(serialize_openai_content(blocks))
            } else {
                serde_json::Value::String(m.content.clone())
            };
            serde_json::json!({ "role": m.role, "content": content })
        })
        .collect();

    let mut body = serde_json::json!({
        "model": model,
        "max_tokens": max_output_tokens,
        "stream": true,
        "messages": api_messages,
    });

    if let Some(rf) = response_format {
        body["response_format"] = rf.clone();
    }

    if let Some(thinking) = thinking {
        body["reasoning"] = thinking.clone();
    }

    if let Some(tools) = tools {
        let tool_defs = build_openai_tools(tools);
        if !tool_defs.is_empty() {
            body["tools"] = serde_json::Value::Array(tool_defs);
        }
    }

    body
}

/// Build the OpenAI Responses API request body.
///
/// System prompt becomes top-level `instructions`; messages use `input` array.
#[allow(clippy::too_many_arguments)]
pub(super) fn build_openai_responses_request_body(
    model: &str,
    max_output_tokens: u32,
    system_prompt: Option<&str>,
    thinking: Option<&serde_json::Value>,
    surface_id: &str,
    messages: &[ChatMessage],
    response_format: Option<&serde_json::Value>,
    tools: Option<&[ToolDefinition]>,
) -> serde_json::Value {
    let api_input: Vec<serde_json::Value> = messages
        .iter()
        .filter(|message| message.role != ChatRole::System)
        .map(|message| {
            let content = if let Some(ref blocks) = message.content_blocks {
                serde_json::Value::Array(serialize_openai_responses_content(blocks))
            } else {
                serde_json::Value::Array(vec![serde_json::json!({
                    "type": "input_text",
                    "text": message.content.clone()
                })])
            };
            serde_json::json!({
                "role": message.role,
                "content": content,
            })
        })
        .collect();

    let mut body = serde_json::json!({
        "model": model,
        "max_output_tokens": max_output_tokens,
        "stream": true,
        "input": api_input,
    });

    if let Some(prompt) = system_prompt {
        body["instructions"] = serde_json::Value::String(prompt.to_string());
    }

    if let Some(response_format) = response_format {
        body["text"] = serde_json::json!({
            "format": normalize_openai_responses_format(response_format)
        });
    }

    if let Some(thinking) = thinking {
        body["reasoning"] = thinking.clone();
    }

    if provider_specs::surface_supports_parameter(surface_id, SurfaceCapabilityKind::Llm, "tools")
        .unwrap_or(false)
    {
        if let Some(tools) = tools {
            let tool_defs = build_openai_responses_tools(tools);
            if !tool_defs.is_empty() {
                body["tools"] = serde_json::Value::Array(tool_defs);
            }
        }
    }

    body
}

/// Parse an OpenAI Chat Completions SSE event data payload into an OutboundMessage.
///
/// OpenAI events:
/// - `data: {"choices":[{"delta":{"content":"chunk"}}]}` -> text chunk
/// - `data: [DONE]` -> stream completion
pub(super) fn parse_openai_chat_sse_event(data: &str) -> Option<OutboundMessage> {
    let trimmed = data.trim();

    if trimmed == "[DONE]" {
        return Some(OutboundMessage::Result {
            content: String::new(),
            done: true,
            usage: None,
        });
    }

    let val: serde_json::Value = serde_json::from_str(trimmed).ok()?;

    // Check for tool_calls in delta
    if let Some(tool_calls) = val
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("delta"))
        .and_then(|d| d.get("tool_calls"))
        .and_then(|tc| tc.as_array())
    {
        for tc in tool_calls {
            let id = tc
                .get("id")
                .and_then(|i| i.as_str())
                .unwrap_or("")
                .to_string();
            let name = tc
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            let args = tc
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(|a| a.as_str())
                .unwrap_or("")
                .to_string();
            let index = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as u32;
            if !id.is_empty() || !name.is_empty() || !args.is_empty() {
                return Some(OutboundMessage::ToolCallDelta {
                    index,
                    id,
                    name,
                    arguments_chunk: args,
                });
            }
        }
    }

    // Check for finish_reason: tool_calls
    if let Some(finish) = val
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("finish_reason"))
        .and_then(|f| f.as_str())
    {
        if finish == "tool_calls" {
            return Some(OutboundMessage::Result {
                content: String::new(),
                done: true,
                usage: None,
            });
        }
    }

    // Check for usage in the final chunk (OpenAI includes usage in last chunk if requested)
    if let Some(usage_obj) = val.get("usage") {
        let input = usage_obj
            .get("prompt_tokens")
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let output = usage_obj
            .get("completion_tokens")
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        if input > 0 || output > 0 {
            return Some(OutboundMessage::Result {
                content: String::new(),
                done: false,
                usage: Some(TokenUsage {
                    input_tokens: input,
                    output_tokens: output,
                }),
            });
        }
    }

    let content = val
        .get("choices")?
        .get(0)?
        .get("delta")?
        .get("content")?
        .as_str()?;

    if content.is_empty() {
        return None;
    }

    Some(OutboundMessage::Text {
        content: content.to_string(),
        done: false,
    })
}

/// Parse an OpenAI Responses API SSE event into an OutboundMessage.
pub(super) fn parse_openai_responses_sse_event(
    event_type: &str,
    data: &str,
) -> Option<OutboundMessage> {
    let trimmed = data.trim();
    if trimmed == "[DONE]" {
        return Some(OutboundMessage::Result {
            content: String::new(),
            done: true,
            usage: None,
        });
    }
    if trimmed.is_empty() {
        return None;
    }

    let val: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    let event_type = val
        .get("type")
        .and_then(|value| value.as_str())
        .filter(|_| event_type.is_empty())
        .unwrap_or(event_type);

    if event_type == "error" || val.get("error").is_some() {
        let message = val
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(|message| message.as_str())
            .unwrap_or("OpenAI Responses API stream error")
            .to_string();
        return Some(OutboundMessage::Error {
            code: "responses_api_error".to_string(),
            message,
            retryable: false,
        });
    }

    match event_type {
        "response.output_text.delta" => {
            let delta = val.get("delta")?.as_str()?;
            if delta.is_empty() {
                return None;
            }
            Some(OutboundMessage::Text {
                content: delta.to_string(),
                done: false,
            })
        }
        "response.reasoning_summary_text.delta" | "response.reasoning.delta" => {
            let delta = val
                .get("delta")
                .and_then(|value| value.as_str())
                .or_else(|| val.get("summary").and_then(|value| value.as_str()))?;
            if delta.is_empty() {
                return None;
            }
            Some(OutboundMessage::Thinking {
                content: delta.to_string(),
                done: false,
            })
        }
        "response.output_item.added" | "response.output_item.done" => {
            let item = val.get("item")?;
            if item.get("type").and_then(|value| value.as_str()) != Some("function_call") {
                return None;
            }

            let id = item
                .get("call_id")
                .or_else(|| item.get("id"))
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string();
            let name = item
                .get("name")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string();
            let arguments_chunk = item
                .get("arguments")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string();
            let index = val
                .get("output_index")
                .and_then(|value| value.as_u64())
                .unwrap_or(0) as u32;

            if id.is_empty() && name.is_empty() && arguments_chunk.is_empty() {
                return None;
            }

            Some(OutboundMessage::ToolCallDelta {
                index,
                id,
                name,
                arguments_chunk,
            })
        }
        "response.function_call_arguments.delta" => {
            let arguments_chunk = val.get("delta")?.as_str()?.to_string();
            let id = val
                .get("item_id")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string();
            let index = val
                .get("output_index")
                .and_then(|value| value.as_u64())
                .unwrap_or(0) as u32;
            Some(OutboundMessage::ToolCallDelta {
                index,
                id,
                name: String::new(),
                arguments_chunk,
            })
        }
        "response.completed" => Some(OutboundMessage::Result {
            content: String::new(),
            done: true,
            usage: extract_openai_responses_usage(val.get("response")?),
        }),
        "response.failed" => {
            let message = val
                .get("response")
                .and_then(|response| response.get("status_details"))
                .and_then(|details| details.get("error"))
                .and_then(|error| error.get("message"))
                .and_then(|message| message.as_str())
                .unwrap_or("OpenAI Responses API request failed")
                .to_string();
            Some(OutboundMessage::Error {
                code: "responses_api_failed".to_string(),
                message,
                retryable: false,
            })
        }
        _ => {
            if let Some(usage) = val.get("response").and_then(extract_openai_responses_usage) {
                return Some(OutboundMessage::Result {
                    content: String::new(),
                    done: false,
                    usage: Some(usage),
                });
            }

            None
        }
    }
}

pub(super) fn extract_openai_responses_usage(response: &serde_json::Value) -> Option<TokenUsage> {
    let usage = response.get("usage")?;
    let input = usage.get("input_tokens")?.as_u64()?;
    let output = usage.get("output_tokens")?.as_u64()?;
    Some(TokenUsage {
        input_tokens: input,
        output_tokens: output,
    })
}
