use axum::extract::{Query, State};
use axum::Json;
use chrono::{NaiveDate, Utc};
use tracing::debug;

use oneshim_api_contracts::dashboard::DashboardDayQuery;
use oneshim_core::models::daily_digest::DailyDigest;

use crate::error::ApiError;
use crate::services::dashboard_service;
use crate::AppState;

/// GET /api/dashboard/day?date=YYYY-MM-DD — daily timetable + insight.
pub async fn get_dashboard_day(
    State(state): State<AppState>,
    Query(params): Query<DashboardDayQuery>,
) -> Result<Json<DailyDigest>, ApiError> {
    let date_str = params
        .date
        .unwrap_or_else(|| Utc::now().format("%Y-%m-%d").to_string());
    debug!("GET /api/dashboard/day date={}", date_str);

    let date = NaiveDate::parse_from_str(&date_str, "%Y-%m-%d")
        .map_err(|e| ApiError::BadRequest(format!("Invalid date format: {e}")))?;

    // Iter-96: CoreError → ApiError via semantic From impl (preserves wire code).
    let digest = dashboard_service::get_or_generate_digest(&state, &date_str, date)
        .map_err(ApiError::from)?;

    Ok(Json(digest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dashboard_day_query_defaults() {
        let json = r#"{}"#;
        let query: DashboardDayQuery = serde_json::from_str(json).unwrap();
        assert!(query.date.is_none());
    }

    #[test]
    fn dashboard_day_query_with_date() {
        let json = r#"{"date": "2026-03-18"}"#;
        let query: DashboardDayQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.date.as_deref(), Some("2026-03-18"));
    }
}
