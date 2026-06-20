// OOS-TBD: ADR-013 file split (cycle 35+) — LOC: 847
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_stream::try_stream;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use tempfile::tempdir;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::{Notify, RwLock};
use uuid::Uuid;

use maekon_api_contracts::provider_specs::{default_surface_model, provider_surface_spec};
use maekon_core::config::AiSessionConfig;
use maekon_core::error::CoreError;
use maekon_core::models::ai_session::{
    ChatMessage, ChatRole, ControlAction, ConversationSessionInfo, OutboundMessage, SessionConfig,
    SessionMessage, SessionState, SessionTransport, TokenUsage, ToolDefinition, ToolUseStatus,
};
use maekon_core::ports::conversation_session::{ConversationSession, ResponseStream};

use crate::session_adapters::prompt_payload::{
    extract_native_response_schema, render_conversation_prompt, render_message_payload,
};
use crate::session_adapters::task_guard::AbortOnDropJoin;
use crate::subprocess_provider::{
    append_model_flag, append_oneshot_flags, classify_subprocess_error_with_redactions,
    sanitize_subprocess_error_output, write_prompt_and_collect_output, DetectedSubprocessCli,
    SubprocessKind,
};
use tracing::debug;

pub struct GenericSubprocessSession {
    session_id: String,
    surface: DetectedSubprocessCli,
    /// Catalog invocation mode that drives conversation dispatch (E21 #4864 B3
    /// SSOT) — replaces hard-coded `surface_id` string comparisons.
    invocation_mode: maekon_api_contracts::provider_specs::SubprocessInvocationMode,
    provider_name: String,
    model: String,
    system_prompt: Option<String>,
    default_tools: Option<Vec<ToolDefinition>>,
    history: Arc<RwLock<Vec<ChatMessage>>>,
    state: Mutex<SessionState>,
    turn_count: AtomicU32,
    created_at: chrono::DateTime<chrono::Utc>,
    // #6518 parity: store wall-clock directly (was Mutex<Instant> + skew-prone
    // `Utc::now() - elapsed()` reconstruction).
    last_active: Mutex<DateTime<Utc>>,
    timeout: Duration,
    max_history_turns: u32,
    cancel_requested: Arc<AtomicBool>,
    cancel_notify: Arc<Notify>,
}

impl GenericSubprocessSession {
    pub fn new(
        surface: DetectedSubprocessCli,
        config: &SessionConfig,
        session_config: Arc<AiSessionConfig>,
        default_tools: Option<Vec<ToolDefinition>>,
    ) -> Self {
        let model = config
            .model
            .clone()
            .or_else(|| {
                default_surface_model(
                    &surface.surface_id,
                    maekon_api_contracts::provider_specs::SurfaceCapabilityKind::Llm,
                )
                .ok()
                .flatten()
            })
            .unwrap_or_else(|| "gpt-5.4".to_string());

        let provider_name = provider_surface_spec(&surface.surface_id)
            .map(|spec| spec.vendor_id.clone())
            .unwrap_or_else(|_| "subprocess".to_string());

        // Resolve the catalog invocation mode once at construction; this drives
        // conversation dispatch (B3 SSOT). An unresolvable surface falls back to
        // `ManualChatGui`, which routes to the "unsupported surface" error path —
        // preserving the prior unknown-surface behavior.
        let invocation_mode =
            maekon_api_contracts::provider_specs::subprocess_invocation_mode(&surface.surface_id)
                .unwrap_or(
                    maekon_api_contracts::provider_specs::SubprocessInvocationMode::ManualChatGui,
                );

        Self {
            session_id: Uuid::new_v4().to_string(),
            surface,
            invocation_mode,
            provider_name,
            model,
            system_prompt: config.system_prompt.clone(),
            default_tools,
            history: Arc::new(RwLock::new(Vec::new())),
            state: Mutex::new(SessionState::Active),
            turn_count: AtomicU32::new(0),
            created_at: Utc::now(),
            last_active: Mutex::new(Utc::now()),
            timeout: Duration::from_secs(session_config.session_timeout_secs),
            max_history_turns: session_config.max_history_turns,
            cancel_requested: Arc::new(AtomicBool::new(false)),
            cancel_notify: Arc::new(Notify::new()),
        }
    }

    /// Catalog invocation mode driving conversation dispatch (B3 SSOT).
    #[cfg(test)]
    pub(crate) fn invocation_mode(
        &self,
    ) -> maekon_api_contracts::provider_specs::SubprocessInvocationMode {
        self.invocation_mode
    }

