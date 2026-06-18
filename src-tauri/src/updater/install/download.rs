//! Download, streaming download, checksum fetch, signature fetch, delta apply.

use futures::StreamExt;
use std::path::{Path, PathBuf};

use super::super::{UpdateError, Updater};

/// #6266: hard upper bound on a downloaded update artifact, before checksum /
/// signature verification. A missing/forged Content-Length or an endless
/// chunked body could otherwise fill the disk (streaming path) or exhaust memory
/// (`bytes()` path). 2 GiB is far above any real installer (tar.gz/zip/dmg/msi/
/// deb are well under 500 MB) while still bounding abuse.
const MAX_UPDATE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

impl Updater {
    /// Apply a delta patch: read current binary, apply bsdiff patch, verify checksum.
    ///
    /// Returns the path to the patched binary (written to a temp file).
    pub async fn apply_delta_update(
        &self,
        patch_path: &Path,
        full_binary_checksum: &str,
    ) -> Result<PathBuf, UpdateError> {
        let current_binary = super::super::delta::current_binary_path()?;
        let old_bytes = tokio::fs::read(&current_binary)
            .await
            .map_err(|e| UpdateError::Install(format!("Failed to read current binary: {e}")))?;
        let patch_bytes = tokio::fs::read(patch_path)
            .await
            .map_err(|e| UpdateError::Install(format!("Failed to read patch file: {e}")))?;

        let new_bytes = super::super::delta::apply_patch(&old_bytes, &patch_bytes)?;

        // Write patched binary to temp — F-RC-10: use tokio::fs in async context
        let temp_dir = std::env::temp_dir();
        let unique = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
        let temp_path = temp_dir.join(format!("maekon-{unique}-patched"));
        tokio::fs::write(&temp_path, &new_bytes)
            .await
            .map_err(|e| UpdateError::Install(format!("Failed to write patched binary: {e}")))?;

        // Verify against FULL binary checksum
        let actual_hash = Self::sha256_hex(&new_bytes);
        if actual_hash != full_binary_checksum {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(UpdateError::Integrity(format!(
                "Patched binary checksum mismatch: expected={full_binary_checksum}, actual={actual_hash}"
            )));
        }

        tracing::info!("Delta update applied: {:?}", temp_path);
        Ok(temp_path)
    }

    pub async fn download_update(&self, download_url: &str) -> Result<PathBuf, UpdateError> {
        let validated_url = self.validate_download_url(download_url)?;
        tracing::info!("Starting update download: {}", validated_url);

        let response = self.http_client.get(validated_url.clone()).send().await?;

        if !response.status().is_success() {
            return Err(UpdateError::Download(format!(
                "Download failed: HTTP {}",
                response.status()
            )));
        }

        // #6266: reject an oversized declared body before buffering it into memory.
        if let Some(len) = response.content_length() {
            if len > MAX_UPDATE_BYTES {
                return Err(UpdateError::Download(format!(
                    "Update artifact too large: {len} bytes exceeds cap {MAX_UPDATE_BYTES}"
                )));
            }
        }

        let bytes = response.bytes().await?;
        if bytes.len() as u64 > MAX_UPDATE_BYTES {
            return Err(UpdateError::Download(format!(
                "Update artifact too large: {} bytes exceeds cap {MAX_UPDATE_BYTES}",
                bytes.len()
            )));
        }

        let expected_hash = self.fetch_expected_sha256(&validated_url).await?;
        let actual_hash = Self::sha256_hex(&bytes);
        if actual_hash != expected_hash {
            return Err(UpdateError::Integrity(format!(
                "Checksum mismatch: expected={}, actual={}",
                expected_hash, actual_hash
            )));
        }

        if self.config.require_signature_verification {
            let signature = self.fetch_signature(&validated_url).await?;
            self.verify_signature(&bytes, &signature)?;
        }

        let temp_dir = std::env::temp_dir();
        let file_name = validated_url
            .path_segments()
            .and_then(|mut s| s.next_back())
            .unwrap_or("maekon-update")
            .trim();
        let unique = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
        let temp_path = temp_dir.join(format!("maekon-{unique}-{file_name}"));

        // F-RR-19: use tokio::fs in async fn — avoids blocking the executor.
        {
            use tokio::io::AsyncWriteExt;
            let mut outfile = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp_path)
                .await
                .map_err(|e| UpdateError::Install(format!("Failed to open temp file: {e}")))?;
            outfile
                .write_all(&bytes)
                .await
                .map_err(|e| UpdateError::Install(format!("Failed to write temp file: {e}")))?;
            outfile
                .sync_all()
                .await
                .map_err(|e| UpdateError::Install(format!("Failed to sync temp file: {e}")))?;
        }

