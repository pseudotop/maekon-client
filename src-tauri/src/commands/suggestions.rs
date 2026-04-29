use futures::StreamExt;
use serde::Serialize;
use tauri::command;
use tauri::{AppHandle, Emitter};
use tokio::time::{timeout, Duration};

use oneshim_core::models::ai_session::{
    MessageRecord, MessageRole, OutboundMessage, SessionMessage, SessionState,
};
use oneshim_core::ports::conversation_session::SessionManager;

use crate::commands::suggestion_parser::try_extract_suggestions;
use crate::ipc_error::IpcError;
use crate::runtime_state::{AiSessionRuntimeState, AppState, SuggestionRuntimeState};

/// Canonical "Suggestions not available" error — suggestion manager missing.
fn suggestions_not_available() -> IpcError {
    IpcError::new("service.unavailable", "Suggestions not available")
}

/// Canonical "AI sessions not available" error — AI session manager missing.
fn ai_sessions_not_available() -> IpcError {
    IpcError::new("service.unavailable", "AI sessions not available")
}

/// Enqueue a failed feedback for background retry and persist to SQLite.
/// SQLite persist happens synchronously (primary durability guarantee).
/// In-memory enqueue uses `tokio::spawn` to avoid blocking the IPC caller;
/// it is best-effort for the current session — on restart, SQLite is restored.
fn enqueue_feedback_retry(
    mgr: &crate::suggestion_manager::SuggestionManager,
    suggestion_id: &str,
    feedback_type: oneshim_core::models::suggestion::FeedbackType,
    comment: Option<String>,
) {
    let record = oneshim_core::models::storage_records::PendingFeedbackRecord::new_for_insert(
        suggestion_id.to_string(),
        &feedback_type,
        comment.clone(),
        0,
        chrono::Utc::now(),
    );
    if let Err(e) = mgr.storage().save_pending_feedback(&record) {
        tracing::warn!(id = %suggestion_id, "failed to persist pending feedback: {e}");
    }
    // Fire-and-forget tokio task to avoid holding the caller's async context.
    let rq = mgr.retry_queue().clone();
    let sid = suggestion_id.to_string();
    tokio::spawn(async move {
        rq.lock()
            .await
            .enqueue(oneshim_suggestion::feedback_retry::PendingFeedback {
                suggestion_id: sid,
                feedback_type,
                comment,
                attempts: 0,
                next_retry_at: chrono::Utc::now(),
            });
    });
}

#[derive(Serialize)]
pub struct SuggestionViewDto {
    pub id: String,
    pub title: String,
    pub body: String,
    pub priority: String,
    pub category: Option<String>,
    pub source: String,
    pub confidence_score: f64,
    pub created_at: String,
    pub is_read: bool,
    pub reasoning: Option<String>,
}

#[derive(Serialize)]
pub struct SuggestionHistoryDto {
    #[serde(flatten)]
    pub suggestion: SuggestionViewDto,
    pub feedback: Option<String>,
}

fn source_label(source: &oneshim_core::models::suggestion::SuggestionSource) -> &'static str {
    match source {
        oneshim_core::models::suggestion::SuggestionSource::LlmServer => "server",
        oneshim_core::models::suggestion::SuggestionSource::LlmLocal => "local",
        oneshim_core::models::suggestion::SuggestionSource::RuleBased => "rule",
    }
}

#[command]
pub async fn get_pending_suggestions(
    state: tauri::State<'_, SuggestionRuntimeState>,
) -> Result<Vec<SuggestionViewDto>, IpcError> {
    let mgr = state.manager().ok_or_else(suggestions_not_available)?;

    // Collect suggestions from queue into a Vec first, then drop the queue lock
    // BEFORE calling is_read() — is_read() acquires its own lock (read_ids),
    // and holding both would cause a nested lock.
    let snapshot: Vec<_> = {
        let queue = mgr.queue().lock().await;
        queue
            .iter()
            .map(|s| {
                (
                    s.suggestion_id.clone(),
                    oneshim_suggestion::presenter::type_to_title(&s.suggestion_type),
                    s.content.clone(),
                    format!("{:?}", s.priority).to_lowercase(),
                    source_label(&s.source).to_string(),
                    s.confidence_score,
                    s.created_at.to_rfc3339(),
                    s.reasoning.clone(),
                )
            })
            .collect()
    }; // queue lock dropped here

    let mut results = Vec::with_capacity(snapshot.len());
    for (id, title, body, priority, source, confidence_score, created_at, reasoning) in snapshot {
        let is_read = mgr.is_read(&id).await;
        results.push(SuggestionViewDto {
            id,
            title,
            body,
            priority,
            category: None, // Suggestion has no category field
            source,
            confidence_score,
            created_at,
            is_read,
            reasoning,
        });
    }
    Ok(results)
}

