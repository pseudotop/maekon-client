//! HTTP API session adapter — direct Anthropic/OpenAI API calls with
//! self-managed conversation history and SSE streaming responses.

mod anthropic;
mod content;
mod google;
mod openai;

use anthropic::*;
use content::*;
use google::*;
use openai::*;

#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use async_stream::try_stream;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use eventsource_stream::Eventsource;
use futures::stream::StreamExt;
use tokio::sync::RwLock;
use tracing::warn;

use maekon_api_contracts::provider_specs::{
    self, ProviderAuthScheme, ProviderRequestShape, ProviderTransportKind,
};
use maekon_core::config::{AiProviderType, AiSessionConfig};
use maekon_core::error::CoreError;
use maekon_core::models::ai_session::{
    truncate_chat_history, ChatMessage, ChatRole, ContentBlock, ConversationSessionInfo,
    OutboundMessage, SessionMessage, SessionState, SessionTransport, ToolDefinition, ToolUseStatus,
};
use maekon_core::ports::conversation_session::{ConversationSession, ResponseStream};
use maekon_core::ports::credential_source::CredentialSource;

use crate::provider_error_body::provider_error_message;

/// Direct HTTP API session adapter for Anthropic and OpenAI providers.
///
/// Manages conversation history locally and streams responses via SSE.
pub struct HttpApiSession {
    session_id: String,
    surface_id: String,
    model: String,
    endpoint: String,
    credential: CredentialSource,
    provider_type: AiProviderType,
    history: Arc<RwLock<Vec<ChatMessage>>>,
    system_prompt: Option<String>,
    default_tools: Option<Vec<ToolDefinition>>,
    state: parking_lot::Mutex<SessionState>,
    turn_count: AtomicU32,
    created_at: DateTime<Utc>,
    // #6518 parity: store wall-clock directly. The old `Mutex<Instant>` +
    // `Utc::now() - elapsed()` reconstruction skews (or lands in the future) if
    // the system clock steps between the write and `info()`.
    last_active: parking_lot::Mutex<DateTime<Utc>>,
    http_client: reqwest::Client,
    config: Arc<AiSessionConfig>,
    /// D7: per-endpoint circuit breaker. Three-tier semantics for streaming
    /// per spec O2: initial HTTP status is the breaker signal; mid-stream
    /// disconnects are not recorded (matches BatchUploader's
    /// "server-acknowledged" = success pattern).
    breaker: Arc<crate::circuit_breaker::CircuitBreaker>,
}

pub struct HttpApiSessionInit {
    pub surface_id: String,
    pub model: String,
    pub endpoint: String,
    pub credential: CredentialSource,
    pub provider_type: AiProviderType,
    pub system_prompt: Option<String>,
    pub config: Arc<AiSessionConfig>,
    pub default_tools: Option<Vec<ToolDefinition>>,
    /// D7: Shared per-endpoint circuit breaker registry (resolved by
    /// constructor using `endpoint`). Allows a single workspace registry
    /// to feed multiple session instances.
    pub breaker_registry: Arc<crate::circuit_breaker::CircuitBreakerRegistry>,
}

#[derive(Debug, Default)]
struct RequestOptions<'a> {
    response_format: Option<&'a serde_json::Value>,
    tools: Option<&'a [ToolDefinition]>,
}

struct PartialToolCall {
    id: String,
    name: String,
    arguments: String,
}

/// Upper bound on the number of tool-call slots a single SSE response may
/// allocate. The `index`/`output_index` field on a `ToolCallDelta` is read
/// verbatim from the wire, so a malicious, buggy, or MITM endpoint could send a
/// huge index and force unbounded `Vec` growth (OOM/DoS, blowing the ~100MB
/// budget). Deltas whose index is at or above this cap are dropped. 256 is far
/// beyond any realistic tool-call count in one turn.
const MAX_TOOL_CALLS: usize = 256;

/// Upper bound on the total bytes a single streaming turn may accumulate across
/// assistant text plus every tool-call's argument buffer.
///
/// Per-event size is already capped, and per-turn tool-call *count* is capped by
/// [`MAX_TOOL_CALLS`], but neither bounds the *aggregate* growth: a hostile or
/// buggy endpoint can stream an unbounded number of in-cap text/argument deltas
/// and slowly exhaust the heap of this 24/7 process (blowing the ~100MB budget).
/// When the running total exceeds this ceiling the turn is terminated with an
/// error instead of accumulating further. 8 MiB is generous for any legitimate
/// assistant turn while staying an order of magnitude under the process budget;
/// it is intentionally larger than the 1 MiB per-event/`MAX_WS_MESSAGE_BYTES`
/// cap so a single in-cap event never trips it.
const MAX_TURN_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

