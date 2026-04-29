//! Local Whisper STT provider using whisper-rs (whisper.cpp bindings).
//!
//! Gated behind `#[cfg(feature = "whisper")]`.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use parking_lot::Mutex;
use tracing::{debug, info};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use oneshim_core::config::{PiiFilterLevel, SttLanguage};
use oneshim_core::error::CoreError;
use oneshim_core::models::audio::{AudioBuffer, TranscriptionResult};
use oneshim_core::ports::pii_sanitizer::PiiSanitizer;
use oneshim_core::ports::stt_provider::SttProvider;

pub struct WhisperSttProvider {
    /// Arc-wrapped so it can be cloned into spawn_blocking closures safely.
    ctx: Arc<Mutex<WhisperContext>>,
    language: SttLanguage,
    transcribing: AtomicBool,
    /// D5 iter-4: injected PII sanitizer (`oneshim-vision::VisionPiiSanitizer`
    /// at DI time). None = test fixtures without DI wiring — falls back to raw.
    pii_sanitizer: Option<Arc<dyn PiiSanitizer>>,
    pii_level: PiiFilterLevel,
}

impl WhisperSttProvider {
    /// Load Whisper model from a GGML file.
    pub fn new(model_path: &Path, language: SttLanguage) -> Result<Self, CoreError> {
        if !model_path.exists() {
            return Err(CoreError::SpeechToText {
                code: oneshim_core::error_codes::AudioCode::SttFailed,
                message: format!("model file not found: {}", model_path.display()),
            });
        }

        let model_str = model_path.to_str().ok_or_else(|| CoreError::SpeechToText {
            code: oneshim_core::error_codes::AudioCode::SttFailed,
            message: format!(
                "model path contains invalid UTF-8: {}",
                model_path.display()
            ),
        })?;

        info!(model = %model_path.display(), "loading Whisper model");
        let ctx = WhisperContext::new_with_params(model_str, WhisperContextParameters::default())
            .map_err(|e| CoreError::SpeechToText {
            code: oneshim_core::error_codes::AudioCode::SttFailed,
            message: format!("failed to load model: {e}"),
        })?;

        Ok(Self {
            ctx: Arc::new(Mutex::new(ctx)),
            language,
            transcribing: AtomicBool::new(false),
            pii_sanitizer: None,
            pii_level: PiiFilterLevel::Standard,
        })
    }

    /// D5 iter-4: attach a `PiiSanitizer` implementation for transcript sanitization.
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

/// Guard that resets the `transcribing` flag on drop.
struct TranscriptionGuard<'a>(&'a AtomicBool);
impl Drop for TranscriptionGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

#[async_trait]
impl SttProvider for WhisperSttProvider {
    async fn transcribe(&self, audio: AudioBuffer) -> Result<TranscriptionResult, CoreError> {
        if self.transcribing.swap(true, Ordering::SeqCst) {
            return Err(CoreError::SpeechToText {
                code: oneshim_core::error_codes::AudioCode::SttFailed,
                message: "transcription already in progress".into(),
            });
        }
        let _guard = TranscriptionGuard(&self.transcribing);

        if audio.is_empty() {
            return Ok(TranscriptionResult {
                text: String::new(),
                language: None,
                duration_secs: 0.0,
                processing_secs: 0.0,
            });
        }

        let lang = self.language;
        let samples = audio.samples;
        let duration_secs = audio.duration_secs;

        // Clone the Arc so the closure is 'static — no unsafe required.
        let ctx = self.ctx.clone();
        // D5 iter-4: capture sanitizer for post-transcribe sanitization.
        let sanitizer = self.pii_sanitizer.clone();
        let pii_level = self.pii_level;

        tokio::task::spawn_blocking(move || {
            let start = Instant::now();
            let ctx_guard = ctx.lock();
            let mut state = ctx_guard
                .create_state()
                .map_err(|e| CoreError::SpeechToText {
                    code: oneshim_core::error_codes::AudioCode::SttFailed,
                    message: format!("create state: {e}"),
                })?;

            let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
            match lang {
                SttLanguage::En => params.set_language(Some("en")),
                SttLanguage::Ko => params.set_language(Some("ko")),
                SttLanguage::Auto => params.set_language(None),
            }
            params.set_print_special(false);
            params.set_print_progress(false);
            params.set_print_realtime(false);
            params.set_translate(false);
            params.set_single_segment(false);
            params.set_no_timestamps(true);

            state
                .full(params, &samples)
                .map_err(|e| CoreError::SpeechToText {
                    code: oneshim_core::error_codes::AudioCode::SttFailed,
                    message: format!("transcription failed: {e}"),
                })?;

            let n_segments = state
                .full_n_segments()
                .map_err(|e| CoreError::SpeechToText {
                    code: oneshim_core::error_codes::AudioCode::SttFailed,
                    message: format!("get segments: {e}"),
                })?;

            let raw_text: String = (0..n_segments)
                .filter_map(|i| state.full_get_segment_text(i).ok())
                .collect::<Vec<_>>()
                .join(" ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");

            // D5 iter-4: sanitize transcript at provider boundary.
            let text = sanitizer
                .as_ref()
                .map(|s| s.sanitize_text(&raw_text, pii_level))
                .unwrap_or(raw_text);

            let detected_lang = state
                .full_lang_id()
                .ok()
                .and_then(|id| whisper_rs::get_lang_str(id).map(|s| s.to_string()));

            let processing_secs = start.elapsed().as_secs_f32();
            debug!(
                text_len = text.len(),
                lang = ?detected_lang,
                processing_secs,
                duration_secs,
                "transcription complete"
            );

            Ok(TranscriptionResult {
                text,
                language: detected_lang,
                duration_secs,
                processing_secs,
            })
        })
        .await
        .map_err(|e| CoreError::SpeechToText {
            code: oneshim_core::error_codes::AudioCode::SttFailed,
            message: format!("spawn_blocking join: {e}"),
        })?
    }

    fn provider_name(&self) -> &str {
        "whisper-local"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcription_guard_resets_flag() {
        let flag = AtomicBool::new(true);
        {
            let _guard = TranscriptionGuard(&flag);
            assert!(flag.load(Ordering::SeqCst));
        }
        assert!(!flag.load(Ordering::SeqCst));
    }
}
