//! LocalLlmSession — Ollama `/api/chat` conversation adapter with NDJSON streaming.
//!
//! Self-managed conversation history targeting local LLM servers.
//! Streams responses line-by-line (NDJSON), mapping Ollama token usage
//! fields (`eval_count` / `prompt_eval_count`) to `TokenUsage`.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;

use async_stream::try_stream;
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::{DateTime, Utc};
use futures::StreamExt;
use serde::Deserialize;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use oneshim_core::config::AiSessionConfig;
use oneshim_core::error::CoreError;

use crate::error::NetworkError;
use oneshim_core::models::ai_session::{
    truncate_chat_history, Attachment, ChatMessage, ChatRole, ContentBlock,
    ConversationSessionInfo, MessageContext, OutboundMessage, SessionMessage, SessionState,
    SessionTransport, TokenUsage,
};
use oneshim_core::ports::conversation_session::{ConversationSession, ResponseStream};

// ── Ollama NDJSON response shapes ────────────────────────────────

const MAX_ATTACHMENT_PREVIEW_BYTES: usize = 8 * 1024;
const MAX_ATTACHMENT_PREVIEW_FILES: usize = 4;

/// Single NDJSON line from Ollama `/api/chat` with `stream: true`.
#[derive(Debug, Deserialize)]
struct OllamaChatChunk {
    #[serde(default)]
    message: Option<OllamaChunkMessage>,
    #[serde(default)]
    done: bool,
    /// Token count for the generated response (present on final chunk).
    #[serde(default)]
    eval_count: Option<u64>,
    /// Token count for the prompt (present on final chunk).
    #[serde(default)]
    prompt_eval_count: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct OllamaChunkMessage {
    #[serde(default)]
    content: String,
}

// ── LocalLlmSession ─────────────────────────────────────────────

pub struct LocalLlmSession {
    session_id: String,
    model: String,
    base_url: String,
    history: Arc<RwLock<Vec<ChatMessage>>>,
    /// Retained for session introspection; content is pre-seeded into `history`.
    #[allow(dead_code)]
    system_prompt: Option<String>,
    state: parking_lot::Mutex<SessionState>,
    turn_count: AtomicU32,
    created_at: DateTime<Utc>,
    last_active: parking_lot::Mutex<Instant>,
    http_client: reqwest::Client,
    config: Arc<AiSessionConfig>,
}

impl LocalLlmSession {
    /// Create a new session targeting an Ollama-compatible server.
    pub fn new(
        session_id: String,
        model: String,
        base_url: String,
        system_prompt: Option<String>,
        config: Arc<AiSessionConfig>,
    ) -> Self {
        let mut initial_history = Vec::new();
        if let Some(ref prompt) = system_prompt {
            initial_history.push(ChatMessage {
                role: ChatRole::System,
                content: prompt.clone(),
                content_blocks: None,
            });
        }

        Self {
            session_id,
            model,
            base_url: base_url.trim_end_matches('/').to_string(),
            history: Arc::new(RwLock::new(initial_history)),
            system_prompt,
            state: parking_lot::Mutex::new(SessionState::Active),
            turn_count: AtomicU32::new(0),
            created_at: Utc::now(),
            last_active: parking_lot::Mutex::new(Instant::now()),
            http_client: reqwest::Client::new(),
            config,
        }
    }
}

/// Parse a single NDJSON line into an `OllamaChatChunk`.
fn parse_ndjson_line(line: &str) -> Result<OllamaChatChunk, NetworkError> {
    serde_json::from_str(line).map_err(|e| {
        NetworkError::Internal(format!("failed to parse Ollama NDJSON chunk: {e}: {line}"))
    })
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

fn extract_response_schema(
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

fn render_local_message_content(message: &SessionMessage) -> String {
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

fn local_content_blocks(
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

fn ollama_message_payload(message: &ChatMessage) -> serde_json::Value {
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

#[async_trait]
impl ConversationSession for LocalLlmSession {
    async fn send_message(&self, message: &SessionMessage) -> Result<ResponseStream, CoreError> {
        // Convert SessionMessage to ChatMessage and append to history.
        let rendered_user_message = render_local_message_content(message);
        let content_blocks = local_content_blocks(&rendered_user_message, &message.attachments);
        let user_msg = ChatMessage {
            role: ChatRole::User,
            content: rendered_user_message,
            content_blocks,
        };

        {
            let mut history = self.history.write().await;
            history.push(user_msg);
        }

        // Build request body with full history.
        let messages: Vec<serde_json::Value> = {
            let history = self.history.read().await;
            history.iter().map(ollama_message_payload).collect()
        };

        let url = format!("{}/api/chat", self.base_url);
        let body = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "stream": true,
        });

        debug!(
            session_id = %self.session_id,
            model = %self.model,
            url = %url,
            history_len = messages.len(),
            "sending Ollama chat request"
        );

        let response = self
            .http_client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                *self.state.lock() = SessionState::Failed;
                // Iter-90: Ollama is local, timeouts are rare but possible when
                // a large model is still loading. Keep the canonical split so
                // Grafana/logs can distinguish slow-model-load from true failure.
                if e.is_timeout() {
                    CoreError::RequestTimeout {
                        code: oneshim_core::error_codes::NetworkCode::Timeout,
                        timeout_ms: 0,
                    }
                } else {
                    CoreError::Network {
                        code: oneshim_core::error_codes::NetworkCode::Generic,
                        message: format!("Ollama request failed: {e}"),
                    }
                }
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body_text = response.text().await.unwrap_or_default();
            *self.state.lock() = SessionState::Failed;
            // Ollama runs locally, so timeouts/gateway errors are rare. 404
            // most commonly means "model not pulled" — distinguish it so the
            // frontend can hint at `ollama pull <model>` rather than generic
            // "network error". (iter-55c)
            return Err(match status.as_u16() {
                404 => CoreError::NotFound {
                    code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
                    resource_type: "ollama_model".to_string(),
                    id: format!("{body_text} (hint: try `ollama pull <model>`)"),
                },
                _ => CoreError::Network {
                    code: oneshim_core::error_codes::NetworkCode::Generic,
                    message: format!("Ollama API error {status}: {body_text}"),
                },
            });
        }

        // Stream NDJSON lines from the response body.
        let mut byte_stream = response.bytes_stream();
        let history = self.history.clone();
        let turn_count = &self.turn_count;
        let max_history = self.config.max_history_turns;

        // Pre-increment turn count.
        turn_count.fetch_add(1, Ordering::Relaxed);
        *self.last_active.lock() = Instant::now();

        // We need to move owned values into the stream closure.
        let session_id = self.session_id.clone();

        let stream: ResponseStream = Box::pin(try_stream! {
            let mut accumulated = String::new();
            let mut line_buffer = String::new();

            while let Some(chunk_result) = byte_stream.next().await {
                let bytes = chunk_result
                    .map_err(|e| {
                        // Iter-90: stream-read timeout gets the dedicated wire
                        // code; keep consistent with send()-time handling above.
                        if e.is_timeout() {
                            CoreError::RequestTimeout {
                                code: oneshim_core::error_codes::NetworkCode::Timeout,
                                timeout_ms: 0,
                            }
                        } else {
                            CoreError::Network {
                                code: oneshim_core::error_codes::NetworkCode::Generic,
                                message: format!("stream read error: {e}"),
                            }
                        }
                    })?;
                let text = String::from_utf8_lossy(&bytes);
                line_buffer.push_str(&text);

                // Process complete lines (NDJSON = one JSON object per line).
                while let Some(newline_pos) = line_buffer.find('\n') {
                    let line = line_buffer[..newline_pos].trim().to_string();
                    line_buffer = line_buffer[newline_pos + 1..].to_string();

                    if line.is_empty() {
                        continue;
                    }

                    let chunk = parse_ndjson_line(&line)?;

                    if chunk.done {
                        // Final chunk — emit Result with token usage.
                        let usage = match (chunk.eval_count, chunk.prompt_eval_count) {
                            (Some(output), Some(input)) => Some(TokenUsage {
                                input_tokens: input,
                                output_tokens: output,
                            }),
                            _ => None,
                        };

                        // Append accumulated assistant message to history.
                        if !accumulated.is_empty() {
                            let mut hist: tokio::sync::RwLockWriteGuard<'_, Vec<ChatMessage>> = history.write().await;
                            hist.push(ChatMessage {
                                role: ChatRole::Assistant,
                                content: accumulated.clone(),
                                content_blocks: None,
                            });
                            truncate_chat_history(&mut hist, max_history);
                        }

                        debug!(
                            session_id = %session_id,
                            accumulated_len = accumulated.len(),
                            ?usage,
                            "Ollama stream completed"
                        );

                        yield OutboundMessage::Result {
                            content: accumulated.clone(),
                            done: true,
                            usage,
                        };
                    } else if let Some(ref msg) = chunk.message {
                        // Streaming content chunk.
                        if !msg.content.is_empty() {
                            accumulated.push_str(&msg.content);
                            yield OutboundMessage::Text {
                                content: msg.content.clone(),
                                done: false,
                            };
                        }
                    }
                }
            }

            // Handle any remaining data in the buffer (no trailing newline).
            let remaining = line_buffer.trim().to_string();
            if !remaining.is_empty() {
                match parse_ndjson_line(&remaining) {
                    Ok(chunk) => {
                        if chunk.done {
                            let usage = match (chunk.eval_count, chunk.prompt_eval_count) {
                                (Some(output), Some(input)) => Some(TokenUsage {
                                    input_tokens: input,
                                    output_tokens: output,
                                }),
                                _ => None,
                            };

                            if !accumulated.is_empty() {
                                let mut hist: tokio::sync::RwLockWriteGuard<'_, Vec<ChatMessage>> = history.write().await;
                                hist.push(ChatMessage {
                                    role: ChatRole::Assistant,
                                    content: accumulated.clone(),
                                    content_blocks: None,
                                });
                                truncate_chat_history(&mut hist, max_history);
                            }

                            yield OutboundMessage::Result {
                                content: accumulated.clone(),
                                done: true,
                                usage,
                            };
                        } else if let Some(ref msg) = chunk.message {
                            if !msg.content.is_empty() {
                                accumulated.push_str(&msg.content);
                                yield OutboundMessage::Text {
                                    content: msg.content.clone(),
                                    done: false,
                                };
                            }
                        }
                    }
                    Err(e) => {
                        warn!("failed to parse trailing NDJSON: {e}");
                    }
                }
            }
        });

        Ok(stream)
    }

    fn info(&self) -> ConversationSessionInfo {
        let elapsed = self.last_active.lock().elapsed();
        let last_active_utc = Utc::now() - chrono::Duration::from_std(elapsed).unwrap_or_default();
        ConversationSessionInfo {
            session_id: self.session_id.clone(),
            provider_name: "ollama".to_string(),
            model: self.model.clone(),
            state: *self.state.lock(),
            transport: SessionTransport::LocalLlm,
            created_at: self.created_at,
            last_active: last_active_utc,
            turn_count: self.turn_count.load(Ordering::Relaxed),
            title: None,
        }
    }

    fn session_id(&self) -> &str {
        &self.session_id
    }

    fn provider_name(&self) -> &str {
        "ollama"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oneshim_core::models::ai_session::{MessageRole, ToolDefinition};

    // ── NDJSON parsing ──────────────────────────────────────────

    #[test]
    fn parse_ndjson_content_chunk() {
        let line =
            r#"{"model":"llama3","message":{"role":"assistant","content":"Hello"},"done":false}"#;
        let chunk = parse_ndjson_line(line).unwrap();
        assert!(!chunk.done);
        assert_eq!(chunk.message.as_ref().unwrap().content, "Hello");
        assert!(chunk.eval_count.is_none());
    }

    #[test]
    fn parse_ndjson_final_chunk_with_token_usage() {
        let line = r#"{"model":"llama3","message":{"role":"assistant","content":""},"done":true,"eval_count":50,"prompt_eval_count":20}"#;
        let chunk = parse_ndjson_line(line).unwrap();
        assert!(chunk.done);
        assert_eq!(chunk.eval_count, Some(50));
        assert_eq!(chunk.prompt_eval_count, Some(20));
    }

    #[test]
    fn parse_ndjson_final_chunk_without_usage() {
        let line = r#"{"done":true}"#;
        let chunk = parse_ndjson_line(line).unwrap();
        assert!(chunk.done);
        assert!(chunk.eval_count.is_none());
        assert!(chunk.prompt_eval_count.is_none());
    }

    #[test]
    fn parse_ndjson_invalid_json_returns_error() {
        let line = "not json at all";
        assert!(parse_ndjson_line(line).is_err());
    }

    #[test]
    fn render_local_message_content_includes_optional_sections() {
        let message = SessionMessage {
            role: MessageRole::User,
            content: "Summarize this".to_string(),
            attachments: vec![Attachment::File {
                path: "/tmp/notes.md".to_string(),
                mime: Some("text/markdown".to_string()),
                data: Some("IyBOb3RlcwoKLSBmaXJzdAo=".to_string()),
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

        let rendered = render_local_message_content(&message);
        assert!(rendered.contains("Additional context JSON"));
        assert!(rendered.contains("Attachments JSON"));
        assert!(rendered.contains("Attachment content previews JSON"));
        assert!(rendered.contains("Available tools JSON"));
        assert!(rendered.contains("Required response schema JSON"));
        assert!(rendered.contains("Notes"));
    }

    #[test]
    fn render_local_message_content_skips_binary_attachment_previews() {
        let message = SessionMessage {
            role: MessageRole::User,
            content: "Summarize this".to_string(),
            attachments: vec![Attachment::File {
                path: "/tmp/photo.png".to_string(),
                mime: Some("image/png".to_string()),
                data: Some("iVBORw0KGgoAAAANSUhEUgAAAAEAAAAB".to_string()),
            }],
            tools: None,
            context: None,
            response_format: None,
        };

        let rendered = render_local_message_content(&message);
        assert!(rendered.contains("Attachments JSON"));
        assert!(!rendered.contains("Attachment content previews JSON"));
    }

    #[test]
    fn render_local_message_content_falls_back_to_response_format_when_schema_missing() {
        let message = SessionMessage {
            role: MessageRole::User,
            content: "Summarize this".to_string(),
            attachments: Vec::new(),
            tools: None,
            context: None,
            response_format: Some(serde_json::json!({
                "type": "json_object"
            })),
        };

        let rendered = render_local_message_content(&message);
        assert!(rendered.contains("Required response format JSON"));
        assert!(!rendered.contains("Required response schema JSON"));
    }

    #[test]
    fn local_content_blocks_include_image_attachments() {
        let blocks = local_content_blocks(
            "Describe this image",
            &[
                Attachment::Image {
                    mime: "image/png".to_string(),
                    path: None,
                    data: Some("iVBORw0KGgo=".to_string()),
                },
                Attachment::File {
                    path: "/tmp/chart.jpg".to_string(),
                    mime: Some("image/jpeg".to_string()),
                    data: Some("/9j/4AAQSkZJRg==".to_string()),
                },
            ],
        )
        .expect("image attachments should produce content blocks");

        assert_eq!(blocks.len(), 3);
        assert!(matches!(blocks[0], ContentBlock::Text { .. }));
        assert!(matches!(blocks[1], ContentBlock::Image { .. }));
        assert!(matches!(blocks[2], ContentBlock::Image { .. }));
    }

    #[test]
    fn ollama_message_payload_emits_images_from_content_blocks() {
        let payload = ollama_message_payload(&ChatMessage {
            role: ChatRole::User,
            content: "fallback".to_string(),
            content_blocks: Some(vec![
                ContentBlock::Text {
                    text: "Describe this image".to_string(),
                },
                ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: "iVBORw0KGgo=".to_string(),
                },
            ]),
        });

        assert_eq!(payload["content"], "Describe this image");
        assert_eq!(payload["images"][0], "iVBORw0KGgo=");
    }

    // ── History truncation ──────────────────────────────────────

    #[test]
    fn truncate_preserves_system_prompt() {
        let mut history = vec![
            ChatMessage {
                role: ChatRole::System,
                content: "You are helpful.".to_string(),
                content_blocks: None,
            },
            ChatMessage {
                role: ChatRole::User,
                content: "msg 1".to_string(),
                content_blocks: None,
            },
            ChatMessage {
                role: ChatRole::Assistant,
                content: "resp 1".to_string(),
                content_blocks: None,
            },
            ChatMessage {
                role: ChatRole::User,
                content: "msg 2".to_string(),
                content_blocks: None,
            },
            ChatMessage {
                role: ChatRole::Assistant,
                content: "resp 2".to_string(),
                content_blocks: None,
            },
            ChatMessage {
                role: ChatRole::User,
                content: "msg 3".to_string(),
                content_blocks: None,
            },
            ChatMessage {
                role: ChatRole::Assistant,
                content: "resp 3".to_string(),
                content_blocks: None,
            },
        ];

        // Keep max 4 messages: system + last 3
        truncate_chat_history(&mut history, 4);

        assert_eq!(history.len(), 4);
        // First message is always the system prompt.
        assert_eq!(history[0].role, ChatRole::System);
        assert_eq!(history[0].content, "You are helpful.");
        // Last 3 messages are the most recent.
        assert_eq!(history[1].content, "resp 2");
        assert_eq!(history[2].content, "msg 3");
        assert_eq!(history[3].content, "resp 3");
    }

    #[test]
    fn truncate_no_op_when_under_limit() {
        let mut history = vec![
            ChatMessage {
                role: ChatRole::System,
                content: "system".to_string(),
                content_blocks: None,
            },
            ChatMessage {
                role: ChatRole::User,
                content: "hello".to_string(),
                content_blocks: None,
            },
        ];

        truncate_chat_history(&mut history, 10);
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn truncate_exact_boundary() {
        let mut history = vec![
            ChatMessage {
                role: ChatRole::System,
                content: "system".to_string(),
                content_blocks: None,
            },
            ChatMessage {
                role: ChatRole::User,
                content: "a".to_string(),
                content_blocks: None,
            },
            ChatMessage {
                role: ChatRole::Assistant,
                content: "b".to_string(),
                content_blocks: None,
            },
        ];

        truncate_chat_history(&mut history, 3);
        assert_eq!(history.len(), 3);
    }

    // ── Session construction and info ───────────────────────────

    #[test]
    fn session_info_returns_correct_metadata() {
        let config = Arc::new(AiSessionConfig::default());
        let session = LocalLlmSession::new(
            "test-session-1".to_string(),
            "llama3".to_string(),
            "http://localhost:11434".to_string(),
            Some("You are helpful.".to_string()),
            config,
        );

        let info = session.info();
        assert_eq!(info.session_id, "test-session-1");
        assert_eq!(info.provider_name, "ollama");
        assert_eq!(info.model, "llama3");
        assert_eq!(info.state, SessionState::Active);
        assert_eq!(info.transport, SessionTransport::LocalLlm);
        assert_eq!(info.turn_count, 0);
    }

    #[test]
    fn session_id_and_provider_name() {
        let config = Arc::new(AiSessionConfig::default());
        let session = LocalLlmSession::new(
            "sid-42".to_string(),
            "qwen3:8b".to_string(),
            "http://localhost:11434".to_string(),
            None,
            config,
        );

        assert_eq!(session.session_id(), "sid-42");
        assert_eq!(session.provider_name(), "ollama");
    }

    #[test]
    fn session_initializes_with_system_prompt_in_history() {
        let config = Arc::new(AiSessionConfig::default());
        let session = LocalLlmSession::new(
            "s1".to_string(),
            "llama3".to_string(),
            "http://localhost:11434".to_string(),
            Some("Be concise.".to_string()),
            config,
        );

        let history = session.history.blocking_read();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].role, ChatRole::System);
        assert_eq!(history[0].content, "Be concise.");
    }

    #[test]
    fn session_initializes_empty_history_without_system_prompt() {
        let config = Arc::new(AiSessionConfig::default());
        let session = LocalLlmSession::new(
            "s2".to_string(),
            "llama3".to_string(),
            "http://localhost:11434".to_string(),
            None,
            config,
        );

        let history = session.history.blocking_read();
        assert_eq!(history.len(), 0);
    }

    #[test]
    fn base_url_trailing_slash_stripped() {
        let config = Arc::new(AiSessionConfig::default());
        let session = LocalLlmSession::new(
            "s3".to_string(),
            "llama3".to_string(),
            "http://localhost:11434/".to_string(),
            None,
            config,
        );

        assert_eq!(session.base_url, "http://localhost:11434");
    }

    // ── NDJSON to OutboundMessage normalization ─────────────────

    #[test]
    fn ndjson_chunk_to_outbound_text() {
        let line =
            r#"{"model":"llama3","message":{"role":"assistant","content":"world"},"done":false}"#;
        let chunk = parse_ndjson_line(line).unwrap();

        assert!(!chunk.done);
        let content = &chunk.message.as_ref().unwrap().content;
        assert_eq!(content, "world");
    }

    #[test]
    fn ndjson_final_to_outbound_result_with_usage() {
        let line = r#"{"model":"llama3","message":{"role":"assistant","content":""},"done":true,"eval_count":123,"prompt_eval_count":45}"#;
        let chunk = parse_ndjson_line(line).unwrap();

        assert!(chunk.done);
        let usage = TokenUsage {
            input_tokens: chunk.prompt_eval_count.unwrap(),
            output_tokens: chunk.eval_count.unwrap(),
        };
        assert_eq!(usage.input_tokens, 45);
        assert_eq!(usage.output_tokens, 123);
    }

    /// iter-73 regression guard for iter-55c Ollama 404 semantic mapping.
    /// Ollama returns 404 when a model isn't pulled; we surface this as
    /// CoreError::NotFound with resource_type="ollama_model" so frontend
    /// can suggest `ollama pull <model>` rather than "network error".
    #[tokio::test]
    async fn ollama_404_maps_to_not_found_with_model_hint() {
        use oneshim_core::models::ai_session::SessionMessage;
        use oneshim_core::ports::conversation_session::ConversationSession;

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/api/chat")
            .with_status(404)
            .with_body("model 'llama3' not found — try `ollama pull llama3`")
            .create_async()
            .await;

        let session = LocalLlmSession::new(
            "test-session".to_string(),
            "llama3".to_string(),
            server.url(),
            None,
            Arc::new(AiSessionConfig::default()),
        );

        let message = SessionMessage {
            role: MessageRole::User,
            content: "hello".to_string(),
            attachments: vec![],
            tools: None,
            context: None,
            response_format: None,
        };

        let result = session.send_message(&message).await;
        match result {
            Err(CoreError::NotFound {
                resource_type, id, ..
            }) => {
                assert_eq!(
                    resource_type, "ollama_model",
                    "resource_type should be ollama_model"
                );
                assert!(
                    id.contains("ollama pull") || id.contains("not found"),
                    "id should carry the pull hint, got: {id}"
                );
            }
            Err(other) => panic!("expected CoreError::NotFound, got: {other:?}"),
            Ok(_) => panic!("expected error, got Ok"),
        }
    }

    /// iter-76 regression guard: non-404 HTTP errors from Ollama fall back
    /// to CoreError::Network (not NotFound), so the "model not pulled" UX
    /// only triggers for the specific 404 case.
    #[tokio::test]
    async fn ollama_500_maps_to_network_generic() {
        use oneshim_core::models::ai_session::SessionMessage;
        use oneshim_core::ports::conversation_session::ConversationSession;

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/api/chat")
            .with_status(500)
            .with_body("Internal Server Error")
            .create_async()
            .await;

        let session = LocalLlmSession::new(
            "test-session".to_string(),
            "llama3".to_string(),
            server.url(),
            None,
            Arc::new(AiSessionConfig::default()),
        );

        let message = SessionMessage {
            role: MessageRole::User,
            content: "hello".to_string(),
            attachments: vec![],
            tools: None,
            context: None,
            response_format: None,
        };

        let result = session.send_message(&message).await;
        match result {
            Err(CoreError::Network { .. }) => {
                // Expected: 500 (non-404) falls back to Network/Generic
            }
            Err(other) => {
                panic!("500 should map to CoreError::Network (not domain-specific), got: {other:?}")
            }
            Ok(_) => panic!("expected error, got Ok"),
        }
    }
}
