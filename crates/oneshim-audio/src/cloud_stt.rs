//! Cloud STT provider using OpenAI Whisper API.
//!
//! Gated behind `#[cfg(feature = "cloud-stt")]`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use reqwest::multipart;
use tracing::debug;

use oneshim_core::config::{PiiFilterLevel, SttLanguage};
use oneshim_core::error::CoreError;
use oneshim_core::models::audio::{AudioBuffer, TranscriptionResult};
use oneshim_core::ports::pii_sanitizer::PiiSanitizer;
use oneshim_core::ports::stt_provider::SttProvider;

pub struct CloudSttProvider {
    client: reqwest::Client,
    api_key: String,
    endpoint: String,
    language: SttLanguage,
    timeout_secs: u32,
    /// D5 iter-4: injected PII sanitizer for transcript sanitization.
    pii_sanitizer: Option<Arc<dyn PiiSanitizer>>,
    pii_level: PiiFilterLevel,
}

impl CloudSttProvider {
    pub fn new(
        api_key: String,
        endpoint: String,
        language: SttLanguage,
        timeout_secs: u32,
    ) -> Result<Self, CoreError> {
        if api_key.is_empty() {
            return Err(CoreError::SpeechToText {
                code: oneshim_core::error_codes::AudioCode::SttFailed,
                message: "cloud STT API key is empty".into(),
            });
        }

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(u64::from(timeout_secs)))
            .build()
            .map_err(|e| CoreError::SpeechToText {
                code: oneshim_core::error_codes::AudioCode::SttFailed,
                message: format!("build HTTP client: {e}"),
            })?;

        Ok(Self {
            client,
            api_key,
            endpoint,
            language,
            timeout_secs,
            pii_sanitizer: None,
            pii_level: PiiFilterLevel::Standard,
        })
    }

    /// D5 iter-4: attach a `PiiSanitizer` for transcript sanitization.
    pub fn with_pii_sanitizer(
        mut self,
        sanitizer: Arc<dyn PiiSanitizer>,
        level: PiiFilterLevel,
    ) -> Self {
        self.pii_sanitizer = Some(sanitizer);
        self.pii_level = level;
        self
    }
}

