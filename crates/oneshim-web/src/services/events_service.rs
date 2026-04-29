use chrono::Duration;
use oneshim_api_contracts::events::EventResponse;

use crate::error::ApiError;
use crate::services::events_assembler::assemble_event_response;
use crate::services::web_contexts::StorageWebContext;
use oneshim_api_contracts::common::TimeRangeQuery;

#[derive(Clone)]
pub struct EventsQueryService {
    ctx: StorageWebContext,
}

pub struct EventPage {
    pub data: Vec<EventResponse>,
    pub total: u64,
    pub offset: usize,
    pub limit: usize,
    pub has_more: bool,
}

impl EventsQueryService {
    pub fn new(ctx: StorageWebContext) -> Self {
        Self { ctx }
    }

    pub async fn get_events(&self, params: &TimeRangeQuery) -> Result<EventPage, ApiError> {
        let window = params
            .to_time_window(Duration::hours(24))
            .map_err(|e| ApiError::BadRequest(e.to_string()))?;
        let limit = params.limit_or_default();
        let offset = params.offset_or_default();

        let total = self
            .ctx
            .storage
            .count_events_in_range(&window)
            .map_err(|error| ApiError::Internal(error.to_string()))?;

        let fetch_limit = limit + offset;
        // get_events is out of plan scope (still takes DateTime<Utc>): decompose.
        let data: Vec<EventResponse> = self
            .ctx
            .storage
            .get_events(window.start, window.end, fetch_limit)
            .await?
            .into_iter()
            .skip(offset)
            .map(assemble_event_response)
            .collect();

        let has_more = (offset + data.len()) < total as usize;

        Ok(EventPage {
            data,
            total,
            offset,
            limit,
            has_more,
        })
    }
}
