//! Port for persisting and managing captured frame images on disk.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};

use crate::error::CoreError;

/// Port for persisting captured frame images to storage.
///
/// Implemented by `FrameFileStorage` in `maekon-storage`.
/// Consumers receive `Arc<dyn FrameStoragePort>` via DI.
///
/// Diagnostic methods (`frames_dir`, `buffer_pool_stats`, `disk_status`)
/// remain on the concrete type — they are infrastructure-level concerns
/// that do not belong in the port contract.
///
/// # Errors
/// - `CoreError::Storage` (wire: `storage.failed`) for SQLite
///   index/retention metadata operations (iter-47 mass fix pattern).
/// - `CoreError::AudioCapture` is NOT used — frame save uses
///   `CoreError::Io` (wire: `internal.io`) via `#[from]` for filesystem
///   write failures (ADR-019 §7).
/// - `save_frames_batch` returns per-frame Results; a single failure
///   does not abort the batch — callers inspect each item.
#[async_trait]
pub trait FrameStoragePort: Send + Sync {
    /// Save a single frame image. Returns the relative path of the saved file.
    async fn save_frame(&self, timestamp: DateTime<Utc>, data: &[u8])
        -> Result<PathBuf, CoreError>;

    /// Save multiple frames in a batch. Returns per-frame results.
    async fn save_frames_batch(
        &self,
        frames: Vec<(DateTime<Utc>, Vec<u8>)>,
    ) -> Vec<Result<PathBuf, CoreError>>;

    /// Load a single frame image by relative path.
    async fn load_frame(&self, relative_path: &Path) -> Result<Vec<u8>, CoreError>;

    /// Load the most recently captured frame image (read-only path used by the
    /// automation OCR element-finder). Returns the decoded frame bytes plus the
    /// image format string (e.g. `"webp"`), or `None` when no frame exists.
    ///
    /// A torn/corrupt newest frame is skipped in favour of the next-older good
    /// frame rather than surfacing an error, so a single bad write never blocks
    /// element-finding. Decryption (when the backing store is encrypted at rest)
    /// happens inside this call, so callers receive plaintext image bytes —
    /// this is why automation MUST share the SAME encrypted store instance as
    /// the capture writer instead of building a keyless one over the same dir.
    async fn load_latest_frame(&self) -> Result<Option<(Vec<u8>, String)>, CoreError>;

    /// Delete frames older than the configured retention period.
    /// Returns the number of deleted files.
    async fn enforce_retention(&self) -> Result<usize, CoreError>;

    /// Delete oldest frames to stay within storage size limits.
    /// Returns the number of deleted files.
    async fn enforce_storage_limit(&self) -> Result<usize, CoreError>;

    /// GDPR Art. 17 local data erasure: deletes all frame image files.
    ///
    /// Deletes every date directory under `<base>/frames/`.
    /// Returns the number of deleted files. Returns 0 when there is nothing
    /// to delete.
    ///
    /// # Errors
    /// - `CoreError::Storage` — returned when directory enumeration fails.
    ///   Failure to delete an individual date directory is best-effort (logged
    ///   and skipped, continuing), but the returned count only includes files
    ///   that were actually deleted.
    async fn delete_all_frames(&self) -> Result<usize, CoreError>;
}
