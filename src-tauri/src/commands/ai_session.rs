//! Tauri IPC commands for AI conversation session management.
//!
//! Provides create/send/kill/list operations. `send_session_message` spawns a
//! background task that streams `OutboundMessage` events to the frontend via
//! Tauri events on the channel `ai-session:<session_id>`.

use futures::StreamExt;
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::Arc;
use tauri::{command, AppHandle, Emitter};

use maekon_core::models::ai_session::{
    validate_session_input_size, Attachment, ConversationSessionInfo, MessageContext,
    MessageRecord, MessageRole, OutboundMessage, SessionConfig, SessionMessage, SessionRecord,
    SessionState, ToolDefinition,
};
use maekon_core::ports::conversation_session::SessionManager;

use crate::ipc_error::IpcError;
use crate::runtime_state::{
    AiSessionRuntimeState, CodexApprovalRuntimeState, SuggestionRuntimeState,
};
use tracing::debug;

/// Require a live session manager; returns a service.unavailable IpcError
/// when the manager is not wired (non-AI-enabled builds or early startup).
fn require_session_manager_impl(
    state: &AiSessionRuntimeState,
) -> Result<Arc<crate::session_manager::SessionManagerImpl>, IpcError> {
    state
        .manager_impl()
        .ok_or_else(|| IpcError::new("service.unavailable", "session manager not available"))
}

