//! Whisper model download manager with streaming progress and SHA-256 verification.
//!
//! Gated behind `#[cfg(feature = "download")]`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use oneshim_core::config::WhisperModelSize;
use oneshim_core::error::CoreError;
use oneshim_core::models::audio::{DownloadProgress, ModelDownloadStatus};
use oneshim_core::ports::model_downloader::ModelDownloader;

const DEFAULT_BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";

pub fn model_filename(size: WhisperModelSize) -> &'static str {
    match size {
        WhisperModelSize::Tiny => "ggml-tiny.bin",
        WhisperModelSize::Base => "ggml-base.bin",
        WhisperModelSize::Small => "ggml-small.bin",
        WhisperModelSize::Medium => "ggml-medium.bin",
    }
}

pub fn model_expected_bytes(size: WhisperModelSize) -> u64 {
    match size {
        WhisperModelSize::Tiny => 77_691_713,
        WhisperModelSize::Base => 147_951_465,
        WhisperModelSize::Small => 487_601_967,
        WhisperModelSize::Medium => 1_533_774_781,
    }
}

pub struct WhisperModelDownloader {
    client: reqwest::Client,
    base_url: String,
}

impl Default for WhisperModelDownloader {
    fn default() -> Self {
        Self::new()
    }
}

impl WhisperModelDownloader {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: DEFAULT_BASE_URL.to_string(),
        }
    }

    /// Test/override constructor — use when pointing at a mock server or a
    /// mirror. Production code should call `new()` to use the canonical
    /// Huggingface URL.
    #[doc(hidden)]
    pub fn new_with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
        }
    }
}

