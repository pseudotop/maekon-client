//! Suggestion feedback submission and explain-payload lookup.
//!
//! ADR-013 split from `suggestions/helpers.rs`.

use maekon_core::models::suggestion::FeedbackTallyRecord;
use maekon_core::ports::feedback_scorer_store::FeedbackScorerStore;
use maekon_storage::sqlite::SqliteStorage;
use tauri::command;

use crate::ipc_error::IpcError;
use crate::runtime_state::{AppState, SuggestionRuntimeState};

use super::helpers::{enqueue_feedback_retry, feedback_type_for_action, suggestion_not_found};

/// #7913 T2.1c: write-through a FeedbackScorer tally so the learned relevance
/// signal survives restart. Best-effort — the tally has already been applied to
/// the in-RAM scorer, and this state is advisory (it only nudges local
/// relevance), so a persist failure is logged, never surfaced to the user.
fn persist_feedback_tally(storage: &SqliteStorage, record: Option<FeedbackTallyRecord>) {
    if let Some(record) = record {
        if let Err(e) = storage.upsert_feedback_tally(&record) {
            tracing::warn!(error = %e, "feedback scorer tally persist failed (advisory)");
        }
    }
}

pub(crate) async fn submit_suggestion_feedback_to_runtime(
    state: &SuggestionRuntimeState,
    storage: &SqliteStorage,
    suggestion_id: &str,
    action: &str,
    snooze_minutes: Option<u32>,
) -> Result<(), IpcError> {
    let feedback_type = feedback_type_for_action(action)?;

    let Some(mgr) = state.manager() else {
        return submit_storage_suggestion_feedback(storage, suggestion_id, action, snooze_minutes);
    };

    // #7600: attach the live regime_id (from the shared cross-loop snapshot)
    // so RegimeClassifier can attribute this reaction to the regime the user
    // was actually in when they acted, not just an aggregate counter.
    let regime_id = state.current_regime_id();

    // Write the local lifecycle transition before updating the in-memory queue.
    // The live manager is rebuilt from SQLite on restart, so removing an item
    // from RAM alone would resurrect accepted/rejected suggestions. A missing
    // row is allowed for ephemeral manager-only suggestions, while an actual
    // storage error fails closed and leaves the queue untouched.
    match action {
        "accept" => {
            storage
                .mark_unified_suggestion_acted(suggestion_id)
                .map_err(IpcError::from)?;
        }
        "reject" => {
            storage
                .dismiss_unified_suggestion(suggestion_id)
                .map_err(IpcError::from)?;
        }
        "defer" => {}
        _ => unreachable!("action was validated by feedback_type_for_action"),
    }

    // Send feedback to server (best-effort — enqueue for retry on failure)
    match action {
        "accept" => {
            if let Err(_e) = mgr.feedback().accept(suggestion_id, None, regime_id).await {
                enqueue_feedback_retry(&mgr, suggestion_id, feedback_type.clone(), None).await;
            }
        }
        "reject" => {
            if let Err(_e) = mgr.feedback().reject(suggestion_id, None, regime_id).await {
                enqueue_feedback_retry(&mgr, suggestion_id, feedback_type.clone(), None).await;
            }
        }
        "defer" => {
            // Server notification is best-effort; local state changes always proceed.
            if let Err(_e) = mgr.feedback().defer(suggestion_id, None, regime_id).await {
                enqueue_feedback_retry(&mgr, suggestion_id, feedback_type.clone(), None).await;
            }

            let (removed, scorer_data) = {
                let mut queue = mgr.queue().lock().await;
                let scorer_data = queue
                    .iter()
                    .find(|s| s.suggestion_id == suggestion_id)
                    .map(|s| (s.suggestion_type.clone(), s.source.clone()));
                let removed = queue.remove_by_id(suggestion_id);
                (removed, scorer_data)
            }; // queue lock dropped
            if let Some((stype, source)) = scorer_data {
                let tally = {
                    let mut scorer = mgr.scorer().lock().await;
                    scorer.record(stype.clone(), source.clone(), &feedback_type);
                    scorer.tally_record(&stype, &source)
                };
                // #7913 T2.1c: persist the updated tally (write-through).
                persist_feedback_tally(storage, tally);
            }

            if let Some(suggestion) = removed {
                let duration_mins = snooze_minutes.unwrap_or(120);
                let duration = chrono::Duration::minutes(duration_mins as i64);
                let deferred_ok = mgr
                    .deferred()
                    .lock()
                    .await
                    .defer(suggestion.clone(), duration);
                if deferred_ok {
                    let mut history = mgr.history().lock().await;
                    history.add(suggestion);
                    history.record_feedback(suggestion_id, feedback_type);
                } else {
                    // review4 re-verify: the deferred set is full and this snooze
                    // would resurface later than every kept entry, so defer()
                    // rejected it. The suggestion was already removed from the queue
                    // above — do NOT silently drop it (the user would get Ok for a
                    // snooze that vanished). Restore it to the active queue so it
                    // stays actionable.
                    tracing::warn!(
                        id = %suggestion_id,
                        "deferred set full — snooze rejected; restoring suggestion to the active queue"
                    );
                    let _ = mgr.queue().lock().await.push(suggestion);
                }
            }

            let count = mgr.queue().lock().await.len();
            if let Some(overlay) = state.overlay() {
                overlay.emit_suggestions_changed(count);
            }
            return Ok(());
        }
        _ => unreachable!("action was validated by feedback_type_for_action"),
    }

    // Move accepted/rejected suggestion from queue to history.
    // Acquire queue lock once to both remove the item and get the remaining count,
    // avoiding a redundant second lock acquisition.
    let (removed, remaining_count) = {
        let mut queue = mgr.queue().lock().await;
        let removed = queue.remove_by_id(suggestion_id);
        let count = queue.len();
        (removed, count)
    }; // queue lock dropped here

    if let Some(suggestion) = removed {
        let tally = {
            let mut scorer = mgr.scorer().lock().await;
            scorer.record(
                suggestion.suggestion_type.clone(),
                suggestion.source.clone(),
                &feedback_type,
            );
            scorer.tally_record(&suggestion.suggestion_type, &suggestion.source)
        };
        // #7913 T2.1c: persist the updated tally (write-through).
        persist_feedback_tally(storage, tally);
        {
            let mut history = mgr.history().lock().await;
            history.add(suggestion);
            history.record_feedback(suggestion_id, feedback_type);
        }
    }

    // Notify overlay that suggestions changed (item removed from queue)
    if let Some(overlay) = state.overlay() {
        overlay.emit_suggestions_changed(remaining_count);
    }

    Ok(())
}

