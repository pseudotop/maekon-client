//! Explicit, consent-gated current-scene to reviewable-suggestion use case.

use chrono::{DateTime, Utc};
use futures::StreamExt;
use maekon_analysis::{
    CurrentContextPromptInput, CURRENT_CONTEXT_SUGGESTION_PROMPT, LOCAL_CURRENT_SCENE_SOURCE_ID,
};
use maekon_core::consent::ConsentPermissions;
use maekon_core::id_generation::generate_id;
use maekon_core::models::ai_session::{MessageRole, OutboundMessage, SessionMessage, SessionState};
use maekon_core::models::suggestion::{Suggestion, SuggestionContextScope, SuggestionSource};
use maekon_core::models::task::{
    compute_dedupe_key, CandidateState, SourceKind, SourceLifecycle, TaskCandidate, TaskSourceRef,
};
use maekon_core::ports::conversation_session::{
    ConversationSession, ResponseStream, SessionManager,
};
use maekon_core::ports::task_store::{IngestCandidateRequest, TaskCommandPort};
use maekon_suggestion::queue::SuggestionQueue;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tauri::{command, AppHandle, Emitter};
use tokio::time::{timeout, Duration};

use crate::commands::capture::{
    analyze_current_scene_snapshot, manual_capture_permissions, CurrentSceneSnapshot,
};
use crate::commands::suggestion_parser::extract_suggestions;
use crate::ipc_error::IpcError;
use crate::runtime_state::{AiSessionRuntimeState, AppState, SuggestionRuntimeState};
use crate::session_manager::SessionManagerImpl;

