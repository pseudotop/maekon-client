//! Port for persisting and managing captured frame images on disk.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};

use crate::error::CoreError;

/// Port for persisting captured frame images to storage.
///
/// Implemented by `FrameFileStorage` in `oneshim-storage`.
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

    /// Delete frames older than the configured retention period.
    /// Returns the number of deleted files.
    async fn enforce_retention(&self) -> Result<usize, CoreError>;

    /// Delete oldest frames to stay within storage size limits.
    /// Returns the number of deleted files.
    async fn enforce_storage_limit(&self) -> Result<usize, CoreError>;
}
