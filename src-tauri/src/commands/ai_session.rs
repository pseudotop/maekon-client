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

use maekon_core::error::CoreError;
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

/// Convert a persisted record into a historical session entry.
///
/// Session runtimes are intentionally process-local. After an app restart, a
/// record can still contain its last live state even though no runtime exists
/// in the current manager. Returning that stale state makes the Chat composer
/// editable and guarantees `send_session_message` will fail with
/// `not_found.resource_missing`. Only manager-owned sessions are live; every
/// other persisted record is exposed as terminated/read-only history.
fn historical_session_info(
    record: &SessionRecord,
    live_session_ids: &HashSet<String>,
) -> Option<ConversationSessionInfo> {
    if live_session_ids.contains(&record.session_id) {
        return None;
    }

    let mut info = ConversationSessionInfo::from(record);
    info.state = SessionState::Terminated;
    Some(info)
}

/// #8057 (P2-2): decide the synthetic terminal the drain task must emit when a
/// stream ended WITHOUT a provider terminal, so the chat UI never hangs on
/// "generating".
///
/// - A `stream_failed` stream already surfaced an `Error`, and a
///   `saw_terminal_result` stream already emitted its own terminal — both
///   return `None` (nothing to synthesize).
/// - A stream cut short by a LOCAL guard (`terminated_early`, e.g. the 1 MB
///   response cap or a dead event channel) is closed with a benign
///   `Result { done: true }`: the content already streamed, the turn is simply
///   over.
/// - A stream that just STOPPED with no terminal (codex app-server process died
///   mid-turn → `recv` None; an SSE close with no `[DONE]`/`message_stop`)
///   surfaces a retryable `incomplete_stream` error so the frontend can clear
///   the spinner and offer a retry.
fn synthetic_stream_terminal(
    stream_failed: bool,
    saw_terminal_result: bool,
    terminated_early: bool,
) -> Option<OutboundMessage> {
    if stream_failed || saw_terminal_result {
        return None;
    }
    Some(if terminated_early {
        OutboundMessage::Result {
            content: String::new(),
            done: true,
            usage: None,
        }
    } else {
        OutboundMessage::Error {
            code: "incomplete_stream".to_string(),
            message: "stream ended before a completion signal was received".to_string(),
            retryable: true,
        }
    })
}