const RESULT_EVENT: &str = "current-context:suggestions-result";
const MAX_RESPONSE_BYTES: usize = 1_048_576;
const PROVIDER_TIMEOUT_SECS: u64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CurrentContextSuggestionStatus {
    Generated,
    NoCandidate,
    Throttled,
    AnalysisUnavailable,
    ConsentRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CurrentContextProvenanceDto {
    pub source_id: &'static str,
    pub observed_at: String,
    pub captured_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CurrentContextSuggestionResult {
    pub status: CurrentContextSuggestionStatus,
    pub reason: Option<&'static str>,
    pub admitted_count: u32,
    pub queue_count: u32,
    pub admitted_suggestion_ids: Vec<String>,
    pub missing_permissions: Vec<&'static str>,
    pub provenance: Option<CurrentContextProvenanceDto>,
}

impl CurrentContextSuggestionResult {
    fn empty(
        status: CurrentContextSuggestionStatus,
        reason: &'static str,
        queue_count: usize,
    ) -> Self {
        Self {
            status,
            reason: Some(reason),
            admitted_count: 0,
            queue_count: bounded_count(queue_count),
            admitted_suggestion_ids: Vec::new(),
            missing_permissions: Vec::new(),
            provenance: None,
        }
    }
}

#[command]
pub async fn request_current_context_suggestions(
    app: AppHandle,
    app_state: tauri::State<'_, AppState>,
    ai_state: tauri::State<'_, AiSessionRuntimeState>,
    suggestion_state: tauri::State<'_, SuggestionRuntimeState>,
    session_id: Option<String>,
) -> Result<CurrentContextSuggestionResult, IpcError> {
    let Some(suggestion_manager) = suggestion_state.manager() else {
        return Ok(emit_result(
            &app,
            CurrentContextSuggestionResult::empty(
                CurrentContextSuggestionStatus::AnalysisUnavailable,
                "suggestion_queue_unavailable",
                0,
            ),
        ));
    };

    let Some(_reservation) = suggestion_state.try_reserve_current_context_request() else {
        let queue_count = suggestion_manager.queue().lock().await.len();
        return Ok(emit_result(
            &app,
            CurrentContextSuggestionResult::empty(
                CurrentContextSuggestionStatus::Throttled,
                "request_in_flight",
                queue_count,
            ),
        ));
    };

    let permissions = manual_capture_permissions(&app_state);
    if !permissions.screen_capture {
        let queue_count = suggestion_manager.queue().lock().await.len();
        let mut result = CurrentContextSuggestionResult::empty(
            CurrentContextSuggestionStatus::ConsentRequired,
            "screen_capture_consent_required",
            queue_count,
        );
        result.missing_permissions.push("screen_capture");
        return Ok(emit_result(&app, result));
    }

    let Some(session_manager) = ai_state.manager_impl() else {
        let queue_count = suggestion_manager.queue().lock().await.len();
        return Ok(emit_result(
            &app,
            CurrentContextSuggestionResult::empty(
                CurrentContextSuggestionStatus::AnalysisUnavailable,
                "provider_unavailable",
                queue_count,
            ),
        ));
    };
    let Some(session) = select_session(&session_manager, session_id.as_deref()).await else {
        let queue_count = suggestion_manager.queue().lock().await.len();
        return Ok(emit_result(
            &app,
            CurrentContextSuggestionResult::empty(
                CurrentContextSuggestionStatus::AnalysisUnavailable,
                "active_session_unavailable",
                queue_count,
            ),
        ));
    };
    let selected_session_id = session.session_id().to_string();

    // The existing external conversation decorator requires this consent and
    // applies the configured PII/egress policy before transmission. Surface the
    // missing own-field as a typed result before any capture work starts.
    if session.is_external() && !permissions.full_text_extraction {
        let queue_count = suggestion_manager.queue().lock().await.len();
        let mut result = CurrentContextSuggestionResult::empty(
            CurrentContextSuggestionStatus::ConsentRequired,
            "external_text_consent_required",
            queue_count,
        );
        result.missing_permissions.push("full_text_extraction");
        return Ok(emit_result(&app, result));
    }

    if !session_manager
        .check_token_budget(&selected_session_id)
        .await
    {
        let queue_count = suggestion_manager.queue().lock().await.len();
        return Ok(emit_result(
            &app,
            CurrentContextSuggestionResult::empty(
                CurrentContextSuggestionStatus::Throttled,
                "daily_token_budget_exhausted",
                queue_count,
            ),
        ));
    }

    let snapshot = match analyze_current_scene_snapshot(&app, &app_state).await {
        Ok(snapshot) => snapshot,
        Err(_) => {
            let queue_count = suggestion_manager.queue().lock().await.len();
            return Ok(emit_result(
                &app,
                CurrentContextSuggestionResult::empty(
                    CurrentContextSuggestionStatus::AnalysisUnavailable,
                    "scene_analysis_unavailable",
                    queue_count,
                ),
            ));
        }
    };

    let (prompt, provenance, app_scope, window_scope) = match build_prompt(snapshot, &permissions) {
        Ok(value) => value,
        Err(_) => {
            let queue_count = suggestion_manager.queue().lock().await.len();
            return Ok(emit_result(
                &app,
                CurrentContextSuggestionResult::empty(
                    CurrentContextSuggestionStatus::AnalysisUnavailable,
                    "prompt_serialization_failed",
                    queue_count,
                ),
            ));
        }
    };
    if !prompt.has_reviewable_context {
        let queue_count = suggestion_manager.queue().lock().await.len();
        let missing_permissions = missing_context_permissions(&permissions);
        let (status, reason) = empty_context_status(&missing_permissions);
        let mut result = CurrentContextSuggestionResult::empty(status, reason, queue_count);
        result.missing_permissions = missing_permissions;
        result.provenance = Some(provenance);
        return Ok(emit_result(&app, result));
    }

    let message_content = format!("{CURRENT_CONTEXT_SUGGESTION_PROMPT}{}", prompt.context_json);
    let message = SessionMessage {
        // #9643 review I2: this prompt is assembled from context_json (OCR
        // regions, window titles, GUI elements) — screen-derived, so the
        // guard must keep the STRICT window gate.
        screen_derived: true,
        role: MessageRole::User,
        content: message_content,
        attachments: Vec::new(),
        tools: None,
        context: None,
        response_format: None,
    };

    session_manager.touch_session(&selected_session_id).await;
    let stream = match session.send_message(&message).await {
        Ok(stream) => stream,
        Err(error) => {
            session_manager
                .report_failure(&selected_session_id, &error)
                .await;
            let queue_count = suggestion_manager.queue().lock().await.len();
            let mut result = CurrentContextSuggestionResult::empty(
                CurrentContextSuggestionStatus::AnalysisUnavailable,
                "provider_error",
                queue_count,
            );
            result.provenance = Some(provenance);
            return Ok(emit_result(&app, result));
        }
    };

    let response_text = match collect_response(&session_manager, &selected_session_id, stream).await
    {
        Ok(text) => text,
        Err(reason) => {
            let queue_count = suggestion_manager.queue().lock().await.len();
            let mut result = CurrentContextSuggestionResult::empty(
                CurrentContextSuggestionStatus::AnalysisUnavailable,
                reason,
                queue_count,
            );
            result.provenance = Some(provenance);
            return Ok(emit_result(&app, result));
        }
    };

    let suggestions = match extract_suggestions(&response_text) {
        Ok(suggestions) => suggestions,
        Err(_) => {
            let queue_count = suggestion_manager.queue().lock().await.len();
            let mut result = CurrentContextSuggestionResult::empty(
                CurrentContextSuggestionStatus::AnalysisUnavailable,
                "malformed_response",
                queue_count,
            );
            result.provenance = Some(provenance);
            return Ok(emit_result(&app, result));
        }
    };
    if suggestions.is_empty() {
        let queue_count = suggestion_manager.queue().lock().await.len();
        let mut result = CurrentContextSuggestionResult::empty(
            CurrentContextSuggestionStatus::NoCandidate,
            "no_valid_candidate",
            queue_count,
        );
        result.provenance = Some(provenance);
        return Ok(emit_result(&app, result));
    }

    let provider_source = if session.is_external() {
        SuggestionSource::LlmServer
    } else {
        SuggestionSource::LlmLocal
    };
    let prepared = prepare_suggestions(suggestions, provider_source, app_scope, window_scope);
    // Snapshot the sanitized (id, content) of each prepared candidate before the
    // suggestions are moved into the queue, so admitted ones can also become
    // reviewable durable task candidates below.
    let candidate_contents: Vec<(String, String)> = prepared
        .iter()
        .map(|suggestion| (suggestion.suggestion_id.clone(), suggestion.content.clone()))
        .collect();
    let (admitted_suggestion_ids, queue_count) = {
        let mut queue = suggestion_manager.queue().lock().await;
        let admitted = admit_suggestions(&mut queue, prepared);
        (admitted, queue.len())
    };
    let admitted_count = bounded_count(admitted_suggestion_ids.len());

    // Bridge admitted current-scene suggestions into reviewable durable task
    // candidates (ADR-028 §1). Generation may create a PROPOSED candidate; only
    // an explicit human confirm turns one into a to-do. Ingestion is dedupe-key
    // guarded and best-effort — a storage hiccup must never fail suggestion
    // delivery. Provenance stays metadata-only (never the raw scene text).
    for (suggestion_id, content) in &candidate_contents {
        if !admitted_suggestion_ids.contains(suggestion_id) {
            continue;
        }
        let candidate =
            build_local_current_scene_candidate(suggestion_id, content, &provenance, Utc::now());
        if let Err(error) = app_state
            .storage
            .ingest_candidate(IngestCandidateRequest { candidate })
            .await
        {
            tracing::warn!(error = %error, "current-context task candidate ingest failed");
        }
    }

    if admitted_count > 0 {
        if let Some(overlay) = suggestion_state.overlay() {
            overlay.emit_suggestions_changed(queue_count);
        }
    }

    let status = if admitted_count > 0 {
        CurrentContextSuggestionStatus::Generated
    } else {
        CurrentContextSuggestionStatus::NoCandidate
    };
    Ok(emit_result(
        &app,
        CurrentContextSuggestionResult {
            status,
            reason: (admitted_count == 0).then_some("not_admitted"),
            admitted_count,
            queue_count: bounded_count(queue_count),
            admitted_suggestion_ids,
            missing_permissions: Vec::new(),
            provenance: Some(provenance),
        },
    ))
}

async fn select_session(
    manager: &SessionManagerImpl,
    requested_session_id: Option<&str>,
) -> Option<Arc<dyn ConversationSession>> {
    if let Some(session_id) = requested_session_id.filter(|value| !value.trim().is_empty()) {
        let session = manager.get_session(session_id).await.ok()?;
        return matches!(
            session.info().state,
            SessionState::Active | SessionState::Idle
        )
        .then_some(session);
    }

    let session_id = manager
        .list_sessions()
        .await
        .into_iter()
        .filter(|info| matches!(info.state, SessionState::Active | SessionState::Idle))
        .max_by_key(|info| info.last_active)
        .map(|info| info.session_id)?;
    manager.get_session(&session_id).await.ok()
}

fn build_prompt(
    snapshot: CurrentSceneSnapshot,
    permissions: &ConsentPermissions,
) -> Result<
    (
        maekon_analysis::BuiltCurrentContextPrompt,
        CurrentContextProvenanceDto,
        Option<String>,
        Option<String>,
    ),
    serde_json::Error,
> {
    let captured_at = DateTime::parse_from_rfc3339(&snapshot.response.timestamp)
        .map(|value| value.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    let observed_at = snapshot.observed_at;
    let app_allowed = permissions.process_monitoring && permissions.app_usage_analytics;
    let window_allowed = permissions.window_title_collection;
    let app_name = app_allowed
        .then(|| snapshot.response.app_name.clone())
        .filter(|value| !value.eq_ignore_ascii_case("unknown"));
    let window_title = window_allowed.then(|| snapshot.response.window_title.clone());
    let accessibility_text = if permissions.full_text_extraction {
        snapshot
            .response
            .accessibility
            .as_ref()
            .and_then(|accessibility| accessibility.focused_element.as_ref())
            .and_then(|element| {
                element
                    .extracted_text
                    .clone()
                    .or_else(|| element.label.clone())
            })
    } else {
        None
    };
    let ocr_text = if permissions.ocr_processing {
        snapshot
            .response
            .ocr_regions
            .iter()
            .map(|region| region.text.clone())
            .collect()
    } else {
        Vec::new()
    };
    let prompt = CurrentContextPromptInput {
        observed_at,
        captured_at,
        app_name: app_name.clone(),
        window_title: window_title.clone(),
        accessibility_text,
        ocr_text,
        work_type: permissions
            .activity_pattern_learning
            .then_some(snapshot.response.work_type)
            .flatten(),
    }
    .build()?;
    let provenance = CurrentContextProvenanceDto {
        source_id: LOCAL_CURRENT_SCENE_SOURCE_ID,
        observed_at: observed_at.to_rfc3339(),
        captured_at: captured_at.to_rfc3339(),
    };
    Ok((prompt, provenance, app_name, window_title))
}

fn bounded_count(count: usize) -> u32 {
    u32::try_from(count).unwrap_or(u32::MAX)
}

fn missing_context_permissions(permissions: &ConsentPermissions) -> Vec<&'static str> {
    let mut missing = Vec::new();
    if !permissions.process_monitoring {
        missing.push("process_monitoring");
    }
    if !permissions.app_usage_analytics {
        missing.push("app_usage_analytics");
    }
    if !permissions.window_title_collection {
        missing.push("window_title_collection");
    }
    if !permissions.full_text_extraction {
        missing.push("full_text_extraction");
    }
    if !permissions.ocr_processing {
        missing.push("ocr_processing");
    }
    missing
}

fn empty_context_status(
    missing_permissions: &[&'static str],
) -> (CurrentContextSuggestionStatus, &'static str) {
    if missing_permissions.is_empty() {
        (
            CurrentContextSuggestionStatus::AnalysisUnavailable,
            "current_context_empty",
        )
    } else {
        (
            CurrentContextSuggestionStatus::ConsentRequired,
            "current_context_fields_not_permitted",
        )
    }
}

async fn collect_response(
    manager: &SessionManagerImpl,
    session_id: &str,
    mut stream: ResponseStream,
) -> Result<String, &'static str> {
    let drain = timeout(Duration::from_secs(PROVIDER_TIMEOUT_SECS), async {
        let mut text_content = String::new();
        let mut terminal_content = String::new();
        let mut saw_text = false;

        while let Some(item) = stream.next().await {
            match item {
                Ok(OutboundMessage::Text { content, .. }) => {
                    saw_text = true;
                    text_content.push_str(&content);
                }
                Ok(OutboundMessage::Result { content, usage, .. }) => {
                    if !content.is_empty() {
                        terminal_content = content;
                    }
                    if let Some(usage) = usage {
                        manager
                            .accumulate_tokens(session_id, usage.input_tokens, usage.output_tokens)
                            .await;
                    }
                }
                Ok(OutboundMessage::Error { .. }) => return Err("provider_error"),
                Err(error) => {
                    manager.report_failure(session_id, &error).await;
                    return Err("provider_error");
                }
                _ => {}
            }

            let response_len = if saw_text {
                text_content.len()
            } else {
                terminal_content.len()
            };
            if response_len > MAX_RESPONSE_BYTES {
                return Err("response_too_large");
            }
        }

        Ok(finalize_response_body(
            saw_text,
            text_content,
            terminal_content,
        ))
    })
    .await;

    match drain {
        Ok(Ok(body)) if !body.trim().is_empty() => {
            manager.record_success(session_id).await;
            Ok(body)
        }
        Ok(Ok(_)) => Err("empty_response"),
        Ok(Err(reason)) => Err(reason),
        Err(_) => Err("provider_timeout"),
    }
}

fn finalize_response_body(
    saw_text: bool,
    text_content: String,
    terminal_content: String,
) -> String {
    if saw_text {
        text_content
    } else {
        terminal_content
    }
}

fn prepare_suggestions(
    suggestions: Vec<Suggestion>,
    source: SuggestionSource,
    app_name: Option<String>,
    window_title: Option<String>,
) -> Vec<Suggestion> {
    suggestions
        .into_iter()
        .map(|mut suggestion| {
            suggestion.suggestion_id = format!("local-current-scene-{}", uuid::Uuid::new_v4());
            suggestion.source = source.clone();
            suggestion.context_scope = if app_name.is_some() || window_title.is_some() {
                Some(SuggestionContextScope {
                    app_name: app_name.clone(),
                    window_title: window_title.clone(),
                    target_id: None,
                })
            } else {
                None
            };
            suggestion
        })
        .collect()
}

fn admit_suggestions(queue: &mut SuggestionQueue, suggestions: Vec<Suggestion>) -> Vec<String> {
    suggestions
        .into_iter()
        .filter_map(|suggestion| {
            let suggestion_id = suggestion.suggestion_id.clone();
            queue.push(suggestion).then_some(suggestion_id)
        })
        .collect()
}

/// Build a PROPOSED `LOCAL_CURRENT_SCENE` task candidate from one admitted
/// suggestion. `LocalCurrentScene` is anchor-less (ADR-028 Amendment B2), so the
/// unique suggestion id — itself minted per capture — is folded into the dedupe
/// namespace to keep each capture occurrence distinct. Only the sanitized
/// next-step text and metadata provenance are carried; never raw OCR/window text.
fn build_local_current_scene_candidate(
    suggestion_id: &str,
    content: &str,
    provenance: &CurrentContextProvenanceDto,
    now: DateTime<Utc>,
) -> TaskCandidate {
    let observed_at = DateTime::parse_from_rfc3339(&provenance.observed_at)
        .map(|value| value.with_timezone(&Utc))
        .unwrap_or(now);
    let occurred_at = DateTime::parse_from_rfc3339(&provenance.captured_at)
        .map(|value| value.with_timezone(&Utc))
        .ok();
    let (title, body) = split_next_step(content);
    let source_ref = TaskSourceRef {
        source_kind: SourceKind::LocalCurrentScene,
        extension_id: None,
        install_id: None,
        account_subject_ref: None,
        upstream_object_id: None,
        upstream_revision: None,
        upstream_etag: None,
        occurred_at,
        observed_at,
        dedupe_namespace: format!("local-current-scene:{suggestion_id}"),
        content_hash: content_hash(content),
        lifecycle: SourceLifecycle::Active,
        source_outcome: None,
    };
    let dedupe_key = compute_dedupe_key(&source_ref);
    TaskCandidate {
        id: generate_id("tcand"),
        state: CandidateState::Proposed,
        title: Some(title),
        body,
        proposed_due: None,
        proposed_owner_ref: None,
        expires_at: now + chrono::Duration::days(7),
        source_ref,
        dedupe_key,
        revision: 1,
        created_at: now,
        updated_at: now,
    }
}

/// Split a single sanitized suggestion string into a bounded to-do title (first
/// line) and an optional body (the remainder). Bounds mirror the confirm-time
/// edit caps so a candidate can never become an unbounded content sink.
fn split_next_step(content: &str) -> (String, Option<String>) {
    let trimmed = content.trim();
    let mut parts = trimmed.splitn(2, '\n');
    let first = parts.next().unwrap_or("").trim();
    let rest = parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let title: String = first.chars().take(200).collect();
    let body = rest.map(|value| value.chars().take(2000).collect());
    (title, body)
}

/// `sha256:<hex>` of the sanitized candidate content — never raw source bytes.
fn content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("sha256:{hex}")
}

fn emit_result(
    app: &AppHandle,
    result: CurrentContextSuggestionResult,
) -> CurrentContextSuggestionResult {
    if let Err(error) = app.emit(RESULT_EVENT, &result) {
        tracing::warn!(error = %error, "current-context result event emission failed");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::capture::{
        AccessibilitySnapshot, FocusedElementDto, OcrRegionDto, SceneAnalysisResponse,
    };
    use chrono::{Duration as ChronoDuration, TimeZone};
    use maekon_core::models::suggestion::{Priority, SuggestionType};

    fn suggestion(id: &str, content: &str, priority: Priority) -> Suggestion {
        let now = Utc::now();
        Suggestion {
            suggestion_id: id.to_string(),
            suggestion_type: SuggestionType::WorkGuidance,
            content: content.to_string(),
            priority,
            confidence_score: 0.7,
            relevance_score: 0.8,
            is_actionable: true,
            created_at: now,
            expires_at: Some(now + ChronoDuration::hours(4)),
            source: SuggestionSource::LlmLocal,
            reasoning: None,
            context_scope: None,
        }
    }

    #[test]
    fn response_body_uses_terminal_only_without_text_frames() {
        assert_eq!(
            finalize_response_body(false, String::new(), "terminal".to_string()),
            "terminal"
        );
        assert_eq!(
            finalize_response_body(true, "streamed".to_string(), "terminal".to_string()),
            "streamed"
        );
    }

    #[test]
    fn admission_count_and_ids_follow_duplicate_and_full_queue_policy() {
        let mut queue = SuggestionQueue::new(1);
        let first = prepare_suggestions(
            vec![suggestion("one", "Same candidate", Priority::High)],
            SuggestionSource::LlmLocal,
            Some("Editor".to_string()),
            Some("Plan".to_string()),
        );
        let duplicate = prepare_suggestions(
            vec![suggestion("two", "Same candidate", Priority::Low)],
            SuggestionSource::LlmLocal,
            Some("Editor".to_string()),
            Some("Plan".to_string()),
        );
        let low = prepare_suggestions(
            vec![suggestion("three", "Lower candidate", Priority::Low)],
            SuggestionSource::LlmLocal,
            Some("Editor".to_string()),
            Some("Plan".to_string()),
        );

        assert_eq!(admit_suggestions(&mut queue, first).len(), 1);
        assert_eq!(admit_suggestions(&mut queue, duplicate).len(), 0);
        assert_eq!(admit_suggestions(&mut queue, low).len(), 0);
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn prepared_candidate_keeps_only_allowed_scope_and_source_identity() {
        let prepared = prepare_suggestions(
            vec![suggestion("chat-id", "Review the plan", Priority::Medium)],
            SuggestionSource::LlmServer,
            Some("Editor".to_string()),
            None,
        );
        let candidate = &prepared[0];

        assert!(candidate.suggestion_id.starts_with("local-current-scene-"));
        assert_eq!(candidate.source, SuggestionSource::LlmServer);
        assert_eq!(
            candidate
                .context_scope
                .as_ref()
                .and_then(|scope| scope.app_name.as_deref()),
            Some("Editor")
        );
        assert!(candidate
            .context_scope
            .as_ref()
            .and_then(|scope| scope.window_title.as_ref())
            .is_none());
    }

    #[test]
    fn typed_result_contains_metadata_only_provenance() {
        let result = CurrentContextSuggestionResult {
            status: CurrentContextSuggestionStatus::Generated,
            reason: None,
            admitted_count: 1,
            queue_count: 2,
            admitted_suggestion_ids: vec!["local-current-scene-id".to_string()],
            missing_permissions: Vec::new(),
            provenance: Some(CurrentContextProvenanceDto {
                source_id: LOCAL_CURRENT_SCENE_SOURCE_ID,
                observed_at: "2026-07-19T04:50:00Z".to_string(),
                captured_at: "2026-07-19T04:50:01Z".to_string(),
            }),
        };
        let wire = serde_json::to_string(&result).unwrap();

        assert!(wire.contains("\"status\":\"generated\""));
        assert!(wire.contains(LOCAL_CURRENT_SCENE_SOURCE_ID));
        assert!(!wire.contains("window_title"));
        assert!(!wire.contains("ocr_text"));
        assert!(!wire.contains("accessibility_text"));
    }

    #[test]
    fn prompt_fields_follow_independent_live_consent_gates() {
        let observed_at = Utc.with_ymd_and_hms(2026, 7, 19, 5, 0, 0).single().unwrap();
        let snapshot = CurrentSceneSnapshot {
            response: SceneAnalysisResponse {
                app_name: "Editor".to_string(),
                window_title: "Private plan".to_string(),
                timestamp: "2026-07-19T05:00:01Z".to_string(),
                accessibility: Some(AccessibilitySnapshot {
                    focused_element: Some(FocusedElementDto {
                        role: "AXTextArea".to_string(),
                        label: Some("Private label".to_string()),
                        extracted_text: Some("Private accessibility text".to_string()),
                    }),
                    element_count: 1,
                }),
                ocr_regions: vec![OcrRegionDto {
                    text: "Private OCR text".to_string(),
                    x: 0,
                    y: 0,
                    width: 10,
                    height: 10,
                    confidence: 0.9,
                }],
                gui_elements: Vec::new(),
                work_type: Some("ActiveCoding".to_string()),
            },
            observed_at,
        };
        let permissions = ConsentPermissions {
            window_title_collection: true,
            ..ConsentPermissions::default()
        };

        let (prompt, _, app_scope, window_scope) = build_prompt(snapshot, &permissions).unwrap();
        let context: serde_json::Value = serde_json::from_str(&prompt.context_json).unwrap();

        assert!(prompt.has_reviewable_context);
        assert!(context["context"].get("app_name").is_none());
        assert_eq!(context["context"]["window_title"], "Private plan");
        assert!(context["context"].get("accessibility_text").is_none());
        assert!(context["context"].get("ocr_text").is_none());
        assert!(context["context"].get("work_type").is_none());
        assert!(app_scope.is_none());
        assert_eq!(window_scope.as_deref(), Some("Private plan"));
    }

    #[test]
    fn derived_work_type_requires_pattern_learning_consent() {
        let snapshot = CurrentSceneSnapshot {
            response: SceneAnalysisResponse {
                app_name: "Editor".to_string(),
                window_title: String::new(),
                timestamp: "2026-07-19T05:00:01Z".to_string(),
                accessibility: None,
                ocr_regions: Vec::new(),
                gui_elements: Vec::new(),
                work_type: Some("ActiveCoding".to_string()),
            },
            observed_at: Utc.with_ymd_and_hms(2026, 7, 19, 5, 0, 0).single().unwrap(),
        };
        let permissions = ConsentPermissions {
            process_monitoring: true,
            app_usage_analytics: true,
            activity_pattern_learning: true,
            ..ConsentPermissions::default()
        };

        let (prompt, _, _, _) = build_prompt(snapshot, &permissions).unwrap();
        let context: serde_json::Value = serde_json::from_str(&prompt.context_json).unwrap();

        assert_eq!(context["context"]["app_name"], "Editor");
        assert_eq!(context["context"]["work_type"], "ActiveCoding");
    }

    #[test]
    fn local_current_scene_candidate_uses_sanitized_content_only() {
        let provenance = CurrentContextProvenanceDto {
            source_id: LOCAL_CURRENT_SCENE_SOURCE_ID,
            observed_at: "2026-07-19T04:50:00Z".to_string(),
            captured_at: "2026-07-19T04:50:01Z".to_string(),
        };
        let now = Utc
            .with_ymd_and_hms(2026, 7, 19, 4, 50, 2)
            .single()
            .unwrap();
        let candidate = build_local_current_scene_candidate(
            "local-current-scene-abc",
            "Reply to the design review thread\nContext: two open comments",
            &provenance,
            now,
        );

        assert_eq!(candidate.state, CandidateState::Proposed);
        assert_eq!(
            candidate.source_ref.source_kind,
            SourceKind::LocalCurrentScene
        );
        assert_eq!(candidate.source_ref.lifecycle, SourceLifecycle::Active);
        assert_eq!(
            candidate.title.as_deref(),
            Some("Reply to the design review thread")
        );
        assert_eq!(
            candidate.body.as_deref(),
            Some("Context: two open comments")
        );
        assert!(candidate.source_ref.content_hash.starts_with("sha256:"));
        assert!(candidate
            .source_ref
            .dedupe_namespace
            .contains("local-current-scene-abc"));
        assert_eq!(candidate.expires_at, now + ChronoDuration::days(7));

        // Two captures of the same content stay distinct candidates (anchor-less
        // dedupe folds the per-capture suggestion id).
        let other = build_local_current_scene_candidate(
            "local-current-scene-xyz",
            "Reply to the design review thread\nContext: two open comments",
            &provenance,
            now,
        );
        assert_ne!(candidate.dedupe_key, other.dedupe_key);

        // Provenance is metadata only — never the raw scene text.
        let wire = serde_json::to_string(&candidate.source_ref).unwrap();
        assert!(!wire.contains("Reply to the design review"));
    }

    #[test]
    fn empty_context_distinguishes_missing_data_from_missing_consent() {
        assert_eq!(
            empty_context_status(&[]),
            (
                CurrentContextSuggestionStatus::AnalysisUnavailable,
                "current_context_empty"
            )
        );
        assert_eq!(
            empty_context_status(&["ocr_processing"]),
            (
                CurrentContextSuggestionStatus::ConsentRequired,
                "current_context_fields_not_permitted"
            )
        );
    }
}
