use axum::extract::{Query, State};
use axum::Json;
#[cfg(test)]
use oneshim_api_contracts::processes::ProcessEntryResponse;
use oneshim_api_contracts::processes::ProcessSnapshotResponse;

use crate::error::ApiError;
use crate::services::processes_service::ProcessesQueryService;
use crate::services::web_contexts::StorageWebContext;

use super::TimeRangeQuery;

/// GET /api/processes?from=&to=&limit=
pub async fn get_processes(
    State(context): State<StorageWebContext>,
    Query(params): Query<TimeRangeQuery>,
) -> Result<Json<Vec<ProcessSnapshotResponse>>, ApiError> {
    Ok(Json(
        ProcessesQueryService::new(context)
            .get_processes(&params)
            .await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_entry_serializes() {
        let entry = ProcessEntryResponse {
            pid: 1234,
            name: "code".to_string(),
            cpu_usage: 10.5,
            memory_bytes: 500_000_000,
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("code"));
    }
}
