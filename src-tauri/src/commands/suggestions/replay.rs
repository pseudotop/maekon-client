use tauri::command;

use crate::ipc_error::IpcError;

use super::types::{SuggestionReplayEventAck, SuggestionReplayEventPayload};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SuggestionReplayLogContext {
    pub(super) app_name_present: bool,
    pub(super) window_title_present: bool,
}

pub(super) fn suggestion_replay_log_context(
    payload: &SuggestionReplayEventPayload,
) -> SuggestionReplayLogContext {
    SuggestionReplayLogContext {
        app_name_present: payload
            .app_name
            .as_deref()
            .is_some_and(|value| !value.is_empty()),
        window_title_present: payload
            .window_title
            .as_deref()
            .is_some_and(|value| !value.is_empty()),
    }
}

pub(super) fn validate_suggestion_replay_payload(
    payload: &SuggestionReplayEventPayload,
) -> Result<SuggestionReplayEventAck, IpcError> {
    if payload.raw_context_included {
        return Err(IpcError::new(
            "validation.invalid_arguments",
            "Suggestion replay events must not include raw GUI context",
        ));
    }
    if !payload.audit_ready {
        return Err(IpcError::new(
            "validation.invalid_arguments",
            "Suggestion replay events require an audit-ready metadata envelope",
        ));
    }

    let allowed_phases = ["marker_opened", "proposal_visible", "feedback_submitted"];
    if !allowed_phases.contains(&payload.phase.as_str()) {
        return Err(IpcError::new(
            "validation.invalid_arguments",
            format!("Unknown suggestion replay phase: {}", payload.phase),
        ));
    }
    let expected_event_name = format!("suggestion.replay.{}", payload.phase);
    if payload.event_name != expected_event_name {
        return Err(IpcError::new(
            "validation.invalid_arguments",
            format!(
                "Suggestion replay event_name must be {expected_event_name}, got {}",
                payload.event_name
            ),
        ));
    }

    let allowed_placements = ["adjacent-popover", "window-side-panel", "bottom-dock"];
    if !allowed_placements.contains(&payload.surface_placement.as_str()) {
        return Err(IpcError::new(
            "validation.invalid_arguments",
            format!(
                "Unknown suggestion replay surface placement: {}",
                payload.surface_placement
            ),
        ));
    }

    if let Some(action) = payload.action.as_deref() {
        let allowed_actions = ["accept", "reject", "defer", "explain"];
        if !allowed_actions.contains(&action) {
            return Err(IpcError::new(
                "validation.invalid_arguments",
                format!("Unknown suggestion replay action: {action}"),
            ));
        }
    }

    let subject = payload
        .suggestion_id
        .as_deref()
        .or(payload.target_id.as_deref())
        .unwrap_or("target");
    Ok(SuggestionReplayEventAck {
        trace_id: format!("suggestion-replay-{}-{}", payload.phase, subject),
        recorded: true,
    })
}

#[command]
pub async fn record_suggestion_replay_event(
    payload: SuggestionReplayEventPayload,
) -> Result<SuggestionReplayEventAck, IpcError> {
    let ack = validate_suggestion_replay_payload(&payload)?;
    let log_context = suggestion_replay_log_context(&payload);
    tracing::info!(
        event_name = %payload.event_name,
        phase = %payload.phase,
        suggestion_id = ?payload.suggestion_id,
        target_id = ?payload.target_id,
        surface_placement = %payload.surface_placement,
        app_name_present = log_context.app_name_present,
        window_title_present = log_context.window_title_present,
        action = ?payload.action,
        audit_ready = payload.audit_ready,
        "suggestion replay event recorded"
    );
    Ok(ack)
}
