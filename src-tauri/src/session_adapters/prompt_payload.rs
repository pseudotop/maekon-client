use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use oneshim_core::models::ai_session::{
    Attachment, ChatMessage, ChatRole, MessageContext, SessionMessage, ToolDefinition,
};

const MAX_ATTACHMENT_PREVIEW_BYTES: usize = 8 * 1024;
const MAX_ATTACHMENT_PREVIEW_FILES: usize = 4;

pub(crate) fn render_message_payload(
    message: &SessionMessage,
    fallback_tools: Option<&[ToolDefinition]>,
) -> String {
    let mut sections = vec![message.content.clone()];

    if let Some(context) = message
        .context
        .as_ref()
        .filter(|context| has_meaningful_context(context))
    {
        sections.push(format!(
            "Additional context JSON:\n{}",
            pretty_json(context)
        ));
    }

    let attachment_manifest = attachment_manifest(&message.attachments);
    if !attachment_manifest.is_empty() {
        sections.push(format!(
            "Attachments JSON:\n{}",
            pretty_json(&attachment_manifest)
        ));
    }

    let attachment_previews = attachment_content_previews(&message.attachments);
    if !attachment_previews.is_empty() {
        sections.push(format!(
            "Attachment content previews JSON:\n{}",
            pretty_json(&attachment_previews)
        ));
    }

    let tools = message
        .tools
        .as_deref()
        .filter(|tools| !tools.is_empty())
        .or_else(|| fallback_tools.filter(|tools| !tools.is_empty()));
    if let Some(tools) = tools {
        sections.push(format!(
            "Available tools JSON:\n{}\nIf you need one of these tools, explain the intended call and arguments explicitly.",
            pretty_json(tools)
        ));
    }

    if let Some(response_format) = message.response_format.as_ref() {
        sections.push(format!(
            "Required response format JSON:\n{}\nReturn the final answer in this format exactly.",
            pretty_json(response_format)
        ));
    }

    sections.join("\n\n")
}

pub(crate) fn extract_native_response_schema(
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

pub(crate) fn render_conversation_prompt(
    system_prompt: Option<&str>,
    history: &[ChatMessage],
) -> String {
    let mut sections = Vec::new();

    if let Some(prompt) = system_prompt
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty())
    {
        sections.push(format!("System instructions:\n{prompt}"));
    }

    if !history.is_empty() {
        let transcript = history
            .iter()
            .map(|message| {
                format!(
                    "{}:\n{}",
                    match message.role {
                        ChatRole::System => "System",
                        ChatRole::User => "User",
                        ChatRole::Assistant => "Assistant",
                    },
                    message.content
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        sections.push(format!(
            "Conversation transcript:\n{transcript}\n\nRespond as the assistant to the latest user message."
        ));
    }

    sections.join("\n\n")
}

fn has_meaningful_context(context: &MessageContext) -> bool {
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

fn attachment_manifest(attachments: &[Attachment]) -> Vec<serde_json::Value> {
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

fn attachment_content_previews(attachments: &[Attachment]) -> Vec<serde_json::Value> {
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

fn is_text_like_attachment(path: &str, mime: Option<&str>) -> bool {
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
            | "mjs"
            | "cjs"
            | "py"
            | "rs"
            | "go"
            | "java"
            | "kt"
            | "swift"
            | "rb"
            | "php"
            | "c"
            | "cc"
            | "cpp"
            | "h"
            | "hpp"
            | "sh"
            | "bash"
            | "zsh"
            | "fish"
            | "sql"
            | "html"
            | "css"
            | "scss"
            | "less"
            | "ini"
            | "cfg"
            | "conf"
            | "env"
            | "log"
    )
}

fn pretty_json<T: serde::Serialize + ?Sized>(value: &T) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oneshim_core::models::ai_session::{MessageRole, ToolDefinition};

    #[test]
    fn render_message_payload_includes_optional_sections() {
        let message = SessionMessage {
            role: MessageRole::User,
            content: "Explain this screenshot".to_string(),
            attachments: vec![Attachment::Image {
                mime: "image/png".to_string(),
                path: None,
                data: Some("abc".to_string()),
            }],
            tools: Some(vec![ToolDefinition {
                name: "get_sessions".to_string(),
                description: "List sessions".to_string(),
                endpoint: "http://localhost/api/sessions".to_string(),
                method: "GET".to_string(),
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                })),
            }]),
            context: Some(MessageContext {
                regime: Some("focus".to_string()),
                active_app: Some("VS Code".to_string()),
            }),
            response_format: Some(serde_json::json!({
                "type": "json_schema",
                "json_schema": {
                    "name": "answer",
                    "schema": { "type": "object" }
                }
            })),
        };

        let rendered = render_message_payload(&message, None);
        assert!(rendered.contains("Additional context JSON"));
        assert!(rendered.contains("Attachments JSON"));
        assert!(rendered.contains("Available tools JSON"));
        assert!(rendered.contains("Required response format JSON"));
    }

    #[test]
    fn render_message_payload_includes_text_attachment_preview() {
        let message = SessionMessage {
            role: MessageRole::User,
            content: "Summarize the attached file".to_string(),
            attachments: vec![Attachment::File {
                path: "notes.md".to_string(),
                mime: Some("text/markdown".to_string()),
                data: Some(BASE64.encode("# Notes\n- ship it\n")),
            }],
            tools: None,
            context: None,
            response_format: None,
        };

        let rendered = render_message_payload(&message, None);
        assert!(rendered.contains("Attachment content previews JSON"));
        assert!(rendered.contains("# Notes"));
        assert!(rendered.contains("ship it"));
    }

    #[test]
    fn render_message_payload_skips_binary_attachment_preview() {
        let message = SessionMessage {
            role: MessageRole::User,
            content: "Inspect the binary".to_string(),
            attachments: vec![Attachment::File {
                path: "archive.bin".to_string(),
                mime: Some("application/octet-stream".to_string()),
                data: Some(BASE64.encode([0_u8, 159, 146, 150])),
            }],
            tools: None,
            context: None,
            response_format: None,
        };

        let rendered = render_message_payload(&message, None);
        assert!(!rendered.contains("Attachment content previews JSON"));
    }

    #[test]
    fn extract_native_response_schema_from_json_schema_wrapper() {
        let response_format = serde_json::json!({
            "type": "json_schema",
            "json_schema": {
                "name": "answer",
                "schema": {
                    "type": "object",
                    "properties": {
                        "result": { "type": "string" }
                    }
                }
            }
        });

        let schema =
            extract_native_response_schema(Some(&response_format)).expect("schema should exist");
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"].get("result").is_some());
    }

    #[test]
    fn extract_native_response_schema_ignores_non_schema_format() {
        let response_format = serde_json::json!({
            "type": "json_object"
        });

        assert!(extract_native_response_schema(Some(&response_format)).is_none());
    }

    #[test]
    fn render_conversation_prompt_includes_system_and_history() {
        let prompt = render_conversation_prompt(
            Some("Be concise."),
            &[
                ChatMessage {
                    role: ChatRole::User,
                    content: "Hello".to_string(),
                    content_blocks: None,
                },
                ChatMessage {
                    role: ChatRole::Assistant,
                    content: "Hi".to_string(),
                    content_blocks: None,
                },
            ],
        );

        assert!(prompt.contains("System instructions"));
        assert!(prompt.contains("Conversation transcript"));
        assert!(prompt.contains("User:\nHello"));
        assert!(prompt.contains("Assistant:\nHi"));
    }
}
