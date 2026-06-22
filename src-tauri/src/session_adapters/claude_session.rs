//! Claude subprocess session -- serial `-p` calls with `--session-id`/`--continue`.

use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

/// Maximum byte length accepted for the combined bare-argv arguments passed to
/// the Claude CLI.  Windows `CreateProcess` imposes a ~32 KiB command-line
/// limit; exceeding it causes silent truncation or `ERROR_FILENAME_EXCD_RANGE`.
/// The threshold is conservative (32 KiB − 1 B) and is intentionally
/// platform-agnostic so the same guard applies everywhere.
///
/// The rendered prompt itself is delivered via stdin (`-p -`) so it never
/// reaches the process table; this guard therefore only bounds the remaining
/// bare-argv values that are still sized by caller input (`--system-prompt`
/// and `--json-schema`).
const MAX_ARGV_PROMPT_BYTES: usize = 32_767;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::{Mutex as AsyncMutex, Notify};
use uuid::Uuid;

use maekon_core::config::AiSessionConfig;
use maekon_core::error::CoreError;
use maekon_core::models::ai_session::{
    ControlAction, ConversationSessionInfo, OutboundMessage, SessionConfig, SessionMessage,
    SessionState, SessionTransport, ToolDefinition,
};
use maekon_core::ports::conversation_session::{ConversationSession, ResponseStream};

use crate::session_adapters::claude_normalizer::ClaudeStreamState;
use crate::session_adapters::prompt_payload::{
    extract_native_response_schema, render_message_payload,
};
use crate::session_adapters::task_guard::AbortOnDropJoin;
use crate::subprocess_provider::{
    append_session_tool_restriction_flags, classify_subprocess_error_with_redactions,
    DetectedSubprocessCli, SubprocessKind,
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
    // #6518 parity: store wall-clock directly (was Mutex<Instant> + skew-prone
    // `Utc::now() - elapsed()` reconstruction).
    last_active: Mutex<DateTime<Utc>>,
    config: Arc<AiSessionConfig>,
    cancel_requested: Arc<AtomicBool>,
    cancel_notify: Arc<Notify>,
    turn_lock: Arc<AsyncMutex<()>>,
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
            last_active: Mutex::new(Utc::now()),
            config: session_config,
            cancel_requested: Arc::new(AtomicBool::new(false)),
            cancel_notify: Arc::new(Notify::new()),
            turn_lock: Arc::new(AsyncMutex::new(())),
        }
    }

    fn build_command(
        &self,
        response_schema: Option<&serde_json::Value>,
    ) -> Result<Command, CoreError> {
        let turn = self.turn_count.load(Ordering::Relaxed);
        let system_prompt = (turn == 0).then_some(self.system_prompt.as_ref()).flatten();
        let schema_json = response_schema.map(serde_json::Value::to_string);

        // Guard against the Windows CreateProcess command-line length limit.
        // The rendered prompt is delivered via stdin (`-p -`), so the only
        // remaining caller-sized bare-argv values are `--system-prompt` and
        // `--json-schema`. Their combined length must stay under the limit or
        // CreateProcess can silently truncate or fail to spawn on Windows.
        let argv_bytes =
            system_prompt.map_or(0, String::len) + schema_json.as_deref().map_or(0, str::len);
        if argv_bytes > MAX_ARGV_PROMPT_BYTES {
            return Err(CoreError::InvalidArguments {
                code: maekon_core::error_codes::ValidationCode::InvalidArguments,
                message: format!(
                    "Claude session argv arguments exceed maximum length \
                     ({argv_bytes} bytes > {MAX_ARGV_PROMPT_BYTES} byte limit); \
                     shorten the system prompt or response schema before sending."
                ),
            });
        }

        let mut cmd = Command::new(&self.surface.executable_path);
        // Pass `-` as the prompt argument so the Claude CLI reads the prompt
        // from stdin, keeping PII (history + context + attachment previews) out
        // of the process table (ps/Activity Monitor /proc/cmdline).
        cmd.arg("-p").arg("-");
        cmd.arg("--output-format").arg("stream-json");
        cmd.arg("--verbose");
        cmd.arg("--include-partial-messages");
        cmd.arg("--permission-mode")
            .arg(&self.config.permission_mode);
        append_session_tool_restriction_flags(&mut cmd, &self.surface.surface_id);
        cmd.arg("--model").arg(&self.model);
        cmd.arg("--session-id").arg(&self.cli_session_id);

        if turn > 0 {
            cmd.arg("--continue");
        }

        if let Some(sp) = system_prompt {
            cmd.arg("--system-prompt").arg(sp);
        }

        if let Some(schema) = schema_json {
            cmd.arg("--json-schema").arg(schema);
        }

        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.kill_on_drop(true);
        Ok(cmd)
    }
}