/// Merge a single `ToolCallDelta` into the accumulated tool-call buffer.
///
/// `index` is read verbatim from the wire, so it is bounds-checked against
/// [`MAX_TOOL_CALLS`] before the buffer is grown: out-of-range deltas are
/// dropped (with a warning) rather than forcing unbounded allocation.
///
/// Returns the number of argument bytes actually appended so the caller can fold
/// them into the per-turn byte budget ([`MAX_TURN_RESPONSE_BYTES`]). A dropped
/// (out-of-range) delta contributes 0.
fn accumulate_tool_call_delta(
    tool_calls: &mut Vec<PartialToolCall>,
    index: u32,
    id: &str,
    name: &str,
    arguments_chunk: &str,
) -> usize {
    let idx = index as usize;
    if idx >= MAX_TOOL_CALLS {
        warn!(
            index = idx,
            cap = MAX_TOOL_CALLS,
            "dropping tool-call delta with out-of-range index"
        );
        return 0;
    }
    // Ensure the vec is large enough (bounded by MAX_TOOL_CALLS above).
    while tool_calls.len() <= idx {
        tool_calls.push(PartialToolCall {
            id: String::new(),
            name: String::new(),
            arguments: String::new(),
        });
    }
    if !id.is_empty() {
        tool_calls[idx].id.clear();
        tool_calls[idx].id.push_str(id);
    }
    if !name.is_empty() {
        tool_calls[idx].name.clear();
        tool_calls[idx].name.push_str(name);
    }
    if !arguments_chunk.is_empty() {
        tool_calls[idx].arguments.push_str(arguments_chunk);
    }
    arguments_chunk.len()
}

impl HttpApiSession {
    /// Create a new HTTP API session.
    pub fn new(init: HttpApiSessionInit) -> Self {
        let session_id = maekon_core::generate_id("ses");
        let http_client = reqwest::Client::new();
        let mut initial_history = Vec::new();

        if let Some(ref prompt) = init.system_prompt {
            initial_history.push(ChatMessage {
                role: ChatRole::System,
                content: prompt.clone(),
                content_blocks: None,
            });
        }

        // D7: resolve per-endpoint breaker.
        let breaker_key = crate::resilience::endpoint_authority(&init.endpoint)
            .unwrap_or_else(|_| format!("malformed::{}", init.endpoint));
        let breaker = init.breaker_registry.get(&breaker_key);

        Self {
            session_id,
            surface_id: init.surface_id,
            model: init.model,
            endpoint: init.endpoint,
            credential: init.credential,
            provider_type: init.provider_type,
            history: Arc::new(RwLock::new(initial_history)),
            system_prompt: init.system_prompt,
            default_tools: init.default_tools,
            state: parking_lot::Mutex::new(SessionState::Active),
            turn_count: AtomicU32::new(0),
            created_at: Utc::now(),
            last_active: parking_lot::Mutex::new(Utc::now()),
            http_client,
            breaker,
            config: init.config,
        }
    }

