//! Debug-only synthetic audio capture adapter for isolated Computer Use QC.
//!
//! The adapter never opens an OS microphone. It is selected only when every
//! explicit fixture gate is present and the app flavor is dedicated to a
//! `qc-*` or `tc-*` profile. Its redacted state file lets the CJ-04-12 oracle
//! prove VAD teardown and transient-buffer zeroization after consent revoke.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use maekon_core::error::CoreError;
#[cfg(debug_assertions)]
use maekon_core::models::audio::TranscriptionResult;
use maekon_core::models::audio::{AudioBuffer, VadConfig};
use maekon_core::ports::audio_capture::AudioCapturePort;
#[cfg(debug_assertions)]
use maekon_core::ports::stt_provider::SttProvider;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

pub(crate) const FIXTURE_GATE_ENV: &str = "MAEKON_DEBUG_QC_AUDIO_FIXTURE";
const ISOLATED_GATE_ENV: &str = "MAEKON_TC_ISOLATED_PROFILE";
const FLAVOR_ENV: &str = "MAEKON_APP_FLAVOR";
pub(crate) const STATE_FILE: &str = "qc-audio-fixture-state.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct FixtureState {
    schema: String,
    synthetic_capture: bool,
    os_microphone_opened: bool,
    ptt_active: bool,
    vad_active: bool,
    buffered_samples: usize,
    buffer_zeroized: bool,
    stop_vad_count: u64,
    stt_egress_attempts: u64,
}

/// Deterministic `AudioCapturePort` used only by the isolated debug fixture.
pub(crate) struct SyntheticAudioCapture {
    ptt_active: AtomicBool,
    vad_active: AtomicBool,
    samples: Mutex<Vec<f32>>,
    buffer_zeroized: AtomicBool,
    stop_vad_count: AtomicU64,
    state_path: PathBuf,
}

/// Local no-op STT paired with the synthetic capture adapter. It makes the
/// product UI honestly ready for the isolated fixture without downloading a
/// real model or creating an external provider/egress path.
#[cfg(debug_assertions)]
struct SyntheticSttProvider;

#[cfg(debug_assertions)]
#[async_trait::async_trait]
impl SttProvider for SyntheticSttProvider {
    async fn transcribe(&self, audio: AudioBuffer) -> Result<TranscriptionResult, CoreError> {
        Ok(TranscriptionResult {
            text: String::new(),
            language: None,
            duration_secs: audio.duration_secs,
            processing_secs: 0.0,
        })
    }

    fn provider_name(&self) -> &str {
        "qc-synthetic-local"
    }
}

#[cfg(debug_assertions)]
pub(crate) fn synthetic_stt_provider() -> Arc<dyn SttProvider> {
    Arc::new(SyntheticSttProvider)
}

impl SyntheticAudioCapture {
    pub(crate) fn new(state_path: PathBuf) -> Result<Self, CoreError> {
        let capture = Self {
            ptt_active: AtomicBool::new(false),
            vad_active: AtomicBool::new(false),
            samples: Mutex::new(Vec::new()),
            buffer_zeroized: AtomicBool::new(true),
            stop_vad_count: AtomicU64::new(0),
            state_path,
        };
        capture.persist_state()?;
        Ok(capture)
    }

    #[cfg(test)]
    fn state_path(&self) -> &Path {
        &self.state_path
    }

    fn state(&self) -> FixtureState {
        FixtureState {
            schema: "maekon.qc-audio-fixture-state.v1".to_string(),
            synthetic_capture: true,
            os_microphone_opened: false,
            ptt_active: self.ptt_active.load(Ordering::SeqCst),
            vad_active: self.vad_active.load(Ordering::SeqCst),
            buffered_samples: self.samples.lock().len(),
            buffer_zeroized: self.buffer_zeroized.load(Ordering::SeqCst),
            stop_vad_count: self.stop_vad_count.load(Ordering::SeqCst),
            stt_egress_attempts: 0,
        }
    }

    fn persist_state(&self) -> Result<(), CoreError> {
        if let Some(parent) = self.state_path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| fixture_error(error.to_string()))?;
        }
        let serialized = serde_json::to_vec_pretty(&self.state())
            .map_err(|error| fixture_error(error.to_string()))?;
        std::fs::write(&self.state_path, serialized)
            .map_err(|error| fixture_error(error.to_string()))
    }

    fn seed_samples(&self) {
        let mut samples = self.samples.lock();
        samples.fill(0.0);
        samples.clear();
        samples.extend(std::iter::repeat(0.125_f32).take(16_000));
        self.buffer_zeroized.store(false, Ordering::SeqCst);
    }

    fn wipe_samples(&self) {
        let mut samples = self.samples.lock();
        samples.fill(0.0);
        samples.clear();
        self.buffer_zeroized.store(true, Ordering::SeqCst);
    }
}

impl AudioCapturePort for SyntheticAudioCapture {
    fn start(&self) -> Result<(), CoreError> {
        self.seed_samples();
        self.ptt_active.store(true, Ordering::SeqCst);
        self.persist_state()
    }

