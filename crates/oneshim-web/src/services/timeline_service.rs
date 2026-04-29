use chrono::{DateTime, Duration, Utc};
use oneshim_api_contracts::timeline::{TimelineQuery, TimelineResponse};

use crate::error::ApiError;
#[cfg(test)]
pub(crate) use crate::services::timeline_assembler::{app_to_color, calculate_app_segments};
use crate::services::timeline_assembler::{
    assemble_event_timeline_item, assemble_frame_timeline_item, assemble_idle_timeline_item,
    assemble_session_info, assemble_timeline_response, timeline_item_timestamp,
};
use crate::services::web_contexts::StorageWebContext;

#[derive(Clone)]
pub struct TimelineQueryService {
    ctx: StorageWebContext,
}

impl TimelineQueryService {
    pub fn new(ctx: StorageWebContext) -> Self {
        Self { ctx }
    }

    pub async fn get_timeline(&self, params: &TimelineQuery) -> Result<TimelineResponse, ApiError> {
        // Preserve TimelineQuery's pre-existing 1h lookback (NOT 24h) — its
        // semantic differs from TimeRangeQuery (timeline is short-window UI).
        let window = params
            .to_time_window(Duration::hours(1))
            .map_err(|e| ApiError::BadRequest(e.to_string()))?;
        let max_events = params.max_events();
        let max_frames = params.max_frames();

        self.build_timeline_response(window.start, window.end, max_events, max_frames)
            .await
    }

    async fn build_timeline_response(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        max_events: usize,
        max_frames: usize,
    ) -> Result<TimelineResponse, ApiError> {
        let events = self.ctx.storage.get_events(from, to, max_events).await?;
        let frames = self.ctx.storage.get_frames(from, to, max_frames)?;
        let idle_periods = self.ctx.storage.get_idle_periods(from, to).await?;

        let mut items = Vec::new();
        items.extend(events.iter().map(assemble_event_timeline_item));
        items.extend(frames.iter().map(assemble_frame_timeline_item));
        items.extend(idle_periods.iter().filter_map(assemble_idle_timeline_item));

        items.sort_by(|left, right| {
            timeline_item_timestamp(left).cmp(timeline_item_timestamp(right))
        });
        let segments = crate::services::timeline_assembler::calculate_app_segments(&items);
        let total_idle_secs: i64 = idle_periods
            .iter()
            .filter_map(|period| period.duration_secs.map(|duration| duration as i64))
            .sum();
        let session = assemble_session_info(from, to, events.len(), frames.len(), total_idle_secs);

        Ok(assemble_timeline_response(session, items, segments))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppState;
    use oneshim_storage::sqlite::SqliteStorage;
    use std::sync::Arc;
    use tokio::sync::broadcast;

    fn test_state() -> AppState {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("in-memory sqlite"));
        let (event_tx, _) = broadcast::channel(8);
        AppState::with_core(storage, event_tx)
    }

    fn test_context() -> StorageWebContext {
        StorageWebContext::from_state(&test_state())
    }

    #[tokio::test]
    async fn build_timeline_response_returns_empty_payload_for_empty_store() {
        let from = Utc::now() - chrono::Duration::minutes(30);
        let to = Utc::now();

        let response = TimelineQueryService::new(test_context())
            .build_timeline_response(from, to, 100, 50)
            .await
            .expect("timeline response");

        assert_eq!(response.session.total_events, 0);
        assert_eq!(response.session.total_frames, 0);
        assert_eq!(response.session.total_idle_secs, 0);
        assert!(response.items.is_empty());
        assert!(response.segments.is_empty());
    }
}