    /// Build provider-specific streaming request body from conversation history.
    fn build_request_body(
        &self,
        messages: &[ChatMessage],
        options: &RequestOptions<'_>,
    ) -> Result<serde_json::Value, CoreError> {
        let shape = provider_specs::resolved_request_shape(
            self.provider_type,
            Some(&self.surface_id),
            ProviderTransportKind::Llm,
        )
        .map_err(|msg| CoreError::Config {
            code: maekon_core::error_codes::ConfigCode::Invalid,
            message: msg,
        })?;

        match shape {
            ProviderRequestShape::AnthropicMessages
            | ProviderRequestShape::AnthropicVisionMessages => Ok(build_anthropic_request_body(
                &self.model,
                self.config.max_output_tokens,
                self.system_prompt.as_deref(),
                self.config.thinking.as_ref(),
                messages,
                options.tools,
            )),
            ProviderRequestShape::OpenAiChatCompletions
            | ProviderRequestShape::OpenAiVisionChatCompletions => {
                Ok(build_openai_chat_request_body(
                    &self.model,
                    self.config.max_output_tokens,
                    self.config.thinking.as_ref(),
                    messages,
                    options.response_format,
                    options.tools,
                ))
            }
            ProviderRequestShape::OpenAiResponses => Ok(build_openai_responses_request_body(
                &self.model,
                self.config.max_output_tokens,
                self.system_prompt.as_deref(),
                self.config.thinking.as_ref(),
                &self.surface_id,
                messages,
                options.response_format,
                options.tools,
            )),
            ProviderRequestShape::GoogleGenerateContent => Ok(build_google_request_body(
                self.config.max_output_tokens,
                self.system_prompt.as_deref(),
                self.config.thinking.as_ref(),
                messages,
                options.response_format,
                options.tools,
            )),
            ProviderRequestShape::BedrockConverse => Err(CoreError::Config {
                code: maekon_core::error_codes::ConfigCode::UnsupportedProviderBedrock,
                message: "AWS Bedrock is intentionally unsupported in this build".into(),
            }),
            _ => Err(CoreError::Config {
                code: maekon_core::error_codes::ConfigCode::Invalid,
                message: format!("unsupported request shape for HTTP API session: {shape:?}"),
            }),
        }
    }

