use maekon_storage::sqlite::SqliteStorage;
use tauri::command;

use crate::ipc_error::IpcError;
use crate::runtime_state::{AppState, SuggestionRuntimeState};

use super::helpers::suggestions_not_available;
use super::mapping::{
    source_label, storage_feedback_label, storage_record_to_view, storage_source_label_pub,
    storage_type_key, suggestion_context_scope_dto_from_domain,
};
use super::types::{
    DailyStatDto, DeferredSuggestionDto, SourceStatsDto, SuggestionHistoryDto, SuggestionStatsDto,
    SuggestionViewDto, TypeCountDto,
};

// ---------------------------------------------------------------------------
// Pending suggestions
// ---------------------------------------------------------------------------

pub(super) async fn pending_suggestions_snapshot(
    state: &SuggestionRuntimeState,
    storage: &SqliteStorage,
) -> Result<Vec<SuggestionViewDto>, IpcError> {
    let Some(mgr) = state.manager() else {
        return storage
            .list_suggestions(50)
            .map_err(IpcError::from)
            .map(|rows| rows.into_iter().map(storage_record_to_view).collect());
    };

    let snapshot: Vec<_> = {
        let queue = mgr.queue().lock().await;
        queue
            .iter()
            .map(|s| {
                (
                    s.suggestion_id.clone(),
                    maekon_suggestion::presenter::type_to_title(&s.suggestion_type),
                    s.content.clone(),
                    format!("{:?}", s.priority).to_lowercase(),
                    source_label(&s.source).to_string(),
                    s.confidence_score,
                    s.created_at.to_rfc3339(),
                    s.reasoning.clone(),
                    s.context_scope.clone(),
                )
            })
            .collect()
    };

    let mut results = Vec::with_capacity(snapshot.len());
    for (
        id,
        title,
        body,
        priority,
        source,
        confidence_score,
        created_at,
        reasoning,
        context_scope,
    ) in snapshot
    {
        let is_read = mgr.is_read(&id).await;
        results.push(SuggestionViewDto {
            id,
            title,
            body,
            priority,
            category: None,
            source,
            confidence_score,
            created_at,
            is_read,
            reasoning,
            context_scope: suggestion_context_scope_dto_from_domain(context_scope),
        });
    }
    Ok(results)
}

#[command]
pub async fn get_pending_suggestions(
    state: tauri::State<'_, SuggestionRuntimeState>,
    app_state: tauri::State<'_, AppState>,
) -> Result<Vec<SuggestionViewDto>, IpcError> {
    pending_suggestions_snapshot(&state, &app_state.storage).await
}

// ---------------------------------------------------------------------------
// History — manager-first, SQLite fallback (#5699)
// ---------------------------------------------------------------------------