#[async_trait]
impl SttProvider for CloudSttProvider {
    async fn transcribe(&self, audio: AudioBuffer) -> Result<TranscriptionResult, CoreError> {
        if audio.is_empty() {
            return Ok(TranscriptionResult {
                text: String::new(),
                language: None,
                duration_secs: 0.0,
                processing_secs: 0.0,
            });
        }

        let duration_secs = audio.duration_secs;
        let start = Instant::now();

        // Convert to WAV for upload
        let wav_bytes = audio.to_wav_bytes();

        let file_part = multipart::Part::bytes(wav_bytes)
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .map_err(|e| CoreError::SpeechToText {
                code: oneshim_core::error_codes::AudioCode::SttFailed,
                message: format!("create multipart: {e}"),
            })?;

        let mut form = multipart::Form::new()
            .part("file", file_part)
            .text("model", "whisper-1");

        // Add language hint if not auto
        match self.language {
            SttLanguage::En => {
                form = form.text("language", "en");
            }
            SttLanguage::Ko => {
                form = form.text("language", "ko");
            }
            SttLanguage::Auto => {}
        }

        debug!(endpoint = %self.endpoint, "sending cloud STT request");

        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    CoreError::RequestTimeout {
                        code: oneshim_core::error_codes::NetworkCode::Timeout,
                        timeout_ms: u64::from(self.timeout_secs) * 1000,
                    }
                } else {
                    CoreError::Network {
                        code: oneshim_core::error_codes::NetworkCode::Generic,
                        message: format!("cloud STT request: {e}"),
                    }
                }
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let message = format!("cloud STT error: HTTP {status} — {body}");
            // Semantic HTTP status mapping per iter-54/55/56 pattern — even STT
            // domain errors benefit from differentiating auth/timeout/rate-limit
            // from generic STT failures.
            return Err(match status.as_u16() {
                401 | 403 => CoreError::Auth {
                    code: oneshim_core::error_codes::AuthCode::Failed,
                    message,
                },
                408 | 504 => CoreError::RequestTimeout {
                    code: oneshim_core::error_codes::NetworkCode::Timeout,
                    timeout_ms: u64::from(self.timeout_secs) * 1000,
                },
                429 => CoreError::RateLimit {
                    code: oneshim_core::error_codes::NetworkCode::RateLimit,
                    retry_after_secs: 60,
                },
                502 | 503 => CoreError::ServiceUnavailable {
                    code: oneshim_core::error_codes::ServiceCode::Unavailable,
                    message,
                },
                _ => CoreError::SpeechToText {
                    code: oneshim_core::error_codes::AudioCode::SttFailed,
                    message,
                },
            });
        }

        #[derive(serde::Deserialize)]
        struct OpenAiResponse {
            text: String,
        }

        let result: OpenAiResponse =
            response.json().await.map_err(|e| CoreError::SpeechToText {
                code: oneshim_core::error_codes::AudioCode::SttFailed,
                message: format!("parse cloud response: {e}"),
            })?;

        let processing_secs = start.elapsed().as_secs_f32();
        debug!(
            text_len = result.text.len(),
            processing_secs, "cloud STT complete"
        );

        // D5 iter-4: sanitize transcript at provider boundary.
        let text = self
            .pii_sanitizer
            .as_ref()
            .map(|s| s.sanitize_text(&result.text, self.pii_level))
            .unwrap_or(result.text);

        Ok(TranscriptionResult {
            text,
            language: None,
            duration_secs,
            processing_secs,
        })
    }

    fn provider_name(&self) -> &str {
        "openai-whisper-cloud"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_rejects_empty_api_key() {
        let result =
            CloudSttProvider::new(String::new(), "http://test".into(), SttLanguage::Auto, 10);
        assert!(result.is_err());
    }

    #[test]
    fn provider_name_correct() {
        let provider = CloudSttProvider::new(
            "sk-test".into(),
            "http://test".into(),
            SttLanguage::Auto,
            10,
        )
        .unwrap();
        assert_eq!(provider.provider_name(), "openai-whisper-cloud");
    }

    // iter-72 regression guards for iter-58 semantic HTTP status mapping
    // in cloud_stt::transcribe. Shared helper pattern matches iter-67..71.
    async fn run_cloud_stt_status_test(status: u16) -> CoreError {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(status as usize)
            .with_body(format!("http {status}"))
            .create_async()
            .await;
        let provider =
            CloudSttProvider::new("sk-test".into(), server.url(), SttLanguage::Auto, 10).unwrap();
        // Non-empty buffer so transcribe doesn't early-return; 1 sec silence
        // at 16kHz = 16_000 samples.
        let audio = oneshim_core::models::audio::AudioBuffer::new(vec![0.0f32; 16_000]);
        provider.transcribe(audio).await.unwrap_err()
    }

    #[tokio::test]
    async fn stt_403_maps_to_auth() {
        let err = run_cloud_stt_status_test(403).await;
        assert!(
            matches!(err, CoreError::Auth { .. }),
            "403 → Auth, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn stt_408_maps_to_timeout() {
        let err = run_cloud_stt_status_test(408).await;
        assert!(
            matches!(err, CoreError::RequestTimeout { .. }),
            "408 → RequestTimeout, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn stt_429_maps_to_rate_limit() {
        let err = run_cloud_stt_status_test(429).await;
        assert!(
            matches!(err, CoreError::RateLimit { .. }),
            "429 → RateLimit, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn stt_502_maps_to_service_unavailable() {
        let err = run_cloud_stt_status_test(502).await;
        assert!(
            matches!(err, CoreError::ServiceUnavailable { .. }),
            "502 → ServiceUnavailable, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn stt_504_maps_to_timeout() {
        let err = run_cloud_stt_status_test(504).await;
        assert!(
            matches!(err, CoreError::RequestTimeout { .. }),
            "504 → RequestTimeout, got: {err:?}"
        );
    }

    /// Domain fallback: generic server error remains as SpeechToText / SttFailed.
    #[tokio::test]
    async fn stt_500_falls_back_to_stt_failed() {
        let err = run_cloud_stt_status_test(500).await;
        assert!(
            matches!(err, CoreError::SpeechToText { .. }),
            "500 → SpeechToText (domain fallback), got: {err:?}"
        );
    }
}
