//! Suggestion statistics and deferred-queue Tauri commands.
//!
//! ADR-013 split from `suggestions/mod.rs`.
//! Commands: `save_suggestion_state`, `get_suggestion_stats`,
//!           `get_suggestion_daily_stats`, `get_deferred_suggestions`.

use tauri::command;

use crate::ipc_error::IpcError;
use crate::runtime_state::{AppState, SuggestionRuntimeState};

use super::dtos::{
    DailyStatDto, DeferredSuggestionDto, SourceStatsDto, SuggestionStatsDto, TypeCountDto,
};
use super::helpers::{source_label, suggestions_not_available};

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

/// Return aggregate statistics from the suggestion history (in-memory).
#[command]
pub async fn get_suggestion_stats(
    state: tauri::State<'_, SuggestionRuntimeState>,
) -> Result<SuggestionStatsDto, IpcError> {
    let mgr = state.manager().ok_or_else(suggestions_not_available)?;
    let history = mgr.history().lock().await;
    let entries = history.recent(10_000);

    let total_shown = entries.len() as u32;
    let mut total_accepted = 0u32;
    let mut total_rejected = 0u32;
    let mut total_deferred = 0u32;
    let mut by_type: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut by_source: std::collections::HashMap<String, (u32, u32, u32)> =
        std::collections::HashMap::new();

    for entry in &entries {
        let type_key = format!("{:?}", entry.suggestion.suggestion_type).to_lowercase();
        let source_key = source_label(&entry.suggestion.source).to_string();

        *by_type.entry(type_key).or_default() += 1;
        let src = by_source.entry(source_key).or_default();
        src.0 += 1;

        match &entry.feedback {
            Some(maekon_core::models::suggestion::FeedbackType::Accepted) => {
                total_accepted += 1;
                src.1 += 1;
            }
            Some(maekon_core::models::suggestion::FeedbackType::Rejected) => {
                total_rejected += 1;
                src.2 += 1;
            }
            Some(maekon_core::models::suggestion::FeedbackType::Deferred) => {
                total_deferred += 1;
            }
            None => {}
        }
    }

    let acceptance_rate = if total_shown > 0 {
        total_accepted as f64 / total_shown as f64
    } else {
        0.0
    };

    let mut by_type_vec: Vec<TypeCountDto> = by_type
        .into_iter()
        .map(|(k, v)| TypeCountDto {
            suggestion_type: k,
            count: v,
        })
        .collect();
    by_type_vec.sort_by(|a, b| b.count.cmp(&a.count));

    let mut by_source_vec: Vec<SourceStatsDto> = by_source
        .into_iter()
        .map(|(k, (count, accepted, rejected))| SourceStatsDto {
            source: k,
            count,
            accepted,
            rejected,
        })
        .collect();
    by_source_vec.sort_by(|a, b| b.count.cmp(&a.count));

    Ok(SuggestionStatsDto {
        total_shown,
        total_accepted,
        total_rejected,
        total_deferred,
        acceptance_rate,
        by_type: by_type_vec,
        by_source: by_source_vec,
    })
}

/// Return daily aggregated suggestion statistics for the last N days (max 90).
#[command]
pub async fn get_suggestion_daily_stats(
    state: tauri::State<'_, SuggestionRuntimeState>,
    days: Option<u32>,
) -> Result<Vec<DailyStatDto>, IpcError> {
    let mgr = state.manager().ok_or_else(suggestions_not_available)?;
    let days = days.unwrap_or(30).min(90) as usize;
    let history = mgr.history().lock().await;
    let entries = history.recent(10_000);

    let mut by_date: std::collections::HashMap<String, DailyStatDto> =
        std::collections::HashMap::new();
    let cutoff = chrono::Utc::now() - chrono::Duration::days(days as i64);

    for entry in entries {
        if entry.suggestion.created_at < cutoff {
            continue;
        }
        let date = entry.suggestion.created_at.format("%Y-%m-%d").to_string();
        let stat = by_date.entry(date.clone()).or_insert(DailyStatDto {
            date,
            shown: 0,
            accepted: 0,
            rejected: 0,
            deferred: 0,
        });
        stat.shown += 1;
        match &entry.feedback {
            Some(maekon_core::models::suggestion::FeedbackType::Accepted) => stat.accepted += 1,
            Some(maekon_core::models::suggestion::FeedbackType::Rejected) => stat.rejected += 1,
            Some(maekon_core::models::suggestion::FeedbackType::Deferred) => stat.deferred += 1,
            None => {}
        }
    }

    let mut result: Vec<DailyStatDto> = by_date.into_values().collect();
    result.sort_by(|a, b| a.date.cmp(&b.date));
    Ok(result)
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
                title: maekon_suggestion::presenter::type_to_title(
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