    fn stop(&self) -> Result<AudioBuffer, CoreError> {
        self.ptt_active.store(false, Ordering::SeqCst);
        let samples = std::mem::take(&mut *self.samples.lock());
        self.buffer_zeroized.store(true, Ordering::SeqCst);
        self.persist_state()?;
        Ok(AudioBuffer::new(samples))
    }

    fn is_capturing(&self) -> bool {
        self.ptt_active.load(Ordering::SeqCst)
    }

    fn start_vad(
        &self,
        _config: VadConfig,
        _on_speech_signal: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<(), CoreError> {
        self.seed_samples();
        self.vad_active.store(true, Ordering::SeqCst);
        self.persist_state()
    }

    fn stop_vad(&self) -> Result<(), CoreError> {
        self.vad_active.store(false, Ordering::SeqCst);
        self.wipe_samples();
        self.stop_vad_count.fetch_add(1, Ordering::SeqCst);
        self.persist_state()
    }

    fn is_vad_active(&self) -> bool {
        self.vad_active.load(Ordering::SeqCst)
    }

    fn drain_speech_buffer(&self) -> Result<AudioBuffer, CoreError> {
        let samples = std::mem::take(&mut *self.samples.lock());
        self.buffer_zeroized.store(true, Ordering::SeqCst);
        self.persist_state()?;
        Ok(AudioBuffer::new(samples))
    }
}

pub(crate) fn build_from_env(
    app_data_dir: &Path,
) -> Result<Option<Arc<dyn AudioCapturePort>>, CoreError> {
    match std::env::var(FIXTURE_GATE_ENV) {
        Err(std::env::VarError::NotPresent) => return Ok(None),
        Ok(value) if value == "1" => {}
        Ok(_) | Err(std::env::VarError::NotUnicode(_)) => {
            return Err(fixture_error(format!(
                "{FIXTURE_GATE_ENV}=1 is required when the fixture variable is present"
            )))
        }
    }
    if std::env::var(ISOLATED_GATE_ENV).ok().as_deref() != Some("1") {
        return Err(fixture_error(format!("{ISOLATED_GATE_ENV}=1 is required")));
    }
    let flavor = std::env::var(FLAVOR_ENV)
        .map_err(|_| fixture_error(format!("{FLAVOR_ENV} is required")))?;
    if !is_isolated_flavor(&flavor) {
        return Err(fixture_error(
            "MAEKON_APP_FLAVOR must be a dedicated qc-* or tc-* flavor".into(),
        ));
    }

    Ok(Some(Arc::new(SyntheticAudioCapture::new(
        app_data_dir.join(STATE_FILE),
    )?)))
}

fn is_isolated_flavor(flavor: &str) -> bool {
    let trimmed = flavor.trim();
    (trimmed.starts_with("qc-") || trimmed.starts_with("tc-"))
        && trimmed.len() > 3
        && trimmed
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

fn fixture_error(message: String) -> CoreError {
    CoreError::AudioCapture {
        code: maekon_core::error_codes::AudioCode::CaptureFailed,
        message: format!("synthetic QC audio fixture: {message}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_vad_stop_zeroizes_buffer_and_never_opens_microphone() {
        let temp = tempfile::tempdir().expect("temp dir");
        let capture =
            SyntheticAudioCapture::new(temp.path().join(STATE_FILE)).expect("synthetic capture");
        capture
            .start_vad(
                VadConfig {
                    threshold: 0.5,
                    silence_ms: 500,
                    min_speech_ms: 100,
                },
                Arc::new(|| {}),
            )
            .expect("start synthetic VAD");
        assert!(capture.is_vad_active());
        assert_eq!(capture.samples.lock().len(), 16_000);

        capture.stop_vad().expect("stop synthetic VAD");
        assert!(!capture.is_vad_active());
        assert!(capture.samples.lock().is_empty());

        let state: FixtureState =
            serde_json::from_slice(&std::fs::read(capture.state_path()).expect("fixture state"))
                .expect("decode fixture state");
        assert!(state.synthetic_capture);
        assert!(!state.os_microphone_opened);
        assert!(!state.vad_active);
        assert_eq!(state.buffered_samples, 0);
        assert!(state.buffer_zeroized);
        assert_eq!(state.stop_vad_count, 1);
        assert_eq!(state.stt_egress_attempts, 0);
    }

    #[tokio::test]
    async fn synthetic_stt_is_local_deterministic_and_model_free() {
        let provider = synthetic_stt_provider();
        let result = provider
            .transcribe(AudioBuffer::new(vec![0.125; 16_000]))
            .await
            .expect("synthetic STT must be infallible");

        assert_eq!(provider.provider_name(), "qc-synthetic-local");
        assert!(result.text.is_empty());
        assert_eq!(result.duration_secs, 1.0);
        assert_eq!(result.processing_secs, 0.0);
    }

    #[test]
    fn flavor_gate_rejects_normal_and_path_like_profiles() {
        assert!(is_isolated_flavor("qc-8566-audio"));
        assert!(is_isolated_flavor("tc-audio"));
        assert!(!is_isolated_flavor("dev"));
        assert!(!is_isolated_flavor("release"));
        assert!(!is_isolated_flavor("qc-../main"));
    }
}