/// Select the canonical assistant body after a passive stream finishes.
/// Providers that emitted any Text frame own the body; terminal Result.content
/// is a fallback only for transports that emitted no Text frames at all.
fn finalize_assistant_content(
    saw_text_frame: bool,
    text_content: String,
    terminal_result_content: String,
) -> String {
    if saw_text_frame {
        text_content
    } else {
        terminal_result_content
    }
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
        screen_derived: false,
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

    // #8057 (P3): reserve an abort-registry slot so `interrupt_session_turn` can
    // cancel this drain task for a backend without a native turn interrupt
    // (HTTP/Ollama). The token is moved into the task (which deregisters itself
    // on completion); the abort handle is bound after spawn.
    let inflight = state.inflight_registry();
    let inflight_token = inflight.reserve_token();
    let inflight_task = inflight.clone();
    let registry_key = session_id.clone();

    // Spawn a background task to drain the stream and emit events.
    let app_clone = app.clone();
    let handle = tokio::spawn(async move {
        /// Safety limit: truncate response if accumulated content exceeds 1 MB.
        const MAX_RESPONSE_BYTES: usize = 1_048_576;

        let mut assistant_content = String::new();
        let mut terminal_result_content = String::new();
        let mut saw_text_frame = false;
        let mut assistant_thinking: Option<String> = None;
        let mut assistant_tool_use: Option<String> = None;
        let mut total_input: u64 = 0;
        let mut total_output: u64 = 0;
        let mut stream_failed = false;
        let mut saw_terminal_result = false;
        // #8057 (P2-2): set when the loop exits via a LOCAL guard (size cap /
        // dead event channel) rather than the stream naturally ending.
        let mut terminated_early = false;

        while let Some(item) = stream.next().await {
            match item {
                Ok(outbound) => {
                    // Accumulate for persistence
                    match &outbound {
                        OutboundMessage::Text { content, .. } => {
                            saw_text_frame = true;
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
                            content,
                            usage,
                            done,
                        } => {
                            if !content.is_empty() {
                                terminal_result_content.clear();
                                terminal_result_content.push_str(content);
                            }
                            // #8057 (P2-1): take the per-field running MAX rather
                            // than overwriting. A turn can report usage in more
                            // than one chunk — Anthropic sends input on
                            // `message_start` and output on `message_delta` — so a
                            // plain overwrite let the later output-only chunk clobber
                            // `total_input` back to 0 in the persisted MessageRecord.
                            // Within one turn input_tokens is constant and
                            // output_tokens is monotonic, so MAX yields the correct
                            // totals for every provider. The durable ledger is fed
                            // the raw per-chunk deltas (disjoint here), so it sums
                            // to the same totals without double counting.
                            if let Some(u) = usage {
                                total_input = total_input.max(u.input_tokens);
                                total_output = total_output.max(u.output_tokens);
                                mgr_clone
                                    .accumulate_tokens(&session_id, u.input_tokens, u.output_tokens)
                                    .await;
                            }
                            if *done {
                                saw_terminal_result = true;
                            }
                        }
                        _ => {}
                    }

                    // Guard: stop accumulating if response exceeds safety limit
                    let response_len = if saw_text_frame {
                        assistant_content.len()
                    } else {
                        terminal_result_content.len()
                    };
                    if response_len > MAX_RESPONSE_BYTES {
                        tracing::warn!(
                            session_id = %session_id,
                            "response exceeded 1 MB limit, truncating stream"
                        );
                        terminated_early = true;
                        break;
                    }

                    if let Err(e) = app_clone.emit(&event_name, &outbound) {
                        tracing::warn!(
                            session_id = %session_id,
                            "failed to emit ai-session event: {e}"
                        );
                        terminated_early = true;
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

        assistant_content =
            finalize_assistant_content(saw_text_frame, assistant_content, terminal_result_content);

        if !stream_failed && saw_terminal_result {
            mgr_clone.record_success(&session_id).await;
        }

        // #8057 (P2-2): a stream that ended without any terminal (Err /
        // Result{done:true}) leaves the frontend spinner stuck forever. Emit a
        // synthetic terminal so the UI always resolves the turn.
        if let Some(terminal) =
            synthetic_stream_terminal(stream_failed, saw_terminal_result, terminated_early)
        {
            tracing::warn!(
                session_id = %session_id,
                "stream ended without a terminal result; emitting synthetic terminal"
            );
            if let Err(e) = app_clone.emit(&event_name, &terminal) {
                debug!("emit synthetic terminal failed: {e}");
            }
        }

        // Auto-extract suggestions from AI response
        if let Some(ref sgn_mgr) = suggestion_mgr {
            let extracted =
                crate::commands::suggestion_parser::try_extract_suggestions(&assistant_content);
            if !extracted.is_empty() {
                let mut queue = sgn_mgr.queue().lock().await;
                let admitted_count =
                    crate::commands::suggestion_parser::admit_suggestions(&mut queue, extracted);
                let queue_count = queue.len();
                drop(queue);

                if admitted_count > 0 {
                    let _ = app_clone.emit(
                        "chat:suggestions-extracted",
                        serde_json::json!({ "count": admitted_count, "sessionId": session_id }),
                    );

                    // Also notify the overlay with the authoritative queue size.
                    let _ = app_clone.emit(
                        "overlay:suggestions-changed",
                        serde_json::json!({ "count": queue_count }),
                    );

                    debug!(
                        count = admitted_count,
                        session_id = %session_id,
                        "auto-extracted suggestions from chat response"
                    );
                }
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

        // #8057 (P3): deregister this drain task's abort slot on natural
        // completion (token-guarded so a newer same-session turn is untouched).
        // An interrupt-abort skips this arm — `abort_inflight` already removed it.
        inflight_task.deregister(&session_id, inflight_token);
    });

    // Bind the abort handle now that the task is spawned (runs before the task is
    // first polled, so it cannot deregister before it is registered).
    inflight.bind(registry_key, inflight_token, handle.abort_handle());

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

/// List AI sessions (active + persisted historical), paginated over the
/// persisted history. #8057 (P3): `offset` pages past the first `limit` records
/// so older sessions beyond the window are reachable; `limit` defaults to
/// `max_history_turns`. Live active sessions are prepended only on the first
/// page (`offset == 0`) — they belong there and must not repeat on deeper pages.
#[command]
pub async fn list_ai_sessions(
    state: tauri::State<'_, AiSessionRuntimeState>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<Vec<ConversationSessionInfo>, IpcError> {
    let offset = offset.unwrap_or(0);
    let limit = limit.unwrap_or_else(|| state.max_history_turns());

    let active: Vec<ConversationSessionInfo> = match state.manager_impl() {
        Some(mgr) => mgr.list_sessions().await,
        _ => vec![],
    };
    let live_session_ids: HashSet<String> = active
        .iter()
        .map(|session| session.session_id.clone())
        .collect();
    let mut result = if offset == 0 { active } else { vec![] };

    // Merge persisted (historical) sessions for the requested page.
    if let Some(ss) = state.session_storage() {
        if let Ok(persisted) = ss.list_sessions(limit, offset).await {
            for record in &persisted {
                if let Some(historical) = historical_session_info(record, &live_session_ids) {
                    result.push(historical);
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
    app: tauri::AppHandle,
    state: tauri::State<'_, AiSessionRuntimeState>,
) -> Result<TokenUsageResponse, IpcError> {
    use tauri::Manager;
    let mgr = require_session_manager_impl(&state)?;

    let (input, output) = mgr.get_global_token_usage().await;
    let budget = state.daily_token_budget().unwrap_or(0);
    // #9466: expose the configured model/provider so the privacy panel can
    // estimate spend from a local price table (never a network lookup).
    let llm_api = app
        .try_state::<crate::runtime_state::ConfigRuntimeState>()
        .and_then(|cs| cs.config_manager().get().ai_provider.llm_api);
    let (model, provider) = match llm_api {
        Some(endpoint) => (
            endpoint.model.clone(),
            Some(format!("{:?}", endpoint.provider_type).to_lowercase()),
        ),
        None => (None, None),
    };
    Ok(TokenUsageResponse {
        total_input_tokens: input,
        total_output_tokens: output,
        daily_budget: budget,
        budget_remaining: if budget == 0 {
            None
        } else {
            Some(budget.saturating_sub(input + output))
        },
        model,
        provider,
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
    app: AppHandle,
    state: tauri::State<'_, AiSessionRuntimeState>,
    session_id: String,
) -> Result<(), IpcError> {
    let mgr = require_session_manager_impl(&state)?;
    let session = mgr.get_session(&session_id).await.map_err(IpcError::from)?;
    match session.interrupt().await {
        Ok(()) => Ok(()),
        // #8057 (P3): backends without a native in-flight-turn interrupt
        // (HTTP/Ollama) return InvalidArguments. Fall back to aborting the
        // background drain task so "stop" actually halts BYOK token consumption,
        // and emit a terminal so the chat UI clears its spinner — the aborted
        // task is cancelled before it can emit one itself.
        Err(CoreError::InvalidArguments { .. }) => {
            if state.abort_inflight(&session_id) {
                let event_name = format!("ai-session:{session_id}");
                let terminal = OutboundMessage::Result {
                    content: String::new(),
                    done: true,
                    usage: None,
                };
                if let Err(e) = app.emit(&event_name, &terminal) {
                    debug!("emit interrupt terminal failed: {e}");
                }
            }
            Ok(())
        }
        Err(other) => Err(IpcError::from(other)),
    }
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
        screen_derived: false,
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
    /// #9466: configured LLM model (from `config.ai.llm_api`), so the
    /// privacy panel can price today's usage from a LOCAL reference table
    /// (no network lookup). `None` when no endpoint/model is configured.
    pub model: Option<String>,
    /// #9466: configured provider type label (e.g. `anthropic`), same source.
    pub provider: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{
        decision_to_bool, finalize_assistant_content, historical_session_info,
        synthetic_stream_terminal,
    };
    use chrono::Utc;
    use maekon_core::models::ai_session::{
        validate_session_input_size, Attachment, OutboundMessage, SessionRecord, SessionState,
        SessionTransport, MAX_SESSION_ATTACHMENTS, MAX_SESSION_INPUT_BYTES,
    };
    use std::collections::HashSet;

    fn session_record(session_id: &str, state: SessionState) -> SessionRecord {
        let now = Utc::now();
        SessionRecord {
            session_id: session_id.to_string(),
            provider_name: "codex".to_string(),
            model: "gpt-5.6-sol".to_string(),
            transport: SessionTransport::Subprocess,
            state,
            system_prompt: None,
            turn_count: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            created_at: now,
            last_active: now,
            terminated_at: None,
            title: None,
        }
    }

    #[test]
    fn persisted_non_live_session_is_returned_as_terminated_history() {
        let record = session_record("persisted-active", SessionState::Active);
        let info = historical_session_info(&record, &HashSet::new())
            .expect("non-live persisted session should be listed");

        assert_eq!(info.session_id, record.session_id);
        assert_eq!(info.state, SessionState::Terminated);
    }

    #[test]
    fn live_session_is_not_duplicated_by_its_persisted_record() {
        let record = session_record("live-session", SessionState::Active);
        let live_ids = HashSet::from([record.session_id.clone()]);

        assert!(historical_session_info(&record, &live_ids).is_none());
    }

    #[test]
    fn result_content_is_used_only_when_no_text_frame_exists() {
        assert_eq!(
            finalize_assistant_content(false, String::new(), "result-only".to_string()),
            "result-only"
        );
        assert_eq!(
            finalize_assistant_content(
                true,
                "streamed text".to_string(),
                "duplicate terminal body".to_string(),
            ),
            "streamed text"
        );
        assert_eq!(
            finalize_assistant_content(true, String::new(), "terminal body".to_string()),
            "",
            "the presence of a Text frame, not its byte length, owns the response body"
        );
    }

    #[test]
    fn synthetic_terminal_none_when_stream_already_resolved() {
        // #8057 (P2-2): a stream that already surfaced an Error, or already
        // emitted a provider terminal, needs no synthetic terminal.
        assert!(synthetic_stream_terminal(true, false, false).is_none());
        assert!(synthetic_stream_terminal(false, true, false).is_none());
        assert!(synthetic_stream_terminal(true, true, true).is_none());
    }

    #[test]
    fn synthetic_terminal_incomplete_error_on_silent_stream_end() {
        // A stream that simply stopped (codex process death / SSE close with no
        // terminal) must surface a retryable incomplete_stream error so the UI
        // clears the "generating" spinner.
        match synthetic_stream_terminal(false, false, false) {
            Some(OutboundMessage::Error {
                code, retryable, ..
            }) => {
                assert_eq!(code, "incomplete_stream");
                assert!(retryable);
            }
            other => panic!("expected incomplete_stream Error, got {other:?}"),
        }
    }

    #[test]
    fn synthetic_terminal_done_result_on_local_early_termination() {
        // A locally truncated stream (size cap / dead channel) already streamed
        // its content — close it with a benign done Result, not an error.
        match synthetic_stream_terminal(false, false, true) {
            Some(OutboundMessage::Result { done, usage, .. }) => {
                assert!(done);
                assert!(usage.is_none());
            }
            other => panic!("expected done Result, got {other:?}"),
        }
    }

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
