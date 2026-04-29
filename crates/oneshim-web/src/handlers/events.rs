use axum::extract::{Query, State};
use axum::Json;
use oneshim_api_contracts::events::EventResponse;

use crate::error::ApiError;
use crate::services::events_service::EventsQueryService;
use crate::services::web_contexts::StorageWebContext;

use super::{PaginatedResponse, PaginationMeta, TimeRangeQuery};

/// GET /api/events?from=&to=&limit=&offset=
pub async fn get_events(
    State(context): State<StorageWebContext>,
    Query(params): Query<TimeRangeQuery>,
) -> Result<Json<PaginatedResponse<EventResponse>>, ApiError> {
    let page = EventsQueryService::new(context).get_events(&params).await?;

    Ok(Json(PaginatedResponse {
        data: page.data,
        pagination: PaginationMeta {
            total: page.total,
            offset: page.offset,
            limit: page.limit,
            has_more: page.has_more,
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_response_serializes() {
        let response = EventResponse {
            event_id: "test_123".to_string(),
            event_type: "User".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            app_name: Some("Code".to_string()),
            window_title: Some("main.rs".to_string()),
            data: serde_json::json!({"event_type": "WindowChange"}),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("WindowChange"));
    }
}
