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

    /// Delete frames older than the configured retention period.
    /// Returns the number of deleted files.
    async fn enforce_retention(&self) -> Result<usize, CoreError>;

    /// Delete oldest frames to stay within storage size limits.
    /// Returns the number of deleted files.
    async fn enforce_storage_limit(&self) -> Result<usize, CoreError>;

    /// GDPR Art. 17 로컬 데이터 삭제: 모든 프레임 이미지 파일을 삭제한다.
    ///
    /// `<base>/frames/` 하위 모든 날짜 디렉터리를 삭제한다.
    /// 삭제된 파일 수를 반환한다. 삭제할 항목이 없으면 0을 반환한다.
    ///
    /// # Errors
    /// - `CoreError::Storage` — 디렉터리 열거 실패 시 반환한다.
    ///   개별 날짜 디렉터리 삭제 실패는 best-effort (로그만 남기고 계속 진행)이지만,
    ///   반환된 카운트는 실제로 삭제된 파일만 포함한다.
    async fn delete_all_frames(&self) -> Result<usize, CoreError>;
}
