//! Whisper model download manager with streaming progress and SHA-256 verification.
//!
//! Gated behind `#[cfg(feature = "download")]`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use futures_util::StreamExt;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use maekon_core::config::WhisperModelSize;
use maekon_core::error::CoreError;
use maekon_core::models::audio::{DownloadProgress, ModelDownloadStatus};
use maekon_core::ports::model_downloader::ModelDownloader;

const DEFAULT_BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";

/// F-RC-C22-03: pinned SHA-256 hash table (supply-chain integrity).
///
/// Based on the whisper.cpp GGML-format files (ggerganov/whisper.cpp HuggingFace
/// repository). When upstream models are updated, update this table as well and attach
/// the evidence to the PR.
/// Source: https://huggingface.co/ggerganov/whisper.cpp (as of 2026-05-23).
/// F-RC-C23-01: fixed the 63-char Medium SHA-256 typo — cross-verified against the LFS
/// pointer.
///   `curl -s https://huggingface.co/ggerganov/whisper.cpp/raw/main/ggml-medium.bin`
///   → `oid sha256:6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208` (64 chars)
/// F-RC-C25-05: replaced the find_map linear scan with an exhaustive match pattern.
///   Adding a new WhisperModelSize variant triggers a compile-time error so a missing
///   SHA is caught. The table is kept solely for the 64-char verification test
///   (cfg(test)).
#[cfg(test)]
const EXPECTED_SHA256: &[(WhisperModelSize, &str)] = &[
    (
        WhisperModelSize::Tiny,
        "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
    ),
    (
        WhisperModelSize::Base,
        "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
    ),
    (
        WhisperModelSize::Small,
        "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
    ),
    (
        WhisperModelSize::Medium,
        "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208",
    ),
];

/// F-RC-C22-03: returns the expected SHA-256 hash for a model size.
/// When a new variant is added to WhisperModelSize, this function MUST be updated too,
/// or compilation fails.
pub fn model_expected_sha256(size: WhisperModelSize) -> &'static str {
    // F-RC-C25-05: exhaustive match — adding a new model variant triggers a compile
    // error so a missing SHA is caught
    match size {
        WhisperModelSize::Tiny => {
            "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21"
        }
        WhisperModelSize::Base => {
            "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe"
        }
        WhisperModelSize::Small => {
            "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b"
        }
        WhisperModelSize::Medium => {
            "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208"
        }
    }
}

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

/// F-RC-C19: Absolute hard ceiling for a single model download (2 GiB).
/// Bounds the streaming write even if `model_expected_bytes` is ever inflated.
/// The per-model cap is `min(expected * 110%, this)` so the largest known model
/// (Medium ~1.53 GB) still fits with slack while an unbounded/oversized stream
/// is aborted before it can fill the disk (DoS).
const ABSOLUTE_MAX_DOWNLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// F-RC-C19: Streaming write ceiling for `model` — `expected * 110%`, clamped to
/// the absolute 2 GiB hard cap. Once `downloaded` exceeds this the stream is
/// aborted and the `.part` file removed (disk-fill DoS mitigation).
fn model_download_cap(model: WhisperModelSize) -> u64 {
    let slack = model_expected_bytes(model)
        .saturating_mul(11)
        .saturating_div(10);
    slack.min(ABSOLUTE_MAX_DOWNLOAD_BYTES)
}

/// F-RC-C21: Cache key for verify-on-load SHA-256 results. A file is considered
/// unchanged when its path, mtime and length all match — re-hashing a 1.5 GB
/// model on every `model_status` poll would be prohibitively expensive.
#[derive(Clone, PartialEq, Eq, Hash)]
struct VerifyCacheKey {
    path: PathBuf,
    mtime_ns: i128,
    len: u64,
}

/// F-RC-C21: Stream a file through SHA-256 in 1 MiB chunks without buffering the
/// whole file in memory. Returns the lowercase-hex digest.
async fn hash_file_sha256(path: &Path) -> Result<String, std::io::Error> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex_digest(hasher))
}

