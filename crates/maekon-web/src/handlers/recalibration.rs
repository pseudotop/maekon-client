//! Recalibration REST endpoints for user-driven regime correction.
//!
//! - `POST /api/recalibration/override` — create a regime override
//! - `DELETE /api/recalibration/override/:id` — delete an override
//! - `GET /api/recalibration/overrides` — list overrides in a time range
//! - `POST /api/recalibration/recluster` — trigger on-demand re-clustering

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Duration, Utc};
use maekon_api_contracts::recalibration::{CreateOverrideRequest, ListOverridesQuery};
use maekon_core::id_generation::generate_id;
use maekon_core::models::recalibration::{RegimeOverride, UserOverrideAction};

use crate::error::ApiError;
use crate::AppState;

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

fn action_kind(action: &UserOverrideAction) -> &'static str {
    match action {
        UserOverrideAction::MarkAsNoise => "mark_as_noise",
        UserOverrideAction::ReassignRegime { .. } => "reassign_regime",
        UserOverrideAction::MarkAsPersonalTime { .. } => "mark_as_personal_time",
    }
}

async fn audit_override_event(state: &AppState, action_type: &str, details: serde_json::Value) {
    if let Some(audit) = state.automation.audit_logger.as_ref() {
        audit
            .log_event(action_type, "recalibration", &details.to_string())
            .await;
    }
}

/// `POST /api/recalibration/override`
pub async fn create_override(
    State(state): State<AppState>,
    Json(body): Json<CreateOverrideRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let store =
        state.analysis.override_store.as_ref().ok_or_else(|| {
            ApiError::ServiceUnavailable("Override store not configured".to_string())
        })?;

    let entry = RegimeOverride {
        override_id: generate_id("ovr"),
        segment_id: body.segment_id,
        original_regime_id: body.original_regime_id,
        user_action: body.action,
        created_at: Utc::now(),
    };

    store.save_override(&entry).await?;

    audit_override_event(
        &state,
        "recalibration.override.created",
        serde_json::json!({
            "override_id": &entry.override_id,
            "action": action_kind(&entry.user_action),
            "lifecycle": "until_removed",
        }),
    )
    .await;

    Ok(Json(serde_json::json!({
        "ok": true,
        "override_id": entry.override_id,
    })))
}

/// `DELETE /api/recalibration/override/:id`
pub async fn delete_override(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let store =
        state.analysis.override_store.as_ref().ok_or_else(|| {
            ApiError::ServiceUnavailable("Override store not configured".to_string())
        })?;

    store.delete_override(&id).await?;

    audit_override_event(
        &state,
        "recalibration.override.removed",
        serde_json::json!({
            "override_id": &id,
            "lifecycle": "removed_by_user",
        }),
    )
    .await;

    Ok(Json(serde_json::json!({
        "ok": true,
        "deleted_id": id,
    })))
}

/// `GET /api/recalibration/overrides?from=...&to=...`
pub async fn list_overrides(
    State(state): State<AppState>,
    Query(query): Query<ListOverridesQuery>,
) -> Result<Json<Vec<RegimeOverride>>, ApiError> {
    let store =
        state.analysis.override_store.as_ref().ok_or_else(|| {
            ApiError::ServiceUnavailable("Override store not configured".to_string())
        })?;

    let from: DateTime<Utc> = query
        .from
        .as_deref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|| Utc::now() - Duration::days(7));

    let to: DateTime<Utc> = query
        .to
        .as_deref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);

    let overrides = store.list_overrides(from, to).await?;

    Ok(Json(overrides))
}

