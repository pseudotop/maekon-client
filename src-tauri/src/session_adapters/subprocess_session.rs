use std::process::Stdio;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_stream::try_stream;
use async_trait::async_trait;
use chrono::Utc;
use parking_lot::Mutex;
use tempfile::tempdir;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::RwLock;
use tokio::time::timeout;
use uuid::Uuid;

use oneshim_api_contracts::provider_specs::{default_surface_model, provider_surface_spec};
use oneshim_core::config::AiSessionConfig;
use oneshim_core::error::CoreError;
use oneshim_core::models::ai_session::{
    ChatMessage, ChatRole, ConversationSessionInfo, OutboundMessage, SessionConfig, SessionMessage,
    SessionState, SessionTransport, TokenUsage, ToolDefinition, ToolUseStatus,
};
use oneshim_core::ports::conversation_session::{ConversationSession, ResponseStream};

use crate::session_adapters::prompt_payload::{
    extract_native_response_schema, render_conversation_prompt, render_message_payload,
};
use crate::subprocess_provider::{
    append_model_flag, append_oneshot_flags, classify_subprocess_error, DetectedSubprocessCli,
    SubprocessKind,
};
use tracing::debug;

pub struct GenericSubprocessSession {
    session_id: String,
    surface: DetectedSubprocessCli,
    provider_name: String,
    model: String,
    system_prompt: Option<String>,
    default_tools: Option<Vec<ToolDefinition>>,
    history: Arc<RwLock<Vec<ChatMessage>>>,
    state: Mutex<SessionState>,
    turn_count: AtomicU32,
    created_at: chrono::DateTime<chrono::Utc>,
    last_active: Mutex<Instant>,
    timeout: Duration,
    max_history_turns: u32,
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
                    oneshim_api_contracts::provider_specs::SurfaceCapabilityKind::Llm,
                )
                .ok()
                .flatten()
            })
            .unwrap_or_else(|| "gpt-5.4".to_string());

        let provider_name = provider_surface_spec(&surface.surface_id)
            .map(|spec| spec.vendor_id.clone())
            .unwrap_or_else(|_| "subprocess".to_string());

        Self {
            session_id: Uuid::new_v4().to_string(),
            surface,
            provider_name,
            model,
            system_prompt: config.system_prompt.clone(),
            default_tools,
            history: Arc::new(RwLock::new(Vec::new())),
            state: Mutex::new(SessionState::Active),
            turn_count: AtomicU32::new(0),
            created_at: Utc::now(),
            last_active: Mutex::new(Instant::now()),
            timeout: Duration::from_secs(session_config.session_timeout_secs),
            max_history_turns: session_config.max_history_turns,
        }
    }

    async fn invoke_surface(&self, prompt: &str) -> Result<String, CoreError> {
        if self
            .surface
            .surface_id
            .eq("provider_surface.openai.subprocess_cli")
        {
            self.run_codex(prompt).await
        } else if self
            .surface
            .surface_id
            .eq("provider_surface.google.subprocess_cli")
        {
            self.run_gemini(prompt).await
        } else {
            // Iter-94: reachable only if caller configured a surface that has
            // no conversation-session implementation on this code path. That's
            // a configuration mismatch (invalid surface for this operation),
            // not an internal error — wire code `config.invalid` lets
            // telemetry/i18n surface "pick a different provider" rather than
            // "something broke inside oneshim".
            Err(CoreError::Config {
                code: oneshim_core::error_codes::ConfigCode::Invalid,
                message: format!(
                    "subprocess conversation sessions are not implemented for surface '{}'",
                    self.surface.surface_id
                ),
            })
        }
    }

    async fn run_codex(&self, prompt: &str) -> Result<String, CoreError> {
        let temp_dir = tempdir().map_err(|err| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
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

        let mut child = child.spawn().map_err(|err| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("Failed to spawn Codex session subprocess: {err}"),
        })?;

        let mut stdin = child.stdin.take().ok_or_else(|| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: "Failed to open stdin for Codex session subprocess".to_string(),
        })?;
        stdin
            .write_all(prompt.as_bytes())
            .await
            .map_err(CoreError::Io)?;
        drop(stdin);

        let output = timeout(self.timeout, child.wait_with_output())
            .await
            .map_err(|_| CoreError::RequestTimeout {
                code: oneshim_core::error_codes::NetworkCode::Timeout,
                timeout_ms: self.timeout.as_millis() as u64,
            })?
            .map_err(CoreError::Io)?;

        if !output.status.success() {
            return Err(classify_subprocess_error(
                SubprocessKind::Llm,
                &self.surface.surface_id,
                &String::from_utf8_lossy(&output.stderr),
            ));
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    async fn run_gemini(&self, prompt: &str) -> Result<String, CoreError> {
        let temp_dir = tempdir().map_err(|err| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("Failed to create Gemini session tempdir: {err}"),
        })?;

        let mut command = Command::new(&self.surface.executable_path);
        command
            .arg("-p")
            .arg(prompt)
            .current_dir(temp_dir.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        append_oneshot_flags(&mut command, &self.surface.surface_id);
        append_model_flag(&mut command, &self.surface.surface_id, &self.model);

        let output = timeout(self.timeout, command.output())
            .await
            .map_err(|_| CoreError::RequestTimeout {
                code: oneshim_core::error_codes::NetworkCode::Timeout,
                timeout_ms: self.timeout.as_millis() as u64,
            })?
            .map_err(CoreError::Io)?;

        if !output.status.success() {
            return Err(classify_subprocess_error(
                SubprocessKind::Llm,
                &self.surface.surface_id,
                &String::from_utf8_lossy(&output.stderr),
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
        *self.last_active.lock() = Instant::now();

        let temp_dir = tempdir().map_err(|err| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
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
            std::fs::write(
                &schema_path,
                serde_json::to_vec_pretty(schema).map_err(|err| CoreError::Internal {
                    code: oneshim_core::error_codes::InternalCode::Generic,
                    message: format!("Failed to serialize Codex output schema for session: {err}"),
                })?,
            )
            .map_err(|err| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("Failed to write Codex output schema for session: {err}"),
            })?;
            child.arg("--output-schema").arg(schema_path);
        }

        let mut child = child.spawn().map_err(|err| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("Failed to spawn Codex session subprocess: {err}"),
        })?;

        let mut stdin = child.stdin.take().ok_or_else(|| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: "Failed to open stdin for Codex session subprocess".to_string(),
        })?;
        stdin
            .write_all(prompt.as_bytes())
            .await
            .map_err(CoreError::Io)?;
        drop(stdin);

        let stdout = child.stdout.take().ok_or_else(|| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: "Failed to capture Codex session stdout".to_string(),
        })?;
        let mut stderr = child.stderr.take().ok_or_else(|| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: "Failed to capture Codex session stderr".to_string(),
        })?;

        let history = self.history.clone();
        let max_history_turns = self.max_history_turns;
        let timeout = self.timeout;
        let surface_id = self.surface.surface_id.clone();
        let provider_name = self.provider_name.clone();

        let stream: ResponseStream = Box::pin(try_stream! {
            let _temp_dir = temp_dir;
            let mut lines = tokio::io::BufReader::new(stdout).lines();
            let deadline = tokio::time::Instant::now() + timeout;
            let stderr_task = tokio::spawn(async move {
                let mut stderr_buf = String::new();
                if let Err(e) = stderr.read_to_string(&mut stderr_buf).await {
                    debug!("read_to_string failed: {e}");
                }
                stderr_buf
            });
            let mut assistant_text = String::new();
            let mut saw_non_empty_event = false;

            loop {
                let line_result = tokio::time::timeout_at(deadline, lines.next_line()).await;
                match line_result {
                    Ok(Ok(Some(line))) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }

                        if let Some(message) = parse_codex_json_event(trimmed) {
                            if matches!(&message, OutboundMessage::Error { message, .. } if message.starts_with("Reconnecting...")) {
                                continue;
                            }

                            if let OutboundMessage::Text { content, .. } = &message {
                                if !content.is_empty() {
                                    assistant_text.push_str(content);
                                    saw_non_empty_event = true;
                                }
                            } else {
                                saw_non_empty_event = true;
                            }

                            yield message;
                            continue;
                        }

                        assistant_text.push_str(trimmed);
                        saw_non_empty_event = true;
                        yield OutboundMessage::Text {
                            content: trimmed.to_string(),
                            done: false,
                        };
                    }
                    Ok(Ok(None)) => break,
                    Ok(Err(err)) => {
                        yield OutboundMessage::Error {
                            code: "io_error".to_string(),
                            message: err.to_string(),
                            retryable: false,
                        };
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
                        if let Err(e) = child.kill().await {
                            debug!("process kill failed: {e}");
                        }
                        break;
                    }
                }
            }

            let status = child.wait().await.map_err(CoreError::Io)?;
            let stderr_output = stderr_task.await.unwrap_or_default();

            if !status.success() {
                let classified =
                    classify_subprocess_error(SubprocessKind::Llm, &surface_id, &stderr_output);
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
        if self
            .surface
            .surface_id
            .eq("provider_surface.openai.subprocess_cli")
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
        *self.last_active.lock() = Instant::now();

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
                Err(CoreError::Analysis { code: oneshim_core::error_codes::ProviderCode::AnalysisFailed, message: format!(
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
        let elapsed = self.last_active.lock().elapsed();
        let last_active_utc = Utc::now() - chrono::Duration::from_std(elapsed).unwrap_or_default();
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
}

fn parse_codex_json_event(line: &str) -> Option<OutboundMessage> {
    let value = serde_json::from_str::<serde_json::Value>(line).ok()?;
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
            message: value
                .get("message")
                .and_then(|message| message.as_str())
                .unwrap_or("Codex CLI error")
                .to_string(),
            retryable: true,
        }),
        _ => None,
    }
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
    use oneshim_core::models::ai_session::{MessageContext, MessageRole};
    use std::path::PathBuf;

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

    #[tokio::test]
    async fn session_prompt_includes_message_metadata() {
        let session = GenericSubprocessSession {
            session_id: "test".to_string(),
            surface: DetectedSubprocessCli {
                surface_id: "provider_surface.google.subprocess_cli".to_string(),
                executable_path: PathBuf::from("/usr/bin/false"),
            },
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
            last_active: Mutex::new(Instant::now()),
            timeout: Duration::from_secs(30),
            max_history_turns: 8,
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
}
