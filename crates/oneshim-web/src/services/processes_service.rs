use chrono::Duration;
use oneshim_api_contracts::processes::ProcessSnapshotResponse;

use crate::error::ApiError;
use crate::services::processes_assembler::assemble_process_snapshot_response;
use crate::services::web_contexts::StorageWebContext;
use oneshim_api_contracts::common::TimeRangeQuery;

#[derive(Clone)]
pub struct ProcessesQueryService {
    ctx: StorageWebContext,
}

impl ProcessesQueryService {
    pub fn new(ctx: StorageWebContext) -> Self {
        Self { ctx }
    }

    pub async fn get_processes(
        &self,
        params: &TimeRangeQuery,
    ) -> Result<Vec<ProcessSnapshotResponse>, ApiError> {
        let window = params
            .to_time_window(Duration::hours(24))
            .map_err(|e| ApiError::BadRequest(e.to_string()))?;
        let limit = params.limit_or_default();

        // get_process_snapshots is out of plan scope (still takes DateTime<Utc>): decompose.
        self.ctx
            .storage
            .get_process_snapshots(window.start, window.end, limit)
            .await
            .map_err(ApiError::from)
            .map(|snapshots| {
                snapshots
                    .into_iter()
                    .map(assemble_process_snapshot_response)
                    .collect()
            })
    }
}
