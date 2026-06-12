use crate::updater::{UpdateCheckResult, UpdateError, Updater};
use async_trait::async_trait;
use maekon_api_contracts::update::DownloadProgress;
use std::path::{Path, PathBuf};
use tokio::sync::watch;

#[async_trait]
pub trait UpdateExecutor: Send + Sync {
    fn should_check_for_updates(&self) -> bool;
    async fn check_for_updates(&self) -> Result<UpdateCheckResult, UpdateError>;
    fn save_last_check_time(&self) -> Result<(), UpdateError>;
    async fn download_update(&self, download_url: &str) -> Result<PathBuf, UpdateError>;

    /// Stream download with progress reporting via a watch channel.
    /// Default implementation delegates to `download_update` (no progress).
    async fn download_update_with_progress(
        &self,
        download_url: &str,
        progress_tx: watch::Sender<DownloadProgress>,
    ) -> Result<PathBuf, UpdateError> {
        let result = self.download_update(download_url).await?;
        // Signal completion
        let _ = progress_tx.send(DownloadProgress {
            bytes_downloaded: 0,
            total_bytes: 0,
            percent: 100.0,
        });
        Ok(result)
    }

    fn install_and_restart(&self, downloaded_path: &Path) -> Result<(), UpdateError>;
}

#[async_trait]
impl UpdateExecutor for Updater {
    fn should_check_for_updates(&self) -> bool {
        self.should_check_for_updates()
    }

    async fn check_for_updates(&self) -> Result<UpdateCheckResult, UpdateError> {
        self.check_for_updates().await
    }

    fn save_last_check_time(&self) -> Result<(), UpdateError> {
        self.save_last_check_time()
    }

    async fn download_update(&self, download_url: &str) -> Result<PathBuf, UpdateError> {
        self.download_update(download_url).await
    }

    async fn download_update_with_progress(
        &self,
        download_url: &str,
        progress_tx: watch::Sender<DownloadProgress>,
    ) -> Result<PathBuf, UpdateError> {
        self.download_update_with_progress(download_url, progress_tx)
            .await
    }

    fn install_and_restart(&self, downloaded_path: &Path) -> Result<(), UpdateError> {
        self.install_and_restart(downloaded_path)
    }
}
