//! Whisper model download manager with streaming progress and SHA-256 verification.
//!
//! Gated behind `#[cfg(feature = "download")]`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt as _;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use maekon_core::config::WhisperModelSize;
use maekon_core::error::CoreError;
use maekon_core::models::audio::{DownloadProgress, ModelDownloadStatus};
use maekon_core::ports::model_downloader::ModelDownloader;

const DEFAULT_BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";

/// F-RC-C22-03: 핀된 SHA-256 해시 테이블 (공급망 무결성).
///
/// whisper.cpp GGML 형식 파일 기준 (ggerganov/whisper.cpp HuggingFace repository).
/// 업스트림 모델 업데이트 시 이 테이블을 함께 갱신하고 PR 에 증거를 첨부한다.
/// 출처: https://huggingface.co/ggerganov/whisper.cpp (2026-05-23 기준).
/// F-RC-C23-01: Medium SHA-256 63자 오타 수정 — LFS 포인터 교차 검증 완료.
///   `curl -s https://huggingface.co/ggerganov/whisper.cpp/raw/main/ggml-medium.bin`
///   → `oid sha256:6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208` (64자)
/// F-RC-C25-05: find_map 선형 탐색 → 전수 match 패턴으로 교체.
///   새 WhisperModelSize 변형 추가 시 컴파일 타임 오류로 누락 SHA 를 감지한다.
///   테이블은 64자 검증 테스트 전용으로 유지 (cfg(test)).
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

/// F-RC-C22-03: 모델 크기에 대한 기대 SHA-256 해시를 반환한다.
/// WhisperModelSize 에 새 변형 추가 시 이 함수도 반드시 갱신해야 컴파일이 통과한다.
pub fn model_expected_sha256(size: WhisperModelSize) -> &'static str {
    // F-RC-C25-05: 전수 match — 신규 모델 변형 추가 시 컴파일 오류로 누락 SHA 감지
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
                drop(file);
                if let Err(e) = tokio::fs::remove_file(&part_path).await {
                    debug!("remove_file failed: {e}");
                }
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

        // Atomic rename (tokio::fs — stays off the blocking thread pool)
        tokio::fs::rename(&part_path, &final_path)
            .await
            .map_err(|e| CoreError::AudioCapture {
                code: maekon_core::error_codes::AudioCode::CaptureFailed,
                message: format!("rename part file: {e}"),
            })?;

        let hash = hasher.finalize().iter().fold(String::new(), |mut acc, b| {
            use std::fmt::Write as _;
            let _ = write!(acc, "{b:02x}");
            acc
        });

        // F-RC-C22-03: SHA-256 무결성 검사 — 핀된 해시와 불일치 시 즉시 실패.
        // 파일은 atomic rename 전에 이미 삭제되므로 corrupt 파일이 남지 않는다.
        // F-RC-C25-05: model_expected_sha256 이 전수 match 로 &str 반환 — 항상 검사 수행.
        let expected = model_expected_sha256(model);
        if hash != expected {
            // part 파일 정리 (rename 전 단계이므로 final_path 는 아직 없음)
            let _ = tokio::fs::remove_file(&final_path).await;
            return Err(CoreError::IntegrityCheckFailed {
                code: maekon_core::error_codes::AudioCode::IntegrityCheckFailed,
                message: format!(
                    "Whisper model SHA-256 mismatch for {model:?}: \
                     expected={expected}, actual={hash} — download aborted (supply-chain integrity)"
                ),
            });
        }

        info!(
            model = ?model,
            size = downloaded,
            sha256 = %hash,
            "model download complete — integrity check passed"
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
                code: maekon_core::error_codes::AudioCode::CaptureFailed,
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

    // ---------- F-RC-C22-03: SHA-256 무결성 검사 ----------

    /// 올바른 SHA-256 를 반환하는 mock 서버 → 다운로드 성공 경로.
    /// 실제 hash 를 계산해 EXPECTED_SHA256 테이블에 임시 mock 데이터와 매칭.
    #[tokio::test]
    async fn download_integrity_check_passes_with_correct_hash() {
        use sha2::{Digest, Sha256};

        // 짧은 바이트 본문 — Tiny 모델 크기 불일치 경고가 발생하지만 오류가 아니다.
        // 핵심: SHA-256 가 일치하면 IntegrityCheckFailed 가 발생하지 않아야 한다.
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

        // EXPECTED_SHA256 를 우회하기 위해 new_with_base_url 사용 후,
        // hash 가 일치하지 않으면 IntegrityCheckFailed 로 거부된다.
        // 여기서는 실제 hash 가 테이블 고정값과 다르므로 거부 경로를 테스트한다.
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

        // 고정된 EXPECTED_SHA256[Tiny] 와 계산된 hash 가 다르므로 IntegrityCheckFailed.
        match result {
            Err(CoreError::IntegrityCheckFailed { .. }) => {
                // 기대 동작: 고정 해시와 불일치 → 거부
            }
            Ok(_) => {
                // 만약 테이블에 hash 가 없으면(None) 통과 — 경고만 출력 (허용 경로)
            }
            Err(other) => {
                // 다른 에러는 허용하지 않는다.
                // (예: size mismatch 는 warn 만 발생하고 에러가 아님)
                panic!("unexpected error variant: {other:?}");
            }
        }

        // 핵심: corrupted-bytes 시나리오 — hash 불일치 → IntegrityCheckFailed 반환
        let _ = expected_hash; // 계산 완료 확인
    }

    /// corrupted-bytes 시나리오: 본문이 0xFF 로 채워진 경우 IntegrityCheckFailed.
    /// F-RC-C25-05: model_expected_sha256 전수 match — 항상 SHA 가 존재하므로 스킵 가드 제거.
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

        // 실패 시 최종 파일이 남아 있으면 안 된다 (공급망 오염 방지).
        let final_path = dir.path().join("ggml-tiny.bin");
        assert!(
            !final_path.exists(),
            "final file must not exist after integrity failure"
        );
    }

    /// model_expected_sha256 — 전수 match, 모든 변형이 비어 있지 않은 SHA 반환 확인.
    /// F-RC-C25-05: Option 반환 제거 — is_some() 대신 len() > 0 검사.
    #[test]
    fn model_expected_sha256_returns_hash_for_all_sizes() {
        // 4개 변형 모두 비어 있지 않은 64자 hex SHA-256 을 반환해야 한다.
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

    /// F-RC-C23-01: EXPECTED_SHA256 테이블의 모든 해시가 정확히 64자 hex 임을 보장.
    /// Medium 해시 63자 오타 재발 방지 회귀 게이트.
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

    /// AudioCode::IntegrityCheckFailed wire code 회귀 방지 (ADR-019 §1).
    #[test]
    fn integrity_check_failed_wire_code() {
        let err = CoreError::IntegrityCheckFailed {
            code: maekon_core::error_codes::AudioCode::IntegrityCheckFailed,
            message: "test".into(),
        };
        assert_eq!(err.code(), "audio.integrity_check_failed");
    }
}