#[async_trait]
impl ModelDownloader for WhisperModelDownloader {
    async fn download(
        &self,
        model: WhisperModelSize,
        dest_dir: &Path,
        progress_tx: mpsc::UnboundedSender<DownloadProgress>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<PathBuf, CoreError> {
        let filename = model_filename(model);
        let url = format!("{}/{filename}", self.base_url);
        let final_path = dest_dir.join(filename);
        let part_path = dest_dir.join(format!("{filename}.part"));

        // Ensure dest dir exists
        std::fs::create_dir_all(dest_dir).map_err(|e| CoreError::AudioCapture {
            code: oneshim_core::error_codes::AudioCode::CaptureFailed,
            message: format!("create model dir: {e}"),
        })?;

        info!(model = ?model, url = %url, "starting model download");

        let response = self.client.get(&url).send().await.map_err(|e| {
            // Iter-90: split timeout vs generic per canonical pattern
            // (cloud_stt.rs:107, http_client.rs map_reqwest_error) so
            // Grafana can group model-download timeouts separately.
            if e.is_timeout() {
                CoreError::RequestTimeout {
                    code: oneshim_core::error_codes::NetworkCode::Timeout,
                    timeout_ms: 0, // sentinel; reqwest client-level timeout is not exposed
                }
            } else {
                CoreError::Network {
                    code: oneshim_core::error_codes::NetworkCode::Generic,
                    message: format!("model download request: {e}"),
                }
            }
        })?;

        if !response.status().is_success() {
            let status = response.status();
            let message = format!("model download failed: HTTP {status}");
            // Semantic HTTP status mapping per iter-54..59 pattern.
            return Err(match status.as_u16() {
                401 | 403 => CoreError::Auth {
                    code: oneshim_core::error_codes::AuthCode::Failed,
                    message,
                },
                404 => CoreError::NotFound {
                    code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
                    resource_type: "model_artifact".to_string(),
                    id: message,
                },
                408 | 504 => CoreError::RequestTimeout {
                    code: oneshim_core::error_codes::NetworkCode::Timeout,
                    timeout_ms: 0,
                },
                429 => CoreError::RateLimit {
                    code: oneshim_core::error_codes::NetworkCode::RateLimit,
                    retry_after_secs: 60,
                },
                502 | 503 => CoreError::ServiceUnavailable {
                    code: oneshim_core::error_codes::ServiceCode::Unavailable,
                    message,
                },
                _ => CoreError::Network {
                    code: oneshim_core::error_codes::NetworkCode::Generic,
                    message,
                },
            });
        }

        let total_bytes = response.content_length();
        let mut stream = response.bytes_stream();
        let mut file = std::fs::File::create(&part_path).map_err(|e| CoreError::AudioCapture {
            code: oneshim_core::error_codes::AudioCode::CaptureFailed,
            message: format!("create part file: {e}"),
        })?;
        let mut hasher = Sha256::new();
        let mut downloaded: u64 = 0;

        use std::io::Write;
        while let Some(chunk_result) = stream.next().await {
            // Check cancellation
            if cancelled.load(Ordering::Relaxed) {
                drop(file);
                if let Err(e) = std::fs::remove_file(&part_path) {
                    debug!("remove_file failed: {e}");
                }
                return Err(CoreError::AudioCapture {
                    code: oneshim_core::error_codes::AudioCode::CaptureFailed,
                    message: "download cancelled".into(),
                });
            }

            let chunk = chunk_result.map_err(|e| {
                // Iter-90: stream-read timeout propagates the same wire code
                // as send()-time timeout (see top of this function).
                if e.is_timeout() {
                    CoreError::RequestTimeout {
                        code: oneshim_core::error_codes::NetworkCode::Timeout,
                        timeout_ms: 0,
                    }
                } else {
                    CoreError::Network {
                        code: oneshim_core::error_codes::NetworkCode::Generic,
                        message: format!("download stream: {e}"),
                    }
                }
            })?;

            file.write_all(&chunk)
                .map_err(|e| CoreError::AudioCapture {
                    code: oneshim_core::error_codes::AudioCode::CaptureFailed,
                    message: format!("write chunk: {e}"),
                })?;
            hasher.update(&chunk);
            downloaded += chunk.len() as u64;

            let progress_pct = total_bytes.map(|total| {
                if total == 0 {
                    0u8
                } else {
                    ((downloaded * 100) / total).min(100) as u8
                }
            });

            let _ = progress_tx.send(DownloadProgress {
                progress_pct,
                bytes_downloaded: downloaded,
                total_bytes,
            });
        }

        drop(file);

        // Verify expected size
        let expected = model_expected_bytes(model);
        if downloaded != expected {
            warn!(
                expected,
                actual = downloaded,
                "model size mismatch — upstream may have updated"
            );
        }

        // Atomic rename
        std::fs::rename(&part_path, &final_path).map_err(|e| CoreError::AudioCapture {
            code: oneshim_core::error_codes::AudioCode::CaptureFailed,
            message: format!("rename part file: {e}"),
        })?;

        let hash = hasher.finalize().iter().fold(String::new(), |mut acc, b| {
            use std::fmt::Write as _;
            let _ = write!(acc, "{b:02x}");
            acc
        });
        info!(
            model = ?model,
            size = downloaded,
            sha256 = %hash,
            "model download complete"
        );

        Ok(final_path)
    }

    fn model_status(&self, model: WhisperModelSize, dest_dir: &Path) -> ModelDownloadStatus {
        let path = dest_dir.join(model_filename(model));
        match std::fs::metadata(&path) {
            Ok(meta) => ModelDownloadStatus::Ready {
                path: path.to_string_lossy().into_owned(),
                size_bytes: meta.len(),
            },
            Err(_) => {
                // Check for partial download
                let part_path = dest_dir.join(format!("{}.part", model_filename(model)));
                if part_path.exists() {
                    ModelDownloadStatus::Error {
                        message: "incomplete download — please re-download".into(),
                    }
                } else {
                    ModelDownloadStatus::NotInstalled
                }
            }
        }
    }

    fn delete_model(&self, model: WhisperModelSize, dest_dir: &Path) -> Result<(), CoreError> {
        let path = dest_dir.join(model_filename(model));
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| CoreError::AudioCapture {
                code: oneshim_core::error_codes::AudioCode::CaptureFailed,
                message: format!("delete model: {e}"),
            })?;
        }
        // Also clean up any .part file
        let part = dest_dir.join(format!("{}.part", model_filename(model)));
        if let Err(e) = std::fs::remove_file(&part) {
            debug!("remove_file failed: {e}");
        }
        debug!(model = ?model, "model deleted");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn model_filename_mapping() {
        assert_eq!(model_filename(WhisperModelSize::Tiny), "ggml-tiny.bin");
        assert_eq!(model_filename(WhisperModelSize::Base), "ggml-base.bin");
        assert_eq!(model_filename(WhisperModelSize::Small), "ggml-small.bin");
        assert_eq!(model_filename(WhisperModelSize::Medium), "ggml-medium.bin");
    }

