use chrono::Duration;
use oneshim_api_contracts::idle::IdlePeriodResponse;

use crate::error::ApiError;
use crate::services::idle_assembler::assemble_idle_period_response;
use crate::services::web_contexts::StorageWebContext;
use oneshim_api_contracts::common::TimeRangeQuery;

#[derive(Clone)]
pub struct IdleQueryService {
    ctx: StorageWebContext,
}

impl IdleQueryService {
    pub fn new(ctx: StorageWebContext) -> Self {
        Self { ctx }
    }

    pub async fn get_idle_periods(
        &self,
        params: &TimeRangeQuery,
    ) -> Result<Vec<IdlePeriodResponse>, ApiError> {
        let window = params
            .to_time_window(Duration::hours(24))
            .map_err(|e| ApiError::BadRequest(e.to_string()))?;

        // get_idle_periods is out of plan scope (still takes DateTime<Utc>): decompose.
        self.ctx
            .storage
            .get_idle_periods(window.start, window.end)
            .await
            .map_err(ApiError::from)
            .map(|periods| {
                periods
                    .into_iter()
                    .map(assemble_idle_period_response)
                    .collect()
            })
    }
}
