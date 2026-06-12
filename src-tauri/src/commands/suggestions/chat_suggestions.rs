//! AI chat ↔ suggestion integration Tauri commands.
//!
//! Includes: `request_chat_suggestions`, `explain_suggestion_in_chat`.

use futures::StreamExt;
use tauri::command;
use tauri::{AppHandle, Emitter};
use tokio::time::{timeout, Duration};

use maekon_core::models::ai_session::{
    MessageRecord, MessageRole, OutboundMessage, SessionMessage, SessionState,
};
use maekon_core::ports::conversation_session::SessionManager;

use crate::commands::suggestion_parser::extract_suggestions;
use crate::ipc_error::IpcError;
use crate::runtime_state::{AiSessionRuntimeState, AppState, SuggestionRuntimeState};

use super::feedback::find_suggestion_explain_payload;
use super::helpers::{ai_sessions_not_available, suggestions_not_available};

const SUGGESTION_PROMPT: &str = r#"Based on our conversation context, generate 1-3 reviewable next-action candidates for the user.
Format each suggestion as JSON on a single line: {"type":"...", "content":"...", "priority":"high|medium|low"}.
Types: work_guidance, focus_reminder, task_suggestion, habit_reminder, context_insight.
Only output the JSON objects, one per line, no other text."#;

/// Generate suggestions from an active chat session by sending a structured
/// prompt and parsing the AI response. Returns the number of suggestions added.
#[command]
pub async fn request_chat_suggestions(
    ai_state: tauri::State<'_, AiSessionRuntimeState>,
    suggestion_state: tauri::State<'_, SuggestionRuntimeState>,
    session_id: String,
) -> Result<u32, IpcError> {
    let mgr = ai_state
        .manager_impl()
        .ok_or_else(ai_sessions_not_available)?;

    let suggestion_mgr = suggestion_state
        .manager()
        .ok_or_else(suggestions_not_available)?;

    // Get session and send structured request
    let session = mgr.get_session(&session_id).await.map_err(IpcError::from)?;

    let msg = SessionMessage {
        role: MessageRole::User,
        content: SUGGESTION_PROMPT.to_string(),
        attachments: Vec::new(),
        tools: None,
        context: None,
        response_format: None,
    };

    let mut stream = session.send_message(&msg).await.map_err(IpcError::from)?;

    // Drain stream and collect response text with a 60s timeout.
    // ResponseStream yields Result<OutboundMessage, CoreError>.
    const MAX_RESPONSE_BYTES: usize = 1_048_576; // 1 MB safety limit
    let drain_result = timeout(Duration::from_secs(60), async {
        let mut text = String::new();
        while let Some(item) = stream.next().await {
            match item {
                Ok(OutboundMessage::Text { content, .. }) => text.push_str(&content),
                Ok(OutboundMessage::Result { content, .. })
                    if !content.is_empty() && text.is_empty() =>
                {
                    text = content;
                }
                Ok(OutboundMessage::Error { message, .. }) => {
                    return Err(IpcError::new(
                        "provider.analysis_failed",
                        format!("AI error: {message}"),
                    ));
                }
                Err(e) => {
                    return Err(IpcError::from(e));
                }
                _ => {}
            }
            // Guard: stop accumulating if response exceeds safety limit
            if text.len() > MAX_RESPONSE_BYTES {
                tracing::warn!("chat suggestion response exceeded 1 MB limit, truncating");
                break;
            }
        }
        Ok::<String, IpcError>(text)
    })
    .await;

    let response_text = match drain_result {
        Ok(Ok(text)) => text,
        Ok(Err(e)) => return Err(e),
        Err(_) => {
            return Err(IpcError::new(
                "network.timeout",
                "Suggestion generation timed out after 60 seconds",
            ));
        }
    };

    // Parse suggestions from response. This explicit request path must surface
    // malformed/empty AI output to the UI instead of returning a silent 0.
    let suggestions = extract_suggestions(&response_text).map_err(|e| {
        IpcError::new(
            "provider.analysis_failed",
            format!("Suggestion generation failed: {e}"),
        )
    })?;
    let count = suggestions.len() as u32;
    if count == 0 {
        return Err(IpcError::new(
            "provider.analysis_failed",
            "Suggestion generation returned no valid suggestions",
        ));
    }

    // Push to queue
    let mut queue = suggestion_mgr.queue().lock().await;
    for suggestion in suggestions {
        queue.push(suggestion);
    }
    let queue_count = queue.len();
    drop(queue);

    if let Some(overlay) = suggestion_state.overlay() {
        overlay.emit_suggestions_changed(queue_count);
    }

    Ok(count)
}

