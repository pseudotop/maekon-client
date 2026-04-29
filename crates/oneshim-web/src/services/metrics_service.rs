use chrono::{Duration, Utc};
use oneshim_api_contracts::metrics::{HourlyMetricsResponse, HourlyQuery, MetricsResponse};

use crate::error::ApiError;
use crate::services::metrics_assembler::{
    assemble_hourly_metrics_response, assemble_metrics_response,
};
use crate::services::web_contexts::StorageWebContext;
use oneshim_api_contracts::common::TimeRangeQuery;

#[derive(Clone)]
pub struct MetricsQueryService {
    ctx: StorageWebContext,
}

impl MetricsQueryService {
    pub fn new(ctx: StorageWebContext) -> Self {
        Self { ctx }
    }

    pub async fn get_metrics(
        &self,
        params: &TimeRangeQuery,
    ) -> Result<Vec<MetricsResponse>, ApiError> {
        let window = params
            .to_time_window(Duration::hours(24))
            .map_err(|e| ApiError::BadRequest(e.to_string()))?;
        let limit = params.limit_or_default();

        // get_metrics is out of plan scope (still takes DateTime<Utc>): decompose.
        self.ctx
            .storage
            .get_metrics(window.start, window.end, limit)
            .await
            .map_err(ApiError::from)
            .map(|metrics| metrics.into_iter().map(assemble_metrics_response).collect())
    }

    pub fn get_hourly_metrics(
        &self,
        params: &HourlyQuery,
    ) -> Result<Vec<HourlyMetricsResponse>, ApiError> {
        let hours = params.hours.unwrap_or(24);
        let now = Utc::now();
        let from = (now - Duration::hours(hours as i64))
            .format("%Y-%m-%dT%H:00:00Z")
            .to_string();

        self.ctx
            .storage
            .list_hourly_metrics_since(&from)
            .map_err(|error| ApiError::Internal(error.to_string()))
            .map(|rows| {
                rows.into_iter()
                    .map(assemble_hourly_metrics_response)
                    .collect()
            })
    }
}