#[command]
pub async fn submit_suggestion_feedback(
    state: tauri::State<'_, SuggestionRuntimeState>,
    app_state: tauri::State<'_, AppState>,
    suggestion_id: String,
    action: String,
    snooze_minutes: Option<u32>,
) -> Result<(), IpcError> {
    submit_suggestion_feedback_to_runtime(
        &state,
        &app_state.storage,
        &suggestion_id,
        &action,
        snooze_minutes,
    )
    .await
}

fn submit_storage_suggestion_feedback(
    storage: &SqliteStorage,
    suggestion_id: &str,
    action: &str,
    snooze_minutes: Option<u32>,
) -> Result<(), IpcError> {
    match action {
        "accept" => {
            let changed = storage
                .mark_unified_suggestion_acted(suggestion_id)
                .map_err(IpcError::from)?;
            if changed {
                Ok(())
            } else {
                Err(suggestion_not_found(suggestion_id))
            }
        }
        "reject" => {
            let changed = storage
                .dismiss_unified_suggestion(suggestion_id)
                .map_err(IpcError::from)?;
            if changed {
                Ok(())
            } else {
                Err(suggestion_not_found(suggestion_id))
            }
        }
        "defer" => {
            let row = storage
                .list_suggestions(500)
                .map_err(IpcError::from)?
                .into_iter()
                .find(|row| row.suggestion_id == suggestion_id)
                .ok_or_else(|| suggestion_not_found(suggestion_id))?;
            let suggestion = row.try_into_suggestion().ok_or_else(|| {
                IpcError::new(
                    "validation.invalid_arguments",
                    format!("Suggestion cannot be deferred from storage: {suggestion_id}"),
                )
            })?;
            let duration_mins = snooze_minutes.unwrap_or(120);
            let resurface_at =
                (chrono::Utc::now() + chrono::Duration::minutes(duration_mins as i64)).to_rfc3339();
            storage
                .save_suggestion_with_state(&suggestion, "deferred", Some(&resurface_at))
                .map_err(IpcError::from)
        }
        _ => Err(IpcError::new(
            "validation.invalid_arguments",
            format!("Unknown action: {action}. Use accept/reject/defer"),
        )),
    }
}

pub(crate) async fn find_suggestion_explain_payload(
    state: &SuggestionRuntimeState,
    storage: &SqliteStorage,
    suggestion_id: &str,
) -> Result<(String, Option<String>), IpcError> {
    if let Some(suggestion_mgr) = state.manager() {
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

        if let Some(pair) = from_queue {
            return Ok(pair);
        }

        let history = suggestion_mgr.history().lock().await;
        if let Some(entry) = history
            .recent(100)
            .into_iter()
            .find(|entry| entry.suggestion.suggestion_id == suggestion_id)
        {
            return Ok((
                entry.suggestion.content.clone(),
                entry.suggestion.reasoning.clone(),
            ));
        }
    }

    storage
        .list_suggestions(500)
        .map_err(IpcError::from)?
        .into_iter()
        .find(|row| row.suggestion_id == suggestion_id)
        .map(|row| (row.content, row.reasoning))
        .ok_or_else(|| suggestion_not_found(suggestion_id))
}
