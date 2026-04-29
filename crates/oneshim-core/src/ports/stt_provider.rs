//! Port for speech-to-text transcription providers.

use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::audio::{AudioBuffer, TranscriptionResult};

/// # Errors
/// - `CoreError::SpeechToText` (wire: `audio.stt_failed`) for STT
///   provider-side failures: empty transcription, model load failure,
///   audio format rejection.
/// - Remote STT: HTTP-layer failures follow the canonical semantic
///   status mapping (`auth.failed` / `network.timeout` /
///   `network.rate_limit` / `service.unavailable`). See
///   `docs/guides/http-status-error-mapping.md`.
/// - Feature-gated adapters (whisper/cloud-stt without the feature
///   flag): `CoreError::ServiceUnavailable` (iter-109 pattern).
#[async_trait]
pub trait SttProvider: Send + Sync {
    /// Transcribe audio buffer to text. Takes ownership to avoid clone.
    async fn transcribe(&self, audio: AudioBuffer) -> Result<TranscriptionResult, CoreError>;

    /// Provider name for logging/UI.
    fn provider_name(&self) -> &str;
}