/// `POST /api/recalibration/recluster`
///
/// Sets the `recluster_requested` flag so the scheduler picks it up on
/// the next cycle. The actual re-clustering is performed asynchronously.
pub async fn trigger_recluster(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let flag =
        state.analysis.recluster_requested.as_ref().ok_or_else(|| {
            ApiError::ServiceUnavailable("Recluster flag not configured".to_string())
        })?;

    flag.store(true, std::sync::atomic::Ordering::Relaxed);

    Ok(Json(serde_json::json!({
        "ok": true,
        "message": "Re-clustering requested. It will run on the next scheduler cycle.",
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ApiError;
    use maekon_automation::audit::{AuditLogAdapter, AuditLogger};
    use maekon_core::ports::audit_log::AuditLogPort;
    use maekon_storage::sqlite::SqliteStorage;
    use std::sync::Arc;

    #[test]
    fn list_overrides_query_defaults() {
        // Verify default parsing when no query params are provided
        let query = ListOverridesQuery {
            from: None,
            to: None,
        };
        // `from` defaults to 7 days ago, `to` defaults to now
        let from: DateTime<Utc> = query
            .from
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|| Utc::now() - Duration::days(7));
        let to: DateTime<Utc> = query
            .to
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);

        assert!(to > from);
        assert!((to - from).num_days() >= 6); // roughly 7 days
    }

    #[test]
    fn list_overrides_query_parses_valid_rfc3339() {
        let query = ListOverridesQuery {
            from: Some("2026-01-01T00:00:00Z".to_string()),
            to: Some("2026-01-02T00:00:00Z".to_string()),
        };
        let from = DateTime::parse_from_rfc3339(query.from.as_deref().unwrap())
            .unwrap()
            .with_timezone(&Utc);
        let to = DateTime::parse_from_rfc3339(query.to.as_deref().unwrap())
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!((to - from).num_hours(), 24);
    }

    #[test]
    fn list_overrides_query_invalid_rfc3339_falls_back() {
        let query = ListOverridesQuery {
            from: Some("not-a-date".to_string()),
            to: Some("also-not".to_string()),
        };
        let from: DateTime<Utc> = query
            .from
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|| Utc::now() - Duration::days(7));
        let to: DateTime<Utc> = query
            .to
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);

        assert!(to > from);
    }

    #[test]
    fn override_store_none_produces_service_unavailable() {
        // Simulate what the handler does when override_store is None
        let store: Option<std::sync::Arc<dyn maekon_core::ports::override_store::OverrideStore>> =
            None;
        let result: Result<
            &std::sync::Arc<dyn maekon_core::ports::override_store::OverrideStore>,
            ApiError,
        > = store.as_ref().ok_or_else(|| {
            ApiError::ServiceUnavailable("Override store not configured".to_string())
        });
        assert!(matches!(
            result.err().unwrap(),
            ApiError::ServiceUnavailable(_)
        ));
    }

    #[test]
    fn recluster_flag_none_produces_service_unavailable() {
        let flag: Option<std::sync::Arc<std::sync::atomic::AtomicBool>> = None;
        let result = flag.as_ref().ok_or_else(|| {
            ApiError::ServiceUnavailable("Recluster flag not configured".to_string())
        });
        assert!(matches!(
            result.err().unwrap(),
            ApiError::ServiceUnavailable(_)
        ));
    }

    #[tokio::test]
    async fn create_and_remove_emit_durable_audit_context() {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
        let (event_tx, _) = tokio::sync::broadcast::channel(4);
        let mut state = AppState::with_core(storage.clone(), event_tx);
        state.analysis.override_store = Some(storage);

        let logger = Arc::new(tokio::sync::RwLock::new(AuditLogger::default()));
        let audit = Arc::new(AuditLogAdapter::new(logger));
        state.automation.audit_logger = Some(audit.clone());

        let Json(created) = create_override(
            State(state.clone()),
            Json(CreateOverrideRequest {
                segment_id: "segment-audit".to_string(),
                original_regime_id: Some("focus".to_string()),
                action: UserOverrideAction::MarkAsNoise,
            }),
        )
        .await
        .unwrap();
        let override_id = created["override_id"].as_str().unwrap().to_string();

        let created_entries = audit
            .entries_by_action_prefix("recalibration.override.created", 10)
            .await;
        assert_eq!(created_entries.len(), 1);
        let created_details = created_entries[0].details.as_deref().unwrap();
        assert!(created_details.contains("until_removed"));
        assert!(created_details.contains("mark_as_noise"));

        let Json(removed) = delete_override(State(state), Path(override_id.clone()))
            .await
            .unwrap();
        assert_eq!(removed["deleted_id"], override_id);
        let removed_entries = audit
            .entries_by_action_prefix("recalibration.override.removed", 10)
            .await;
        assert_eq!(removed_entries.len(), 1);
        let removed_details = removed_entries[0].details.as_deref().unwrap();
        assert!(removed_details.contains("removed_by_user"));
        assert!(removed_details.contains(&override_id));
    }
}