#[async_trait]
impl ConversationSession for ClaudeSubprocessSession {
    async fn send_message(&self, message: &SessionMessage) -> Result<ResponseStream, CoreError> {
        let turn_guard = self.turn_lock.clone().lock_owned().await;
        let prompt = render_message_payload(message, self.default_tools.as_deref());
        let response_schema = extract_native_response_schema(message.response_format.as_ref());
        let mut cmd = self
            .build_command(response_schema.as_ref())
            .inspect_err(|_err| {
                *self.state.lock() = SessionState::Failed;
            })?;

        let mut child = cmd.spawn().map_err(|err| {
            *self.state.lock() = SessionState::Failed;
            CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!("Failed to spawn Claude session subprocess: {err}"),
            }
        })?;

        // Deliver the rendered prompt over stdin so it never appears in the
        // process table. Mirrors the one-shot `run_claude` stdin contract.
        let mut stdin = child.stdin.take().ok_or_else(|| {
            *self.state.lock() = SessionState::Failed;
            CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: "Failed to open stdin for Claude session subprocess".to_string(),
            }
        })?;
        // #6266 (P-3 sibling): write the prompt to stdin CONCURRENTLY with the
        // stdout/stderr draining below, not inline before the stream loop — an
        // inline blocking write could deadlock if the child fills its stdout/stderr
        // pipe before it finishes reading stdin. The writer task is moved into the
        // stream so it lives for the stream's duration and is aborted on drop; a
        // write error (e.g. BrokenPipe from an early-exiting child) surfaces via
        // the stream's exit/stderr handling rather than a pre-stream failure.
        let prompt_bytes = prompt.as_bytes().to_vec();
        let stdin_writer = AbortOnDropJoin::new(tokio::spawn(async move {
            let _ = stdin.write_all(&prompt_bytes).await;
            // `stdin` drops here, closing the pipe (EOF) so the child can finish.
        }));

        let stdout = child.stdout.take().ok_or_else(|| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: "Failed to capture Claude session stdout".to_string(),
        })?;
        let mut stderr = child.stderr.take().ok_or_else(|| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: "Failed to capture Claude session stderr".to_string(),
        })?;

        self.turn_count.fetch_add(1, Ordering::Relaxed);
        *self.last_active.lock() = Utc::now();

        let timeout_secs = self.config.session_timeout_secs;
        let surface_id = self.surface.surface_id.clone();
        let reader = tokio::io::BufReader::new(stdout);
        let cancel_requested = self.cancel_requested.clone();
        let cancel_notify = self.cancel_notify.clone();
        let prompt_redaction = prompt.clone();

        let stream = async_stream::try_stream! {
            let _turn_guard = turn_guard;
            // #6266 (P-3 sibling): keep the concurrent stdin writer alive for the
            // stream's lifetime (aborted on drop) — it drains the prompt into the
            // child while the loop below drains stdout, avoiding the pipe deadlock.
            let _stdin_writer = stdin_writer;
            let mut lines = reader.lines();
            let deadline = tokio::time::Instant::now()
                + tokio::time::Duration::from_secs(timeout_secs);
            let mut force_kill = false;
            let mut emitted_terminal_error = false;
            let stderr_task = AbortOnDropJoin::new(tokio::spawn(async move {
                let mut stderr_buf = String::new();
                if let Err(e) = stderr.read_to_string(&mut stderr_buf).await {
                    debug!("read_to_string failed: {e}");
                }
                stderr_buf
            }));
            let mut stream_state = ClaudeStreamState::default();

            loop {
                if cancel_requested.load(Ordering::Acquire) {
                    yield OutboundMessage::Control {
                        action: ControlAction::Cancel,
                    };
                    force_kill = true;
                    emitted_terminal_error = true;
                    break;
                }

                let line_result = tokio::select! {
                    line_result = tokio::time::timeout_at(deadline, lines.next_line()) => line_result,
                    _ = cancel_notify.notified() => {
                        yield OutboundMessage::Control {
                            action: ControlAction::Cancel,
                        };
                        force_kill = true;
                        emitted_terminal_error = true;
                        break;
                    }
                };
                match line_result {
                    Ok(Ok(Some(line))) => {
                        if let Some(message) = stream_state.normalize_line(&line) {
                            yield message;
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
            let stderr_output = stderr_task.join().await.unwrap_or_default();

            if !status.success() && !emitted_terminal_error {
                let classified =
                    classify_subprocess_error_with_redactions(
                        SubprocessKind::Llm,
                        &surface_id,
                        &stderr_output,
                        &[prompt_redaction.as_str()],
                    );
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
        let last_active_utc = *self.last_active.lock();
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

    fn is_external(&self) -> bool {
        // Claude Code CLI subprocess transmits chat content off-device.
        true
    }

    async fn terminate(&self) {
        self.cancel_requested.store(true, Ordering::Release);
        self.cancel_notify.notify_waiters();
        *self.state.lock() = SessionState::Terminated;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::models::ai_session::MessageRole;
    use std::path::PathBuf;

    fn false_executable_path() -> PathBuf {
        #[cfg(windows)]
        {
            std::env::var_os("COMSPEC")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("cmd.exe"))
        }
        #[cfg(not(windows))]
        {
            PathBuf::from("/usr/bin/false")
        }
    }

    #[tokio::test]
    async fn claude_terminate_requests_stream_cancel() {
        let config = SessionConfig {
            transport: SessionTransport::Subprocess,
            surface_id: Some("provider_surface.anthropic.subprocess_cli".to_string()),
            model: Some("sonnet".to_string()),
            system_prompt: None,
            tools_enabled: false,
            cwd: None,
            sandbox_policy: None,
            approval_policy: None,
        };
        let session = ClaudeSubprocessSession::new(
            DetectedSubprocessCli {
                surface_id: "provider_surface.anthropic.subprocess_cli".to_string(),
                executable_path: false_executable_path(),
            },
            &config,
            Arc::new(AiSessionConfig::default()),
            None,
        );

        session.terminate().await;

        assert!(session.cancel_requested.load(Ordering::Acquire));
        assert_eq!(*session.state.lock(), SessionState::Terminated);
    }

    #[test]
    fn claude_session_is_external() {
        // Claude Code CLI transmits chat content off-device → must be guarded.
        let config = SessionConfig {
            transport: SessionTransport::Subprocess,
            surface_id: Some("provider_surface.anthropic.subprocess_cli".to_string()),
            model: Some("sonnet".to_string()),
            system_prompt: None,
            tools_enabled: false,
            cwd: None,
            sandbox_policy: None,
            approval_policy: None,
        };
        let session = ClaudeSubprocessSession::new(
            DetectedSubprocessCli {
                surface_id: "provider_surface.anthropic.subprocess_cli".to_string(),
                executable_path: false_executable_path(),
            },
            &config,
            Arc::new(AiSessionConfig::default()),
            None,
        );
        assert!(session.is_external());
    }

    #[tokio::test]
    async fn claude_send_message_waits_for_inflight_cli_turn() {
        let session = Arc::new(session_with_system_prompt(None));
        let guard = session.turn_lock.clone().lock_owned().await;
        let message = SessionMessage {
            role: MessageRole::User,
            content: "hello".to_string(),
            attachments: vec![],
            tools: None,
            context: None,
            response_format: None,
        };

        let send_task = {
            let session = session.clone();
            tokio::spawn(async move { session.send_message(&message).await })
        };

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        assert!(
            !send_task.is_finished(),
            "second Claude turn must wait until the in-flight stream releases the turn lock"
        );

        drop(guard);
        let stream = tokio::time::timeout(tokio::time::Duration::from_secs(1), send_task)
            .await
            .expect("send_message should proceed after the lock is released")
            .expect("send task should not panic")
            .expect("send_message should build a stream");
        drop(stream);
    }

    fn session_with_system_prompt(system_prompt: Option<String>) -> ClaudeSubprocessSession {
        let config = SessionConfig {
            transport: SessionTransport::Subprocess,
            surface_id: Some("provider_surface.anthropic.subprocess_cli".to_string()),
            model: Some("sonnet".to_string()),
            system_prompt,
            tools_enabled: false,
            cwd: None,
            sandbox_policy: None,
            approval_policy: None,
        };
        ClaudeSubprocessSession::new(
            DetectedSubprocessCli {
                surface_id: "provider_surface.anthropic.subprocess_cli".to_string(),
                executable_path: false_executable_path(),
            },
            &config,
            Arc::new(AiSessionConfig::default()),
            None,
        )
    }

    /// The prompt travels over stdin (`-p -`), so even an enormous prompt no
    /// longer contributes to the argv length: build_command must accept it
    /// without rejecting on the CreateProcess limit.
    #[test]
    fn build_command_ignores_prompt_length_now_that_prompt_is_piped() {
        let session = session_with_system_prompt(None);
        // No system prompt and no schema → zero caller-sized argv bytes,
        // regardless of how large the (stdin-delivered) prompt would be.
        session
            .build_command(None)
            .expect("piped prompt must not be argv-length-bounded");
    }

    /// Regression guard: the argv-length guard must count the bare-argv values
    /// that remain (`--system-prompt` + `--json-schema`), not just the prompt.
    /// A previous undercount excluded these, allowing a Windows CreateProcess
    /// spawn failure to slip through.
    #[test]
    fn build_command_rejects_oversized_argv_arguments() {
        let oversized_system_prompt = "x".repeat(MAX_ARGV_PROMPT_BYTES + 1);
        let session = session_with_system_prompt(Some(oversized_system_prompt));
        // turn_count == 0 → the system prompt is emitted as a bare argv value.
        let err = session
            .build_command(None)
            .expect_err("oversized argv arguments must be rejected");
        assert_eq!(err.code(), "validation.invalid_arguments");
    }

    /// Regression guard: combined argv values at exactly the limit are accepted.
    #[test]
    fn build_command_accepts_argv_arguments_at_limit() {
        let at_limit_system_prompt = "y".repeat(MAX_ARGV_PROMPT_BYTES);
        let session = session_with_system_prompt(Some(at_limit_system_prompt));
        // Unwrap panics with the CoreError diagnostic if build_command rejects
        // the arguments, making the failure reason explicit rather than opaque.
        session
            .build_command(None)
            .expect("argv arguments at exactly the limit must be accepted");
    }

    #[test]
    fn build_command_disables_claude_tools_for_conversation_sessions() {
        let session = session_with_system_prompt(None);
        let command = session.build_command(None).expect("command builds");
        let args: Vec<String> = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert!(
            args.iter().any(|arg| arg == "--tools="),
            "Claude conversation sessions must mirror the one-shot tool denial"
        );
        assert!(
            !args.iter().any(|arg| arg == "--no-session-persistence"),
            "conversation sessions keep --session-id/--continue semantics"
        );
    }
}