    async fn invoke_surface(&self, prompt: &str) -> Result<String, CoreError> {
        use maekon_api_contracts::provider_specs::SubprocessInvocationMode;
        match self.invocation_mode {
            SubprocessInvocationMode::CodexExecJson => self.run_codex(prompt).await,
            SubprocessInvocationMode::GeminiCliPrompt => self.run_gemini(prompt).await,
            // Iter-94: reachable only if caller configured a surface whose mode
            // has no conversation-session implementation on this code path
            // (e.g. ClaudePrintJson routes to ClaudeSubprocessSession; app-server
            // mode's runtime lands in #4865). Config mismatch, not internal fault
            // — wire code `config.invalid` lets telemetry/i18n surface "pick a
            // different provider" rather than "something broke inside maekon".
            _ => Err(CoreError::Config {
                code: maekon_core::error_codes::ConfigCode::Invalid,
                message: format!(
                    "subprocess conversation sessions are not implemented for surface '{}' (invocation mode {:?})",
                    self.surface.surface_id, self.invocation_mode
                ),
            }),
        }
    }

    async fn run_codex(&self, prompt: &str) -> Result<String, CoreError> {
        let temp_dir = tempdir().map_err(|err| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("Failed to create Codex session tempdir: {err}"),
        })?;

        let mut child = Command::new(&self.surface.executable_path);
        child
            .arg("exec")
            .arg("-C")
            .arg(temp_dir.path())
            .arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        append_oneshot_flags(&mut child, &self.surface.surface_id);
        append_model_flag(&mut child, &self.surface.surface_id, &self.model);

