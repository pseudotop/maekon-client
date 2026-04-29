//! Anthropic-specific serialization and SSE parsing for HTTP API sessions.

use oneshim_core::models::ai_session::{
    ChatMessage, ChatRole, ContentBlock, OutboundMessage, TokenUsage, ToolDefinition,
};
use tracing::debug;

use super::content::empty_tool_schema;

/// Check if a MIME type is supported as an Anthropic document upload.
pub(super) fn supports_anthropic_document(media_type: &str) -> bool {
    media_type == "application/pdf"
}

/// Serialize content blocks to Anthropic Messages API format.
///
/// Anthropic uses `{"type": "image", "source": {"type": "base64", ...}}` for images.
pub(super) fn serialize_anthropic_content(blocks: &[ContentBlock]) -> Vec<serde_json::Value> {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(serde_json::json!({"type": "text", "text": text})),
            ContentBlock::Image { media_type, data } => Some(serde_json::json!({
                "type": "image",
                "source": {"type": "base64", "media_type": media_type, "data": data}
            })),
            ContentBlock::File {
                media_type,
                data,
                filename,
            } if supports_anthropic_document(media_type) => {
                let mut document = serde_json::json!({
                    "type": "document",
                    "source": {"type": "base64", "media_type": media_type, "data": data}
                });
                if let Some(filename) = filename {
                    document["title"] = serde_json::json!(filename);
                }
                Some(document)
            }
            _ => None,
        })
        .collect()
}

/// Serialize tool definitions to Anthropic Messages API format.
pub(super) fn build_anthropic_tools(tools: &[ToolDefinition]) -> Vec<serde_json::Value> {
    tools
        .iter()
        .map(|t| {
            let schema = t.input_schema.clone().unwrap_or_else(empty_tool_schema);
            serde_json::json!({"name": t.name, "description": t.description, "input_schema": schema})
        })
        .collect()
}

/// Build the Anthropic Messages API request body.
///
/// System prompt is top-level; messages exclude the system role.
pub(super) fn build_anthropic_request_body(
    model: &str,
    max_output_tokens: u32,
    system_prompt: Option<&str>,
    thinking: Option<&serde_json::Value>,
    messages: &[ChatMessage],
    tools: Option<&[ToolDefinition]>,
) -> serde_json::Value {
    let api_messages: Vec<serde_json::Value> = messages
        .iter()
        .filter(|m| m.role != ChatRole::System)
        .map(|m| {
            let content = if let Some(ref blocks) = m.content_blocks {
                serde_json::Value::Array(serialize_anthropic_content(blocks))
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

    if let Some(prompt) = system_prompt {
        body["system"] = serde_json::Value::String(prompt.to_string());
    }

    // Anthropic ignores response_format — no injection needed.

    if let Some(thinking) = thinking {
        body["thinking"] = thinking.clone();
    }

    if let Some(tools) = tools {
        let tool_defs = build_anthropic_tools(tools);
        if !tool_defs.is_empty() {
            body["tools"] = serde_json::Value::Array(tool_defs);
        }
    }

    body
}

/// Parse an Anthropic SSE event into an OutboundMessage.
///
/// Anthropic event types:
/// - `content_block_delta` -> text chunk
/// - `message_stop` -> stream completion
/// - `message_delta` -> usage info (optional)
pub(super) fn parse_anthropic_sse_event(event_type: &str, data: &str) -> Option<OutboundMessage> {
    match event_type {
        "content_block_start" => {
            let val: serde_json::Value = serde_json::from_str(data).ok()?;
            let block = val.get("content_block")?;
            if block.get("type")?.as_str()? == "tool_use" {
                let id = block.get("id")?.as_str()?.to_string();
                let name = block.get("name")?.as_str()?.to_string();
                Some(OutboundMessage::ToolCallDelta {
                    index: 0,
                    id,
                    name,
                    arguments_chunk: String::new(),
                })
            } else {
                None
            }
        }
        "content_block_delta" => {
            let val: serde_json::Value = serde_json::from_str(data).ok()?;
            let delta = val.get("delta")?;
            let delta_type = delta.get("type")?.as_str()?;
            match delta_type {
                "text_delta" => {
                    let text = delta.get("text")?.as_str()?.to_string();
                    Some(OutboundMessage::Text {
                        content: text,
                        done: false,
                    })
                }
                "thinking_delta" => {
                    let thinking = delta.get("thinking")?.as_str()?.to_string();
                    Some(OutboundMessage::Thinking {
                        content: thinking,
                        done: false,
                    })
                }
                "input_json_delta" => {
                    let partial = delta.get("partial_json")?.as_str()?.to_string();
                    Some(OutboundMessage::ToolCallDelta {
                        index: 0,
                        id: String::new(),
                        name: String::new(),
                        arguments_chunk: partial,
                    })
                }
                _ => None,
            }
        }
        "message_delta" => {
            // Extract usage from message_delta if present
            let val: serde_json::Value = serde_json::from_str(data).ok()?;
            let usage = val.get("usage").and_then(|u| {
                let input = u.get("input_tokens")?.as_u64()?;
                let output = u.get("output_tokens")?.as_u64()?;
                Some(TokenUsage {
                    input_tokens: input,
                    output_tokens: output,
                })
            });
            // message_delta may contain stop_reason but we handle completion in message_stop
            if usage.is_some() {
                Some(OutboundMessage::Result {
                    content: String::new(),
                    done: false,
                    usage,
                })
            } else {
                None
            }
        }
        "message_stop" => Some(OutboundMessage::Result {
            content: String::new(),
            done: true,
            usage: None,
        }),
        _ => {
            debug!(event_type, "ignoring Anthropic SSE event");
            None
        }
    }
}
