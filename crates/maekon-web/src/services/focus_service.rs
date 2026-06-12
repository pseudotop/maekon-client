use chrono::{Duration, Utc};
use maekon_api_contracts::focus::{
    FocusMetricsResponse, InterruptionDto, LocalSuggestionDto, SuggestionFeedbackRequest,
    WorkSessionDto,
};

use crate::error::ApiError;
use crate::services::focus_assembler::{
    assemble_focus_metrics, assemble_focus_metrics_response, assemble_interruption,
    assemble_local_suggestion, assemble_work_session,
};
use crate::services::web_contexts::StorageWebContext;
use maekon_api_contracts::common::TimeRangeQuery;

#[derive(Clone)]
pub struct FocusQueryService {
    ctx: StorageWebContext,
}

impl FocusQueryService {
    pub fn new(ctx: StorageWebContext) -> Self {
        Self { ctx }
    }

    pub async fn get_focus_metrics(&self) -> Result<FocusMetricsResponse, ApiError> {
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let today_metrics = self
            .ctx
            .storage
            .get_or_create_focus_metrics(&today)
            .await
            .map_err(|error| ApiError::Internal(error.to_string()))?;

        let history = self
            .ctx
            .storage
            .get_recent_focus_metrics(7)
            .await
            .map_err(|error| ApiError::Internal(error.to_string()))?
            .into_iter()
            .filter(|(date, _)| date != &today)
            .map(|(date, metrics)| assemble_focus_metrics(date, metrics))
            .collect();

        Ok(assemble_focus_metrics_response(
            assemble_focus_metrics(today, today_metrics),
            history,
        ))
    }

    pub async fn get_work_sessions(
        &self,
        query: &TimeRangeQuery,
    ) -> Result<Vec<WorkSessionDto>, ApiError> {
        let window = query
            .to_time_window(Duration::hours(24))
            .map_err(|e| ApiError::BadRequest(e.to_string()))?;
        let limit = query.limit_or_default();

        self.ctx
            .storage
            .list_work_sessions(&window.start.to_rfc3339(), &window.end.to_rfc3339(), limit)
            .await
            .map_err(|error| ApiError::Internal(error.to_string()))
            .map(|rows| rows.into_iter().map(assemble_work_session).collect())
    }

    pub async fn get_interruptions(
        &self,
        query: &TimeRangeQuery,
    ) -> Result<Vec<InterruptionDto>, ApiError> {
        let window = query
            .to_time_window(Duration::hours(24))
            .map_err(|e| ApiError::BadRequest(e.to_string()))?;
        let limit = query.limit_or_default();

        self.ctx
            .storage
            .list_interruptions(&window.start.to_rfc3339(), &window.end.to_rfc3339(), limit)
            .await
            .map_err(|error| ApiError::Internal(error.to_string()))
            .map(|rows| rows.into_iter().map(assemble_interruption).collect())
    }

    pub async fn get_suggestions(&self) -> Result<Vec<LocalSuggestionDto>, ApiError> {
        let cutoff = (Utc::now() - Duration::hours(24)).to_rfc3339();

        self.ctx
            .storage
            .list_recent_local_suggestions(&cutoff, 50)
            .await
            .map_err(|error| ApiError::Internal(error.to_string()))
            .map(|rows| rows.into_iter().map(assemble_local_suggestion).collect())
    }
}

#[derive(Clone)]
pub struct FocusCommandService {
    ctx: StorageWebContext,
}

impl FocusCommandService {
    pub fn new(ctx: StorageWebContext) -> Self {
        Self { ctx }
    }

    pub async fn submit_suggestion_feedback(
        &self,
        id: i64,
        request: &SuggestionFeedbackRequest,
    ) -> Result<(), ApiError> {
        match request.action.as_str() {
            "shown" => self
                .ctx
                .storage
                .mark_suggestion_shown(id)
                .await
                .map_err(|error| ApiError::Internal(error.to_string()))?,
            "dismissed" => self
                .ctx
                .storage
                .mark_suggestion_dismissed(id)
                .await
                .map_err(|error| ApiError::Internal(error.to_string()))?,
            "acted" => self
                .ctx
                .storage
                .mark_suggestion_acted(id)
                .await
                .map_err(|error| ApiError::Internal(error.to_string()))?,
            _ => {
                return Err(ApiError::BadRequest(format!(
                    "Invalid suggestion feedback action: {}",
                    request.action
                )));
            }
        }

        Ok(())
    }
}