        let child = child.spawn().map_err(|err| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("Failed to spawn Codex session subprocess: {err}"),
        })?;

        // #6266: bounded stdin write + output collection with concurrent pipe
        // draining (avoids the stdin/stdout deadlock the prior bare write+timeout
        // allowed; same fix as the one-shot subprocess_provider sites).
        let output =
            write_prompt_and_collect_output(child, prompt, "Codex session", self.timeout).await?;

        if !output.status.success() {
            return Err(classify_subprocess_error_with_redactions(
                SubprocessKind::Llm,
                &self.surface.surface_id,
                &String::from_utf8_lossy(&output.stderr),
                &[prompt],
            ));
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    async fn run_gemini(&self, prompt: &str) -> Result<String, CoreError> {
        let temp_dir = tempdir().map_err(|err| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("Failed to create Gemini session tempdir: {err}"),
        })?;

        let mut command = Command::new(&self.surface.executable_path);
        // Pass `-` as the prompt argument so the Gemini CLI reads the prompt
        // from stdin, keeping PII (history + context + attachment previews) out
        // of the process table (ps/Activity Monitor /proc/cmdline). Mirrors the
        // one-shot `run_gemini` stdin contract.
        command
            .arg("-p")
            .arg("-")
            .current_dir(temp_dir.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        append_oneshot_flags(&mut command, &self.surface.surface_id);
        append_model_flag(&mut command, &self.surface.surface_id, &self.model);

        let child = command.spawn().map_err(|err| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("Failed to spawn Gemini session subprocess: {err}"),
        })?;

        // #6266: bounded stdin write + output collection (see run_codex).
        let output =
            write_prompt_and_collect_output(child, prompt, "Gemini session", self.timeout).await?;

        if !output.status.success() {
            return Err(classify_subprocess_error_with_redactions(
                SubprocessKind::Llm,
                &self.surface.surface_id,
                &String::from_utf8_lossy(&output.stderr),
                &[prompt],
            ));
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    async fn send_codex_message(
        &self,
        message: &SessionMessage,
    ) -> Result<ResponseStream, CoreError> {
        let rendered_user_message = render_message_payload(message, self.default_tools.as_deref());

        {
            let mut history = self.history.write().await;
            history.push(ChatMessage {
                role: ChatRole::User,
                content: rendered_user_message,
                content_blocks: None,
            });
        }

        let prompt = {
            let history = self.history.read().await;
            render_conversation_prompt(self.system_prompt.as_deref(), &history)
        };

        self.turn_count.fetch_add(1, Ordering::Relaxed);
        *self.last_active.lock() = Utc::now();

        let temp_dir = tempdir().map_err(|err| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("Failed to create Codex session tempdir: {err}"),
        })?;
        let response_schema = extract_native_response_schema(message.response_format.as_ref());

        let mut child = Command::new(&self.surface.executable_path);
        child
            .arg("exec")
            .arg("--json")
            .arg("-C")
            .arg(temp_dir.path())
            .arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        append_oneshot_flags(&mut child, &self.surface.surface_id);
        append_model_flag(&mut child, &self.surface.surface_id, &self.model);

        if let Some(schema) = response_schema.as_ref() {
            let schema_path = temp_dir.path().join("output-schema.json");
            tokio::fs::write(
                &schema_path,
                serde_json::to_vec_pretty(schema).map_err(|err| CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
                    message: format!("Failed to serialize Codex output schema for session: {err}"),
                })?,
            )
            .await
            .map_err(|err| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!("Failed to write Codex output schema for session: {err}"),
            })?;
            child.arg("--output-schema").arg(schema_path);
        }

        let mut child = child.spawn().map_err(|err| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("Failed to spawn Codex session subprocess: {err}"),
        })?;

        let mut stdin = child.stdin.take().ok_or_else(|| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: "Failed to open stdin for Codex session subprocess".to_string(),
        })?;
        // #6266: write the prompt to stdin CONCURRENTLY with the stdout/stderr
        // draining below. Writing it inline here (before the stream loop starts
        // consuming stdout) could deadlock if the child fills its stdout/stderr
        // pipe before it finishes reading stdin. The writer task is moved into the
        // stream so it lives for the stream's duration and is aborted on drop.
        let prompt_bytes = prompt.as_bytes().to_vec();
        let stdin_writer = AbortOnDropJoin::new(tokio::spawn(async move {
            // Ignore write errors: an early-exiting child closes the pipe
            // (BrokenPipe), surfaced via its exit status/stderr instead.
            let _ = stdin.write_all(&prompt_bytes).await;
            // `stdin` drops here, closing the pipe (EOF) so the child can finish.
        }));

        let stdout = child.stdout.take().ok_or_else(|| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: "Failed to capture Codex session stdout".to_string(),
        })?;
        let mut stderr = child.stderr.take().ok_or_else(|| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: "Failed to capture Codex session stderr".to_string(),
        })?;

        let history = self.history.clone();
        let max_history_turns = self.max_history_turns;
        let timeout = self.timeout;
        let surface_id = self.surface.surface_id.clone();
        let provider_name = self.provider_name.clone();
        let cancel_requested = self.cancel_requested.clone();
        let cancel_notify = self.cancel_notify.clone();
        let prompt_redaction = prompt.clone();

        let stream: ResponseStream = Box::pin(try_stream! {
            let _temp_dir = temp_dir;
            // #6266: keep the concurrent stdin writer alive for the stream's
            // lifetime (aborted on drop) — it drains the prompt into the child
            // while the loop below drains stdout, avoiding the pipe deadlock.
            let _stdin_writer = stdin_writer;
            let mut lines = tokio::io::BufReader::new(stdout).lines();
            let deadline = tokio::time::Instant::now() + timeout;
            let stderr_task = AbortOnDropJoin::new(tokio::spawn(async move {
                let mut stderr_buf = String::new();
                if let Err(e) = stderr.read_to_string(&mut stderr_buf).await {
                    debug!("read_to_string failed: {e}");
                }
                stderr_buf
            }));
            let mut assistant_text = String::new();
            let mut saw_non_empty_event = false;
            let mut emitted_terminal_error = false;
            let mut codex_stream_state =
                CodexStreamState::with_sensitive_values(vec![prompt_redaction]);

            loop {
                if cancel_requested.load(Ordering::Acquire) {
                    yield OutboundMessage::Control {
                        action: ControlAction::Cancel,
                    };
                    emitted_terminal_error = true;
                    if let Err(e) = child.kill().await {
                        debug!("process kill failed: {e}");
                    }
                    break;
                }

                let line_result = tokio::select! {
                    line_result = tokio::time::timeout_at(deadline, lines.next_line()) => line_result,
                    _ = cancel_notify.notified() => {
                        yield OutboundMessage::Control {
                            action: ControlAction::Cancel,
                        };
                        emitted_terminal_error = true;
                        if let Err(e) = child.kill().await {
                            debug!("process kill failed: {e}");
                        }
                        break;
                    }
                };
                match line_result {
                    Ok(Ok(Some(line))) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }

                        if let Some(message) = codex_stream_state.normalize_line(trimmed) {
                            if let OutboundMessage::Text { content, .. } = &message {
                                if !content.is_empty() {
                                    assistant_text.push_str(content);
                                    saw_non_empty_event = true;
                                }
                            } else {
                                saw_non_empty_event = true;
                            }

                            yield message;
                        }
                    }
                    Ok(Ok(None)) => break,
                    Ok(Err(err)) => {
                        yield OutboundMessage::Error {
                            code: "io_error".to_string(),
                            message: err.to_string(),
                            retryable: false,
                        };
                        emitted_terminal_error = true;
                        if let Err(e) = child.kill().await {
                            debug!("process kill failed: {e}");
                        }
                        break;
                    }
                    Err(_) => {
                        yield OutboundMessage::Error {
                            code: "timeout".to_string(),
                            message: format!("Session response timeout ({}s)", timeout.as_secs()),
                            retryable: true,
                        };
                        emitted_terminal_error = true;
                        if let Err(e) = child.kill().await {
                            debug!("process kill failed: {e}");
                        }
                        break;
                    }
                }
            }

            let status = child.wait().await.map_err(CoreError::Io)?;
            let stderr_output = stderr_task.join().await.unwrap_or_default();

            if !status.success() && !emitted_terminal_error {
                let classified =
                    classify_subprocess_error_with_redactions(
                        SubprocessKind::Llm,
                        &surface_id,
                        &stderr_output,
                        &[codex_stream_state.sensitive_value()],
                    );
                yield OutboundMessage::Error {
                    code: "subprocess_error".to_string(),
                    message: classified.to_string(),
                    retryable: false,
                };
                return;
            }

            if assistant_text.is_empty() {
                if !saw_non_empty_event {
                    yield OutboundMessage::Error {
                        code: "empty_response".to_string(),
                        message: format!("{provider_name} CLI returned an empty session response"),
                        retryable: false,
                    };
                }
                return;
            }

            let mut history = history.write().await;
            history.push(ChatMessage {
                role: ChatRole::Assistant,
                content: assistant_text,
                content_blocks: None,
            });
            truncate_history(&mut history, max_history_turns);
        });

        Ok(stream)
    }
}

