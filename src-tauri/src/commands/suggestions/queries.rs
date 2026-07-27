use maekon_core::config::AutomationConfig;
use maekon_storage::sqlite::SqliteStorage;
use tauri::command;

use crate::ipc_error::IpcError;
use crate::runtime_state::{AppState, SuggestionRuntimeState};

use super::mapping::{
    source_label, storage_feedback_label, storage_record_to_view, storage_source_label_pub,
    storage_type_key, suggestion_action_dto, suggestion_action_dto_from_storage,
    suggestion_context_scope_dto_from_domain,
};
use super::types::{
    DailyStatDto, SourceStatsDto, SuggestionHistoryDto, SuggestionStatsDto, SuggestionViewDto,
    TypeCountDto,
};

// ---------------------------------------------------------------------------
// Pending suggestions
// ---------------------------------------------------------------------------

const PENDING_SUGGESTION_FALLBACK_LIMIT: usize = 50;

pub(super) async fn pending_suggestions_snapshot(
    state: &SuggestionRuntimeState,
    storage: &SqliteStorage,
    automation: &AutomationConfig,
) -> Result<Vec<SuggestionViewDto>, IpcError> {
    let Some(mgr) = state.manager() else {
        // Storage fallback (no manager). Enrich each PENDING row with its derived
        // action BEFORE `storage_record_to_view` consumes it (T4.1 #7917).
        return storage
            .list_suggestions(PENDING_SUGGESTION_FALLBACK_LIMIT)
            .map_err(IpcError::from)
            .map(|rows| {
                rows.into_iter()
                    .map(|row| {
                        let action = suggestion_action_dto_from_storage(&row, automation);
                        let mut view = storage_record_to_view(row);
                        view.action = action;
                        view
                    })
                    .collect()
            });
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
                    // Derive the action while the full domain struct is still in
                    // hand — one pure, non-blocking predicate, no lock held over it.
                    suggestion_action_dto(&s.suggestion_type, &s.source, automation),
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
        action,
    ) in snapshot
    {
        results.push(SuggestionViewDto {
            id,
            title,
            body,
            priority,
            category: None,
            source,
            confidence_score,
            created_at,
            reasoning,
            context_scope: suggestion_context_scope_dto_from_domain(context_scope),
            action,
        });
    }
    Ok(results)
}

/// Return only the authoritative pending count without constructing or
/// exposing suggestion content to the caller.
pub(super) async fn pending_suggestion_count_snapshot(
    state: &SuggestionRuntimeState,
    storage: &SqliteStorage,
) -> Result<u32, IpcError> {
    let count = if let Some(mgr) = state.manager() {
        mgr.queue().lock().await.len()
    } else {
        storage
            .list_suggestions(PENDING_SUGGESTION_FALLBACK_LIMIT)
            .map_err(IpcError::from)?
            .len()
    };

    Ok(u32::try_from(count).unwrap_or(u32::MAX))
}

#[command]
pub async fn get_pending_suggestions(
    state: tauri::State<'_, SuggestionRuntimeState>,
    app_state: tauri::State<'_, AppState>,
) -> Result<Vec<SuggestionViewDto>, IpcError> {
    pending_suggestions_snapshot(&state, &app_state.storage, &app_state.config.automation).await
}

#[command]
pub async fn get_pending_suggestion_count(
    state: tauri::State<'_, SuggestionRuntimeState>,
    app_state: tauri::State<'_, AppState>,
) -> Result<u32, IpcError> {
    pending_suggestion_count_snapshot(&state, &app_state.storage).await
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
                    reasoning: entry.suggestion.reasoning.clone(),
                    context_scope: suggestion_context_scope_dto_from_domain(
                        entry.suggestion.context_scope.clone(),
                    ),
                    // History is deliberately UNBOUND: a suggestion the user
                    // already reacted to (or that has aged out of the live queue)
                    // must never offer a one-click run (ADR-027). (T4.1 #7917)
                    action: None,
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