#[command]
pub async fn get_suggestion_history(
    state: tauri::State<'_, SuggestionRuntimeState>,
    limit: Option<u32>,
) -> Result<Vec<SuggestionHistoryDto>, IpcError> {
    let mgr = state.manager().ok_or_else(suggestions_not_available)?;

    // Snapshot history entries and drop lock before calling is_read()
    let snapshot: Vec<_> = {
        let history = mgr.history().lock().await;
        history
            .recent(limit.unwrap_or(50) as usize)
            .into_iter()
            .cloned()
            .collect()
    }; // history lock dropped here

    let mut results = Vec::with_capacity(snapshot.len());
    for entry in snapshot {
        let is_read = mgr.is_read(&entry.suggestion.suggestion_id).await;
        let feedback = entry.feedback.as_ref().map(|f| match f {
            oneshim_core::models::suggestion::FeedbackType::Accepted => "accepted".to_string(),
            oneshim_core::models::suggestion::FeedbackType::Rejected => "rejected".to_string(),
            oneshim_core::models::suggestion::FeedbackType::Deferred => "deferred".to_string(),
        });
        results.push(SuggestionHistoryDto {
            suggestion: SuggestionViewDto {
                id: entry.suggestion.suggestion_id.clone(),
                title: oneshim_suggestion::presenter::type_to_title(
                    &entry.suggestion.suggestion_type,
                ),
                body: entry.suggestion.content.clone(),
                priority: format!("{:?}", entry.suggestion.priority).to_lowercase(),
                category: None,
                source: source_label(&entry.suggestion.source).to_string(),
                confidence_score: entry.suggestion.confidence_score,
                created_at: entry.suggestion.created_at.to_rfc3339(),
                is_read,
                reasoning: entry.suggestion.reasoning.clone(),
            },
            feedback,
        });
    }
    Ok(results)
}

#[command]
pub async fn submit_suggestion_feedback(
    state: tauri::State<'_, SuggestionRuntimeState>,
    suggestion_id: String,
    action: String,
    snooze_minutes: Option<u32>,
) -> Result<(), IpcError> {
    let mgr = state.manager().ok_or_else(suggestions_not_available)?;

    // Send feedback to server (best-effort — enqueue for retry on failure)
    match action.as_str() {
        "accept" => {
            if let Err(_e) = mgr.feedback().accept(&suggestion_id, None).await {
                enqueue_feedback_retry(
                    &mgr,
                    &suggestion_id,
                    oneshim_core::models::suggestion::FeedbackType::Accepted,
                    None,
                );
            }
        }
        "reject" => {
            if let Err(_e) = mgr.feedback().reject(&suggestion_id, None).await {
                enqueue_feedback_retry(
                    &mgr,
                    &suggestion_id,
                    oneshim_core::models::suggestion::FeedbackType::Rejected,
                    None,
                );
            }
        }
        "defer" => {
            // Server notification is best-effort; local state changes always proceed.
            if let Err(_e) = mgr.feedback().defer(&suggestion_id, None).await {
                enqueue_feedback_retry(
                    &mgr,
                    &suggestion_id,
                    oneshim_core::models::suggestion::FeedbackType::Deferred,
                    None,
                );
            }

            let (removed, scorer_data) = {
                let mut queue = mgr.queue().lock().await;
                let scorer_data = queue
                    .iter()
                    .find(|s| s.suggestion_id == suggestion_id)
                    .map(|s| (s.suggestion_type.clone(), s.source.clone()));
                let removed = queue.remove_by_id(&suggestion_id);
                (removed, scorer_data)
            }; // queue lock dropped
            if let Some((stype, source)) = scorer_data {
                mgr.scorer().lock().await.record(
                    stype,
                    source,
                    &oneshim_core::models::suggestion::FeedbackType::Deferred,
                );
            }

            if let Some(suggestion) = removed {
                {
                    let mut history = mgr.history().lock().await;
                    history.add(suggestion.clone());
                    history.record_feedback(
                        &suggestion_id,
                        oneshim_core::models::suggestion::FeedbackType::Deferred,
                    );
                }
                let duration_mins = snooze_minutes.unwrap_or(120);
                let duration = chrono::Duration::minutes(duration_mins as i64);
                mgr.deferred().lock().await.defer(suggestion, duration);
            }

            let count = mgr.queue().lock().await.len();
            if let Some(overlay) = state.overlay() {
                overlay.emit_suggestions_changed(count);
            }
            // Don't fall through to the accept/reject history block
            return Ok(());
        }
        _ => {
            return Err(IpcError::new(
                "validation.invalid_arguments",
                format!("Unknown action: {action}. Use accept/reject/defer"),
            ));
        }
    }

    // Move accepted/rejected suggestion from queue to history.
    // Acquire queue lock once to both remove the item and get the remaining count,
    // avoiding a redundant second lock acquisition.
    let (removed, remaining_count) = {
        let mut queue = mgr.queue().lock().await;
        let removed = queue.remove_by_id(&suggestion_id);
        let count = queue.len();
        (removed, count)
    }; // queue lock dropped here

    if let Some(suggestion) = removed {
        let feedback_type = match action.as_str() {
            "accept" => oneshim_core::models::suggestion::FeedbackType::Accepted,
            "reject" => oneshim_core::models::suggestion::FeedbackType::Rejected,
            _ => unreachable!(),
        };
        mgr.scorer().lock().await.record(
            suggestion.suggestion_type.clone(),
            suggestion.source.clone(),
            &feedback_type,
        );
        {
            let mut history = mgr.history().lock().await;
            history.add(suggestion);
            history.record_feedback(&suggestion_id, feedback_type);
        }
    }

    // Notify overlay that suggestions changed (item removed from queue)
    if let Some(overlay) = state.overlay() {
        overlay.emit_suggestions_changed(remaining_count);
    }

    Ok(())
}

