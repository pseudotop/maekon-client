//! Port for audio capture devices.

use std::sync::Arc;

use crate::error::CoreError;
use crate::models::audio::{AudioBuffer, VadConfig};

/// Port trait for microphone capture (Push-to-Talk + Voice Activity Detection).
/// Implementations: `oneshim_audio::AudioCapture` (cpal).
///
/// # Errors
/// - `CoreError::AudioCapture` (wire: `audio.capture_failed`) for all
///   capture-side failures: device not available, cpal stream errors,
///   PTT state conflicts, VAD not supported by this adapter.
/// - Feature-gated adapters (e.g., `whisper` / `cloud-stt`) without the
///   feature flag: `CoreError::ServiceUnavailable` (wire:
///   `service.unavailable`) per iter-109 feature-gate pattern.
pub trait AudioCapturePort: Send + Sync {
    /// Start capturing audio from the default input device (PTT mode).
    fn start(&self) -> Result<(), CoreError>;

    /// Stop capturing and return the accumulated audio buffer (16kHz mono).
    fn stop(&self) -> Result<AudioBuffer, CoreError>;

    /// Whether currently capturing (PTT mode).
    fn is_capturing(&self) -> bool;

    // ── VAD methods (default impls for backward compat) ──

    /// Start VAD listening mode. The `on_speech_signal` callback is invoked
    /// (on the audio thread) when speech ends — keep it lightweight
    /// (e.g., send a signal to a channel).
    fn start_vad(
        &self,
        _config: VadConfig,
        _on_speech_signal: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<(), CoreError> {
        Err(CoreError::AudioCapture {
            code: crate::error_codes::AudioCode::CaptureFailed,
            message: "VAD not supported".into(),
        })
    }

    /// Stop VAD listening mode.
    fn stop_vad(&self) -> Result<(), CoreError> {
        Err(CoreError::AudioCapture {
            code: crate::error_codes::AudioCode::CaptureFailed,
            message: "VAD not supported".into(),
        })
    }

    /// Whether VAD listening is currently active.
    fn is_vad_active(&self) -> bool {
        false
    }

    /// Drain the speech buffer and return resampled 16kHz audio.
    /// Called by the receiver task after on_speech_signal fires.
    fn drain_speech_buffer(&self) -> Result<AudioBuffer, CoreError> {
        Err(CoreError::AudioCapture {
            code: crate::error_codes::AudioCode::CaptureFailed,
            message: "VAD not supported".into(),
        })
    }
}
