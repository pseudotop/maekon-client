use maekon_api_contracts::data::{DeleteRangeRequest, DeleteResult};

use crate::error::ApiError;
use crate::services::web_contexts::StorageWebContext;
use std::path::{Path, PathBuf};
use tracing::debug;

#[derive(Clone)]
pub struct DataCommandService {
    ctx: StorageWebContext,
}

impl DataCommandService {
    pub fn new(ctx: StorageWebContext) -> Self {
        Self { ctx }
    }

    pub fn delete_data_range(
        &self,
        request: &DeleteRangeRequest,
    ) -> Result<DeleteResult, ApiError> {
        if request.from.is_empty() || request.to.is_empty() {
            return Err(ApiError::BadRequest(
                "Both `from` and `to` dates are required.".to_string(),
            ));
        }

        let window = request
            .period()
            .map_err(|e| ApiError::BadRequest(e.to_string()))?;

        let mut result = DeleteResult::empty();
        let delete_all = request.data_types.is_empty();
        let data_types = &request.data_types;

        if delete_all || data_types.iter().any(|item| item == "frames") {
            if let Some(ref frames_dir) = self.ctx.frames_dir {
                let paths = self
                    .ctx
                    .storage
                    .list_frame_file_paths_in_range(&window)
                    .map_err(|error| ApiError::Internal(error.to_string()))?;

                for path in paths {
                    match resolve_stored_frame_path(frames_dir, &path) {
                        Ok(full_path) => {
                            if let Err(e) = std::fs::remove_file(&full_path) {
                                debug!("remove_file failed: {e}");
                            }
                        }
                        Err(e) => debug!("skip invalid frame path: {e}"),
                    }
                }
            }
        }

        let deleted = self
            .ctx
            .storage
            .delete_data_in_range(
                &window,
                delete_all || data_types.iter().any(|item| item == "events"),
                delete_all || data_types.iter().any(|item| item == "frames"),
                delete_all || data_types.iter().any(|item| item == "metrics"),
                delete_all || data_types.iter().any(|item| item == "processes"),
                delete_all || data_types.iter().any(|item| item == "idle"),
            )
            .map_err(|error| ApiError::Internal(error.to_string()))?;

        result.events_deleted = deleted.events_deleted;
        result.frames_deleted = deleted.frames_deleted;
        result.metrics_deleted = deleted.metrics_deleted;
        result.process_snapshots_deleted = deleted.process_snapshots_deleted;
        result.idle_periods_deleted = deleted.idle_periods_deleted;
        result.message = format!("{} records were deleted", result.total());

        Ok(result)
    }

    pub fn delete_all_data(&self) -> Result<DeleteResult, ApiError> {
        let frame_paths = if self.ctx.frames_dir.is_some() {
            self.ctx
                .storage
                .list_all_frame_file_paths()
                .map_err(|error| ApiError::Internal(error.to_string()))?
        } else {
            Vec::new()
        };

        // Phase 1: Atomic DB deletion (transaction — all-or-nothing)
        self.ctx
            .storage
            .delete_all_data()
            .map_err(|error| ApiError::Internal(error.to_string()))?;

        // Phase 2: Best-effort frame file deletion (after DB commit)
        if let Some(ref frames_dir) = self.ctx.frames_dir {
            for path in frame_paths {
                match resolve_stored_frame_path(frames_dir, &path) {
                    Ok(file_path) => {
                        if let Err(e) = std::fs::remove_file(&file_path) {
                            debug!("remove_file failed: {e}");
                        }
                    }
                    Err(error) => {
                        debug!("skip frame file deletion: {error}");
                    }
                }
            }
        }

        let mut result = DeleteResult::empty();
        result.message = "All data was deleted".to_string();

        Ok(result)
    }
}

fn resolve_stored_frame_path(frames_dir: &Path, stored_path: &str) -> Result<PathBuf, ApiError> {
    let candidate = Path::new(stored_path);
    if candidate.is_absolute() {
        return Err(ApiError::BadRequest("Invalid frame path".to_string()));
    }

    let joined = frames_dir.join(candidate);
    let canonical = joined
        .canonicalize()
        .map_err(|_| ApiError::NotFound(format!("Frame file not found: {stored_path}")))?;
    let frames_canonical = frames_dir
        .canonicalize()
        .map_err(|_| ApiError::Internal("Frame directory is not available".to_string()))?;

    if !canonical.starts_with(&frames_canonical) {
        return Err(ApiError::BadRequest("Invalid frame path".to_string()));
    }

    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppState;
    use maekon_storage::sqlite::SqliteStorage;
    use std::sync::Arc;
    use tokio::sync::broadcast;

    fn test_state() -> AppState {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("in-memory sqlite"));
        let (event_tx, _) = broadcast::channel(8);
        AppState::with_core(storage, event_tx)
    }

    #[test]
    fn delete_data_range_requires_from_and_to() {
        let service = DataCommandService::new(StorageWebContext::from_state(&test_state()));
        let request = DeleteRangeRequest {
            from: String::new(),
            to: String::new(),
            data_types: vec![],
        };

        let result = service.delete_data_range(&request);
        assert!(matches!(result, Err(ApiError::BadRequest(_))));
    }

    #[test]
    fn resolve_stored_frame_path_rejects_escape() {
        let temp = tempfile::tempdir().expect("tempdir");
        let frames_dir = temp.path().join("frames");
        std::fs::create_dir_all(&frames_dir).expect("frames dir");
        let outside = temp.path().join("outside.png");
        std::fs::write(&outside, b"outside").expect("outside file");

        let result = resolve_stored_frame_path(&frames_dir, "../outside.png");
        assert!(matches!(result, Err(ApiError::BadRequest(_))));
    }

    #[test]
    fn resolve_stored_frame_path_accepts_child_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let frames_dir = temp.path().join("frames");
        std::fs::create_dir_all(frames_dir.join("2026-05-07")).expect("frames dir");
        let child = frames_dir.join("2026-05-07/frame.png");
        std::fs::write(&child, b"frame").expect("frame file");

        let resolved =
            resolve_stored_frame_path(&frames_dir, "2026-05-07/frame.png").expect("resolved");
        assert_eq!(resolved, child.canonicalize().expect("canonical child"));
    }
}
