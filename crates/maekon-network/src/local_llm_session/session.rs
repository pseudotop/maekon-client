use std::sync::atomic::Ordering;

use async_stream::try_stream;
use async_trait::async_trait;
use chrono::Utc;
use futures::StreamExt;
use tracing::{debug, warn};

use maekon_core::error::CoreError;
use maekon_core::models::ai_session::{
    truncate_chat_history, ChatMessage, ChatRole, ConversationSessionInfo, OutboundMessage,
    SessionMessage, SessionState, SessionTransport, TokenUsage,
};
use maekon_core::ports::conversation_session::{ConversationSession, ResponseStream};

use super::helpers::{
    local_content_blocks, ollama_message_payload, parse_ndjson_line, render_local_message_content,
};
use super::types::LocalLlmSession;

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
                        code: maekon_core::error_codes::NetworkCode::Timeout,
                        timeout_ms: 0,
                    }
                } else {
                    CoreError::Network {
                        code: maekon_core::error_codes::NetworkCode::Generic,
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
                    code: maekon_core::error_codes::NotFoundCode::ResourceMissing,
                    resource_type: "ollama_model".to_string(),
                    id: format!("{body_text} (hint: try `ollama pull <model>`)"),
                },
                _ => CoreError::Network {
                    code: maekon_core::error_codes::NetworkCode::Generic,
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
        *self.last_active.lock() = std::time::Instant::now();

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
                                code: maekon_core::error_codes::NetworkCode::Timeout,
                                timeout_ms: 0,
                            }
                        } else {
                            CoreError::Network {
                                code: maekon_core::error_codes::NetworkCode::Generic,
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

    /// Returns `true` when the session targets a non-loopback host — i.e. the
    /// user configured a LAN/remote Ollama server. Non-loopback → the session is
    /// treated as external, engaging GuardedConversationSession PII sanitization
    /// and the #4869 fail-closed no-guard refusal (E21 B1 invariant). Loopback
    /// (127.0.0.0/8, ::1, localhost) → on-device, passes through unsanitized per
    /// the existing local-session contract.
    ///
    /// Mirrors ADR-023 MG-PII-02 loopback gating logic.
    fn is_external(&self) -> bool {
        !crate::http_client::host_is_loopback(&format!("{}/", self.base_url))
    }
}