    /// Resolve auth headers for the provider.
    async fn apply_auth(
        &self,
        builder: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, CoreError> {
        let auth_scheme = provider_specs::resolved_auth_scheme(
            self.provider_type,
            Some(&self.surface_id),
            ProviderTransportKind::Llm,
        )
        .map_err(|msg| CoreError::Config {
            code: maekon_core::error_codes::ConfigCode::Invalid,
            message: msg,
        })?;

        let builder = match auth_scheme {
            ProviderAuthScheme::None => builder,
            ProviderAuthScheme::XApiKey => {
                let token = self.credential.resolve_bearer_token().await?;
                builder
                    .header("x-api-key", &token)
                    .header("anthropic-version", crate::ANTHROPIC_API_VERSION)
            }
            ProviderAuthScheme::XGoogApiKey => {
                let token = self.credential.resolve_bearer_token().await?;
                builder.header("x-goog-api-key", &token)
            }
            ProviderAuthScheme::Bearer => {
                let token = self.credential.resolve_bearer_token().await?;
                builder.header("Authorization", format!("Bearer {}", token))
            }
            ProviderAuthScheme::AwsSignatureV4 => {
                return Err(CoreError::Config {
                    code: maekon_core::error_codes::ConfigCode::UnsupportedProviderBedrock,
                    message: "AWS Bedrock is intentionally unsupported in this build".into(),
                });
            }
        };

        Ok(builder)
    }

    /// Resolve the streaming endpoint URL.
    ///
    /// Google requires `streamGenerateContent?alt=sse` instead of `generateContent`.
    fn streaming_endpoint(&self) -> String {
        let shape = provider_specs::resolved_request_shape(
            self.provider_type,
            Some(&self.surface_id),
            ProviderTransportKind::Llm,
        );
        if matches!(shape, Ok(ProviderRequestShape::GoogleGenerateContent)) {
            self.endpoint
                .replace(":generateContent", ":streamGenerateContent")
                + if self.endpoint.contains('?') {
                    "&alt=sse"
                } else {
                    "?alt=sse"
                }
        } else {
            self.endpoint.clone()
        }
    }

    /// Truncate history to keep the system prompt (first message) + last (max-1) messages.
    #[cfg(test)]
    fn truncate_history(history: &mut Vec<ChatMessage>, max_turns: u32) {
        truncate_chat_history(history, max_turns);
    }
}

/// Convert a `SessionMessage` into a `ChatMessage` with optional multimodal content blocks.
fn prepare_chat_message(message: &SessionMessage, shape: &ProviderRequestShape) -> ChatMessage {
    let message_content = render_message_content(message, shape);

    let content_blocks = {
        let mut blocks = Vec::new();
        if !message_content.trim().is_empty() {
            blocks.push(ContentBlock::Text {
                text: message_content.clone(),
            });
        }
        for att in &message.attachments {
            if let Some(block) = native_content_block(shape, att) {
                blocks.push(block);
            }
        }
        let starts_with_non_text = blocks
            .first()
            .is_some_and(|block| !matches!(block, ContentBlock::Text { .. }));
        if blocks.len() > 1 || starts_with_non_text {
            Some(blocks)
        } else {
            None
        }
    };

    ChatMessage {
        role: ChatRole::User,
        content: message_content,
        content_blocks,
    }
}

/// Append an assistant response to conversation history and truncate to the configured limit.
async fn save_assistant_response(
    history: &RwLock<Vec<ChatMessage>>,
    content: &str,
    max_turns: u32,
) {
    let assistant_msg = ChatMessage {
        role: ChatRole::Assistant,
        content: content.to_owned(),
        content_blocks: None,
    };
    let mut hist = history.write().await;
    hist.push(assistant_msg);
    truncate_chat_history(&mut hist, max_turns);
}

#[async_trait]
impl ConversationSession for HttpApiSession {
    async fn send_message(&self, message: &SessionMessage) -> Result<ResponseStream, CoreError> {
        // D7: pre-flight breaker check. If the endpoint is persistently
        // failing, fast-fail before doing any message history mutation or
        // request assembly.
        if matches!(
            self.breaker.check(),
            crate::circuit_breaker::CircuitState::Open { .. }
        ) {
            return Err(CoreError::ServiceUnavailable {
                code: maekon_core::error_codes::ServiceCode::CircuitOpen,
                message: format!("circuit open for {}", self.endpoint),
            });
        }

        let shape = provider_specs::resolved_request_shape(
            self.provider_type,
            Some(&self.surface_id),
            ProviderTransportKind::Llm,
        )
        .map_err(|msg| CoreError::Config {
            code: maekon_core::error_codes::ConfigCode::Invalid,
            message: msg,
        })?;

        let user_msg = prepare_chat_message(message, &shape);

        // #6125: Do NOT commit the user message into the shared `self.history`
        // until the handshake (auth + transport + 2xx status) succeeds. The
        // session is reused on retry (error_recovery resets transient
        // 429/503/timeout/network errors back to Active without clearing
        // history), so committing before the handshake means a failed send
        // leaves the user message in history and a re-send pushes a SECOND
        // copy — egressing two identical user messages to the provider.
        //
        // Instead build the request body from a local snapshot that includes
        // the user message, and only push it into live history once the 2xx
        // response is confirmed.
        let mut messages_snapshot = self.history.read().await.clone();
        messages_snapshot.push(user_msg.clone());
        let effective_tools = message
            .tools
            .as_deref()
            .or(self.default_tools.as_deref())
            .filter(|tools| !tools.is_empty());
        let request_body = self.build_request_body(
            &messages_snapshot,
            &RequestOptions {
                response_format: message.response_format.as_ref(),
                tools: effective_tools,
            },
        )?;

        let streaming_url = self.streaming_endpoint();
        let builder = self
            .http_client
            .post(&streaming_url)
            .header("Content-Type", "application/json")
            .json(&request_body);

        let builder = self.apply_auth(builder).await.inspect_err(|_| {
            *self.state.lock() = SessionState::Failed;
        })?;

        let send_result = builder.send().await;
        // D7 three-tier semantics (spec O2): classify based on initial HTTP
        // status ONLY. Mid-stream disconnects in the returned stream do NOT
        // affect breaker state (indistinguishable from client-side cancel).
        let signal = match &send_result {
            Ok(resp) => {
                crate::resilience::classify_for_breaker(Some(resp.status().as_u16()), false)
            }
            Err(_) => crate::resilience::classify_for_breaker(None, true),
        };
        match signal {
            crate::resilience::BreakerSignal::Success => self.breaker.record_success(),
            crate::resilience::BreakerSignal::Failure => self.breaker.record_failure(),
            crate::resilience::BreakerSignal::Neutral => {}
        }
        let response = send_result.map_err(|e| {
            *self.state.lock() = SessionState::Failed;
            // Iter-90: split timeout vs generic per canonical pattern.
            if e.is_timeout() {
                CoreError::RequestTimeout {
                    code: maekon_core::error_codes::NetworkCode::Timeout,
                    timeout_ms: 0,
                }
            } else {
                CoreError::Network {
                    code: maekon_core::error_codes::NetworkCode::Generic,
                    message: format!("HTTP API session request failed: {e}"),
                }
            }
        })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.ok();
            *self.state.lock() = SessionState::Failed;
            let message = provider_error_message("HTTP API", status, body.as_deref());
            // Semantic HTTP status mapping per iter-54..59 pattern.
            return Err(match status.as_u16() {
                401 | 403 => CoreError::Auth {
                    code: maekon_core::error_codes::AuthCode::Failed,
                    message,
                },
                408 | 504 => CoreError::RequestTimeout {
                    code: maekon_core::error_codes::NetworkCode::Timeout,
                    timeout_ms: 0,
                },
                429 => CoreError::RateLimit {
                    code: maekon_core::error_codes::NetworkCode::RateLimit,
                    retry_after_secs: 60,
                },
                502 | 503 => CoreError::ServiceUnavailable {
                    code: maekon_core::error_codes::ServiceCode::Unavailable,
                    message,
                },
                _ => CoreError::Network {
                    code: maekon_core::error_codes::NetworkCode::Generic,
                    message,
                },
            });
        }

