//! Google Gemini API serialization and SSE parsing for HTTP API sessions.

use oneshim_core::models::ai_session::{
    ChatMessage, ChatRole, ContentBlock, OutboundMessage, TokenUsage, ToolDefinition, ToolUseStatus,
};

use super::content::empty_tool_schema;

/// Serialize content blocks to Google Gemini API format.
///
/// Google uses `{"inlineData": {"mimeType": ..., "data": ...}}` for images.
pub(super) fn serialize_google_parts(blocks: &[ContentBlock]) -> Vec<serde_json::Value> {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(serde_json::json!({"text": text})),
            ContentBlock::Image { media_type, data } => Some(serde_json::json!({
                "inlineData": {"mimeType": media_type, "data": data}
            })),
            ContentBlock::File {
                media_type, data, ..
            } => Some(serde_json::json!({
                "inlineData": {"mimeType": media_type, "data": data}
            })),
            _ => None,
        })
        .collect()
}

/// Serialize tool definitions to Google Gemini API format.
pub(super) fn build_google_tools(tools: &[ToolDefinition]) -> serde_json::Value {
    let decls: Vec<serde_json::Value> = tools
        .iter()
        .map(|t| {
            let schema = t.input_schema.clone().unwrap_or_else(empty_tool_schema);
            serde_json::json!({"name": t.name, "description": t.description, "parameters": schema})
        })
        .collect();
    serde_json::json!([{"function_declarations": decls}])
}

/// Build the Google Gemini API request body.
///
/// Uses `contents` array with `system_instruction` and `generationConfig`.
pub(super) fn build_google_request_body(
    max_output_tokens: u32,
    system_prompt: Option<&str>,
    thinking: Option<&serde_json::Value>,
    messages: &[ChatMessage],
    response_format: Option<&serde_json::Value>,
    tools: Option<&[ToolDefinition]>,
) -> serde_json::Value {
    let api_contents: Vec<serde_json::Value> = messages
        .iter()
        .filter(|m| m.role != ChatRole::System)
        .map(|m| {
            let parts = if let Some(ref blocks) = m.content_blocks {
                serialize_google_parts(blocks)
            } else {
                vec![serde_json::json!({"text": m.content})]
            };
            serde_json::json!({
                "role": match m.role {
                    ChatRole::User => "user",
                    ChatRole::Assistant => "model",
                    _ => "user",
                },
                "parts": parts,
            })
        })
        .collect();

    let mut body = serde_json::json!({
        "contents": api_contents,
        "generationConfig": {
            "maxOutputTokens": max_output_tokens,
        },
    });

    if let Some(prompt) = system_prompt {
        body["system_instruction"] = serde_json::json!({"parts": [{"text": prompt}]});
    }

    if let Some(rf) = response_format {
        if let Some(schema) = rf
            .get("schema")
            .or_else(|| rf.get("json_schema").and_then(|js| js.get("schema")))
        {
            body["generationConfig"]["responseMimeType"] = serde_json::json!("application/json");
            body["generationConfig"]["responseSchema"] = schema.clone();
        }
    }

    if let Some(thinking) = thinking {
        body["generationConfig"]["thinking_config"] = thinking.clone();
    }

    if let Some(tools) = tools {
        let tool_defs = build_google_tools(tools);
        if let Some(arr) = tool_defs.as_array() {
            if !arr.is_empty()
                && !arr[0]
                    .get("function_declarations")
                    .and_then(|d| d.as_array())
                    .map_or(true, |a| a.is_empty())
            {
                body["tools"] = tool_defs;
            }
        }
    }

    body
}

/// Parse a Google Gemini SSE event data payload into an OutboundMessage.
///
/// Google events:
/// - `data: {"candidates":[{"content":{"parts":[{"text":"chunk"}]}}]}` -> text chunk
/// - Final chunk includes `usageMetadata` with token counts
/// - Stream ends without explicit `[DONE]` marker
pub(super) fn parse_google_sse_event(data: &str) -> Option<OutboundMessage> {
    let trimmed = data.trim();
    if trimmed.is_empty() {
        return None;
    }

    let val: serde_json::Value = serde_json::from_str(trimmed).ok()?;

    let parts = val
        .get("candidates")?
        .get(0)?
        .get("content")?
        .get("parts")?
        .as_array()?;

    // Check for usage metadata (present in chunks, especially the last one)
    let usage = val.get("usageMetadata").and_then(|u| {
        let input = u.get("promptTokenCount")?.as_u64()?;
        let output = u.get("candidatesTokenCount")?.as_u64()?;
        Some(TokenUsage {
            input_tokens: input,
            output_tokens: output,
        })
    });

    for part in parts {
        if let Some(fc) = part.get("functionCall") {
            let name = fc
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            let args = fc.get("args").cloned().unwrap_or(serde_json::json!({}));
            return Some(OutboundMessage::ToolUse {
                tool: name,
                input: Some(args),
                status: ToolUseStatus::Started,
                result: None,
            });
        }

        if let Some(thinking) = part.get("thinking").and_then(|t| t.as_str()) {
            if !thinking.is_empty() {
                return Some(OutboundMessage::Thinking {
                    content: thinking.to_string(),
                    done: usage.is_some(),
                });
            }
        }

        if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
            if !text.is_empty() {
                if usage.is_some() {
                    return Some(OutboundMessage::Result {
                        content: text.to_string(),
                        done: true,
                        usage,
                    });
                }
                return Some(OutboundMessage::Text {
                    content: text.to_string(),
                    done: false,
                });
            }
        }
    }

    if let Some(usage) = usage {
        return Some(OutboundMessage::Result {
            content: String::new(),
            done: true,
            usage: Some(usage),
        });
    }

    None
}
