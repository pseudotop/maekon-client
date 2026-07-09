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

    /// ADR-026 PR-4: the SQLite calls (`list_frame_file_paths_in_range`,
    /// `delete_data_in_range`) are now async and route through the storage
    /// funnel internally, so the hand-rolled `spawn_blocking` wrapper is removed
    /// and each call is awaited directly. Best-effort frame-file deletion uses
    /// `tokio::fs::remove_file` to keep the worker thread free.
    pub async fn delete_data_range(
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

        let delete_all = request.data_types.is_empty();
        let data_types = request.data_types.clone();
        let frames_dir = self.ctx.frames_dir.clone();
        let storage = self.ctx.storage.clone();

        let mut result = DeleteResult::empty();

        if delete_all || data_types.iter().any(|item| item == "frames") {
            if let Some(ref dir) = frames_dir {
                let paths = storage
                    .list_frame_file_paths_in_range(&window)
                    .await
                    .map_err(|error| ApiError::Internal(error.to_string()))?;

                for path in paths {
                    match resolve_stored_frame_path(dir, &path) {
                        Ok(full_path) => {
                            if let Err(e) = tokio::fs::remove_file(&full_path).await {
                                debug!("remove_file failed: {e}");
                            }
                        }
                        Err(e) => debug!("skip invalid frame path: {e}"),
                    }
                }
            }
        }

        let deleted = storage
            .delete_data_in_range(
                &window,
                delete_all || data_types.iter().any(|item| item == "events"),
                delete_all || data_types.iter().any(|item| item == "frames"),
                delete_all || data_types.iter().any(|item| item == "metrics"),
                delete_all || data_types.iter().any(|item| item == "processes"),
                delete_all || data_types.iter().any(|item| item == "idle"),
            )
            .await
            .map_err(|error| ApiError::Internal(error.to_string()))?;

        result.events_deleted = deleted.events_deleted;
        result.frames_deleted = deleted.frames_deleted;
        result.metrics_deleted = deleted.metrics_deleted;
        result.process_snapshots_deleted = deleted.process_snapshots_deleted;
        result.idle_periods_deleted = deleted.idle_periods_deleted;
        result.message = format!("{} records were deleted", result.total());

        Ok(result)
    }

    /// ADR-026 PR-4: the SQLite calls (`list_all_frame_file_paths`,
    /// `delete_all_data`) are now async and offload internally, so the
    /// hand-rolled `spawn_blocking` wrapper is removed and each call is awaited
    /// directly. `delete_all_data` is the GDPR erase body — its impl runs the
    /// `lock_for_erase` transaction on `spawn_blocking` (it must NOT route
    /// through `write_lock`, which would skip the wipe once `deletion_flag` is
    /// set). Best-effort frame-file deletion uses `tokio::fs::remove_file`.
    pub async fn delete_all_data(&self) -> Result<DeleteResult, ApiError> {
        let frames_dir = self.ctx.frames_dir.clone();
        let storage = self.ctx.storage.clone();

        let outcome: Result<DeleteResult, ApiError> = async {
            let frame_paths = if frames_dir.is_some() {
                storage
                    .list_all_frame_file_paths()
                    .await
                    .map_err(|error| ApiError::Internal(error.to_string()))?
            } else {
                Vec::new()
            };

            // Phase 1: Atomic DB deletion (transaction — all-or-nothing)
            storage
                .delete_all_data()
                .await
                .map_err(|error| ApiError::Internal(error.to_string()))?;

            // Phase 2: Best-effort frame file deletion (after DB commit)
            if let Some(ref dir) = frames_dir {
                for path in frame_paths {
                    match resolve_stored_frame_path(dir, &path) {
                        Ok(file_path) => {
                            if let Err(e) = tokio::fs::remove_file(&file_path).await {
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
        .await;

        // #4478 G3: on a SUCCESSFUL local erasure, ask the SyncEngine to propagate a
        // device-wide DeletionEvent to LAN peers so the erased rows cannot be
        // re-hydrated from a peer. Set only on success (a failed wipe must not
        // propagate); a no-op when sync is disabled (nobody reads the flag).
        if outcome.is_ok() {
            if let Some(ref flag) = self.ctx.erasure_requested {
                flag.store(true, std::sync::atomic::Ordering::Release);
            }
        }
        outcome
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
    use std::sync::Arc;

    // #7738 D-4: funnel through the canonical test-state helper.
    fn test_state() -> AppState {
        crate::test_local_auth::test_app_state_with_event_capacity(8)
    }

    #[tokio::test]
    async fn delete_all_data_sets_erasure_propagation_flag() {
        // #4478 G3: a successful "Delete all data" sets the erasure-propagation
        // signal so the SyncEngine pushes a device-wide DeletionEvent to peers.
        use std::sync::atomic::{AtomicBool, Ordering};
        let flag = Arc::new(AtomicBool::new(false));
        let mut ctx = StorageWebContext::from_state(&test_state());
        ctx.erasure_requested = Some(flag.clone());
        let service = DataCommandService::new(ctx);

        service.delete_all_data().await.expect("delete_all_data");
        assert!(
            flag.load(Ordering::Acquire),
            "erasure_requested must be set after a successful local erasure"
        );
    }

    #[tokio::test]
    async fn delete_data_range_requires_from_and_to() {
        let service = DataCommandService::new(StorageWebContext::from_state(&test_state()));
        let request = DeleteRangeRequest {
            from: String::new(),
            to: String::new(),
            data_types: vec![],
        };

        let result = service.delete_data_range(&request).await;
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
