use axum::extract::{Query, State};
use axum::Json;
use oneshim_api_contracts::idle::IdlePeriodResponse;

use crate::error::ApiError;
use crate::services::idle_service::IdleQueryService;
use crate::services::web_contexts::StorageWebContext;

use super::TimeRangeQuery;

/// GET /api/idle?from=&to=
pub async fn get_idle_periods(
    State(context): State<StorageWebContext>,
    Query(params): Query<TimeRangeQuery>,
) -> Result<Json<Vec<IdlePeriodResponse>>, ApiError> {
    Ok(Json(
        IdleQueryService::new(context)
            .get_idle_periods(&params)
            .await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_period_serializes() {
        let period = IdlePeriodResponse {
            start_time: "2024-01-01T12:00:00Z".to_string(),
            end_time: Some("2024-01-01T12:05:00Z".to_string()),
            duration_secs: Some(300),
        };
        let json = serde_json::to_string(&period).unwrap();
        assert!(json.contains("300"));
    }
}
