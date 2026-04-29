//! Claude subprocess session -- serial `-p` calls with `--session-id`/`--continue`.

use std::process::Stdio;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use chrono::Utc;
use parking_lot::Mutex;
use tokio::io::{AsyncBufReadExt, AsyncReadExt};
use tokio::process::Command;
use uuid::Uuid;

use oneshim_core::config::AiSessionConfig;
use oneshim_core::error::CoreError;
use oneshim_core::models::ai_session::{
    ConversationSessionInfo, OutboundMessage, SessionConfig, SessionMessage, SessionState,
    SessionTransport, ToolDefinition,
};
use oneshim_core::ports::conversation_session::{ConversationSession, ResponseStream};

use crate::session_adapters::claude_normalizer::normalize_claude_stream_event;
use crate::session_adapters::prompt_payload::{
    extract_native_response_schema, render_message_payload,
};
use crate::subprocess_provider::{
    classify_subprocess_error, DetectedSubprocessCli, SubprocessKind,
};
use tracing::debug;

pub struct ClaudeSubprocessSession {
    session_id: String,
    cli_session_id: String,
    surface: DetectedSubprocessCli,
    model: String,
    system_prompt: Option<String>,
    default_tools: Option<Vec<ToolDefinition>>,
    state: Mutex<SessionState>,
    turn_count: AtomicU32,
    created_at: chrono::DateTime<chrono::Utc>,
    last_active: Mutex<Instant>,
    config: Arc<AiSessionConfig>,
}

impl ClaudeSubprocessSession {
    pub fn new(
        surface: DetectedSubprocessCli,
        config: &SessionConfig,
        session_config: Arc<AiSessionConfig>,
        default_tools: Option<Vec<ToolDefinition>>,
    ) -> Self {
        Self {
            session_id: Uuid::new_v4().to_string(),
            cli_session_id: Uuid::new_v4().to_string(),
            created_at: Utc::now(),
            surface,
            model: config.model.clone().unwrap_or_else(|| "sonnet".to_string()),
            system_prompt: config.system_prompt.clone(),
            default_tools,
            state: Mutex::new(SessionState::Active),
            turn_count: AtomicU32::new(0),
            last_active: Mutex::new(Instant::now()),
            config: session_config,
        }
    }

    fn build_command(&self, prompt: &str, response_schema: Option<&serde_json::Value>) -> Command {
        let mut cmd = Command::new(&self.surface.executable_path);
        cmd.arg("-p");
        cmd.arg("--output-format").arg("stream-json");
        cmd.arg("--verbose");
        cmd.arg("--include-partial-messages");
        cmd.arg("--permission-mode")
            .arg(&self.config.permission_mode);
        cmd.arg("--model").arg(&self.model);
        cmd.arg("--session-id").arg(&self.cli_session_id);

        let turn = self.turn_count.load(Ordering::Relaxed);
        if turn > 0 {
            cmd.arg("--continue");
        }

        if turn == 0 {
            if let Some(ref sp) = self.system_prompt {
                cmd.arg("--system-prompt").arg(sp);
            }
        }

        if let Some(schema) = response_schema {
            cmd.arg("--json-schema").arg(schema.to_string());
        }

        cmd.arg(prompt);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.kill_on_drop(true);
        cmd
    }
}

#[async_trait]
impl ConversationSession for ClaudeSubprocessSession {
    async fn send_message(&self, message: &SessionMessage) -> Result<ResponseStream, CoreError> {
        let prompt = render_message_payload(message, self.default_tools.as_deref());
        let response_schema = extract_native_response_schema(message.response_format.as_ref());
        let mut cmd = self.build_command(&prompt, response_schema.as_ref());

        let mut child = cmd.spawn().map_err(|err| {
            *self.state.lock() = SessionState::Failed;
            CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("Failed to spawn Claude session subprocess: {err}"),
            }
        })?;

        let stdout = child.stdout.take().ok_or_else(|| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: "Failed to capture Claude session stdout".to_string(),
        })?;
        let mut stderr = child.stderr.take().ok_or_else(|| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: "Failed to capture Claude session stderr".to_string(),
        })?;

        self.turn_count.fetch_add(1, Ordering::Relaxed);
        *self.last_active.lock() = Instant::now();

        let timeout_secs = self.config.session_timeout_secs;
        let surface_id = self.surface.surface_id.clone();
        let reader = tokio::io::BufReader::new(stdout);

        let stream = async_stream::try_stream! {
            let mut lines = reader.lines();
            let deadline = tokio::time::Instant::now()
                + tokio::time::Duration::from_secs(timeout_secs);
            let mut saw_text_chunk = false;
            let mut force_kill = false;
            let mut emitted_terminal_error = false;
            let stderr_task = tokio::spawn(async move {
                let mut stderr_buf = String::new();
                if let Err(e) = stderr.read_to_string(&mut stderr_buf).await {
                    debug!("read_to_string failed: {e}");
                }
                stderr_buf
            });

            loop {
                let line_result = tokio::time::timeout_at(deadline, lines.next_line()).await;
                match line_result {
                    Ok(Ok(Some(line))) => {
                        if let Some(mut normalized) = normalize_claude_stream_event(&line) {
                            if matches!(normalized.kind, crate::session_adapters::claude_normalizer::ClaudeEventKind::AssistantSummary) && saw_text_chunk {
                                continue;
                            }
                            if matches!(normalized.kind, crate::session_adapters::claude_normalizer::ClaudeEventKind::Result)
                                && saw_text_chunk
                            {
                                if let OutboundMessage::Result { content, .. } = &mut normalized.message {
                                    content.clear();
                                }
                            }
                            if matches!(&normalized.message, OutboundMessage::Text { content, .. } if !content.is_empty()) {
                                saw_text_chunk = true;
                            }
                            yield normalized.message;
                        }
                    }
                    Ok(Ok(None)) => break, // EOF
                    Ok(Err(err)) => {
                        yield OutboundMessage::Error {
                            code: "io_error".to_string(),
                            message: err.to_string(),
                            retryable: false,
                        };
                        force_kill = true;
                        emitted_terminal_error = true;
                        break;
                    }
                    Err(_) => {
                        yield OutboundMessage::Error {
                            code: "timeout".to_string(),
                            message: format!("Session response timeout ({timeout_secs}s)"),
                            retryable: true,
                        };
                        force_kill = true;
                        emitted_terminal_error = true;
                        break;
                    }
                }
            }

            if force_kill {
                if let Err(e) = child.kill().await {
                    debug!("process kill failed: {e}");
                }
            }

            let status = child.wait().await.map_err(CoreError::Io)?;
            let stderr_output = stderr_task.await.unwrap_or_default();

            if !status.success() && !emitted_terminal_error {
                let classified =
                    classify_subprocess_error(SubprocessKind::Llm, &surface_id, &stderr_output);
                yield OutboundMessage::Error {
                    code: "subprocess_error".to_string(),
                    message: classified.to_string(),
                    retryable: false,
                };
            }
        };

        Ok(Box::pin(stream))
    }

    fn info(&self) -> ConversationSessionInfo {
        let elapsed = self.last_active.lock().elapsed();
        let last_active_utc = Utc::now() - chrono::Duration::from_std(elapsed).unwrap_or_default();
        ConversationSessionInfo {
            session_id: self.session_id.clone(),
            provider_name: "claude".to_string(),
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
        "claude"
    }
}
