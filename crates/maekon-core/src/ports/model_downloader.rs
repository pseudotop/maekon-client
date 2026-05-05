//! Port for downloading and managing Whisper STT model files.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::config::WhisperModelSize;
use crate::error::CoreError;
use crate::models::audio::{DownloadProgress, ModelDownloadStatus};

/// # Errors
/// - HTTP-layer failures follow the canonical semantic status mapping
///   (`auth.failed` / `not_found.resource_missing` for 404 /
///   `network.timeout` / `network.rate_limit` / `service.unavailable`).
///   See `docs/guides/http-status-error-mapping.md`.
/// - `CoreError::AudioCapture` (wire: `audio.capture_failed`) for ALL
///   filesystem failures across `download` AND `delete_model` (tempdir
///   creation, chunk write, rename to final path, final-file delete).
///   Also emitted on user cancellation. This is a deliberate grouping
///   decision: every filesystem fault that blocks the Whisper audio
///   feature surfaces under a single wire code so the frontend can
///   show a unified "audio unavailable" recovery path.
/// - reqwest-level timeouts (pre-HTTP-status) route to
///   `CoreError::RequestTimeout` per iter-90 canonical split.
#[async_trait]
pub trait ModelDownloader: Send + Sync {
    /// Start downloading a Whisper model. Sends progress to `progress_tx`.
    /// Checks `cancelled` between chunks — cleans up `.part` file on cancellation.
    async fn download(
        &self,
        model: WhisperModelSize,
        dest_dir: &Path,
        progress_tx: mpsc::UnboundedSender<DownloadProgress>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<PathBuf, CoreError>;

    /// Check if a model file exists and return its status. Fast (file metadata only).
    fn model_status(&self, model: WhisperModelSize, dest_dir: &Path) -> ModelDownloadStatus;

    /// Delete a downloaded model file.
    fn delete_model(&self, model: WhisperModelSize, dest_dir: &Path) -> Result<(), CoreError>;
}
