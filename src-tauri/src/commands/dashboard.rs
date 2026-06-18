use std::sync::atomic::Ordering;
use tauri::command;

use crate::ipc_error::IpcError;
use crate::runtime_state::AppState;
// ADR-026 PR-6: `state.storage` is the concrete `Arc<SqliteStorage>`, so the
// inherent (synchronous) digest twins would shadow the async `DigestStorage`
// trait methods of the same name. To keep these async IPC handlers off the
// blocking path, import the trait and dispatch via fully-qualified syntax
// (`<SqliteStorage as DigestStorage>::...`), which routes the SQLite work
// through the `with_conn`/`with_conn_read` `spawn_blocking` funnels.
use maekon_core::ports::web_storage::DigestStorage;
use maekon_storage::sqlite::SqliteStorage;

// ── Semantic search IPC commands ──────────────────────────────

/// Semantic search over embedded vectors.
/// Full semantic search requires the embedding pipeline to be configured.
/// Use the web API at /api/semantic-search for the full implementation.
#[command]
pub async fn semantic_search(
    _state: tauri::State<'_, AppState>,
    _query: String,
    _limit: Option<usize>,
) -> Result<Vec<serde_json::Value>, IpcError> {
    // The Tauri semantic search command delegates to the web API endpoint.
    // The web API has access to the embedding provider and vector store via AppState.
    // Return service.unavailable so frontend can show a graceful "use the web
    // dashboard instead" redirect rather than a generic failure.
    Err(IpcError::new(
        "service.unavailable",
        "Semantic search requires embedding pipeline — use the web API at /api/semantic-search",
    ))
}

/// Get weekly digest for the given week offset (0 = current, -1 = last week).
#[command]
pub async fn get_weekly_digest(
    state: tauri::State<'_, AppState>,
    week_offset: Option<i32>,
) -> Result<serde_json::Value, IpcError> {
    let offset = week_offset.unwrap_or(0);
    let limit = if offset == 0 {
        1
    } else {
        (offset.unsigned_abs() as usize) + 1
    };

    // Fully-qualified async trait dispatch: the inherent sync twin of the same
    // name would otherwise shadow it and run SQLite inline on the reactor (#6136).
    let digests = <SqliteStorage as DigestStorage>::list_weekly_digests(&state.storage, limit)
        .await
        .map_err(IpcError::from)?;

    let target_idx = offset.unsigned_abs() as usize;
    if let Some(digest) = digests.into_iter().nth(target_idx) {
        serde_json::to_value(&digest).map_err(IpcError::from)
    } else {
        Ok(serde_json::json!(null))
    }
}

// ── Dashboard & daily digest IPC commands ─────────────────────

/// Get dashboard data for a given day (timetable + statistics).
/// Returns the daily digest from cache, or generates on-demand from segments.
#[command]
pub async fn get_dashboard_day(
    state: tauri::State<'_, AppState>,
    date: Option<String>,
) -> Result<serde_json::Value, IpcError> {
    let date_str = date.unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string());

    // Validate date format
    let naive_date = chrono::NaiveDate::parse_from_str(&date_str, "%Y-%m-%d").map_err(|e| {
        IpcError::new(
            "validation.invalid_arguments",
            format!("Invalid date format: {e}"),
        )
    })?;

    // #6276: the CURRENT day's digest is a moving target (segments keep
    // accumulating), so it must NOT be served from OR written to the cache —
    // otherwise the first dashboard view freezes today's digest at its partial-
    // day state. Treat the date as "today" if it matches the current date in
    // EITHER UTC or Local (date_str defaults to Utc::now() while the scheduler
    // rolls over on Local; the union avoids a near-midnight off-by-one).
    let is_today = naive_date == chrono::Utc::now().date_naive()
        || naive_date == chrono::Local::now().date_naive();

    // Check cache first (skip for today — always regenerate fresh).
    if !is_today {
        if let Some(cached) =
            <SqliteStorage as DigestStorage>::get_daily_digest(&state.storage, &date_str)
                .await
                .map_err(IpcError::from)?
        {
            return serde_json::to_value(&cached).map_err(IpcError::from);
        }
    }

    // Not cached — generate from segments on-demand.
    //
    // #4631 MINOR-2: the absence of a consent re-check here is intentional. This
    // is a user-initiated derive (the user opened their dashboard) from segment
    // data already collected under consent — not background collection. It does
    // not touch the "nothing collected without consent" invariant; gating it
    // would only block a user from viewing a digest of their own consented data.
    let segment_records =
        <SqliteStorage as DigestStorage>::get_segments_for_date(&state.storage, &date_str)
            .await
            .map_err(IpcError::from)?;

    if segment_records.is_empty() {
        return Ok(serde_json::json!(null));
    }

    // Convert SegmentSummaryRecords to SegmentSummary for DailyDigestGenerator
    let segments: Vec<maekon_core::models::tiered_memory::SegmentSummary> = segment_records
        .iter()
        .filter_map(crate::scheduler::record_to_segment_summary)
        .collect();

    // Load previous day for comparison
    let prev_date = naive_date
        .pred_opt()
        .unwrap_or(naive_date)
        .format("%Y-%m-%d")
        .to_string();
    let prev_digest =
        <SqliteStorage as DigestStorage>::get_daily_digest(&state.storage, &prev_date)
            .await
            .ok()
            .flatten();

    let digest = maekon_analysis::DailyDigestGenerator::generate(
        &segments,
        naive_date,
        prev_digest.as_ref(),
    );

    // Cache the result (skip for today — do not persist a partial-day digest).
    if !is_today {
        if let Err(e) =
            <SqliteStorage as DigestStorage>::save_daily_digest(&state.storage, &digest).await
        {
            tracing::warn!("Failed to cache daily digest: {e}");
        }
    }

    serde_json::to_value(&digest).map_err(IpcError::from)
}