/// Inner snapshot logic for `get_suggestion_history`.  Exposed as
/// `pub(super)` so the test module can call it directly without a live Tauri
/// runtime.
pub(super) async fn suggestion_history_snapshot(
    state: &SuggestionRuntimeState,
    storage: &SqliteStorage,
    limit: Option<u32>,
) -> Result<Vec<SuggestionHistoryDto>, IpcError> {
    // Manager path — use the in-memory history ring when the manager is live.
    if let Some(mgr) = state.manager() {
        let snapshot: Vec<_> = {
            let history = mgr.history().lock().await;
            history
                .recent(limit.unwrap_or(50) as usize)
                .into_iter()
                .cloned()
                .collect()
        };

        let mut results = Vec::with_capacity(snapshot.len());
        for entry in snapshot {
            let is_read = mgr.is_read(&entry.suggestion.suggestion_id).await;
            let feedback = entry.feedback.as_ref().map(|f| match f {
                maekon_core::models::suggestion::FeedbackType::Accepted => "accepted".to_string(),
                maekon_core::models::suggestion::FeedbackType::Rejected => "rejected".to_string(),
                maekon_core::models::suggestion::FeedbackType::Deferred => "deferred".to_string(),
            });
            results.push(SuggestionHistoryDto {
                suggestion: SuggestionViewDto {
                    id: entry.suggestion.suggestion_id.clone(),
                    title: maekon_suggestion::presenter::type_to_title(
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
                    context_scope: suggestion_context_scope_dto_from_domain(
                        entry.suggestion.context_scope.clone(),
                    ),
                },
                feedback,
            });
        }
        return Ok(results);
    }

    // Fallback — manager not running (standalone / offline).  Read from SQLite
    // ordered by created_at DESC (ledger, no dedup).  (#5699)
    let rows = storage
        .list_recent_suggestions(limit.unwrap_or(50) as usize)
        .map_err(IpcError::from)?;

    let results = rows
        .into_iter()
        .map(|r| {
            let feedback = storage_feedback_label(&r);
            SuggestionHistoryDto {
                suggestion: storage_record_to_view(r),
                feedback,
            }
        })
        .collect();

    Ok(results)
}

#[command]
pub async fn get_suggestion_history(
    state: tauri::State<'_, SuggestionRuntimeState>,
    app_state: tauri::State<'_, AppState>,
    limit: Option<u32>,
) -> Result<Vec<SuggestionHistoryDto>, IpcError> {
    suggestion_history_snapshot(&state, &app_state.storage, limit).await
}

// ---------------------------------------------------------------------------
// save_suggestion_state (manager required — no fallback)
// ---------------------------------------------------------------------------

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
    let queue = mgr.queue().lock().await;
    for suggestion in queue.iter() {
        if let Err(e) = storage.save_suggestion_with_state(suggestion, "pending", None) {
            tracing::warn!(id = %suggestion.suggestion_id, "failed to persist suggestion: {e}");
        } else {
            saved += 1;
        }
    }
    drop(queue);

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

// ---------------------------------------------------------------------------
// Stats fold helper — shared by manager and SQLite fallback paths (#5699)
// ---------------------------------------------------------------------------

/// Fold an iterator of `(type_key, source_key, feedback_label)` tuples into a
/// `SuggestionStatsDto`.  Both the manager path and the SQLite fallback path
/// produce these same three strings and pass them here so the counting logic
/// is not duplicated.
///
/// `total_shown` = row count; `acceptance_rate` = accepted / total as a
/// fraction in `0.0..=1.0` (matches the existing manager path).
fn fold_stats(items: impl Iterator<Item = (String, String, Option<String>)>) -> SuggestionStatsDto {
    let mut total_shown = 0u32;
    let mut total_accepted = 0u32;
    let mut total_rejected = 0u32;
    let mut total_deferred = 0u32;
    let mut by_type: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    // (count, accepted, rejected)
    let mut by_source: std::collections::HashMap<String, (u32, u32, u32)> =
        std::collections::HashMap::new();

    for (type_key, source_key, feedback) in items {
        total_shown += 1;
        *by_type.entry(type_key).or_default() += 1;
        let src = by_source.entry(source_key).or_default();
        src.0 += 1;

        match feedback.as_deref() {
            Some("accepted") => {
                total_accepted += 1;
                src.1 += 1;
            }
            Some("rejected") => {
                total_rejected += 1;
                src.2 += 1;
            }
            Some("deferred") => {
                total_deferred += 1;
            }
            _ => {}
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
    by_type_vec.sort_by_key(|t| std::cmp::Reverse(t.count));

    let mut by_source_vec: Vec<SourceStatsDto> = by_source
        .into_iter()
        .map(|(k, (count, accepted, rejected))| SourceStatsDto {
            source: k,
            count,
            accepted,
            rejected,
        })
        .collect();
    by_source_vec.sort_by_key(|s| std::cmp::Reverse(s.count));

    SuggestionStatsDto {
        total_shown,
        total_accepted,
        total_rejected,
        total_deferred,
        acceptance_rate,
        by_type: by_type_vec,
        by_source: by_source_vec,
    }
}

// ---------------------------------------------------------------------------
// Suggestion stats — manager-first, SQLite fallback (#5699)
// ---------------------------------------------------------------------------

#[command]
pub async fn get_suggestion_stats(
    state: tauri::State<'_, SuggestionRuntimeState>,
    app_state: tauri::State<'_, AppState>,
) -> Result<SuggestionStatsDto, IpcError> {
    if let Some(mgr) = state.manager() {
        let history = mgr.history().lock().await;
        let entries = history.recent(10_000);
        let tuples = entries.iter().map(|entry| {
            let type_key = format!("{:?}", entry.suggestion.suggestion_type).to_lowercase();
            let source_key = source_label(&entry.suggestion.source).to_string();
            let feedback = entry.feedback.as_ref().map(|f| match f {
                maekon_core::models::suggestion::FeedbackType::Accepted => "accepted".to_string(),
                maekon_core::models::suggestion::FeedbackType::Rejected => "rejected".to_string(),
                maekon_core::models::suggestion::FeedbackType::Deferred => "deferred".to_string(),
            });
            (type_key, source_key, feedback)
        });
        return Ok(fold_stats(tuples));
    }

    // Fallback — read up to 10 000 rows from SQLite and fold. (#5699)
    let rows = app_state
        .storage
        .list_recent_suggestions(10_000)
        .map_err(IpcError::from)?;

    let tuples = rows.iter().map(|r| {
        let type_key = storage_type_key(&r.suggestion_type);
        let source_key = storage_source_label_pub(&r.source);
        let feedback = storage_feedback_label(r);
        (type_key, source_key, feedback)
    });

    Ok(fold_stats(tuples))
}

// ---------------------------------------------------------------------------
// Daily stats — manager-first, SQLite fallback (#5699)
// ---------------------------------------------------------------------------

#[command]
pub async fn get_suggestion_daily_stats(
    state: tauri::State<'_, SuggestionRuntimeState>,
    app_state: tauri::State<'_, AppState>,
    days: Option<u32>,
) -> Result<Vec<DailyStatDto>, IpcError> {
    let days = days.unwrap_or(30).min(90) as usize;
    let cutoff = chrono::Utc::now() - chrono::Duration::days(days as i64);

    if let Some(mgr) = state.manager() {
        let history = mgr.history().lock().await;
        let entries = history.recent(10_000);

        let mut by_date: std::collections::HashMap<String, DailyStatDto> =
            std::collections::HashMap::new();

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
        return Ok(result);
    }

    // Fallback — use a Rust fold over SQLite rows.  All writers use
    // `to_rfc3339()` which produces `YYYY-MM-DDTHH:MM:SSZ` so the first 10
    // characters are the date.  Do NOT reuse the dead SQL helper
    // `suggestion_daily_stats` (zero callers, can't fill rejected/deferred). (#5699)
    let rows = app_state
        .storage
        .list_recent_suggestions(10_000)
        .map_err(IpcError::from)?;

    let mut by_date: std::collections::HashMap<String, DailyStatDto> =
        std::collections::HashMap::new();

    for r in rows {
        // Parse the RFC3339 timestamp to check against the cutoff.  On failure
        // treat the row as too old (safe fallback — avoids panic on malformed data).
        let created = chrono::DateTime::parse_from_rfc3339(&r.created_at)
            .ok()
            .map(|dt| dt.with_timezone(&chrono::Utc));
        if let Some(ts) = created {
            if ts < cutoff {
                continue;
            }
        } else {
            continue;
        }

        // The date prefix is always YYYY-MM-DD (positions 0..10 of RFC3339).
        let date = r.created_at[..10].to_string();
        let stat = by_date.entry(date.clone()).or_insert(DailyStatDto {
            date,
            shown: 0,
            accepted: 0,
            rejected: 0,
            deferred: 0,
        });
        stat.shown += 1;
        match storage_feedback_label(&r).as_deref() {
            Some("accepted") => stat.accepted += 1,
            Some("rejected") => stat.rejected += 1,
            Some("deferred") => stat.deferred += 1,
            _ => {}
        }
    }

    let mut result: Vec<DailyStatDto> = by_date.into_values().collect();
    result.sort_by(|a, b| a.date.cmp(&b.date));
    Ok(result)
}

// ---------------------------------------------------------------------------
// Deferred suggestions — manager-first, SQLite fallback (#5699)
// ---------------------------------------------------------------------------

#[command]
pub async fn get_deferred_suggestions(
    state: tauri::State<'_, SuggestionRuntimeState>,
    app_state: tauri::State<'_, AppState>,
) -> Result<Vec<DeferredSuggestionDto>, IpcError> {
    if let Some(mgr) = state.manager() {
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

        return Ok(items);
    }

    // Fallback — query SQLite for state="deferred" rows (same contract as the
    // restore path in suggestion_wiring.rs:246).  Parse resurface_at as RFC3339;
    // on failure warn and skip the row (suggestion_wiring.rs:257-275 precedent).
    // Priority is stored as "MEDIUM" etc. in SQLite — normalise to lowercase to
    // match the manager path. (#5699)
    // #6938: order by SOONEST resurface_at (not created_at DESC) — same sibling as
    // the launch restore (suggestion_wiring.rs). With created_at DESC + LIMIT, a
    // deferred backlog over 50 dropped the snoozes about to resurface; this snooze
    // list must surface those first.
    let rows = app_state
        .storage
        .list_deferred_suggestions_by_resurface(50)
        .map_err(IpcError::from)?;

    let now = chrono::Utc::now();
    let mut items = Vec::with_capacity(rows.len());

    for r in rows {
        let resurface_at = match r
            .resurface_at
            .as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc))
        {
            Some(t) => t,
            None => {
                tracing::warn!(
                    id = %r.suggestion_id,
                    "skipping deferred row: missing or malformed resurface_at"
                );
                continue;
            }
        };

        let remaining = (resurface_at - now).num_minutes().max(0);
        items.push(DeferredSuggestionDto {
            id: r.suggestion_id.clone(),
            title: super::mapping::storage_suggestion_title_pub(&r.suggestion_type),
            body: r.content.clone(),
            // Storage stores 'MEDIUM' / 'HIGH' etc.; normalise to lowercase to
            // match the manager path (#5699).
            priority: r.priority.to_lowercase(),
            source: storage_source_label_pub(&r.source),
            // No dedicated defer-time column — use created_at as an approximation.
            deferred_at: r.created_at.clone(),
            resurface_at: resurface_at.to_rfc3339(),
            remaining_minutes: remaining,
        });
    }

    Ok(items)
}