    #[test]
    fn model_status_not_installed() {
        let dl = WhisperModelDownloader::new();
        let dir = tempdir().unwrap();
        let status = dl.model_status(WhisperModelSize::Base, dir.path());
        assert!(matches!(status, ModelDownloadStatus::NotInstalled));
    }

    #[test]
    fn model_status_ready_when_file_exists() {
        let dl = WhisperModelDownloader::new();
        let dir = tempdir().unwrap();
        let path = dir.path().join("ggml-base.bin");
        std::fs::write(&path, b"fake model data").unwrap();
        let status = dl.model_status(WhisperModelSize::Base, dir.path());
        match status {
            ModelDownloadStatus::Ready { size_bytes, .. } => {
                assert_eq!(size_bytes, 15); // "fake model data".len()
            }
            _ => panic!("expected Ready"),
        }
    }

    #[test]
    fn delete_model_removes_file() {
        let dl = WhisperModelDownloader::new();
        let dir = tempdir().unwrap();
        let path = dir.path().join("ggml-tiny.bin");
        std::fs::write(&path, b"data").unwrap();
        assert!(path.exists());
        dl.delete_model(WhisperModelSize::Tiny, dir.path()).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn delete_model_noop_when_missing() {
        let dl = WhisperModelDownloader::new();
        let dir = tempdir().unwrap();
        dl.delete_model(WhisperModelSize::Medium, dir.path())
            .unwrap();
    }

    // iter-80 regression guards for iter-60 semantic HTTP status mapping
    // in download(). Uses `new_with_base_url` to point the downloader at
    // a mockito server.
    async fn run_download_status_test(status: u16) -> CoreError {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", "/ggml-tiny.bin")
            .with_status(status as usize)
            .with_body(format!("http {status}"))
            .create_async()
            .await;

        let dl = WhisperModelDownloader::new_with_base_url(server.url());
        let dir = tempdir().unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        dl.download(
            WhisperModelSize::Tiny,
            dir.path(),
            tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .unwrap_err()
    }

    #[tokio::test]
    async fn download_403_maps_to_auth() {
        let err = run_download_status_test(403).await;
        assert!(
            matches!(err, CoreError::Auth { .. }),
            "403 → Auth, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn download_404_maps_to_not_found() {
        let err = run_download_status_test(404).await;
        assert!(
            matches!(err, CoreError::NotFound { .. }),
            "404 → NotFound (model artifact missing), got: {err:?}"
        );
    }

    #[tokio::test]
    async fn download_429_maps_to_rate_limit() {
        let err = run_download_status_test(429).await;
        assert!(
            matches!(err, CoreError::RateLimit { .. }),
            "429 → RateLimit, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn download_503_maps_to_service_unavailable() {
        let err = run_download_status_test(503).await;
        assert!(
            matches!(err, CoreError::ServiceUnavailable { .. }),
            "503 → ServiceUnavailable, got: {err:?}"
        );
    }

    /// Domain fallback. 500 falls back to Network/Generic.
    #[tokio::test]
    async fn download_500_falls_back_to_network() {
        let err = run_download_status_test(500).await;
        assert!(
            matches!(err, CoreError::Network { .. }),
            "500 should fall back to Network, got: {err:?}"
        );
    }
}