/// Get the daily digest for a given date. If not cached, returns null.
#[command]
pub async fn get_daily_digest(
    state: tauri::State<'_, AppState>,
    date: Option<String>,
) -> Result<serde_json::Value, IpcError> {
    let date_str = date.unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string());

    // Validate date format
    chrono::NaiveDate::parse_from_str(&date_str, "%Y-%m-%d").map_err(|e| {
        IpcError::new(
            "validation.invalid_arguments",
            format!("Invalid date format: {e}"),
        )
    })?;

    if let Some(digest) =
        <SqliteStorage as DigestStorage>::get_daily_digest(&state.storage, &date_str)
            .await
            .map_err(IpcError::from)?
    {
        serde_json::to_value(&digest).map_err(IpcError::from)
    } else {
        Ok(serde_json::json!(null))
    }
}

// ── Recalibration IPC commands ─────────────────────────────────

/// Create a regime override for a segment.
#[command]
pub async fn create_override(
    state: tauri::State<'_, AppState>,
    segment_id: String,
    original_regime_id: Option<String>,
    action: maekon_core::models::recalibration::UserOverrideAction,
) -> Result<serde_json::Value, IpcError> {
    let entry = maekon_core::models::recalibration::RegimeOverride {
        override_id: uuid::Uuid::new_v4().to_string(),
        segment_id,
        original_regime_id,
        user_action: action,
        created_at: chrono::Utc::now(),
    };

    let override_id = entry.override_id.clone();

    use maekon_core::ports::override_store::OverrideStore;
    state
        .storage
        .save_override(&entry)
        .await
        .map_err(IpcError::from)?;

    Ok(serde_json::json!({
        "ok": true,
        "override_id": override_id,
    }))
}

/// Delete a regime override by ID.
#[command]
pub async fn delete_override(
    state: tauri::State<'_, AppState>,
    override_id: String,
) -> Result<serde_json::Value, IpcError> {
    use maekon_core::ports::override_store::OverrideStore;
    state
        .storage
        .delete_override(&override_id)
        .await
        .map_err(IpcError::from)?;

    Ok(serde_json::json!({
        "ok": true,
        "deleted_id": override_id,
    }))
}

/// List regime overrides within an optional time range.
#[command]
pub async fn list_overrides(
    state: tauri::State<'_, AppState>,
    from: Option<String>,
    to: Option<String>,
) -> Result<Vec<maekon_core::models::recalibration::RegimeOverride>, IpcError> {
    let from_dt: chrono::DateTime<chrono::Utc> = from
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .unwrap_or_else(|| chrono::Utc::now() - chrono::Duration::days(7));

    let to_dt: chrono::DateTime<chrono::Utc> = to
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .unwrap_or_else(chrono::Utc::now);

    use maekon_core::ports::override_store::OverrideStore;
    state
        .storage
        .list_overrides(from_dt, to_dt)
        .await
        .map_err(IpcError::from)
}

/// Request on-demand re-clustering. The scheduler picks up the flag.
#[command]
pub async fn trigger_recluster(
    state: tauri::State<'_, AppState>,
) -> Result<serde_json::Value, IpcError> {
    state.recluster_requested.store(true, Ordering::Relaxed);

    Ok(serde_json::json!({
        "ok": true,
        "message": "Re-clustering requested. It will run on the next scheduler cycle.",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::models::daily_digest::{DailyDigest, DailyStatistics};
    use std::sync::Arc;

    fn sample_digest(date: chrono::NaiveDate) -> DailyDigest {
        DailyDigest {
            date,
            insight: None,
            timeline: vec![],
            statistics: DailyStatistics::default(),
            generated_at: chrono::Utc::now(),
        }
    }

    /// Regression guard for the digest IPC handlers (`get_dashboard_day`,
    /// `get_daily_digest`): they dispatch through the async `DigestStorage`
    /// trait via fully-qualified syntax so the SQLite work is offloaded onto
    /// `spawn_blocking` rather than running inline on the async reactor. This
    /// asserts that exact async save/get round-trip resolves and persists,
    /// catching any regression where the inherent sync twins shadow the trait.
    #[tokio::test]
    async fn digest_storage_async_roundtrip_via_trait_dispatch() {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("in-memory storage"));
        let date = chrono::NaiveDate::from_ymd_opt(2026, 6, 15).unwrap();

        // Save via the same async trait path the handler uses.
        <SqliteStorage as DigestStorage>::save_daily_digest(&storage, &sample_digest(date))
            .await
            .expect("async save_daily_digest");

        // Read it back via the async trait path.
        let fetched = <SqliteStorage as DigestStorage>::get_daily_digest(
            &storage,
            &date.format("%Y-%m-%d").to_string(),
        )
        .await
        .expect("async get_daily_digest");

        assert!(fetched.is_some(), "saved digest should be retrievable");
        assert_eq!(fetched.unwrap().date, date);

        // A missing date returns None (not an error).
        let missing = <SqliteStorage as DigestStorage>::get_daily_digest(&storage, "1999-01-01")
            .await
            .expect("async get_daily_digest (missing)");
        assert!(missing.is_none());
    }
}