        // #6125: Handshake confirmed (2xx) — now atomically commit the user
        // message to the shared history. Every Err path above returns before
        // reaching here, so a failed send never mutates history and a retry
        // cannot push a duplicate copy.
        {
            let mut history = self.history.write().await;
            history.push(user_msg);
        }

        let history = self.history.clone();
        let max_turns = self.config.max_history_turns;
        let turn_count = &self.turn_count;
        turn_count.fetch_add(1, Ordering::Relaxed);
        *self.last_active.lock() = Utc::now();

        // Build the ResponseStream using SSE parsing
        let stream: ResponseStream = Box::pin(try_stream! {
            let mut accumulated = String::new();
            let mut tool_calls: Vec<PartialToolCall> = Vec::new();
            // Running total across accumulated assistant text + every tool-call's
            // argument bytes. Bounds aggregate growth so a stream of individually
            // in-cap deltas cannot exhaust the heap (see MAX_TURN_RESPONSE_BYTES).
            let mut turn_bytes: usize = 0;

            let is_anthropic = matches!(
                shape,
                ProviderRequestShape::AnthropicMessages
                    | ProviderRequestShape::AnthropicVisionMessages
            );
            let is_google = matches!(shape, ProviderRequestShape::GoogleGenerateContent);

            let byte_stream = response.bytes_stream();
            let mut event_stream = byte_stream.eventsource();

            while let Some(event_result) = event_stream.next().await {
                match event_result {
                    Ok(event) => {
                        let parsed = if is_anthropic {
                            parse_anthropic_sse_event(&event.event, &event.data)
                        } else if is_google {
                            parse_google_sse_event(&event.data)
                        } else if matches!(shape, ProviderRequestShape::OpenAiResponses) {
                            parse_openai_responses_sse_event(&event.event, &event.data)
                        } else {
                            parse_openai_chat_sse_event(&event.data)
                        };

                        if let Some(msg) = parsed {
                            match &msg {
                                OutboundMessage::ToolCallDelta { index, id, name, arguments_chunk } => {
                                    // Accumulation is guarded against unbounded `index` values
                                    // (OOM/DoS) inside the helper.
                                    let appended = accumulate_tool_call_delta(&mut tool_calls, *index, id, name, arguments_chunk);
                                    turn_bytes = turn_bytes.saturating_add(appended);
                                    if turn_bytes > MAX_TURN_RESPONSE_BYTES {
                                        warn!(
                                            turn_bytes,
                                            cap_bytes = MAX_TURN_RESPONSE_BYTES,
                                            "turn response exceeded byte cap — terminating turn"
                                        );
                                        Err(CoreError::Network {
                                            code: maekon_core::error_codes::NetworkCode::Generic,
                                            message: format!(
                                                "streaming turn exceeded {MAX_TURN_RESPONSE_BYTES}-byte response cap"
                                            ),
                                        })?;
                                    }
                                    // ToolCallDelta is internal — don't yield to consumer
                                    continue;
                                }
                                OutboundMessage::Text { content, .. } => {
                                    accumulated.push_str(content);
                                    turn_bytes = turn_bytes.saturating_add(content.len());
                                    if turn_bytes > MAX_TURN_RESPONSE_BYTES {
                                        warn!(
                                            turn_bytes,
                                            cap_bytes = MAX_TURN_RESPONSE_BYTES,
                                            "turn response exceeded byte cap — terminating turn"
                                        );
                                        Err(CoreError::Network {
                                            code: maekon_core::error_codes::NetworkCode::Generic,
                                            message: format!(
                                                "streaming turn exceeded {MAX_TURN_RESPONSE_BYTES}-byte response cap"
                                            ),
                                        })?;
                                    }
                                }
                                OutboundMessage::Result { content, done, .. } => {
                                    // #6197: the Result arm matches BOTH the
                                    // intermediate usage-only chunk (done=false —
                                    // Anthropic message_delta, OpenAI-Chat usage
                                    // chunk) AND the terminal chunk (done=true —
                                    // message_stop, [DONE], finish_reason). Only
                                    // the terminal chunk must mutate history;
                                    // saving on the usage-only chunk pushed a
                                    // SECOND identical assistant message per turn.
                                    // The intermediate Result is still yielded
                                    // below for live token display.
                                    if *done {
                                        // #6203: when a provider (Gemini) delivers
                                        // the final text in the SAME chunk as
                                        // usageMetadata/finishReason, that text
                                        // arrives as Result{content,done:true} and
                                        // was never seen by the Text arm. Fold it
                                        // into `accumulated` before saving so the
                                        // final text is preserved in history and
                                        // available for suggestion extraction.
                                        if !content.is_empty() {
                                            accumulated.push_str(content);
                                        }

                                        // Emit accumulated tool calls before saving text history
                                        for tc in tool_calls.drain(..) {
                                            if tc.name.is_empty() { continue; }
                                            let parsed_args = serde_json::from_str(&tc.arguments).unwrap_or(serde_json::json!({}));
                                            yield OutboundMessage::ToolUse {
                                                tool: tc.name,
                                                input: Some(parsed_args),
                                                status: ToolUseStatus::Started,
                                                result: None,
                                            };
                                        }

                                        save_assistant_response(&history, &accumulated, max_turns).await;
                                    }
                                }
                                OutboundMessage::Thinking { .. } => {
                                    // Stream to frontend but don't accumulate in history
                                }
                                _ => {}
                            }
                            yield msg;
                        }
                    }
                    Err(e) => {
                        warn!("SSE stream error: {e}");
                        // Append whatever we accumulated so far
                        if !accumulated.is_empty() {
                            save_assistant_response(&history, &accumulated, max_turns).await;
                        }
                        Err(CoreError::Network { code: maekon_core::error_codes::NetworkCode::Generic, message: format!("SSE stream error: {e}") })?;
                    }
                }
            }

            // If stream ended without a message_stop/[DONE] event, still save history
            if !accumulated.is_empty() {
                let has_result = {
                    let hist: tokio::sync::RwLockReadGuard<'_, Vec<ChatMessage>> = history.read().await;
                    hist.last().is_some_and(|m| m.role == ChatRole::Assistant)
                };
                if !has_result {
                    save_assistant_response(&history, &accumulated, max_turns).await;

                    yield OutboundMessage::Result {
                        content: accumulated,
                        done: true,
                        usage: None,
                    };
                }
            }
        });

        Ok(stream)
    }

    fn info(&self) -> ConversationSessionInfo {
        let last_active_utc = *self.last_active.lock();
        ConversationSessionInfo {
            session_id: self.session_id.clone(),
            provider_name: format!("{:?}", self.provider_type).to_lowercase(),
            model: self.model.clone(),
            state: *self.state.lock(),
            transport: SessionTransport::HttpApi,
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
        match self.provider_type {
            AiProviderType::Anthropic => "anthropic",
            AiProviderType::OpenAi => "openai",
            AiProviderType::Google => "google",
            AiProviderType::Ollama => "ollama",
            AiProviderType::Bedrock => "bedrock",
            AiProviderType::Copilot => "copilot",
            AiProviderType::Generic => "generic",
        }
    }

    fn is_external(&self) -> bool {
        // Cloud HTTP APIs egress chat content off-device and must be guarded.
        // Ollama is a localhost endpoint (on-device) — kept unguarded, matching
        // `LocalLlmSession`'s default. See E21 review B1.
        !matches!(self.provider_type, AiProviderType::Ollama)
    }

    fn egress_endpoint_urls(&self) -> Vec<&str> {
        vec![&self.endpoint]
    }
}