/// Canonical "session storage not available" error — used by commands that
/// require the persistent SessionStorage adapter (historical queries, renames,
/// deletes).
fn session_storage_not_available() -> IpcError {
    IpcError::new("service.unavailable", "session storage not available")
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendSessionMessageRequest {
    pub session_id: String,
    pub message: String,
    pub attachments: Option<Vec<Attachment>>,
    pub tools: Option<Vec<ToolDefinition>>,
    pub context: Option<MessageContext>,
    pub response_format: Option<serde_json::Value>,
}

/// Create a new AI conversation session.
#[command]
pub async fn create_ai_session(
    state: tauri::State<'_, AiSessionRuntimeState>,
    config: SessionConfig,
) -> Result<ConversationSessionInfo, IpcError> {
    let mgr = require_session_manager_impl(&state)?;

    let system_prompt = config.system_prompt.clone();
    let session = mgr.create_session(config).await.map_err(IpcError::from)?;
    let info = session.info();

    // Fire-and-forget: persist session metadata
    if let Some(ss) = state.session_storage() {
        let record = SessionRecord {
            session_id: info.session_id.clone(),
            provider_name: info.provider_name.clone(),
            model: info.model.clone(),
            transport: info.transport,
            state: info.state,
            system_prompt,
            turn_count: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            created_at: info.created_at,
            last_active: info.last_active,
            terminated_at: None,
            title: None,
        };
        if let Err(e) = ss.save_session(&record).await {
            tracing::warn!("failed to persist session metadata: {e}");
        }
    }

    Ok(info)
}

/// Send a message to an existing session. Spawns a background task that emits
/// `ai-session:<session_id>` Tauri events as `OutboundMessage` chunks arrive.
#[command]
pub async fn send_session_message(
    app: AppHandle,
    state: tauri::State<'_, AiSessionRuntimeState>,
    suggestion_state: tauri::State<'_, SuggestionRuntimeState>,
    request: SendSessionMessageRequest,
) -> Result<(), IpcError> {
    let mgr = require_session_manager_impl(&state)?;
    let suggestion_mgr = suggestion_state.manager();

    // Check daily token budget before sending. Dedicated wire code
    // so the frontend can surface this distinctly from other policy
    // denials (show a budget-exhausted UI, prompt upgrade, etc.).
    if !mgr.check_token_budget(&request.session_id).await {
        return Err(IpcError::new(
            "policy.denied",
            "Daily token budget exhausted",
        ));
    }

    let attachments = request.attachments.unwrap_or_default();
    validate_session_input_size("message", &request.message, &attachments)
        .map_err(IpcError::from)?;

    let session = mgr
        .get_session(&request.session_id)
        .await
        .map_err(IpcError::from)?;

    // Reset idle timer — keeps the session in Active state.
    mgr.touch_session(&request.session_id).await;

    let user_content = request.message.clone();
    let msg = SessionMessage {
        role: MessageRole::User,
        content: request.message,
        attachments,
        tools: request.tools,
        context: request.context,
        response_format: request.response_format,
    };

    let mgr_clone = require_session_manager_impl(&state)?;
    let session_storage = state.session_storage();
    let mut stream = match session.send_message(&msg).await {
        Ok(s) => s,
        Err(err) => {
            mgr_clone.report_failure(&request.session_id, &err).await;
            return Err(IpcError::from(err));
        }
    };

    let event_name = format!("ai-session:{}", request.session_id);
    let session_id = request.session_id;

    // Spawn a background task to drain the stream and emit events.
    let app_clone = app.clone();
    tokio::spawn(async move {
        /// Safety limit: truncate response if accumulated content exceeds 1 MB.
        const MAX_RESPONSE_BYTES: usize = 1_048_576;

        let mut assistant_content = String::new();
        let mut assistant_thinking: Option<String> = None;
        let mut assistant_tool_use: Option<String> = None;
        let mut total_input: u64 = 0;
        let mut total_output: u64 = 0;
        let mut stream_failed = false;
        let mut saw_terminal_result = false;

        while let Some(item) = stream.next().await {
            match item {
                Ok(outbound) => {
                    // Accumulate for persistence
                    match &outbound {
                        OutboundMessage::Text { content, .. } => {
                            assistant_content.push_str(content);
                        }
                        OutboundMessage::Thinking { content, .. } => {
                            assistant_thinking
                                .get_or_insert_with(String::new)
                                .push_str(content);
                        }
                        OutboundMessage::ToolUse { tool, status, .. } => {
                            assistant_tool_use = Some(
                                serde_json::json!({
                                    "tool": tool,
                                    "status": status,
                                })
                                .to_string(),
                            );
                        }
                        OutboundMessage::Result {
                            usage: Some(ref u),
                            done,
                            ..
                        } => {
                            total_input = u.input_tokens;
                            total_output = u.output_tokens;
                            mgr_clone
                                .accumulate_tokens(&session_id, u.input_tokens, u.output_tokens)
                                .await;
                            if *done {
                                saw_terminal_result = true;
                            }
                        }
                        OutboundMessage::Result { done: true, .. } => {
                            saw_terminal_result = true;
                        }
                        _ => {}
                    }

                    // Guard: stop accumulating if response exceeds safety limit
                    if assistant_content.len() > MAX_RESPONSE_BYTES {
                        tracing::warn!(
                            session_id = %session_id,
                            "response exceeded 1 MB limit, truncating stream"
                        );
                        break;
                    }

                    if let Err(e) = app_clone.emit(&event_name, &outbound) {
                        tracing::warn!(
                            session_id = %session_id,
                            "failed to emit ai-session event: {e}"
                        );
                        break;
                    }
                }
                Err(err) => {
                    stream_failed = true;
                    tracing::warn!(
                        session_id = %session_id,
                        "stream error: {err}"
                    );
                    let new_state = mgr_clone.report_failure(&session_id, &err).await;
                    let retryable = new_state == SessionState::Active;
                    let error_msg = OutboundMessage::Error {
                        code: "stream_error".to_string(),
                        message: err.to_string(),
                        retryable,
                    };
                    if let Err(e) = app_clone.emit(&event_name, &error_msg) {
                        debug!("emit event failed: {e}");
                    }
                    break;
                }
            }
        }

        if !stream_failed && saw_terminal_result {
            mgr_clone.record_success(&session_id).await;
        }

        // Auto-extract suggestions from AI response
        if let Some(ref sgn_mgr) = suggestion_mgr {
            let extracted =
                crate::commands::suggestion_parser::try_extract_suggestions(&assistant_content);
            if !extracted.is_empty() {
                let count = extracted.len();
                let mut queue = sgn_mgr.queue().lock().await;
                for suggestion in extracted {
                    queue.push(suggestion);
                }
                let queue_count = queue.len();
                drop(queue);

                let _ = app_clone.emit(
                    "chat:suggestions-extracted",
                    serde_json::json!({ "count": count, "sessionId": session_id }),
                );

                // Also notify overlay
                let _ = app_clone.emit(
                    "overlay:suggestions-changed",
                    serde_json::json!({ "count": queue_count }),
                );

                debug!(
                    count,
                    session_id = %session_id,
                    "auto-extracted suggestions from chat response"
                );
            }
        }

        // Persist messages after stream completes
        if let Some(ref ss) = session_storage {
            if let Ok(seq) = ss.next_seq(&session_id).await {
                let now = chrono::Utc::now();
                let user_msg = MessageRecord {
                    id: None,
                    session_id: session_id.clone(),
                    role: "user".to_string(),
                    content: user_content,
                    thinking: None,
                    tool_use: None,
                    usage_input: None,
                    usage_output: None,
                    created_at: now,
                    seq,
                };
                let assistant_msg = MessageRecord {
                    id: None,
                    session_id: session_id.clone(),
                    role: "assistant".to_string(),
                    content: assistant_content,
                    thinking: assistant_thinking,
                    tool_use: assistant_tool_use,
                    usage_input: Some(total_input),
                    usage_output: Some(total_output),
                    created_at: now,
                    seq: seq + 1,
                };
                if let Err(e) = ss
                    .save_messages(&session_id, &[user_msg, assistant_msg])
                    .await
                {
                    tracing::warn!("failed to persist messages: {e}");
                }
                // Increment session usage (additive — SQL does +=)
                let _ = ss
                    .update_session_usage(&session_id, total_input, total_output)
                    .await;
            }
        }
    });

    Ok(())
}

/// Terminate an active AI session.
#[command]
pub async fn kill_ai_session(
    state: tauri::State<'_, AiSessionRuntimeState>,
    session_id: String,
) -> Result<(), IpcError> {
    let mgr = require_session_manager_impl(&state)?;

    mgr.kill_session(&session_id)
        .await
        .map_err(IpcError::from)?;

    // Fire-and-forget: mark terminated in DB
    if let Some(ss) = state.session_storage() {
        if let Err(e) = ss.terminate_session(&session_id).await {
            debug!("terminate_session failed: {e}");
        }
    }

    Ok(())
}

/// Retry (recover) a failed or errored session. Increments retry_count and
/// returns the session info if successful. Fails when max retries exceeded.
#[command]
pub async fn retry_ai_session(
    state: tauri::State<'_, AiSessionRuntimeState>,
    session_id: String,
) -> Result<ConversationSessionInfo, IpcError> {
    let mgr = require_session_manager_impl(&state)?;

    let session = mgr
        .recover_session(&session_id)
        .await
        .map_err(IpcError::from)?;
    Ok(session.info())
}

/// List all AI sessions (active + persisted historical).
#[command]
pub async fn list_ai_sessions(
    state: tauri::State<'_, AiSessionRuntimeState>,
) -> Result<Vec<ConversationSessionInfo>, IpcError> {
    let mut result = vec![];

    if let Some(mgr) = state.manager_impl() {
        result.extend(mgr.list_sessions().await);
    }

    // Merge persisted (historical) sessions.
    // Reuse max_history_turns (default 100) as the session list limit.
    if let Some(ss) = state.session_storage() {
        let limit = state.max_history_turns();
        if let Ok(persisted) = ss.list_sessions(limit).await {
            let active_ids: HashSet<String> = result.iter().map(|s| s.session_id.clone()).collect();
            for record in &persisted {
                if !active_ids.contains(&record.session_id) {
                    result.push(ConversationSessionInfo::from(record));
                }
            }
        }
    }

    Ok(result)
}

/// Load conversation history for a session (active or persisted).
#[command]
pub async fn load_session_messages(
    state: tauri::State<'_, AiSessionRuntimeState>,
    session_id: String,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<Vec<MessageRecord>, IpcError> {
    let ss = state
        .session_storage()
        .ok_or_else(session_storage_not_available)?;

    ss.load_messages(&session_id, limit.unwrap_or(100), offset.unwrap_or(0))
        .await
        .map_err(IpcError::from)
}

/// Delete a persisted session and all its messages.
#[command]
pub async fn delete_session_history(
    state: tauri::State<'_, AiSessionRuntimeState>,
    session_id: String,
) -> Result<(), IpcError> {
    let ss = state
        .session_storage()
        .ok_or_else(session_storage_not_available)?;

    ss.delete_session(&session_id).await.map_err(IpcError::from)
}

/// Rename (set display title) for an AI session.
#[command]
pub async fn rename_ai_session(
    state: tauri::State<'_, AiSessionRuntimeState>,
    session_id: String,
    new_title: String,
) -> Result<(), IpcError> {
    let ss = state
        .session_storage()
        .ok_or_else(session_storage_not_available)?;

    ss.update_session_title(&session_id, &new_title)
        .await
        .map_err(IpcError::from)
}

/// Get token usage for the current day across all sessions.
#[command]
pub async fn get_token_usage(
    state: tauri::State<'_, AiSessionRuntimeState>,
) -> Result<TokenUsageResponse, IpcError> {
    let mgr = require_session_manager_impl(&state)?;

    let (input, output) = mgr.get_global_token_usage().await;
    let budget = state.daily_token_budget().unwrap_or(0);
    Ok(TokenUsageResponse {
        total_input_tokens: input,
        total_output_tokens: output,
        daily_budget: budget,
        budget_remaining: if budget == 0 {
            None
        } else {
            Some(budget.saturating_sub(input + output))
        },
    })
}

// ── E21 #5017: mid-turn control + #5044: approval response ───────────────────

/// Interrupt (stop) the in-flight turn of an AI session (E21 #5017). Mirrors
/// `send_session_message`'s manager/get_session pattern; returns `Result`
/// (no stream), so no `AppHandle` is needed. The call traverses the decorator
/// stack (Auditing → Guarded → CodexAppServerSession) — the LIVE consumer of
/// `ConversationSession::interrupt`.
#[command]
pub async fn interrupt_session_turn(
    state: tauri::State<'_, AiSessionRuntimeState>,
    session_id: String,
) -> Result<(), IpcError> {
    let mgr = require_session_manager_impl(&state)?;
    let session = mgr.get_session(&session_id).await.map_err(IpcError::from)?;
    session.interrupt().await.map_err(IpcError::from)
}

/// Steer (course-correct) the in-flight turn of an AI session with additional
/// user input (E21 #5017). Applies the same 256 KiB input guard as
/// `send_session_message`. The steering content traverses the privacy guard
/// (Guarded decorator) fail-closed before reaching an external backend.
#[command]
pub async fn steer_session_turn(
    state: tauri::State<'_, AiSessionRuntimeState>,
    session_id: String,
    message: String,
) -> Result<(), IpcError> {
    validate_session_input_size("steer message", &message, &[])?;
    let mgr = require_session_manager_impl(&state)?;

    // Check daily token budget before steering — mirrors send_session_message
    // so a course-correction turn cannot bypass the budget gate.
    if !mgr.check_token_budget(&session_id).await {
        return Err(IpcError::new(
            "policy.denied",
            "Daily token budget exhausted",
        ));
    }

    let session = mgr.get_session(&session_id).await.map_err(IpcError::from)?;
    let msg = SessionMessage {
        role: MessageRole::User,
        content: message,
        attachments: vec![],
        tools: None,
        context: None,
        response_format: None,
    };
    session.steer(&msg).await.map_err(IpcError::from)
}

/// Map a UI decision verb to the fail-closed boolean the decider's oneshot
/// expects (E21 #5044). `accept` → true; everything else (`decline`, `cancel`,
/// or any unexpected verb) → false. CANCEL and any unknown verb are FAIL-CLOSED
/// declines — only the explicit `accept` verb approves.
fn decision_to_bool(decision: &str) -> bool {
    decision == "accept"
}

/// Resolve a parked Codex approval request with the user's UI decision (E21
/// #5044). Removes the parked `oneshot::Sender<bool>` from the shared registry
/// and resolves it. A missing/already-resolved id returns `not_found` (the
/// decider has already timed out + declined — fail-closed).
#[command]
pub async fn respond_codex_approval(
    state: tauri::State<'_, CodexApprovalRuntimeState>,
    request_id: u64,
    decision: String,
) -> Result<(), IpcError> {
    let tx = state
        .registry()
        .lock()
        .await
        .remove(&request_id)
        .ok_or_else(|| {
            IpcError::new(
                "not_found.resource_missing",
                "approval request not found or already resolved",
            )
        })?;
    let accept = decision_to_bool(&decision);
    // A dropped receiver (decider already declined on timeout) is benign — the
    // send just returns Err and the system stays fail-closed.
    let _ = tx.send(accept);
    Ok(())
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsageResponse {
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub daily_budget: u64,
    pub budget_remaining: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::decision_to_bool;
    use maekon_core::models::ai_session::{
        validate_session_input_size, Attachment, MAX_SESSION_ATTACHMENTS, MAX_SESSION_INPUT_BYTES,
    };

    #[test]
    fn accept_maps_true_decline_and_cancel_map_false() {
        // E21 #5044 fail-closed UI mapping: only `accept` approves; `cancel` is a
        // UI verb that fails closed to decline; an unknown verb also declines.
        assert!(decision_to_bool("accept"));
        assert!(!decision_to_bool("decline"));
        assert!(!decision_to_bool("cancel"));
        assert!(!decision_to_bool("anything-else"));
        assert!(!decision_to_bool(""));
    }

    #[test]
    fn session_input_size_counts_inline_attachment_data() {
        let attachment = Attachment::File {
            path: "huge.txt".to_string(),
            mime: Some("text/plain".to_string()),
            data: Some("a".repeat(MAX_SESSION_INPUT_BYTES)),
        };

        let err = validate_session_input_size("message", "hi", &[attachment])
            .expect_err("oversized inline attachment data must be rejected");
        assert_eq!(err.code, "input.too_large");
    }

    #[test]
    fn session_input_size_limits_attachment_count() {
        let attachments = (0..=MAX_SESSION_ATTACHMENTS)
            .map(|idx| Attachment::Directory {
                path: format!("dir-{idx}"),
            })
            .collect::<Vec<_>>();

        let err = validate_session_input_size("message", "hi", &attachments)
            .expect_err("too many attachments must be rejected");
        assert_eq!(err.code, "input.too_large");
    }
}
