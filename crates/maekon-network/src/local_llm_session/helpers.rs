use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use maekon_core::models::ai_session::{
    Attachment, ChatMessage, ContentBlock, MessageContext, SessionMessage,
};

use crate::error::NetworkError;

use super::types::{OllamaChatChunk, MAX_ATTACHMENT_PREVIEW_BYTES, MAX_ATTACHMENT_PREVIEW_FILES};

pub(super) fn parse_ndjson_line(line: &str) -> Result<OllamaChatChunk, NetworkError> {
    serde_json::from_str(line).map_err(|e| {
        NetworkError::Internal(format!("failed to parse Ollama NDJSON chunk: {e}: {line}"))
    })
}

pub(super) fn has_meaningful_context(context: &MessageContext) -> bool {
    context
        .regime
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
        || context
            .active_app
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
}

pub(super) fn attachment_manifest(attachments: &[Attachment]) -> Vec<serde_json::Value> {
    attachments
        .iter()
        .map(|attachment| match attachment {
            Attachment::Image { mime, path, data } => serde_json::json!({
                "kind": "image",
                "mime": mime,
                "path": path,
                "has_inline_data": data.as_ref().is_some_and(|value| !value.is_empty()),
            }),
            Attachment::File { path, mime, data } => serde_json::json!({
                "kind": "file",
                "path": path,
                "mime": mime,
                "has_inline_data": data.as_ref().is_some_and(|value| !value.is_empty()),
            }),
            Attachment::Directory { path } => serde_json::json!({
                "kind": "directory",
                "path": path,
            }),
            Attachment::Skill {
                skill_id,
                display_name,
            } => serde_json::json!({
                "kind": "skill",
                "skill_id": skill_id,
                "display_name": display_name,
            }),
            Attachment::AppReference {
                app_name,
                window_title,
            } => serde_json::json!({
                "kind": "app_reference",
                "app_name": app_name,
                "window_title": window_title,
            }),
        })
        .collect()
}

pub(super) fn attachment_content_previews(attachments: &[Attachment]) -> Vec<serde_json::Value> {
    attachments
        .iter()
        .filter_map(|attachment| match attachment {
            Attachment::File { path, mime, data } => {
                let mime_ref = mime.as_deref();
                let encoded = data.as_deref()?;
                if !is_text_like_attachment(path, mime_ref) {
                    return None;
                }

                let decoded = BASE64.decode(encoded).ok()?;
                let truncated = decoded.len() > MAX_ATTACHMENT_PREVIEW_BYTES;
                let preview_bytes = if truncated {
                    &decoded[..MAX_ATTACHMENT_PREVIEW_BYTES]
                } else {
                    decoded.as_slice()
                };
                let preview = String::from_utf8_lossy(preview_bytes).to_string();
                if preview.trim().is_empty() {
                    return None;
                }

                Some(serde_json::json!({
                    "kind": "file",
                    "path": path,
                    "mime": mime_ref,
                    "truncated": truncated,
                    "preview": preview,
                }))
            }
            _ => None,
        })
        .take(MAX_ATTACHMENT_PREVIEW_FILES)
        .collect()
}

pub(super) fn is_text_like_attachment(path: &str, mime: Option<&str>) -> bool {
    if let Some(mime) = mime.map(|value| value.trim().to_ascii_lowercase()) {
        if mime.starts_with("text/") {
            return true;
        }

        if matches!(
            mime.as_str(),
            "application/json"
                | "application/ld+json"
                | "application/xml"
                | "application/yaml"
                | "application/x-yaml"
                | "application/toml"
                | "application/javascript"
                | "application/x-javascript"
                | "application/sql"
                | "application/x-sh"
                | "application/x-python-code"
        ) {
            return true;
        }
    }

    let ext = path
        .rsplit('.')
        .next()
        .map(|value| value.trim().to_ascii_lowercase())
        .unwrap_or_default();

    matches!(
        ext.as_str(),
        "txt"
            | "md"
            | "markdown"
            | "json"
            | "yaml"
            | "yml"
            | "toml"
            | "xml"
            | "csv"
            | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "rs"
            | "py"
            | "sh"
            | "sql"
            | "java"
            | "kt"
            | "go"
            | "c"
            | "cc"
            | "cpp"
            | "h"
            | "hpp"
    )
}

