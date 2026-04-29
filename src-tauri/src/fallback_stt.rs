//! Fallback STT provider — tries primary, falls back to secondary on transient errors.

use std::sync::Arc;

use async_trait::async_trait;
use tracing::warn;

use oneshim_core::error::CoreError;
use oneshim_core::models::audio::{AudioBuffer, TranscriptionResult};
use oneshim_core::ports::stt_provider::SttProvider;

pub struct FallbackSttProvider {
    primary: Arc<dyn SttProvider>,
    secondary: Arc<dyn SttProvider>,
}

impl FallbackSttProvider {
    pub fn new(primary: Arc<dyn SttProvider>, secondary: Arc<dyn SttProvider>) -> Self {
        Self { primary, secondary }
    }

    /// Whether an error should trigger fallback (transient errors only, NOT timeouts).
    fn should_fallback(err: &CoreError) -> bool {
        !matches!(err, CoreError::RequestTimeout { .. })
    }
}

#[async_trait]
impl SttProvider for FallbackSttProvider {
    async fn transcribe(&self, audio: AudioBuffer) -> Result<TranscriptionResult, CoreError> {
        match self.primary.transcribe(audio.clone()).await {
            Ok(result) => Ok(result),
            Err(primary_err) => {
                if Self::should_fallback(&primary_err) {
                    warn!(
                        primary = self.primary.provider_name(),
                        error = %primary_err,
                        fallback = self.secondary.provider_name(),
                        "primary STT failed, falling back"
                    );
                    self.secondary.transcribe(audio).await
                } else {
                    Err(primary_err)
                }
            }
        }
    }

    fn provider_name(&self) -> &str {
        "fallback"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct SuccessProvider {
        name: &'static str,
        text: &'static str,
    }

    #[async_trait]
    impl SttProvider for SuccessProvider {
        async fn transcribe(&self, _audio: AudioBuffer) -> Result<TranscriptionResult, CoreError> {
            Ok(TranscriptionResult {
                text: self.text.into(),
                language: None,
                duration_secs: 1.0,
                processing_secs: 0.5,
            })
        }
        fn provider_name(&self) -> &str {
            self.name
        }
    }

    struct ErrorProvider {
        name: &'static str,
        error: fn() -> CoreError,
    }

    #[async_trait]
    impl SttProvider for ErrorProvider {
        async fn transcribe(&self, _audio: AudioBuffer) -> Result<TranscriptionResult, CoreError> {
            Err((self.error)())
        }
        fn provider_name(&self) -> &str {
            self.name
        }
    }

    #[tokio::test]
    async fn primary_success_returns_primary() {
        let fb = FallbackSttProvider::new(
            Arc::new(SuccessProvider {
                name: "cloud",
                text: "hello",
            }),
            Arc::new(SuccessProvider {
                name: "local",
                text: "fallback hello",
            }),
        );
        let result = fb
            .transcribe(AudioBuffer::new(vec![0.0; 100]))
            .await
            .unwrap();
        assert_eq!(result.text, "hello");
    }

    #[tokio::test]
    async fn primary_fail_falls_back_to_secondary() {
        let fb = FallbackSttProvider::new(
            Arc::new(ErrorProvider {
                name: "cloud",
                error: || CoreError::Network {
                    code: oneshim_core::error_codes::NetworkCode::Generic,
                    message: "connection refused".into(),
                },
            }),
            Arc::new(SuccessProvider {
                name: "local",
                text: "fallback hello",
            }),
        );
        let result = fb
            .transcribe(AudioBuffer::new(vec![0.0; 100]))
            .await
            .unwrap();
        assert_eq!(result.text, "fallback hello");
    }

    #[tokio::test]
    async fn timeout_does_not_fallback() {
        let fb = FallbackSttProvider::new(
            Arc::new(ErrorProvider {
                name: "cloud",
                error: || CoreError::RequestTimeout {
                    code: oneshim_core::error_codes::NetworkCode::Timeout,
                    timeout_ms: 10000,
                },
            }),
            Arc::new(SuccessProvider {
                name: "local",
                text: "fallback hello",
            }),
        );
        let result = fb.transcribe(AudioBuffer::new(vec![0.0; 100])).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn both_fail_returns_secondary_error() {
        let fb = FallbackSttProvider::new(
            Arc::new(ErrorProvider {
                name: "cloud",
                error: || CoreError::Network {
                    code: oneshim_core::error_codes::NetworkCode::Generic,
                    message: "cloud down".into(),
                },
            }),
            Arc::new(ErrorProvider {
                name: "local",
                error: || CoreError::SpeechToText {
                    code: oneshim_core::error_codes::AudioCode::SttFailed,
                    message: "model missing".into(),
                },
            }),
        );
        let result = fb.transcribe(AudioBuffer::new(vec![0.0; 100])).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("model missing"));
    }
}