        tracing::info!("Update download completed: {:?}", temp_path);
        Ok(temp_path)
    }

    /// Download with real-time progress reporting, streaming chunks to disk.
    pub async fn download_update_with_progress(
        &self,
        download_url: &str,
        progress_tx: tokio::sync::watch::Sender<maekon_api_contracts::update::DownloadProgress>,
    ) -> Result<PathBuf, UpdateError> {
        let validated_url = self.validate_download_url(download_url)?;
        tracing::info!("Starting streaming download: {}", validated_url);

        let response = self.http_client.get(validated_url.clone()).send().await?;
        if !response.status().is_success() {
            return Err(UpdateError::Download(format!(
                "Download failed: HTTP {}",
                response.status()
            )));
        }

        // #6266: reject an oversized declared body up front.
        if let Some(len) = response.content_length() {
            if len > MAX_UPDATE_BYTES {
                return Err(UpdateError::Download(format!(
                    "Update artifact too large: {len} bytes exceeds cap {MAX_UPDATE_BYTES}"
                )));
            }
        }

        let total_bytes = response.content_length().unwrap_or(0);
        let mut downloaded: u64 = 0;

        // F-RR-19: use tokio::fs in async fn — avoids blocking the executor.
        let temp_dir = std::env::temp_dir();
        let file_name = validated_url
            .path_segments()
            .and_then(|mut s| s.next_back())
            .unwrap_or("maekon-update")
            .trim();
        let unique = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
        let temp_path = temp_dir.join(format!("maekon-{unique}-{file_name}"));

        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .await
            .map_err(|e| UpdateError::Install(format!("Failed to open temp file: {e}")))?;

        use tokio::io::AsyncWriteExt;
        let mut stream = response.bytes_stream();
        while let Some(chunk_result) = stream.next().await {
            let chunk =
                chunk_result.map_err(|e| UpdateError::Download(format!("Stream error: {e}")))?;
            downloaded += chunk.len() as u64;
            // #6266: enforce the size cap mid-stream so a body with no/forged
            // Content-Length cannot fill the disk. Delete the partial temp file.
            if downloaded > MAX_UPDATE_BYTES {
                drop(file);
                let _ = tokio::fs::remove_file(&temp_path).await;
                return Err(UpdateError::Download(format!(
                    "Update artifact exceeded cap {MAX_UPDATE_BYTES} bytes mid-stream; aborted"
                )));
            }
            file.write_all(&chunk)
                .await
                .map_err(|e| UpdateError::Install(format!("Chunk write error: {e}")))?;
            let percent = if total_bytes > 0 {
                (downloaded as f32 / total_bytes as f32) * 100.0
            } else {
                0.0
            };
            let _ = progress_tx.send(maekon_api_contracts::update::DownloadProgress {
                bytes_downloaded: downloaded,
                total_bytes,
                percent,
            });
        }
        file.sync_all()
            .await
            .map_err(|e| UpdateError::Install(format!("sync_all failed: {e}")))?;
        drop(file);

        // Re-read for checksum verification — F-RC-10: use tokio::fs in async context
        let bytes = tokio::fs::read(&temp_path)
            .await
            .map_err(|e| UpdateError::Install(format!("Failed to read downloaded file: {e}")))?;

        let expected_hash = self.fetch_expected_sha256(&validated_url).await?;
        let actual_hash = Self::sha256_hex(&bytes);
        if actual_hash != expected_hash {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(UpdateError::Integrity(format!(
                "Checksum mismatch: expected={expected_hash}, actual={actual_hash}"
            )));
        }

        if self.config.require_signature_verification {
            let signature = self.fetch_signature(&validated_url).await?;
            self.verify_signature(&bytes, &signature)?;
        }

        tracing::info!(
            "Streaming download completed: {:?} ({downloaded} bytes)",
            temp_path
        );
        Ok(temp_path)
    }

    pub(super) async fn fetch_signature(
        &self,
        download_url: &reqwest::Url,
    ) -> Result<Vec<u8>, UpdateError> {
        let sig_url = reqwest::Url::parse(&format!("{}.sig", download_url))
            .map_err(|e| UpdateError::Integrity(format!("Failed to parse signature URL: {}", e)))?;

        self.validate_download_url(sig_url.as_str())?;

        let response = self.http_client.get(sig_url.clone()).send().await?;
        if !response.status().is_success() {
            return Err(UpdateError::Integrity(format!(
                "Failed to download signature file: HTTP {} ({})",
                response.status(),
                sig_url
            )));
        }

        let body = response.bytes().await?;
        let body = String::from_utf8(body.to_vec()).map_err(|e| {
            UpdateError::Integrity(format!("Invalid signature file encoding: {}", e))
        })?;

        let sig_b64 = body
            .split_whitespace()
            .next()
            .ok_or_else(|| UpdateError::Integrity("Signature file is empty".to_string()))?;

        use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
        BASE64.decode(sig_b64).map_err(|e| {
            UpdateError::Integrity(format!("Failed to decode signature base64: {}", e))
        })
    }

    pub(super) async fn fetch_expected_sha256(
        &self,
        download_url: &reqwest::Url,
    ) -> Result<String, UpdateError> {
        let checksum_url = reqwest::Url::parse(&format!("{}.sha256", download_url))
            .map_err(|e| UpdateError::Integrity(format!("Failed to parse checksum URL: {}", e)))?;

        self.validate_download_url(checksum_url.as_str())?;

        let response = self.http_client.get(checksum_url.clone()).send().await?;
        if !response.status().is_success() {
            return Err(UpdateError::Integrity(format!(
                "Failed to download checksum file: HTTP {} ({})",
                response.status(),
                checksum_url
            )));
        }

        let body = response.bytes().await?;
        let body = String::from_utf8(body.to_vec()).map_err(|e| {
            UpdateError::Integrity(format!("Invalid checksum file encoding: {}", e))
        })?;

        Self::parse_sha256_manifest(&body)
    }
}
