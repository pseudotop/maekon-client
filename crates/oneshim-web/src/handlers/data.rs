use axum::{extract::State, Json};
use oneshim_api_contracts::data::{DeleteRangeRequest, DeleteResult};

use crate::{
    error::ApiError,
    services::{data_web_service::DataCommandService, web_contexts::StorageWebContext},
};

pub async fn delete_data_range(
    State(context): State<StorageWebContext>,
    Json(request): Json<DeleteRangeRequest>,
) -> Result<Json<DeleteResult>, ApiError> {
    Ok(Json(
        DataCommandService::new(context).delete_data_range(&request)?,
    ))
}

pub async fn delete_all_data(
    State(context): State<StorageWebContext>,
) -> Result<Json<DeleteResult>, ApiError> {
    Ok(Json(DataCommandService::new(context).delete_all_data()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delete_result_total() {
        let result = DeleteResult {
            success: true,
            events_deleted: 10,
            frames_deleted: 5,
            metrics_deleted: 100,
            process_snapshots_deleted: 20,
            idle_periods_deleted: 3,
            message: String::new(),
        };

        assert_eq!(result.total(), 138);
    }

    #[test]
    fn delete_result_empty() {
        let result = DeleteResult::empty();
        assert!(result.success);
        assert_eq!(result.total(), 0);
    }

    #[test]
    fn delete_range_request_deserializes() {
        let json =
            r#"{"from": "2024-01-01", "to": "2024-01-31", "data_types": ["events", "frames"]}"#;
        let request: DeleteRangeRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.from, "2024-01-01");
        assert_eq!(request.to, "2024-01-31");
        assert_eq!(request.data_types.len(), 2);
    }

    #[test]
    fn delete_range_request_default_data_types() {
        let json = r#"{"from": "2024-01-01", "to": "2024-01-31"}"#;
        let request: DeleteRangeRequest = serde_json::from_str(json).unwrap();
        assert!(request.data_types.is_empty());
    }
}