/// Explain a suggestion in a chat session. Finds the suggestion from the queue
/// or history, sends an explain prompt to the session, and spawns a streaming
/// task that emits events. Emits `navigate:chat` for overlay navigation.
/// Returns the session ID used.
#[command]
pub async fn explain_suggestion_in_chat(
    app: AppHandle,
    ai_state: tauri::State<'_, AiSessionRuntimeState>,
    suggestion_state: tauri::State<'_, SuggestionRuntimeState>,
    app_state: tauri::State<'_, AppState>,
    suggestion_id: String,
    session_id: Option<String>,
) -> Result<String, IpcError> {
    let ai_mgr = ai_state
        .manager_impl()
        .ok_or_else(ai_sessions_not_available)?;

    let (content, reasoning) =
        find_suggestion_explain_payload(&suggestion_state, &app_state.storage, &suggestion_id)
            .await?;

    // Find or validate session
    let sid = match session_id {
        Some(id) => id,
        None => {
            // Find most recent active/idle session
            let sessions = ai_mgr.list_sessions().await;
            sessions
                .into_iter()
                .filter(|s| s.state == SessionState::Active || s.state == SessionState::Idle)
                .max_by_key(|s| s.last_active)
                .map(|s| s.session_id)
                .ok_or_else(|| {
                    IpcError::new(
                        "service.unavailable",
                        "No active chat session — open a chat first",
                    )
                })?
        }
    };

    // Validate session state
    let sessions = ai_mgr.list_sessions().await;
    let session_info = sessions.iter().find(|s| s.session_id == sid);
    match session_info {
        Some(info) if info.state == SessionState::Active || info.state == SessionState::Idle => {}
        Some(info) => {
            return Err(IpcError::new(
                "validation.invalid_arguments",
                format!("Session {} is not active (state: {:?})", sid, info.state),
            ));
        }
        None => {
            return Err(IpcError::new(
                "not_found.resource_missing",
                format!("Session {sid} not found"),
            ));
        }
    }

    // Compose explain message
    let mut prompt = format!(
        "Explain this suggestion in detail and help me understand how to act on it:\n\n{}",
        content
    );
    if let Some(reasoning) = reasoning {
        prompt.push_str(&format!("\n\nReasoning provided: {reasoning}"));
    }

    // Call session.send_message() directly and spawn a streaming task
    // that emits OutboundMessage events — replicating the pattern from ai_session.rs.
    let session = ai_mgr.get_session(&sid).await.map_err(IpcError::from)?;

    let user_content = prompt.clone();
    let msg = SessionMessage {
        role: MessageRole::User,
        content: prompt,
        attachments: Vec::new(),
        tools: None,
        context: None,
        response_format: None,
    };

    let session_storage = ai_state.session_storage();
    let stream = session.send_message(&msg).await.map_err(IpcError::from)?;

    // Spawn streaming task to emit events + persist messages
    // (same pattern as send_session_message in ai_session.rs)
    let event_name = format!("ai-session:{sid}");
    let session_id = sid.clone();
    let app_clone = app.clone();
    tokio::spawn(async move {
        tokio::pin!(stream);
        let mut assistant_content = String::new();
        let mut total_input: u64 = 0;
        let mut total_output: u64 = 0;

        while let Some(item) = stream.next().await {
            match item {
                Ok(outbound) => {
                    // Accumulate for persistence
                    match &outbound {
                        OutboundMessage::Text { content, .. } => {
                            assistant_content.push_str(content);
                        }
                        OutboundMessage::Result {
                            usage: Some(ref u), ..
                        } => {
                            total_input = u.input_tokens;
                            total_output = u.output_tokens;
                        }
                        _ => {}
                    }
                    let _ = app_clone.emit(&event_name, &outbound);
                }
                Err(e) => {
                    let err_msg = OutboundMessage::Error {
                        code: "stream_error".to_string(),
                        message: e.to_string(),
                        retryable: false,
                    };
                    let _ = app_clone.emit(&event_name, &err_msg);
                    break;
                }
            }
        }

        // Persist user + assistant messages after stream completes
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
                    thinking: None,
                    tool_use: None,
                    usage_input: Some(total_input),
                    usage_output: Some(total_output),
                    created_at: now,
                    seq: seq + 1,
                };
                if let Err(e) = ss
                    .save_messages(&session_id, &[user_msg, assistant_msg])
                    .await
                {
                    tracing::warn!("failed to persist explain messages: {e}");
                }
                let _ = ss
                    .update_session_usage(&session_id, total_input, total_output)
                    .await;
            }
        }
    });

    // Emit navigation event for overlay -> chat
    let _ = app.emit("navigate:chat", serde_json::json!({ "sessionId": sid }));

    Ok(sid)
}