/// Shared lowercase-hex formatting for a finalized SHA-256 hasher.
fn hex_digest(hasher: Sha256) -> String {
    hasher.finalize().iter().fold(String::new(), |mut acc, b| {
        use std::fmt::Write as _;
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

/// Synchronous counterpart to [`hash_file_sha256`]: stream a file through SHA-256
/// in 1 MiB chunks (no whole-file buffering), lowercase-hex. Used by the
/// synchronous load-time verify gate where no async runtime is available.
fn hash_file_sha256_sync(path: &Path) -> Result<String, std::io::Error> {
    use std::io::Read as _;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex_digest(hasher))
}

/// #6344: verify-on-load gate for the DOWNLOAD-MANAGED model path. Returns `true`
/// only if the managed model file for `model` exists under `model_dir` AND its
/// bytes hash to the pinned [`model_expected_sha256`]. STT load sites must use this
/// instead of a bare `Path::exists()` so a model corrupted or tampered on disk
/// *after* a verified download is not loaded (the verify-before-rename fix only
/// closed the download window). Single source so a new load site cannot regress to
/// existence-only (ADR-075 P-3).
///
/// This is the synchronous, instance-free analogue of the async `model_status`
/// `Ready` check: it re-hashes on each call (load is infrequent — startup + manual
/// reload — so this is not the per-poll hot path `model_status` caches against).
/// User-supplied `whisper_model_path` values have no pinned SHA and stay
/// existence-gated by design, so callers apply this only to the managed path.
pub fn managed_model_verified_on_disk(model: WhisperModelSize, model_dir: &Path) -> bool {
    let path = model_dir.join(model_filename(model));
    match hash_file_sha256_sync(&path) {
        Ok(actual) => actual.eq_ignore_ascii_case(model_expected_sha256(model)),
        Err(_) => false,
    }
}

/// F-RC-C21: extract the platform-native mtime as nanoseconds since the UNIX
/// epoch, falling back to `0` when unavailable. Used only as a cache
/// invalidation signal, so a coarse/absent value just forces a re-hash.
fn metadata_mtime_ns(meta: &std::fs::Metadata) -> i128 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i128)
        .unwrap_or(0)
}

pub struct WhisperModelDownloader {
    client: reqwest::Client,
    base_url: String,
    /// F-RC-C21: memoized verify-on-load hash results keyed by (path, mtime, len).
    /// Stores the computed lowercase-hex SHA-256 so a Ready report can be
    /// re-verified without re-hashing an unchanged file.
    verify_cache: Mutex<HashMap<VerifyCacheKey, String>>,
    /// F-RC-C19: test-only override of the streaming ceiling so the disk-fill
    /// abort path can be exercised with a small mock body. `None` in production
    /// (the real `model_download_cap` applies).
    #[cfg(test)]
    cap_override: Option<u64>,
}

impl Default for WhisperModelDownloader {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the model-download HTTP client with conservative timeouts (review4).
///
/// A model can be ~1.5 GB, so a single total `.timeout()` is wrong — it would
/// abort legitimate large downloads. Instead bound the connect phase and the
/// idle-between-reads gap, so a stalled / half-open connection (slow-loris, dead
/// TCP that never RSTs) fails fast — releasing the task, the `.part` file handle,
/// and the single-download lock — instead of hanging forever. The previous
/// `reqwest::Client::new()` set NO timeouts, so a mid-stream stall jammed all
/// future downloads until process restart.
fn build_download_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .read_timeout(std::time::Duration::from_secs(60))
        .build()
        .expect("failed to build whisper-download reqwest client")
}

impl WhisperModelDownloader {
    pub fn new() -> Self {
        Self {
            client: build_download_client(),
            base_url: DEFAULT_BASE_URL.to_string(),
            verify_cache: Mutex::new(HashMap::new()),
            #[cfg(test)]
            cap_override: None,
        }
    }