#[async_trait]
impl ConversationSession for GenericSubprocessSession {
    async fn send_message(&self, message: &SessionMessage) -> Result<ResponseStream, CoreError> {
        // B3 SSOT: route by catalog invocation mode, not surface_id string.
        // Codex exec gets the streaming JSON-event path; other modes fall
        // through to the one-shot `invoke_surface` path.
        if self.invocation_mode
            == maekon_api_contracts::provider_specs::SubprocessInvocationMode::CodexExecJson
        {
            return self.send_codex_message(message).await;
        }

        let rendered_user_message = render_message_payload(message, self.default_tools.as_deref());

        {
            let mut history = self.history.write().await;
            history.push(ChatMessage {
                role: ChatRole::User,
                content: rendered_user_message,
                content_blocks: None,
            });
        }

        let prompt = {
            let history = self.history.read().await;
            render_conversation_prompt(self.system_prompt.as_deref(), &history)
        };

        self.turn_count.fetch_add(1, Ordering::Relaxed);
        *self.last_active.lock() = Utc::now();

        let output = self.invoke_surface(&prompt).await.inspect_err(|_| {
            *self.state.lock() = SessionState::Failed;
        })?;
        let history = self.history.clone();
        let max_history_turns = self.max_history_turns;
        let provider_name = self.provider_name.clone();

        let stream: ResponseStream = Box::pin(try_stream! {
            if output.is_empty() {
                // Iter-106: subprocess CLI returning empty output is a
                // provider (Analysis) failure, consistent with iter-93's
                // subprocess parser fix (parse_interpreted_action_output
                // empty case). Wire code `provider.analysis_failed`.
                Err(CoreError::Analysis { code: maekon_core::error_codes::ProviderCode::AnalysisFailed, message: format!(
                    "{} CLI returned an empty session response",
                    provider_name
                ) })?;
            }

            {
                let mut history = history.write().await;
                history.push(ChatMessage {
                    role: ChatRole::Assistant,
                    content: output.clone(),
                    content_blocks: None,
                });
                truncate_history(&mut history, max_history_turns);
            }

            yield OutboundMessage::Result {
                content: output,
                done: true,
                usage: None,
            };
        });

        Ok(stream)
    }

    fn info(&self) -> ConversationSessionInfo {
        let last_active_utc = *self.last_active.lock();
        ConversationSessionInfo {
            session_id: self.session_id.clone(),
            provider_name: self.provider_name.clone(),
            model: self.model.clone(),
            state: *self.state.lock(),
            transport: SessionTransport::Subprocess,
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
        &self.provider_name
    }

    fn is_external(&self) -> bool {
        // Codex/Gemini CLI subprocess transmits chat content off-device.
        true
    }

    async fn terminate(&self) {
        self.cancel_requested.store(true, Ordering::Release);
        self.cancel_notify.notify_waiters();
        *self.state.lock() = SessionState::Terminated;
    }
}

#[derive(Default)]
struct CodexStreamState {
    saw_terminal_result: bool,
    sensitive_values: Vec<String>,
}

impl CodexStreamState {
    fn with_sensitive_values(sensitive_values: Vec<String>) -> Self {
        Self {
            saw_terminal_result: false,
            sensitive_values,
        }
    }

    fn sensitive_value(&self) -> &str {
        self.sensitive_values
            .first()
            .map(String::as_str)
            .unwrap_or("")
    }