// ── Chat ↔ Suggestion integration ────────────────────────────

const SUGGESTION_PROMPT: &str = r#"Based on our conversation context, generate 1-3 actionable suggestions for the user.
Each suggestion should be specific, practical, and relevant to the current discussion.

Respond ONLY with a JSON object matching this schema:
{"suggestions": [{"type": "<type>", "content": "<text>", "priority": "<priority>", "reasoning": "<why>"}]}

Valid types: work_guidance, email_draft, productivity_tip, workflow_optimization, context_based
Valid priorities: low, medium, high, critical"#;

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

    // Parse suggestions from response
    let suggestions = try_extract_suggestions(&response_text);
    let count = suggestions.len() as u32;

    // Push to queue
    if !suggestions.is_empty() {
        let mut queue = suggestion_mgr.queue().lock().await;
        for suggestion in suggestions {
            queue.push(suggestion);
        }
        let queue_count = queue.len();
        drop(queue);

        if let Some(overlay) = suggestion_state.overlay() {
            overlay.emit_suggestions_changed(queue_count);
        }
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
    suggestion_id: String,
    session_id: Option<String>,
) -> Result<String, IpcError> {
    let suggestion_mgr = suggestion_state
        .manager()
        .ok_or_else(suggestions_not_available)?;

    let ai_mgr = ai_state
        .manager_impl()
        .ok_or_else(ai_sessions_not_available)?;

    // Find suggestion from queue or history.
    // Two-phase lookup: check queue first, then fall back to history.
    let from_queue = {
        let queue = suggestion_mgr.queue().lock().await;
        let found = queue
            .iter()
            .find(|s| s.suggestion_id == suggestion_id)
            .map(|s| (s.content.clone(), s.reasoning.clone()));
        found
    }; // queue lock dropped

    let (content, reasoning) = if let Some(pair) = from_queue {
        pair
    } else {
        let history = suggestion_mgr.history().lock().await;
        let entry = history
            .recent(100)
            .into_iter()
            .find(|e| e.suggestion.suggestion_id == suggestion_id);
        match entry {
            Some(e) => (e.suggestion.content.clone(), e.suggestion.reasoning.clone()),
            None => {
                return Err(IpcError::new(
                    "not_found.resource_missing",
                    format!("Suggestion {suggestion_id} not found"),
                ));
            }
        }
    };

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

// ── Queue persistence ────────────────────────────────────────

/// Save current suggestion queue and deferred items to SQLite for offline persistence.
/// Queue items are saved with state="pending", deferred items with state="deferred".
#[command]
pub async fn save_suggestion_state(
    suggestion_state: tauri::State<'_, SuggestionRuntimeState>,
    app_state: tauri::State<'_, AppState>,
) -> Result<u32, IpcError> {
    let mgr = suggestion_state
        .manager()
        .ok_or_else(suggestions_not_available)?;
    let storage = &app_state.storage;

    let mut saved = 0u32;

    // Save queue items with state="pending"
    let queue = mgr.queue().lock().await;
    for suggestion in queue.iter() {
        if let Err(e) = storage.save_suggestion_with_state(suggestion, "pending", None) {
            tracing::warn!(id = %suggestion.suggestion_id, "failed to persist suggestion: {e}");
        } else {
            saved += 1;
        }
    }
    drop(queue);

    // Save deferred items with state="deferred" and resurface_at
    let deferred = mgr.deferred().lock().await;
    for entry in deferred.list_deferred() {
        let resurface = entry.resurface_at.to_rfc3339();
        if let Err(e) =
            storage.save_suggestion_with_state(&entry.suggestion, "deferred", Some(&resurface))
        {
            tracing::warn!(id = %entry.suggestion.suggestion_id, "failed to persist deferred: {e}");
        } else {
            saved += 1;
        }
    }

    Ok(saved)
}

// ── Suggestion statistics ────────────────────────────────────

#[derive(Serialize)]
pub struct TypeCountDto {
    pub suggestion_type: String,
    pub count: u32,
}

#[derive(Serialize)]
pub struct SourceStatsDto {
    pub source: String,
    pub count: u32,
    pub acceptance_rate: f64,
}

#[derive(Serialize)]
pub struct SuggestionStatsDto {
    pub total: u32,
    pub accepted: u32,
    pub rejected: u32,
    pub deferred: u32,
    pub pending: u32,
    pub acceptance_rate: f64,
    pub by_type: Vec<TypeCountDto>,
    pub by_source: Vec<SourceStatsDto>,
}

/// Return aggregate statistics from the suggestion history (in-memory).
#[command]
pub async fn get_suggestion_stats(
    state: tauri::State<'_, SuggestionRuntimeState>,
) -> Result<SuggestionStatsDto, IpcError> {
    let mgr = state.manager().ok_or_else(suggestions_not_available)?;
    let stats = mgr.history().lock().await.stats();
    let rate = if stats.total > 0 {
        (stats.accepted as f64 / stats.total as f64) * 100.0
    } else {
        0.0
    };
    let by_type = stats
        .by_type
        .iter()
        .map(|(t, c)| TypeCountDto {
            suggestion_type: t.clone(),
            count: *c,
        })
        .collect();
    let by_source = stats
        .by_source
        .iter()
        .map(|(s, c, r)| SourceStatsDto {
            source: s.clone(),
            count: *c,
            acceptance_rate: *r,
        })
        .collect();
    Ok(SuggestionStatsDto {
        total: stats.total,
        accepted: stats.accepted,
        rejected: stats.rejected,
        deferred: stats.deferred,
        pending: stats.pending,
        acceptance_rate: (rate * 10.0).round() / 10.0,
        by_type,
        by_source,
    })
}

// ── Daily time-series stats ─────────────────────────────────

#[derive(Serialize)]
pub struct DailyStatDto {
    pub day: String,
    pub total: u32,
    pub acted: u32,
    pub suggestion_type: String,
    pub source: String,
}

/// Return daily aggregated suggestion statistics for the last N days (max 90).
#[command]
pub async fn get_suggestion_daily_stats(
    app_state: tauri::State<'_, AppState>,
    days: Option<u32>,
) -> Result<Vec<DailyStatDto>, IpcError> {
    let days = days.unwrap_or(7).min(90);
    let records = app_state
        .storage
        .suggestion_daily_stats(days)
        .map_err(IpcError::from)?;
    Ok(records
        .into_iter()
        .map(|r| DailyStatDto {
            day: r.day,
            total: r.total,
            acted: r.acted,
            suggestion_type: r.suggestion_type,
            source: r.source,
        })
        .collect())
}

// ── Deferred suggestions ────────────────────────────────────

#[derive(Serialize)]
pub struct DeferredSuggestionDto {
    pub id: String,
    pub title: String,
    pub body: String,
    pub priority: String,
    pub source: String,
    pub deferred_at: String,
    pub resurface_at: String,
    pub remaining_minutes: i64,
}

/// Return the list of currently deferred (snoozed) suggestions.
#[command]
pub async fn get_deferred_suggestions(
    state: tauri::State<'_, SuggestionRuntimeState>,
) -> Result<Vec<DeferredSuggestionDto>, IpcError> {
    let mgr = state.manager().ok_or_else(suggestions_not_available)?;
    let deferred = mgr.deferred().lock().await;
    let now = chrono::Utc::now();

    let items: Vec<DeferredSuggestionDto> = deferred
        .list_deferred()
        .into_iter()
        .map(|entry| {
            let remaining = (entry.resurface_at - now).num_minutes().max(0);
            DeferredSuggestionDto {
                id: entry.suggestion.suggestion_id.clone(),
                title: oneshim_suggestion::presenter::type_to_title(
                    &entry.suggestion.suggestion_type,
                ),
                body: entry.suggestion.content.clone(),
                priority: format!("{:?}", entry.suggestion.priority).to_lowercase(),
                source: source_label(&entry.suggestion.source).to_string(),
                deferred_at: entry.deferred_at.to_rfc3339(),
                resurface_at: entry.resurface_at.to_rfc3339(),
                remaining_minutes: remaining,
            }
        })
        .collect();

    Ok(items)
}