pub(super) fn extract_response_schema(
    response_format: Option<&serde_json::Value>,
) -> Option<serde_json::Value> {
    let response_format = response_format?;

    if response_format.get("type").and_then(|value| value.as_str()) == Some("json_schema") {
        if let Some(schema) = response_format
            .get("json_schema")
            .and_then(|value| value.get("schema"))
        {
            return Some(schema.clone());
        }
    }

    if let Some(schema) = response_format.get("schema") {
        return Some(schema.clone());
    }

    if response_format.get("properties").is_some()
        || response_format.get("required").is_some()
        || response_format.get("$schema").is_some()
    {
        return Some(response_format.clone());
    }

    None
}

pub(super) fn render_local_message_content(message: &SessionMessage) -> String {
    let mut sections = vec![message.content.clone()];

    if let Some(context) = message
        .context
        .as_ref()
        .filter(|context| has_meaningful_context(context))
    {
        sections.push(format!(
            "Additional context JSON:\n{}",
            serde_json::to_string_pretty(context).unwrap_or_else(|_| "{}".to_string())
        ));
    }

    let attachments = attachment_manifest(&message.attachments);
    if !attachments.is_empty() {
        sections.push(format!(
            "Attachments JSON:\n{}",
            serde_json::to_string_pretty(&attachments).unwrap_or_else(|_| "[]".to_string())
        ));
    }

    let attachment_previews = attachment_content_previews(&message.attachments);
    if !attachment_previews.is_empty() {
        sections.push(format!(
            "Attachment content previews JSON:\n{}",
            serde_json::to_string_pretty(&attachment_previews).unwrap_or_else(|_| "[]".to_string())
        ));
    }

    let tools = message.tools.as_deref().filter(|tools| !tools.is_empty());
    if let Some(tools) = tools {
        sections.push(format!(
            "Available tools JSON:\n{}\nIf you need one of these tools, explain the intended call and arguments explicitly.",
            serde_json::to_string_pretty(tools).unwrap_or_else(|_| "[]".to_string())
        ));
    }

    if let Some(schema) = extract_response_schema(message.response_format.as_ref()) {
        sections.push(format!(
            "Required response schema JSON:\n{}\nReturn the final answer as valid JSON matching this schema exactly.",
            serde_json::to_string_pretty(&schema).unwrap_or_else(|_| "{}".to_string())
        ));
    } else if let Some(response_format) = message.response_format.as_ref() {
        sections.push(format!(
            "Required response format JSON:\n{}\nReturn the final answer in this format exactly.",
            serde_json::to_string_pretty(response_format).unwrap_or_else(|_| "{}".to_string())
        ));
    }

    sections.join("\n\n")
}

pub(super) fn local_content_blocks(
    rendered_text: &str,
    attachments: &[Attachment],
) -> Option<Vec<ContentBlock>> {
    let mut blocks = vec![ContentBlock::Text {
        text: rendered_text.to_string(),
    }];

    blocks.extend(
        attachments
            .iter()
            .filter_map(|attachment| match attachment {
                Attachment::Image { mime, data, .. } => {
                    data.as_ref().map(|payload| ContentBlock::Image {
                        media_type: mime.clone(),
                        data: payload.clone(),
                    })
                }
                Attachment::File {
                    mime: Some(mime),
                    data: Some(data),
                    ..
                } if mime.trim().to_ascii_lowercase().starts_with("image/") => {
                    Some(ContentBlock::Image {
                        media_type: mime.clone(),
                        data: data.clone(),
                    })
                }
                _ => None,
            }),
    );

    (blocks.len() > 1).then_some(blocks)
}

pub(super) fn ollama_message_payload(message: &ChatMessage) -> serde_json::Value {
    let mut content = message.content.clone();
    let mut images = Vec::new();

    if let Some(blocks) = message.content_blocks.as_ref() {
        let mut text_segments = Vec::new();
        for block in blocks {
            match block {
                ContentBlock::Text { text } => text_segments.push(text.clone()),
                ContentBlock::Image { data, .. } => images.push(data.clone()),
                ContentBlock::File { .. }
                | ContentBlock::ToolUse { .. }
                | ContentBlock::ToolResult { .. }
                | ContentBlock::Thinking { .. } => {}
            }
        }
        if !text_segments.is_empty() {
            content = text_segments.join("\n\n");
        }
    }

    let mut payload = serde_json::json!({
        "role": message.role,
        "content": content,
    });

    if !images.is_empty() {
        payload["images"] =
            serde_json::Value::Array(images.into_iter().map(serde_json::Value::String).collect());
    }

    payload
}