    fn normalize_line(&mut self, line: &str) -> Option<OutboundMessage> {
        let message = parse_codex_json_event_with_redactions(line, &self.sensitive_values)?;
        match &message {
            OutboundMessage::Error { message, .. } if message.starts_with("Reconnecting...") => {
                None
            }
            OutboundMessage::Result { done: true, .. } if self.saw_terminal_result => None,
            OutboundMessage::Result { done: true, .. } => {
                self.saw_terminal_result = true;
                Some(message)
            }
            _ => Some(message),
        }
    }
}

#[cfg(test)]
fn parse_codex_json_event(line: &str) -> Option<OutboundMessage> {
    parse_codex_json_event_with_redactions(line, &[])
}

fn parse_codex_json_event_with_redactions(
    line: &str,
    sensitive_values: &[String],
) -> Option<OutboundMessage> {
    let value = match serde_json::from_str::<serde_json::Value>(line) {
        Ok(value) => value,
        Err(_) => {
            return Some(OutboundMessage::Error {
                code: "malformed_event".to_string(),
                message: format!(
                    "Codex CLI emitted malformed JSON event: {}",
                    sanitize_session_event_text(line, sensitive_values)
                ),
                retryable: false,
            });
        }
    };
    let event_type = value.get("type")?.as_str()?;

    match event_type {
        "item.started" | "item.completed" => parse_codex_item_event(event_type, value.get("item")?),
        "turn.completed" => Some(OutboundMessage::Result {
            content: String::new(),
            done: true,
            usage: parse_codex_usage(value.get("usage")),
        }),
        "error" => Some(OutboundMessage::Error {
            code: "subprocess_error".to_string(),
            message: sanitize_session_event_text(
                value
                    .get("message")
                    .and_then(|message| message.as_str())
                    .unwrap_or("Codex CLI error"),
                sensitive_values,
            ),
            retryable: true,
        }),
        _ => None,
    }
}

fn sanitize_session_event_text(raw: &str, sensitive_values: &[String]) -> String {
    let exact_redacted =
        sensitive_values
            .iter()
            .map(String::as_str)
            .fold(raw.to_string(), |output, value| {
                if value.trim().is_empty() {
                    output
                } else {
                    output.replace(value, "<redacted-payload>")
                }
            });

    sanitize_subprocess_error_output(&exact_redacted)
}

fn parse_codex_item_event(event_type: &str, item: &serde_json::Value) -> Option<OutboundMessage> {
    let item_type = item.get("type")?.as_str()?;

    match item_type {
        "agent_message" => extract_stringish(item, &["text", "message", "content"]).map(|text| {
            OutboundMessage::Text {
                content: text,
                done: false,
            }
        }),
        "reasoning" => extract_stringish(item, &["summary", "text", "content"]).map(|content| {
            OutboundMessage::Thinking {
                content,
                done: event_type == "item.completed",
            }
        }),
        "command_execution" | "mcp_tool_call" | "web_search" => Some(OutboundMessage::ToolUse {
            tool: codex_tool_name(item_type, item),
            input: codex_tool_input(item_type, item),
            status: codex_tool_status(event_type, item),
            result: if event_type == "item.completed" {
                extract_stringish(
                    item,
                    &[
                        "aggregated_output",
                        "result",
                        "output",
                        "message",
                        "content",
                        "text",
                    ],
                )
            } else {
                None
            },
        }),
        _ => None,
    }
}

fn parse_codex_usage(value: Option<&serde_json::Value>) -> Option<TokenUsage> {
    let usage = value?;
    Some(TokenUsage {
        input_tokens: usage.get("input_tokens")?.as_u64()?,
        output_tokens: usage.get("output_tokens")?.as_u64()?,
    })
}

fn extract_stringish(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    match value {
        serde_json::Value::String(text) if !text.is_empty() => Some(text.clone()),
        serde_json::Value::Array(items) => {
            let parts: Vec<String> = items
                .iter()
                .filter_map(|item| extract_stringish(item, keys))
                .collect();
            if parts.is_empty() {
                None
            } else {
                Some(parts.join("\n"))
            }
        }
        serde_json::Value::Object(map) => {
            for key in keys {
                if let Some(text) = map
                    .get(*key)
                    .and_then(|nested| extract_stringish(nested, keys))
                {
                    return Some(text);
                }
            }
            None
        }
        _ => None,
    }
}

fn codex_tool_status(event_type: &str, item: &serde_json::Value) -> ToolUseStatus {
    match item.get("status").and_then(|status| status.as_str()) {
        Some("failed") => ToolUseStatus::Failed,
        Some("completed") | Some("success") => ToolUseStatus::Completed,
        Some("in_progress") | Some("running") => ToolUseStatus::Started,
        _ if event_type == "item.started" => ToolUseStatus::Started,
        _ => ToolUseStatus::Completed,
    }
}

fn codex_tool_name(item_type: &str, item: &serde_json::Value) -> String {
    match item_type {
        "command_execution" => "command_execution".to_string(),
        "mcp_tool_call" => {
            let server = item
                .get("server")
                .and_then(|value| value.as_str())
                .unwrap_or("mcp");
            let tool = item
                .get("tool")
                .or_else(|| item.get("name"))
                .and_then(|value| value.as_str())
                .unwrap_or("tool");
            format!("{server}:{tool}")
        }
        "web_search" => "web_search".to_string(),
        _ => item_type.to_string(),
    }
}

fn codex_tool_input(item_type: &str, item: &serde_json::Value) -> Option<serde_json::Value> {
    let mut payload = serde_json::Map::new();
    match item_type {
        "command_execution" => {
            for key in ["command", "cwd"] {
                if let Some(value) = item.get(key) {
                    payload.insert(key.to_string(), value.clone());
                }
            }
        }
        "mcp_tool_call" => {
            for key in ["server", "tool", "name", "arguments"] {
                if let Some(value) = item.get(key) {
                    payload.insert(key.to_string(), value.clone());
                }
            }
        }
        "web_search" => {
            for key in ["query", "url"] {
                if let Some(value) = item.get(key) {
                    payload.insert(key.to_string(), value.clone());
                }
            }
        }
        _ => {}
    }

    if payload.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(payload))
    }
}

