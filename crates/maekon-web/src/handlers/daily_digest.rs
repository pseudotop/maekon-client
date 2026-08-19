use axum::extract::{Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use tracing::debug;

use maekon_core::models::daily_digest::{DailyDigest, DigestExporter};
use maekon_core::models::memory_graph::ClaimStatus;

use crate::error::ApiError;
use crate::services::dashboard_service;
use maekon_api_contracts::dashboard::DashboardDayQuery;

use crate::AppState;

/// GET /api/digests/daily?date=YYYY-MM-DD — returns a daily digest.
pub async fn get_daily_digest(
    State(state): State<AppState>,
    Query(params): Query<DashboardDayQuery>,
) -> Result<Json<DailyDigest>, ApiError> {
    let date_str = params
        .date
        .unwrap_or_else(|| Utc::now().format("%Y-%m-%d").to_string());
    debug!("GET /api/digests/daily date={}", date_str);

    let date = chrono::NaiveDate::parse_from_str(&date_str, "%Y-%m-%d")
        .map_err(|e| ApiError::BadRequest(format!("Invalid date format: {e}")))?;

    // Iter-96: CoreError → ApiError via semantic From impl (preserves wire code).
    let digest = dashboard_service::get_or_generate_digest(&state, &date_str, date)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(digest))
}

/// GET /api/digests/daily/today — shortcut for today's daily digest.
pub async fn get_daily_digest_today(
    State(state): State<AppState>,
) -> Result<Json<DailyDigest>, ApiError> {
    let today = Utc::now().format("%Y-%m-%d").to_string();
    debug!("GET /api/digests/daily/today ({})", today);

    let date = chrono::NaiveDate::parse_from_str(&today, "%Y-%m-%d")
        .map_err(|e| ApiError::BadRequest(format!("Invalid date format: {e}")))?;

    // Iter-96: CoreError → ApiError via semantic From impl.
    let digest = dashboard_service::get_or_generate_digest(&state, &today, date)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(digest))
}

/// GET /api/digests/daily/export?date=YYYY-MM-DD&format=markdown — download digest as Markdown.
pub async fn export_daily_digest(
    State(state): State<AppState>,
    Query(params): Query<DashboardDayQuery>,
) -> Result<Response, ApiError> {
    let date_str = params
        .date
        .unwrap_or_else(|| Utc::now().format("%Y-%m-%d").to_string());
    debug!("GET /api/digests/daily/export date={}", date_str);

    let date = chrono::NaiveDate::parse_from_str(&date_str, "%Y-%m-%d")
        .map_err(|e| ApiError::BadRequest(format!("Invalid date format: {e}")))?;

    // Iter-96: CoreError → ApiError via semantic From impl (preserves wire code).
    let digest = dashboard_service::get_or_generate_digest(&state, &date_str, date)
        .await
        .map_err(ApiError::from)?;

    // ADR-023: when the memory graph is wired, append the accumulated claims
    // (the local second-brain view); otherwise render the plain digest.
    let markdown = match state.core.memory_graph.as_ref() {
        Some(mg) => {
            let claims = mg
                .list_claims_by_status(ClaimStatus::Active)
                .await
                .unwrap_or_default();
            DigestExporter::to_markdown_with_claims(&digest, &claims)
        }
        None => DigestExporter::to_markdown(&digest),
    };
    let filename = format!("daily-digest-{date_str}.md");

    Ok((
        [
            (header::CONTENT_TYPE, "text/markdown; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                &format!("attachment; filename=\"{filename}\""),
            ),
        ],
        markdown,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use maekon_core::models::memory_graph::{ClaimKind, MemoryClaim};
    use maekon_core::ports::memory_graph_port::MemoryGraphPort;

    use crate::services::memory_claims_service;

    use super::*;

    #[test]
    fn daily_digest_query_defaults() {
        let json = r#"{}"#;
        let query: DashboardDayQuery = serde_json::from_str(json).unwrap();
        assert!(query.date.is_none());
    }

    #[test]
    fn daily_digest_query_with_date() {
        let json = r#"{"date": "2026-03-18"}"#;
        let query: DashboardDayQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.date.as_deref(), Some("2026-03-18"));
    }

    fn active_claim(id: &str, text: &str) -> MemoryClaim {
        MemoryClaim {
            claim_id: id.to_string(),
            kind: ClaimKind::Reflective,
            text: text.to_string(),
            source: "digest_highlight".to_string(),
            confidence: 0.8,
            status: ClaimStatus::Active,
            created_at: 1_000,
            updated_at: 1_000,
        }
    }

    async fn export_markdown(state: AppState, date: &str) -> String {
        let response = export_daily_digest(
            State(state),
            Query(DashboardDayQuery {
                date: Some(date.to_string()),
            }),
        )
        .await
        .expect("export should succeed");
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        String::from_utf8(bytes.to_vec()).expect("utf-8 markdown")
    }

    /// Decision #3 regression: a retracted claim must disappear from the daily
    /// digest's "Accumulated Claims" appendix — the one pre-existing claims
    /// read surface. Retraction flips the claim to `Retracted`, so the
    /// appendix's `list_claims_by_status(Active)` no longer surfaces it. Drives
    /// the REAL `export_daily_digest` handler + real `SqliteStorage` + real
    /// retraction service end-to-end.
    #[tokio::test]
    async fn retracted_claim_disappears_from_digest_appendix() {
        const DATE: &str = "2026-01-02"; // a past date (cacheable, not "today")
        let (mut state, storage) = crate::test_local_auth::test_app_state_with_storage();
        let mg: Arc<dyn MemoryGraphPort> = storage.clone();
        mg.save_claim(&active_claim("clm_keep", "morning deep-work blocks"))
            .await
            .unwrap();
        mg.save_claim(&active_claim("clm_drop", "afternoon email triage"))
            .await
            .unwrap();
        state.core.memory_graph = Some(mg.clone());

        // Before retraction: both claims appear in the appendix.
        let before = export_markdown(state.clone(), DATE).await;
        assert!(before.contains("## Accumulated Claims"));
        assert!(before.contains("morning deep-work blocks"));
        assert!(before.contains("afternoon email triage"));

        // Retract one claim through the real service.
        let outcome = memory_claims_service::retract_claim(&mg, "clm_drop", 5_000)
            .await
            .unwrap()
            .expect("claim exists");
        assert!(!outcome.already_retracted);

        // After retraction: the retracted claim is gone; the other remains.
        let after = export_markdown(state, DATE).await;
        assert!(after.contains("morning deep-work blocks"));
        assert!(
            !after.contains("afternoon email triage"),
            "retracted claim must not appear in the digest appendix"
        );
    }
}
