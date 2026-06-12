use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::annotation::FrameAnnotation;

/// Async storage port for frame annotations (highlights, memos, arrows).
///
/// Converted to `#[async_trait]` async per ADR-026 PR-3 (async storage
/// convergence). The impl routes every SQLite operation through the
/// `with_conn`/`with_conn_read` funnel (`spawn_blocking`), so the
/// single-connection `parking_lot` guard is acquired on a blocking-pool
/// thread and never held across an `.await` (#4928 erase-barrier preserved).
///
/// # Errors
/// `CoreError::Storage` (wire: `storage.failed`) for all SQLite operations
/// (iter-47 mass fix pattern: query/execute/commit/prepare). Annotation
/// not found is expressed as `Ok(None)` / `Ok(Vec::new())`, not an Err
/// variant.
#[async_trait]
pub trait AnnotationStorage: Send + Sync {
    /// List all annotations for a given frame.
    async fn list_annotations(&self, frame_id: i64) -> Result<Vec<FrameAnnotation>, CoreError>;

    /// Persist a new annotation.
    async fn save_annotation(&self, annotation: &FrameAnnotation) -> Result<(), CoreError>;

    /// Delete an annotation by ID. Returns `Ok(())` even if the ID does not exist.
    async fn delete_annotation(&self, annotation_id: &str) -> Result<(), CoreError>;
}