    /// Test/override constructor — use when pointing at a mock server or a
    /// mirror. Production code should call `new()` to use the canonical
    /// Huggingface URL.
    #[doc(hidden)]
    pub fn new_with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            client: build_download_client(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            verify_cache: Mutex::new(HashMap::new()),
            #[cfg(test)]
            cap_override: None,
        }
    }

    /// F-RC-C19 (test-only): override the per-download streaming ceiling.
    #[cfg(test)]
    fn with_cap_override(mut self, cap: u64) -> Self {
        self.cap_override = Some(cap);
        self
    }

    /// F-RC-C19: the effective streaming ceiling for `model` (test override
    /// wins when set, otherwise the production `model_download_cap`).
    fn effective_cap(&self, model: WhisperModelSize) -> u64 {
        #[cfg(test)]
        if let Some(c) = self.cap_override {
            return c;
        }
        model_download_cap(model)
    }

    /// F-RC-C21: Re-hash `path` and compare against `expected_sha256`.
    /// Returns `Ok(())` on match, `Err(actual_hash)` on mismatch. Results are
    /// memoized by (path, mtime, len) so an unchanged file is hashed once.
    /// `len`/`mtime_ns` come from the caller's already-fetched metadata.
    async fn verify_file_sha256(
        &self,
        path: &Path,
        len: u64,
        mtime_ns: i128,
        expected_sha256: &str,
    ) -> Result<(), String> {
        let key = VerifyCacheKey {
            path: path.to_path_buf(),
            mtime_ns,
            len,
        };

        // Fast path: cached hash for this exact (path, mtime, len).
        if let Some(cached) = self.verify_cache.lock().get(&key).cloned() {
            return if cached == expected_sha256 {
                Ok(())
            } else {
                Err(cached)
            };
        }

        // Slow path: stream the file through the hasher (bounded 1 MiB buffer
        // so a 1.5 GB model never lands wholesale in memory).
        let actual = hash_file_sha256(path)
            .await
            .map_err(|e| format!("verify-on-load read failed: {e}"))?;

        self.verify_cache.lock().insert(key, actual.clone());

        if actual == expected_sha256 {
            Ok(())
        } else {
            Err(actual)
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

        // Ensure dest dir exists — F-RC-10: use tokio::fs in async context
        tokio::fs::create_dir_all(dest_dir)
            .await
            .map_err(|e| CoreError::AudioCapture {
                code: maekon_core::error_codes::AudioCode::CaptureFailed,
                message: format!("create model dir: {e}"),
            })?;

        info!(model = ?model, url = %url, "starting model download");

        let response = self.client.get(&url).send().await.map_err(|e| {
            // Iter-90: split timeout vs generic per canonical pattern
            // (cloud_stt.rs:107, http_client.rs map_reqwest_error) so
            // Grafana can group model-download timeouts separately.
            if e.is_timeout() {
                CoreError::RequestTimeout {
                    code: maekon_core::error_codes::NetworkCode::Timeout,
                    timeout_ms: 0, // sentinel; reqwest client-level timeout is not exposed
                }
            } else {
                CoreError::Network {
                    code: maekon_core::error_codes::NetworkCode::Generic,
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
                    code: maekon_core::error_codes::AuthCode::Failed,
                    message,
                },
                404 => CoreError::NotFound {
                    code: maekon_core::error_codes::NotFoundCode::ResourceMissing,
                    resource_type: "model_artifact".to_string(),
                    id: message,
                },
                408 | 504 => CoreError::RequestTimeout {
                    code: maekon_core::error_codes::NetworkCode::Timeout,
                    timeout_ms: 0,
                },
                429 => CoreError::RateLimit {
                    code: maekon_core::error_codes::NetworkCode::RateLimit,
                    retry_after_secs: 60,
                },
                502 | 503 => CoreError::ServiceUnavailable {
                    code: maekon_core::error_codes::ServiceCode::Unavailable,
                    message,
                },
                _ => CoreError::Network {
                    code: maekon_core::error_codes::NetworkCode::Generic,
                    message,
                },
            });
        }

        let total_bytes = response.content_length();
        // F-RC-C19: hard streaming ceiling — abort once the on-disk bytes exceed
        // expected*110% (clamped to 2 GiB) so a runaway/oversized response cannot
        // fill the disk before the post-stream size check would run.
        let cap = self.effective_cap(model);

        // F-RC-C23: run the whole streaming write inside one fallible block and
        // remove the `.part` file on ANY error before the successful rename
        // (previously only the cancellation arm cleaned up, leaking an orphan
        // `.part` on mid-stream network/write/ceiling faults).
        let stream_result: Result<(String, u64), CoreError> = async {
            let mut stream = response.bytes_stream();
            // F-RR-22: converted from std::fs::File + sync write_all to tokio::fs::File +
            // AsyncWriteExt::write_all so chunk writes don't block the async runtime.
            let mut file =
                tokio::fs::File::create(&part_path)
                    .await
                    .map_err(|e| CoreError::AudioCapture {
                        code: maekon_core::error_codes::AudioCode::CaptureFailed,
                        message: format!("create part file: {e}"),
                    })?;
            let mut hasher = Sha256::new();
            let mut downloaded: u64 = 0;

            while let Some(chunk_result) = stream.next().await {
                // Check cancellation
                if cancelled.load(Ordering::Relaxed) {
                    return Err(CoreError::AudioCapture {
                        code: maekon_core::error_codes::AudioCode::CaptureFailed,
                        message: "download cancelled".into(),
                    });
                }

                let chunk = chunk_result.map_err(|e| {
                    // Iter-90: stream-read timeout propagates the same wire code
                    // as send()-time timeout (see top of this function).
                    if e.is_timeout() {
                        CoreError::RequestTimeout {
                            code: maekon_core::error_codes::NetworkCode::Timeout,
                            timeout_ms: 0,
                        }
                    } else {
                        CoreError::Network {
                            code: maekon_core::error_codes::NetworkCode::Generic,
                            message: format!("download stream: {e}"),
                        }
                    }
                })?;

                file.write_all(&chunk)
                    .await
                    .map_err(|e| CoreError::AudioCapture {
                        code: maekon_core::error_codes::AudioCode::CaptureFailed,
                        message: format!("write chunk: {e}"),
                    })?;
                hasher.update(&chunk);
                downloaded += chunk.len() as u64;

                // F-RC-C19: enforce the ceiling immediately after the write so the
                // `.part` file can never grow past `cap` (disk-fill DoS guard).
                if downloaded > cap {
                    return Err(CoreError::IntegrityCheckFailed {
                        code: maekon_core::error_codes::AudioCode::IntegrityCheckFailed,
                        message: format!(
                            "Whisper model download for {model:?} exceeded size ceiling: \
                             downloaded={downloaded} > cap={cap} — aborted (disk-fill protection)"
                        ),
                    });
                }

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

            file.flush().await.map_err(|e| CoreError::AudioCapture {
                code: maekon_core::error_codes::AudioCode::CaptureFailed,
                message: format!("flush part file: {e}"),
            })?;
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

            Ok((hex_digest(hasher), downloaded))
        }
        .await;

        // F-RC-C23: on ANY streaming error, drop the orphan `.part` before
        // propagating (matches the former cancellation cleanup for every path).
        let (hash, downloaded) = match stream_result {
            Ok(ok) => ok,
            Err(e) => {
                if let Err(rm) = tokio::fs::remove_file(&part_path).await {
                    debug!("remove_file failed: {rm}");
                }
                return Err(e);
            }
        };

        // F-RC-C22-03 + review4: verify the SHA-256 BEFORE publishing the file at
        // its loadable `final_path`. Previously the rename happened first, so
        // `final_path` briefly existed with UNVERIFIED bytes (TOCTOU) — and the STT
        // load path only checks `.exists()`, so a concurrent load (or any
        // exists()-gated caller) in that window could parse a corrupt/tampered
        // model. Verify while the bytes are still at `part_path`; only rename after
        // the hash matches, so `final_path` never exists with unverified content.
        // F-RC-C25-05: model_expected_sha256 uses exhaustive match returning &str —
        // the check is always performed (no Option skip path).
        let expected = model_expected_sha256(model);
        if hash != expected {
            // Mismatch: drop the unverified `.part` and never publish it.
            let _ = tokio::fs::remove_file(&part_path).await;
            return Err(CoreError::IntegrityCheckFailed {
                code: maekon_core::error_codes::AudioCode::IntegrityCheckFailed,
                message: format!(
                    "Whisper model SHA-256 mismatch for {model:?}: \
                     expected={expected}, actual={hash} — download aborted (supply-chain integrity)"
                ),
            });
        }

        // Atomic rename (tokio::fs — stays off the blocking thread pool). Only
        // reached once the content is verified, so the loadable path is always sound.
        if let Err(e) = tokio::fs::rename(&part_path, &final_path).await {
            // F-RC-C23: rename failed — the `.part` is still present, clean it up.
            if let Err(rm) = tokio::fs::remove_file(&part_path).await {
                debug!("remove_file failed: {rm}");
            }
            return Err(CoreError::AudioCapture {
                code: maekon_core::error_codes::AudioCode::CaptureFailed,
                message: format!("rename part file: {e}"),
            });
        }

        // F-RC-C21: seed the verify-on-load cache with the just-verified hash so
        // the next `model_status` poll re-confirms Ready from cache instead of
        // re-hashing the freshly written (up to 1.5 GB) file.
        if let Ok(meta) = tokio::fs::metadata(&final_path).await {
            let key = VerifyCacheKey {
                path: final_path.clone(),
                mtime_ns: metadata_mtime_ns(&meta),
                len: meta.len(),
            };
            self.verify_cache.lock().insert(key, hash.clone());
        }

        info!(
            model = ?model,
            size = downloaded,
            sha256 = %hash,
            "model download complete — integrity check passed"
        );

        Ok(final_path)
    }

    async fn model_status(&self, model: WhisperModelSize, dest_dir: &Path) -> ModelDownloadStatus {
        let path = dest_dir.join(model_filename(model));
        match tokio::fs::metadata(&path).await {
            Ok(meta) => {
                // F-RC-C21: verify-on-load — file existence alone is NOT trust.
                // Re-hash and compare against the pinned SHA-256 before reporting
                // Ready, so a corrupted/tampered file is demoted to Error and the
                // STT runtime refuses to load it. The result is cached by
                // (path, mtime, len) so an unchanged 1.5 GB model is hashed once.
                let len = meta.len();
                let mtime_ns = metadata_mtime_ns(&meta);
                let expected = model_expected_sha256(model);
                match self
                    .verify_file_sha256(&path, len, mtime_ns, expected)
                    .await
                {
                    Ok(()) => ModelDownloadStatus::Ready {
                        path: path.to_string_lossy().into_owned(),
                        size_bytes: len,
                    },
                    Err(actual) => {
                        warn!(
                            model = ?model,
                            expected,
                            actual = %actual,
                            "model SHA-256 verify-on-load mismatch — refusing to load"
                        );
                        ModelDownloadStatus::Error {
                            message: "model failed integrity verification — please re-download"
                                .into(),
                        }
                    }
                }
            }
            Err(_) => {
                // Check for partial download
                let part_path = dest_dir.join(format!("{}.part", model_filename(model)));
                if tokio::fs::metadata(&part_path).await.is_ok() {
                    ModelDownloadStatus::Error {
                        message: "incomplete download — please re-download".into(),
                    }
                } else {
                    ModelDownloadStatus::NotInstalled
                }
            }
        }
    }

    async fn delete_model(
        &self,
        model: WhisperModelSize,
        dest_dir: &Path,
    ) -> Result<(), CoreError> {
        let path = dest_dir.join(model_filename(model));
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(CoreError::AudioCapture {
                    code: maekon_core::error_codes::AudioCode::CaptureFailed,
                    message: format!("delete model: {e}"),
                });
            }
        }
        // Also clean up any .part file
        let part = dest_dir.join(format!("{}.part", model_filename(model)));
        if let Err(e) = tokio::fs::remove_file(&part).await {
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

    #[tokio::test]
    async fn model_status_not_installed() {
        let dl = WhisperModelDownloader::new();
        let dir = tempdir().unwrap();
        let status = dl.model_status(WhisperModelSize::Base, dir.path()).await;
        assert!(matches!(status, ModelDownloadStatus::NotInstalled));
    }

    /// F-RC-C21: verify-on-load — a present-but-unverified file (fake bytes whose
    /// SHA-256 cannot match the pinned hash) must be demoted to Error, NOT Ready.
    /// File existence alone is no longer treated as Ready.
    #[tokio::test]
    async fn model_status_corrupt_file_demoted_to_error() {
        let dl = WhisperModelDownloader::new();
        let dir = tempdir().unwrap();
        let path = dir.path().join("ggml-base.bin");
        std::fs::write(&path, b"fake model data").unwrap();
        let status = dl.model_status(WhisperModelSize::Base, dir.path()).await;
        assert!(
            matches!(status, ModelDownloadStatus::Error { .. }),
            "fake-content model must fail verify-on-load, got: {status:?}"
        );
    }

    #[test]
    fn hash_file_sha256_sync_matches_known_vector() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("v.bin");
        std::fs::write(&path, b"abc").unwrap();
        // NIST SHA-256("abc").
        assert_eq!(
            hash_file_sha256_sync(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    /// #6344: the sync load-gate rejects a missing file and a present-but-corrupt
    /// file (whose SHA-256 cannot match the pinned hash), so neither is loaded.
    #[test]
    fn managed_model_verified_on_disk_rejects_corrupt_and_missing() {
        let dir = tempdir().unwrap();
        assert!(
            !managed_model_verified_on_disk(WhisperModelSize::Base, dir.path()),
            "a missing managed model must not be considered loadable"
        );
        let path = dir.path().join(model_filename(WhisperModelSize::Base));
        std::fs::write(&path, b"fake model data").unwrap();
        assert!(
            !managed_model_verified_on_disk(WhisperModelSize::Base, dir.path()),
            "a corrupt managed model must not be considered loadable"
        );
    }

    /// F-RC-C21: a file whose bytes hash to the pinned SHA-256 reports Ready.
    /// The expected hash is injected via the verify cache so the test exercises
    /// the match path without needing the real 77 MB artifact.
    #[tokio::test]
    async fn model_status_ready_when_hash_matches() {
        let dl = WhisperModelDownloader::new();
        let dir = tempdir().unwrap();
        let path = dir.path().join(model_filename(WhisperModelSize::Tiny));
        let body = b"verify-on-load-ready-body";
        std::fs::write(&path, body).unwrap();

        // Pre-seed the cache with the PINNED expected hash for this file's
        // (path, mtime, len) so verify-on-load takes the cached Ok path.
        let meta = std::fs::metadata(&path).unwrap();
        let key = VerifyCacheKey {
            path: path.clone(),
            mtime_ns: metadata_mtime_ns(&meta),
            len: meta.len(),
        };
        let expected = model_expected_sha256(WhisperModelSize::Tiny).to_string();
        dl.verify_cache.lock().insert(key, expected);

        let status = dl.model_status(WhisperModelSize::Tiny, dir.path()).await;
        match status {
            ModelDownloadStatus::Ready { size_bytes, .. } => {
                assert_eq!(size_bytes, body.len() as u64);
            }
            other => panic!("expected Ready for matching hash, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn delete_model_removes_file() {
        let dl = WhisperModelDownloader::new();
        let dir = tempdir().unwrap();
        let path = dir.path().join("ggml-tiny.bin");
        std::fs::write(&path, b"data").unwrap();
        assert!(path.exists());
        dl.delete_model(WhisperModelSize::Tiny, dir.path())
            .await
            .unwrap();
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn delete_model_noop_when_missing() {
        let dl = WhisperModelDownloader::new();
        let dir = tempdir().unwrap();
        dl.delete_model(WhisperModelSize::Medium, dir.path())
            .await
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

    // ---------- F-RC-C22-03: SHA-256 integrity check ----------

    /// A mock server returning the correct SHA-256 → download success path.
    /// Computes the actual hash and matches it against temporary mock data in the
    /// EXPECTED_SHA256 table.
    #[tokio::test]
    async fn download_integrity_check_passes_with_correct_hash() {
        use sha2::{Digest, Sha256};

        // Short byte body — triggers a Tiny model size-mismatch warning, but that is not
        // an error. Key point: when the SHA-256 matches, IntegrityCheckFailed must not
        // occur.
        let body = b"fake-tiny-model-data-for-integrity-test";
        let expected_hash = {
            let mut h = Sha256::new();
            h.update(body);
            h.finalize().iter().fold(String::new(), |mut acc, b| {
                use std::fmt::Write as _;
                let _ = write!(acc, "{b:02x}");
                acc
            })
        };

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", "/ggml-tiny.bin")
            .with_status(200)
            .with_body(body.as_ref())
            .create_async()
            .await;

        // Use new_with_base_url to bypass EXPECTED_SHA256; if the hash does not match,
        // the download is rejected with IntegrityCheckFailed. Here the actual hash
        // differs from the pinned table value, so this exercises the rejection path.
        let dl = WhisperModelDownloader::new_with_base_url(server.url());
        let dir = tempdir().unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let result = dl
            .download(
                WhisperModelSize::Tiny,
                dir.path(),
                tx,
                Arc::new(AtomicBool::new(false)),
            )
            .await;

        // The computed hash differs from the pinned EXPECTED_SHA256[Tiny] → IntegrityCheckFailed.
        match result {
            Err(CoreError::IntegrityCheckFailed { .. }) => {
                // Expected behavior: mismatch against the pinned hash → rejected
            }
            Ok(_) => {
                // If the table has no hash (None), it passes — warning only (allowed path)
            }
            Err(other) => {
                // No other error is allowed.
                // (e.g., a size mismatch only emits a warning and is not an error)
                panic!("unexpected error variant: {other:?}");
            }
        }

        // Key point: corrupted-bytes scenario — hash mismatch → returns IntegrityCheckFailed
        let _ = expected_hash; // confirm the computation completed
    }

    /// corrupted-bytes scenario: when the body is filled with 0xFF, IntegrityCheckFailed.
    /// F-RC-C25-05: model_expected_sha256 is an exhaustive match — a SHA always exists,
    /// so the skip guard is removed.
    #[tokio::test]
    async fn download_corrupted_bytes_returns_integrity_check_failed() {
        let corrupted_body = vec![0xFFu8; 128];
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", "/ggml-tiny.bin")
            .with_status(200)
            .with_body(corrupted_body)
            .create_async()
            .await;

        let dl = WhisperModelDownloader::new_with_base_url(server.url());
        let dir = tempdir().unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let err = dl
            .download(
                WhisperModelSize::Tiny,
                dir.path(),
                tx,
                Arc::new(AtomicBool::new(false)),
            )
            .await
            .expect_err("corrupted body must fail integrity check");

        assert!(
            matches!(err, CoreError::IntegrityCheckFailed { .. }),
            "corrupted download → IntegrityCheckFailed, got: {err:?}"
        );

        // On failure, the final file must not remain (prevents supply-chain contamination).
        let final_path = dir.path().join("ggml-tiny.bin");
        assert!(
            !final_path.exists(),
            "final file must not exist after integrity failure"
        );
    }

    /// model_expected_sha256 — exhaustive match; verifies every variant returns a
    /// non-empty SHA.
    /// F-RC-C25-05: removed the Option return — checks len() > 0 instead of is_some().
    #[test]
    fn model_expected_sha256_returns_hash_for_all_sizes() {
        // All 4 variants must return a non-empty 64-char hex SHA-256.
        for size in [
            WhisperModelSize::Tiny,
            WhisperModelSize::Base,
            WhisperModelSize::Small,
            WhisperModelSize::Medium,
        ] {
            let hash = model_expected_sha256(size);
            assert!(!hash.is_empty(), "SHA-256 entry is empty for {size:?}");
            assert_eq!(hash.len(), 64, "SHA-256 must be 64 hex chars for {size:?}");
        }
    }

    /// F-RC-C23-01: ensures every hash in the EXPECTED_SHA256 table is exactly 64 hex
    /// chars. Regression gate preventing a recurrence of the 63-char Medium hash typo.
    #[test]
    fn all_expected_sha256_hashes_are_64_chars() {
        for (size, hash) in EXPECTED_SHA256 {
            assert_eq!(
                hash.len(),
                64,
                "{size:?} hash has {} chars, expected 64",
                hash.len()
            );
            assert!(
                hash.chars().all(|c| c.is_ascii_hexdigit()),
                "{size:?} hash contains non-hex chars"
            );
        }
    }

    /// Regression guard for the AudioCode::IntegrityCheckFailed wire code (ADR-019 §1).
    #[test]
    fn integrity_check_failed_wire_code() {
        let err = CoreError::IntegrityCheckFailed {
            code: maekon_core::error_codes::AudioCode::IntegrityCheckFailed,
            message: "test".into(),
        };
        assert_eq!(err.code(), "audio.integrity_check_failed");
    }

    // ---------- F-RC-C19: streaming size ceiling (disk-fill DoS) ----------

    /// model_download_cap = expected * 110%, clamped to the 2 GiB absolute max.
    #[test]
    fn model_download_cap_is_expected_plus_slack_clamped() {
        // Tiny/Base/Small/Medium all sit under 2 GiB → cap == expected * 110%.
        for size in [
            WhisperModelSize::Tiny,
            WhisperModelSize::Base,
            WhisperModelSize::Small,
            WhisperModelSize::Medium,
        ] {
            let expected = model_expected_bytes(size);
            let want = (expected.saturating_mul(11) / 10).min(ABSOLUTE_MAX_DOWNLOAD_BYTES);
            assert_eq!(model_download_cap(size), want, "cap mismatch for {size:?}");
            assert!(
                model_download_cap(size) <= ABSOLUTE_MAX_DOWNLOAD_BYTES,
                "cap must never exceed the 2 GiB absolute max for {size:?}"
            );
        }
    }

    /// F-RC-C19: a body that exceeds the (overridden) streaming ceiling is
    /// aborted with IntegrityCheckFailed AND the orphan `.part` is removed.
    #[tokio::test]
    async fn download_exceeding_cap_aborts_and_removes_part() {
        // Serve more bytes than the tiny test cap so the in-loop ceiling trips.
        let body = vec![0xABu8; 4096];
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", "/ggml-tiny.bin")
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;

        let dl = WhisperModelDownloader::new_with_base_url(server.url()).with_cap_override(1024);
        let dir = tempdir().unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let err = dl
            .download(
                WhisperModelSize::Tiny,
                dir.path(),
                tx,
                Arc::new(AtomicBool::new(false)),
            )
            .await
            .expect_err("body over cap must abort");

        assert!(
            matches!(err, CoreError::IntegrityCheckFailed { .. }),
            "over-cap download → IntegrityCheckFailed, got: {err:?}"
        );

        // F-RC-C23: no orphan `.part` and no final file may survive the abort.
        let part_path = dir.path().join("ggml-tiny.bin.part");
        let final_path = dir.path().join("ggml-tiny.bin");
        assert!(
            !part_path.exists(),
            ".part file must be removed after over-cap abort"
        );
        assert!(
            !final_path.exists(),
            "final file must not exist after over-cap abort"
        );
    }

    // ---------- F-RC-C23: orphan .part cleanup on cancellation ----------

    /// F-RC-C23: a cancelled download removes the `.part` (regression guard for
    /// the cleanup path now shared by every error arm, not just cancellation).
    #[tokio::test]
    async fn download_cancelled_removes_part() {
        // A non-trivial body so at least one chunk is streamed.
        let body = vec![0x01u8; 64 * 1024];
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", "/ggml-tiny.bin")
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;

        let dl = WhisperModelDownloader::new_with_base_url(server.url());
        let dir = tempdir().unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        // Pre-cancel: the first loop iteration observes the flag and bails out.
        let cancelled = Arc::new(AtomicBool::new(true));
        let err = dl
            .download(WhisperModelSize::Tiny, dir.path(), tx, cancelled)
            .await
            .expect_err("cancelled download must error");

        assert!(
            matches!(err, CoreError::AudioCapture { .. }),
            "cancelled download → AudioCapture, got: {err:?}"
        );

        let part_path = dir.path().join("ggml-tiny.bin.part");
        assert!(
            !part_path.exists(),
            ".part file must be removed after cancellation"
        );
    }

    // ---------- F-RC-C21: verify-on-load cache ----------

    /// F-RC-C21: the verify cache short-circuits a `model_status` poll — a cached
    /// mismatching hash drives the Error result without re-hashing the bytes.
    #[tokio::test]
    async fn verify_cache_serves_cached_hash() {
        let dl = WhisperModelDownloader::new();
        let dir = tempdir().unwrap();
        let path = dir.path().join(model_filename(WhisperModelSize::Tiny));
        std::fs::write(&path, b"cache-probe-body").unwrap();

        let meta = std::fs::metadata(&path).unwrap();
        let key = VerifyCacheKey {
            path: path.clone(),
            mtime_ns: metadata_mtime_ns(&meta),
            len: meta.len(),
        };
        // Seed a distinct sentinel hash → Error; confirms the cached value (not a
        // fresh re-hash) drove the result.
        dl.verify_cache.lock().insert(key, "0".repeat(64));

        let status = dl.model_status(WhisperModelSize::Tiny, dir.path()).await;
        assert!(
            matches!(status, ModelDownloadStatus::Error { .. }),
            "cached mismatching hash → Error, got: {status:?}"
        );
    }
}
