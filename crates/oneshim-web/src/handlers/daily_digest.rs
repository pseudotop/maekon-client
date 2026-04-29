use axum::extract::{Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use tracing::debug;

use oneshim_core::models::daily_digest::{DailyDigest, DigestExporter};

use crate::error::ApiError;
use crate::services::dashboard_service;
use oneshim_api_contracts::dashboard::DashboardDayQuery;

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
    let digest =
        dashboard_service::get_or_generate_digest(&state, &today, date).map_err(ApiError::from)?;

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
        .map_err(ApiError::from)?;

    let markdown = DigestExporter::to_markdown(&digest);
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
}