fn truncate_history(history: &mut Vec<ChatMessage>, max_turns: u32) {
    let max = max_turns as usize;
    if max == 0 || history.len() <= max {
        return;
    }

    let drain_count = history.len() - max;
    history.drain(0..drain_count);
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::models::ai_session::{MessageContext, MessageRole};
    use std::path::{Path, PathBuf};

    #[test]
    fn truncate_history_keeps_latest_turns_without_system_header() {
        let mut history = vec![
            ChatMessage {
                role: ChatRole::User,
                content: "one".to_string(),
                content_blocks: None,
            },
            ChatMessage {
                role: ChatRole::Assistant,
                content: "two".to_string(),
                content_blocks: None,
            },
            ChatMessage {
                role: ChatRole::User,
                content: "three".to_string(),
                content_blocks: None,
            },
        ];

        truncate_history(&mut history, 2);
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].content, "two");
        assert_eq!(history[1].content, "three");
    }

    #[test]
    fn parses_codex_agent_message_event() {
        let event = parse_codex_json_event(
            r#"{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"OK"}}"#,
        )
        .expect("codex agent message should parse");

        match event {
            OutboundMessage::Text { content, done } => {
                assert_eq!(content, "OK");
                assert!(!done);
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn parses_codex_turn_completed_usage() {
        let event = parse_codex_json_event(
            r#"{"type":"turn.completed","usage":{"input_tokens":12,"cached_input_tokens":3,"output_tokens":7}}"#,
        )
        .expect("codex usage event should parse");

        match event {
            OutboundMessage::Result {
                content,
                done,
                usage,
            } => {
                assert!(content.is_empty());
                assert!(done);
                let usage = usage.expect("usage should be present");
                assert_eq!(usage.input_tokens, 12);
                assert_eq!(usage.output_tokens, 7);
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn parses_codex_command_execution_item() {
        let event = parse_codex_json_event(
            r#"{"type":"item.completed","item":{"type":"command_execution","status":"completed","command":"pwd","aggregated_output":"/tmp"}}"#,
        )
        .expect("codex command execution should parse");

        match event {
            OutboundMessage::ToolUse {
                tool,
                status,
                input,
                result,
            } => {
                assert_eq!(tool, "command_execution");
                assert_eq!(status, ToolUseStatus::Completed);
                assert_eq!(
                    input.expect("tool input should exist")["command"],
                    serde_json::json!("pwd")
                );
                assert_eq!(result.as_deref(), Some("/tmp"));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn parses_codex_malformed_json_as_sanitized_error() {
        let event = parse_codex_json_event("not json alice@example.com sk-secret")
            .expect("malformed Codex JSON should surface a safe error");

        match event {
            OutboundMessage::Error {
                code,
                message,
                retryable,
            } => {
                assert_eq!(code, "malformed_event");
                assert!(!retryable);
                assert!(!message.contains("alice@example.com"));
                assert!(!message.contains("sk-secret"));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn parses_codex_error_event_with_redactions() {
        let event = parse_codex_json_event(
            r#"{"type":"error","message":"failed for alice@example.com token sk-secret"}"#,
        )
        .expect("Codex error event should parse");

        match event {
            OutboundMessage::Error { message, .. } => {
                assert!(!message.contains("alice@example.com"));
                assert!(!message.contains("sk-secret"));
                assert!(message.contains("<redacted-email>"));
                assert!(message.contains("<redacted-token>"));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn codex_stream_state_skips_unknown_and_duplicate_result_events() {
        let mut state = CodexStreamState::default();

        assert!(state
            .normalize_line(r#"{"type":"session.created","provider_debug":"ignore me"}"#)
            .is_none());

        let first_result = state
            .normalize_line(r#"{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":2},"debug":{"prompt":"secret"}}"#)
            .expect("first result should be emitted");
        assert!(matches!(
            first_result,
            OutboundMessage::Result { done: true, .. }
        ));

        assert!(
            state
                .normalize_line(
                    r#"{"type":"turn.completed","usage":{"input_tokens":3,"output_tokens":4}}"#
                )
                .is_none(),
            "duplicate terminal results should not reach the UI"
        );
    }

    #[tokio::test]
    async fn abort_on_drop_join_aborts_drain_task() {
        use std::future::pending;
        use tokio::sync::oneshot;

        struct DropProbe(Option<oneshot::Sender<()>>);

        impl Drop for DropProbe {
            fn drop(&mut self) {
                if let Some(sender) = self.0.take() {
                    let _ = sender.send(());
                }
            }
        }

        let (started_tx, started_rx) = oneshot::channel();
        let (dropped_tx, dropped_rx) = oneshot::channel();
        let guard = AbortOnDropJoin::new(tokio::spawn(async move {
            let _probe = DropProbe(Some(dropped_tx));
            let _ = started_tx.send(());
            pending::<String>().await
        }));

        started_rx
            .await
            .expect("drain task should start before guard drop");
        drop(guard);

        tokio::time::timeout(Duration::from_secs(1), dropped_rx)
            .await
            .expect("drain task should be aborted on guard drop")
            .expect("drop probe should report task cancellation");
    }

    #[tokio::test]
    async fn generic_subprocess_terminate_requests_stream_cancel() {
        let config = SessionConfig {
            transport: SessionTransport::Subprocess,
            surface_id: Some("provider_surface.openai.subprocess_cli".to_string()),
            model: Some("gpt-5.4".to_string()),
            system_prompt: None,
            tools_enabled: false,
            cwd: None,
            sandbox_policy: None,
            approval_policy: None,
        };
        let session = GenericSubprocessSession::new(
            DetectedSubprocessCli {
                surface_id: "provider_surface.openai.subprocess_cli".to_string(),
                executable_path: PathBuf::from("/usr/bin/false"),
            },
            &config,
            Arc::new(AiSessionConfig::default()),
            None,
        );

        session.terminate().await;

        assert!(session.cancel_requested.load(Ordering::Acquire));
        assert_eq!(*session.state.lock(), SessionState::Terminated);
    }

    #[tokio::test]
    async fn session_prompt_includes_message_metadata() {
        let session = GenericSubprocessSession {
            session_id: "test".to_string(),
            surface: DetectedSubprocessCli {
                surface_id: "provider_surface.google.subprocess_cli".to_string(),
                executable_path: PathBuf::from("/usr/bin/false"),
            },
            invocation_mode:
                maekon_api_contracts::provider_specs::SubprocessInvocationMode::GeminiCliPrompt,
            provider_name: "google".to_string(),
            model: "gemini-2.5-pro".to_string(),
            system_prompt: Some("Be concise.".to_string()),
            default_tools: Some(vec![ToolDefinition {
                name: "search".to_string(),
                description: "Search".to_string(),
                endpoint: "http://localhost/api/search".to_string(),
                method: "GET".to_string(),
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": { "q": { "type": "string" } },
                    "required": ["q"],
                    "additionalProperties": false
                })),
            }]),
            history: Arc::new(RwLock::new(Vec::new())),
            state: Mutex::new(SessionState::Active),
            turn_count: AtomicU32::new(0),
            created_at: Utc::now(),
            last_active: Mutex::new(Utc::now()),
            timeout: Duration::from_secs(30),
            max_history_turns: 8,
            cancel_requested: Arc::new(AtomicBool::new(false)),
            cancel_notify: Arc::new(Notify::new()),
        };

        let message = SessionMessage {
            role: MessageRole::User,
            content: "Summarize this".to_string(),
            attachments: vec![],
            tools: None,
            context: Some(MessageContext {
                regime: Some("focus".to_string()),
                active_app: Some("Maekon".to_string()),
            }),
            response_format: Some(serde_json::json!({
                "type": "json_schema",
                "json_schema": {
                    "name": "summary",
                    "schema": { "type": "object" }
                }
            })),
        };

        {
            let mut history = session.history.write().await;
            history.push(ChatMessage {
                role: ChatRole::User,
                content: render_message_payload(&message, session.default_tools.as_deref()),
                content_blocks: None,
            });
        }

        let prompt = {
            let history = session.history.read().await;
            render_conversation_prompt(session.system_prompt.as_deref(), &history)
        };

        assert!(prompt.contains("Available tools JSON"));
        assert!(prompt.contains("Required response format JSON"));
        assert!(prompt.contains("Additional context JSON"));
    }

    #[test]
    fn generic_session_routes_by_invocation_mode_not_surface_id() {
        use maekon_api_contracts::provider_specs::SubprocessInvocationMode;
        // E21 #4864 (B3 SSOT): the conversation dispatch must key off the
        // catalog invocation_mode, not a hard-coded surface_id string.
        let codex = GenericSubprocessSession::new(
            DetectedSubprocessCli {
                surface_id: "provider_surface.openai.subprocess_cli".to_string(),
                executable_path: std::path::PathBuf::from("/usr/bin/false"),
            },
            &SessionConfig {
                transport: SessionTransport::Subprocess,
                surface_id: Some("provider_surface.openai.subprocess_cli".to_string()),
                model: None,
                system_prompt: None,
                tools_enabled: false,
                cwd: None,
                sandbox_policy: None,
                approval_policy: None,
            },
            Arc::new(AiSessionConfig::default()),
            None,
        );
        assert_eq!(
            codex.invocation_mode(),
            SubprocessInvocationMode::CodexExecJson
        );

        let gemini = GenericSubprocessSession::new(
            DetectedSubprocessCli {
                surface_id: "provider_surface.google.subprocess_cli".to_string(),
                executable_path: std::path::PathBuf::from("/usr/bin/false"),
            },
            &SessionConfig {
                transport: SessionTransport::Subprocess,
                surface_id: Some("provider_surface.google.subprocess_cli".to_string()),
                model: None,
                system_prompt: None,
                tools_enabled: false,
                cwd: None,
                sandbox_policy: None,
                approval_policy: None,
            },
            Arc::new(AiSessionConfig::default()),
            None,
        );
        assert_eq!(
            gemini.invocation_mode(),
            SubprocessInvocationMode::GeminiCliPrompt
        );
    }

    /// Security regression (#6084): the Gemini conversation session must
    /// deliver the rendered prompt over stdin (`-p -`), never as a bare argv
    /// argument visible in the process table. The fake CLI exits non-zero if
    /// the prompt slot after `-p` is anything other than the `-` stdin
    /// sentinel, and echoes the stdin-delivered prompt back so the test can
    /// confirm it actually arrived there.
    #[tokio::test]
    async fn gemini_session_delivers_prompt_via_stdin_not_argv() {
        let temp_dir = tempdir().expect("tempdir");
        let executable_path = write_fake_gemini_session_cli(temp_dir.path());
        let session = GenericSubprocessSession {
            session_id: "test".to_string(),
            surface: DetectedSubprocessCli {
                surface_id: "provider_surface.google.subprocess_cli".to_string(),
                executable_path,
            },
            invocation_mode:
                maekon_api_contracts::provider_specs::SubprocessInvocationMode::GeminiCliPrompt,
            provider_name: "google".to_string(),
            model: "gemini-2.5-pro".to_string(),
            system_prompt: None,
            default_tools: None,
            history: Arc::new(RwLock::new(Vec::new())),
            state: Mutex::new(SessionState::Active),
            turn_count: AtomicU32::new(0),
            created_at: Utc::now(),
            last_active: Mutex::new(Utc::now()),
            timeout: Duration::from_secs(30),
            max_history_turns: 8,
            cancel_requested: Arc::new(AtomicBool::new(false)),
            cancel_notify: Arc::new(Notify::new()),
        };

        let prompt = "Gemini session prompt with spaces && | metacharacters via stdin";
        let output = session
            .run_gemini(prompt)
            .await
            .expect("fake Gemini CLI should receive prompt via stdin");
        assert_eq!(output, format!("STDIN_OK:{prompt}"));
    }

    fn write_fake_gemini_session_cli(base_dir: &Path) -> PathBuf {
        let bin_dir = base_dir.join("bin");
        std::fs::create_dir_all(&bin_dir).expect("fake cli dir");
        let source_path = bin_dir.join("fake_gemini_session.rs");
        let executable_path = bin_dir.join(if cfg!(windows) {
            "gemini.exe"
        } else {
            "gemini"
        });
        std::fs::write(
            &source_path,
            r##"
use std::io::Read;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // The prompt slot after -p must be the "-" stdin sentinel, never the raw
    // prompt (which would leak into the process table).
    let prompt_slot_index = args
        .iter()
        .position(|arg| arg == "-p")
        .and_then(|index| index.checked_add(1))
        .expect("prompt index");
    let prompt_slot = args.get(prompt_slot_index).expect("prompt slot");
    if prompt_slot != "-" {
        eprintln!("expected stdin sentinel '-' after -p, got: {prompt_slot}");
        std::process::exit(10);
    }
    let mut prompt = String::new();
    std::io::stdin().read_to_string(&mut prompt).expect("stdin");
    print!("STDIN_OK:{prompt}");
}
"##,
        )
        .expect("fake cli source");

        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let status = std::process::Command::new(rustc)
            .arg(&source_path)
            .arg("-o")
            .arg(&executable_path)
            .status()
            .expect("compile fake cli");
        assert!(status.success(), "fake cli should compile");
        executable_path
    }

    #[test]
    fn generic_subprocess_session_is_external() {
        // Codex/Gemini CLI transmit chat content off-device → must be guarded.
        let config = SessionConfig {
            transport: SessionTransport::Subprocess,
            surface_id: Some("provider_surface.openai.subprocess_cli".to_string()),
            model: Some("gpt-5.4".to_string()),
            system_prompt: None,
            tools_enabled: false,
            cwd: None,
            sandbox_policy: None,
            approval_policy: None,
        };
        let session = GenericSubprocessSession::new(
            DetectedSubprocessCli {
                surface_id: "provider_surface.openai.subprocess_cli".to_string(),
                executable_path: std::path::PathBuf::from("/usr/bin/false"),
            },
            &config,
            Arc::new(AiSessionConfig::default()),
            None,
        );
        assert!(session.is_external());
    }
}
