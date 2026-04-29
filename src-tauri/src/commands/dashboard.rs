use std::sync::atomic::Ordering;
use tauri::command;

use crate::ipc_error::IpcError;
use crate::runtime_state::AppState;
use oneshim_core::ports::web_storage::DigestStorage;

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

    let digests = state
        .storage
        .list_weekly_digests(limit)
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

    // Check cache first
    if let Some(cached) = state
        .storage
        .get_daily_digest(&date_str)
        .map_err(IpcError::from)?
    {
        return serde_json::to_value(&cached).map_err(IpcError::from);
    }

    // Not cached — generate from segments on-demand
    let segment_records = state
        .storage
        .get_segments_for_date(&date_str)
        .map_err(IpcError::from)?;

    if segment_records.is_empty() {
        return Ok(serde_json::json!(null));
    }

    // Convert SegmentSummaryRecords to SegmentSummary for DailyDigestGenerator
    let segments: Vec<oneshim_core::models::tiered_memory::SegmentSummary> = segment_records
        .iter()
        .filter_map(crate::scheduler::record_to_segment_summary)
        .collect();

    // Load previous day for comparison
    let prev_date = naive_date
        .pred_opt()
        .unwrap_or(naive_date)
        .format("%Y-%m-%d")
        .to_string();
    let prev_digest = state.storage.get_daily_digest(&prev_date).ok().flatten();

    let digest = oneshim_analysis::DailyDigestGenerator::generate(
        &segments,
        naive_date,
        prev_digest.as_ref(),
    );

    // Cache the result
    if let Err(e) = state.storage.save_daily_digest(&digest) {
        tracing::warn!("Failed to cache daily digest: {e}");
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

    if let Some(digest) = state
        .storage
        .get_daily_digest(&date_str)
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
    action: oneshim_core::models::recalibration::UserOverrideAction,
) -> Result<serde_json::Value, IpcError> {
    let entry = oneshim_core::models::recalibration::RegimeOverride {
        override_id: uuid::Uuid::new_v4().to_string(),
        segment_id,
        original_regime_id,
        user_action: action,
        created_at: chrono::Utc::now(),
    };

    let override_id = entry.override_id.clone();

    use oneshim_core::ports::override_store::OverrideStore;
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
    use oneshim_core::ports::override_store::OverrideStore;
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
) -> Result<Vec<oneshim_core::models::recalibration::RegimeOverride>, IpcError> {
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

    use oneshim_core::ports::override_store::OverrideStore;
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
