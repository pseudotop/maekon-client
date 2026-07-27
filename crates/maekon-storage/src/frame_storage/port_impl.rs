use super::fs::FrameFileStorage;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use maekon_core::error::CoreError;
use maekon_core::ports::frame_storage::FrameStoragePort;
use std::path::{Path, PathBuf};

#[async_trait]
impl FrameStoragePort for FrameFileStorage {
    async fn save_frame(
        &self,
        timestamp: DateTime<Utc>,
        data: &[u8],
    ) -> Result<PathBuf, CoreError> {
        self.save_frame(timestamp, data).await.map_err(Into::into)
    }

    async fn save_frames_batch(
        &self,
        frames: Vec<(DateTime<Utc>, Vec<u8>)>,
    ) -> Vec<Result<PathBuf, CoreError>> {
        self.save_frames_batch(frames)
            .await
            .into_iter()
            .map(|r| r.map_err(Into::into))
            .collect()
    }

    async fn load_frame(&self, relative_path: &Path) -> Result<Vec<u8>, CoreError> {
        self.load_frame(relative_path).await.map_err(Into::into)
    }

    /// Load the most recent frame (decrypting at rest when a key is installed).
    ///
    /// Delegates to `FrameFileStorage::load_latest_frame` and converts
    /// `StorageError` into `CoreError`.
    async fn load_latest_frame(&self) -> Result<Option<(Vec<u8>, String)>, CoreError> {
        self.load_latest_frame().await.map_err(Into::into)
    }

    async fn enforce_retention(&self) -> Result<usize, CoreError> {
        self.enforce_retention().await.map_err(Into::into)
    }

    async fn enforce_storage_limit(&self) -> Result<usize, CoreError> {
        self.enforce_storage_limit().await.map_err(Into::into)
    }

    /// GDPR Art. 17: delete all frame files.
    ///
    /// Delegates to `FrameFileStorage::delete_all_files` and converts
    /// `StorageError` into `CoreError`.
    async fn delete_all_frames(&self) -> Result<usize, CoreError> {
        self.delete_all_files().await.map_err(Into::into)
    }
}
