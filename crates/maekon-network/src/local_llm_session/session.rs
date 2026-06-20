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
use crate::provider_error_body::provider_error_message;

/// #6206/#6207: Hard cap on the in-flight NDJSON line buffer. Ollama streams one
/// JSON object per line, so a single un-terminated line should never approach
/// this size. A newline-free (or pathologically long) body would otherwise grow
/// `line_buffer` without bound and OOM the process. Mirrors
/// `live_channel.rs::MAX_WS_MESSAGE_BYTES` (1 MiB) to stay within the agent's
/// <100 MB RSS budget.
const MAX_NDJSON_LINE_BYTES: usize = 1024 * 1024; // 1 MiB

impl LocalLlmSession {
    /// #6129: Roll back the trailing user message appended at the start of
    /// `send_message` when the send never reaches the success-commit path.
    ///
    /// This keeps history mutation transactional with the send outcome: a
    /// persistent Ollama outage must not grow history one user entry per failed
    /// retry. Only a trailing `User` message is popped — assistant replies are
    /// appended exclusively on the streaming success path, so the last entry is
    /// guaranteed to be the just-pushed user turn here.
    async fn pop_pending_user_message(&self) {
        let mut history = self.history.write().await;
        if matches!(history.last().map(|m| m.role), Some(ChatRole::User)) {
            history.pop();
        }
    }
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

        // #6129: Bound history at push time so a turn that never reaches the
        // `done` chunk (or fails before truncation on the success path) cannot
        // grow history across calls. The success path truncates again after the
        // assistant reply is appended.
        {
            let mut history = self.history.write().await;
            history.push(user_msg);
            truncate_chat_history(&mut history, self.config.max_history_turns);
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

        let send_result = self.http_client.post(&url).json(&body).send().await;

        let response = match send_result {
            Ok(response) => response,
            Err(e) => {
                // #6129: Roll back the just-pushed user message so a persistent
                // outage cannot grow history one entry per failed send.
                self.pop_pending_user_message().await;
                *self.state.lock() = SessionState::Failed;
                // Iter-90: Ollama is local, timeouts are rare but possible when
                // a large model is still loading. Keep the canonical split so
                // Grafana/logs can distinguish slow-model-load from true failure.
                return Err(if e.is_timeout() {
                    CoreError::RequestTimeout {
                        code: maekon_core::error_codes::NetworkCode::Timeout,
                        timeout_ms: 0,
                    }
                } else {
                    CoreError::Network {
                        code: maekon_core::error_codes::NetworkCode::Generic,
                        message: format!("Ollama request failed: {e}"),
                    }
                });
            }
        };

        if !response.status().is_success() {
            let status = response.status();
            let body_text = response.text().await.unwrap_or_default();
            // #6129: Roll back the just-pushed user message on non-success
            // status (e.g. 404 model-not-pulled, 5xx) for the same reason.
            self.pop_pending_user_message().await;
            *self.state.lock() = SessionState::Failed;
            // Ollama runs locally, so timeouts/gateway errors are rare. 404
            // most commonly means "model not pulled" — distinguish it so the
            // frontend can hint at `ollama pull <model>` rather than generic
            // "network error". (iter-55c)
            // Never echo the raw Ollama response body: a non-loopback (LAN/remote)
            // Ollama is an untrusted endpoint, and the body can echo the user's
            // prompt or other private content. Mirror the http_api_session masking
            // standard (`provider_error_body`) — keep status + a model hint only.
            return Err(match status.as_u16() {
                404 => CoreError::NotFound {
                    code: maekon_core::error_codes::NotFoundCode::ResourceMissing,
                    resource_type: "ollama_model".to_string(),
                    id: format!(
                        "model `{0}` not pulled (hint: try `ollama pull {0}`)",
                        self.model
                    ),
                },
                _ => CoreError::Network {
                    code: maekon_core::error_codes::NetworkCode::Generic,
                    message: provider_error_message("Ollama", status, Some(&body_text)),
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
        *self.last_active.lock() = Utc::now();

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

                // #6206/#6207: Bound the line buffer. A newline-free body would
                // otherwise accumulate every chunk into `line_buffer` and OOM
                // the process. If we are over the cap and there is still no line
                // terminator to drain against, there is no legitimate NDJSON the
                // server could be sending — fail the stream instead of growing.
                if line_buffer.len() > MAX_NDJSON_LINE_BYTES && !line_buffer.contains('\n') {
                    warn!(
                        session_id = %session_id,
                        bytes = line_buffer.len(),
                        limit = MAX_NDJSON_LINE_BYTES,
                        "Ollama NDJSON line exceeds size limit with no newline; terminating stream"
                    );
                    Err(CoreError::Network {
                        code: maekon_core::error_codes::NetworkCode::Generic,
                        message: format!(
                            "Ollama NDJSON line exceeded {MAX_NDJSON_LINE_BYTES} bytes without a newline"
                        ),
                    })?;
                }

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
        // #6506 follow-up: read the stored wall-clock directly. Previously this
        // subtracted a monotonic `Instant::elapsed()` from `Utc::now()`, which
        // produced a skewed (or future) timestamp if the system clock shifted
        // between the last activity and this call.
        let last_active_utc = *self.last_active.lock();
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
