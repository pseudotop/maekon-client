//! Cross-platform microphone capture with automatic resampling to 16kHz mono.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use parking_lot::Mutex;
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use tracing::{debug, warn};

use oneshim_core::error::CoreError;
use oneshim_core::models::audio::{AudioBuffer, VadConfig};

use crate::vad::{VadDetector, VadEvent};

const TARGET_SAMPLE_RATE: u32 = 16000;
/// Hard cap: 120s at 48kHz mono = 5.76M samples (~23MB). Prevents unbounded growth.
const MAX_BUFFER_SAMPLES: usize = 120 * 48_000;

pub struct AudioCapture {
    buffer: Arc<Mutex<Vec<f32>>>,
    capturing: Arc<AtomicBool>,
    stream: Mutex<Option<cpal::Stream>>,
    /// Native sample rate of the input device (set during start).
    native_rate: Mutex<Option<u32>>,
    /// Whether VAD listening mode is active.
    vad_active: Arc<AtomicBool>,
    /// Accumulated speech samples during VAD mode (raw, native rate).
    speech_buffer: Arc<Mutex<Vec<f32>>>,
}

impl Default for AudioCapture {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioCapture {
    pub fn new() -> Self {
        Self {
            buffer: Arc::new(Mutex::new(Vec::new())),
            capturing: Arc::new(AtomicBool::new(false)),
            stream: Mutex::new(None),
            native_rate: Mutex::new(None),
            vad_active: Arc::new(AtomicBool::new(false)),
            speech_buffer: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Start capturing audio from the default input device (PTT mode).
    pub fn start(&self) -> Result<(), CoreError> {
        if self.vad_active.load(Ordering::SeqCst) {
            return Err(CoreError::AudioCapture {
                code: oneshim_core::error_codes::AudioCode::CaptureFailed,
                message: "cannot start PTT while VAD is active".into(),
            });
        }
        if self.capturing.load(Ordering::SeqCst) {
            return Err(CoreError::AudioCapture {
                code: oneshim_core::error_codes::AudioCode::CaptureFailed,
                message: "already capturing".into(),
            });
        }

        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| CoreError::AudioCapture {
                code: oneshim_core::error_codes::AudioCode::CaptureFailed,
                message: "no input device available".into(),
            })?;

        let config = device
            .default_input_config()
            .map_err(|e| CoreError::AudioCapture {
                code: oneshim_core::error_codes::AudioCode::CaptureFailed,
                message: format!("input config error: {e}"),
            })?;

        let native_rate = config.sample_rate();
        let channels = config.channels() as usize;

        debug!(
            device = device
                .description()
                .map(|d| d.name().to_string())
                .unwrap_or_default(),
            sample_rate = native_rate,
            channels,
            "starting audio capture"
        );

        // Store native rate for resampling in stop()
        *self.native_rate.lock() = Some(native_rate);

        // Clear buffer for new recording
        self.buffer.lock().clear();

        let buffer = self.buffer.clone();
        let capturing = self.capturing.clone();

        let err_fn = |err: cpal::StreamError| {
            warn!("audio stream error: {err}");
        };

        // Callback: downmix to mono and accumulate raw samples (no resampling).
        // Resampling is done in stop() over the full buffer — avoids
        // SincFixedIn chunk-size constraint in variable-size callbacks.
        // Buffer is capped at MAX_BUFFER_SAMPLES to prevent unbounded growth.
        let stream = match config.sample_format() {
            SampleFormat::F32 => device.build_input_stream(
                &config.into(),
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if !capturing.load(Ordering::SeqCst) {
                        return;
                    }
                    let mono: Vec<f32> = data
                        .chunks(channels)
                        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
                        .collect();
                    let mut buf = buffer.lock();
                    if buf.len() < MAX_BUFFER_SAMPLES {
                        buf.extend_from_slice(&mono);
                    }
                },
                err_fn,
                None,
            ),
            SampleFormat::I16 => {
                let buffer = self.buffer.clone();
                let capturing = self.capturing.clone();
                device.build_input_stream(
                    &config.into(),
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        if !capturing.load(Ordering::SeqCst) {
                            return;
                        }
                        let mono: Vec<f32> = data
                            .chunks(channels)
                            .map(|frame| {
                                frame
                                    .iter()
                                    .map(|&s| s as f32 / i16::MAX as f32)
                                    .sum::<f32>()
                                    / channels as f32
                            })
                            .collect();
                        let mut buf = buffer.lock();
                        if buf.len() < MAX_BUFFER_SAMPLES {
                            buf.extend_from_slice(&mono);
                        }
                    },
                    err_fn,
                    None,
                )
            }
            format => {
                return Err(CoreError::AudioCapture {
                    code: oneshim_core::error_codes::AudioCode::CaptureFailed,
                    message: format!("unsupported sample format: {format:?}"),
                });
            }
        }
        .map_err(|e| CoreError::AudioCapture {
            code: oneshim_core::error_codes::AudioCode::CaptureFailed,
            message: format!("build stream: {e}"),
        })?;

        stream.play().map_err(|e| CoreError::AudioCapture {
            code: oneshim_core::error_codes::AudioCode::CaptureFailed,
            message: format!("play stream: {e}"),
        })?;

        // Set capturing = true only AFTER stream.play() succeeds.
        // If build_input_stream or play() fails above, capturing stays false
        // so start() can be retried (no permanent bricking).
        self.capturing.store(true, Ordering::SeqCst);
        *self.stream.lock() = Some(stream);
        Ok(())
    }

    /// Stop capturing and return the accumulated audio buffer resampled to 16kHz.
    pub fn stop(&self) -> Result<AudioBuffer, CoreError> {
        self.capturing.store(false, Ordering::SeqCst);

        // Drop the stream to release the device
        if let Some(stream) = self.stream.lock().take() {
            drop(stream);
        }

        let raw_samples: Vec<f32> = std::mem::take(&mut *self.buffer.lock());
        let native_rate = (*self.native_rate.lock()).unwrap_or(TARGET_SAMPLE_RATE);

        debug!(
            samples = raw_samples.len(),
            native_rate, "audio capture stopped"
        );

        if raw_samples.is_empty() {
            return Ok(AudioBuffer::new(Vec::new()));
        }

        // Resample to 16kHz if needed
        let samples_16k = if native_rate != TARGET_SAMPLE_RATE {
            resample(&raw_samples, native_rate, TARGET_SAMPLE_RATE)?
        } else {
            raw_samples
        };

        Ok(AudioBuffer::new(samples_16k))
    }

    /// Whether currently capturing (PTT mode).
    pub fn is_capturing(&self) -> bool {
        self.capturing.load(Ordering::SeqCst)
    }

    /// Start VAD listening mode. `on_speech_signal` is called (on audio thread)
    /// when speech ends — keep it lightweight (e.g., channel send).
    ///
    /// `#[allow(clippy::significant_drop_tightening)]`: the audio callback
    /// closure takes `speech_buffer.lock()` briefly to append samples. This is
    /// a hot path on the audio thread; wrapping in extra scope doesn't help
    /// because the lock is dropped at the end of the match arm anyway.
    #[allow(clippy::significant_drop_tightening)]
    pub fn start_vad(
        &self,
        config: VadConfig,
        on_speech_signal: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<(), CoreError> {
        if self.capturing.load(Ordering::SeqCst) {
            return Err(CoreError::AudioCapture {
                code: oneshim_core::error_codes::AudioCode::CaptureFailed,
                message: "cannot start VAD while PTT is active".into(),
            });
        }
        if self.vad_active.load(Ordering::SeqCst) {
            return Err(CoreError::AudioCapture {
                code: oneshim_core::error_codes::AudioCode::CaptureFailed,
                message: "VAD already active".into(),
            });
        }

        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| CoreError::AudioCapture {
                code: oneshim_core::error_codes::AudioCode::CaptureFailed,
                message: "no input device available".into(),
            })?;

        let stream_config = device
            .default_input_config()
            .map_err(|e| CoreError::AudioCapture {
                code: oneshim_core::error_codes::AudioCode::CaptureFailed,
                message: format!("input config error: {e}"),
            })?;

        let native_rate = stream_config.sample_rate();
        let channels = stream_config.channels() as usize;

        debug!(
            sample_rate = native_rate,
            channels, "starting VAD audio capture"
        );

        *self.native_rate.lock() = Some(native_rate);
        self.speech_buffer.lock().clear();

        let speech_buffer = self.speech_buffer.clone();
        let vad_active = self.vad_active.clone();

        // VadDetector is owned by the closure — no mutex needed for VAD state.
        // Pre-buffer: retain last ~400ms of audio to capture speech onset before
        // min_speech_ms confirmation. Uses VecDeque as a ring buffer.
        let pre_buffer_samples = (native_rate as usize) * 400 / 1000; // 400ms at native rate

        let err_fn = |err: cpal::StreamError| {
            warn!("audio stream error: {err}");
        };

        // Shared VAD callback logic extracted to avoid F32/I16 duplication (I7 fix).
        #[allow(clippy::type_complexity)]
        let build_vad_callback = |speech_buffer: Arc<Mutex<Vec<f32>>>,
                                  vad_active: Arc<AtomicBool>,
                                  on_signal: Arc<dyn Fn() + Send + Sync>|
         -> Box<dyn FnMut(&[f32]) + Send> {
            let mut vad =
                VadDetector::new(config.threshold, config.silence_ms, config.min_speech_ms);
            let mut pre_buf: std::collections::VecDeque<f32> =
                std::collections::VecDeque::with_capacity(pre_buffer_samples);
            let mut speech_active = false;

            Box::new(move |mono: &[f32]| {
                if !vad_active.load(Ordering::SeqCst) {
                    return;
                }

                let event = vad.process_chunk(mono);
                match event {
                    VadEvent::SpeechStarted => {
                        speech_active = true;
                        // Flush pre-buffer into speech_buffer (captures onset audio)
                        let mut buf = speech_buffer.lock();
                        if buf.len() < MAX_BUFFER_SAMPLES {
                            buf.extend(pre_buf.iter());
                            buf.extend_from_slice(mono);
                        }
                        pre_buf.clear();
                    }
                    VadEvent::SpeechContinuing => {
                        let mut buf = speech_buffer.lock();
                        if buf.len() < MAX_BUFFER_SAMPLES {
                            buf.extend_from_slice(mono);
                        }
                    }
                    VadEvent::SpeechEnded => {
                        speech_active = false;
                        on_signal();
                    }
                    VadEvent::None => {
                        if !speech_active {
                            // Maintain rolling pre-buffer
                            for &s in mono {
                                if pre_buf.len() >= pre_buffer_samples {
                                    pre_buf.pop_front();
                                }
                                pre_buf.push_back(s);
                            }
                        }
                    }
                }
            })
        };

        let stream = match stream_config.sample_format() {
            SampleFormat::F32 => {
                let mut cb = build_vad_callback(
                    speech_buffer.clone(),
                    vad_active.clone(),
                    on_speech_signal.clone(),
                );
                device.build_input_stream(
                    &stream_config.into(),
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        let mono: Vec<f32> = data
                            .chunks(channels)
                            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
                            .collect();
                        cb(&mono);
                    },
                    err_fn,
                    None,
                )
            }
            SampleFormat::I16 => {
                let mut cb = build_vad_callback(
                    self.speech_buffer.clone(),
                    self.vad_active.clone(),
                    on_speech_signal.clone(),
                );
                device.build_input_stream(
                    &stream_config.into(),
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        let mono: Vec<f32> = data
                            .chunks(channels)
                            .map(|frame| {
                                frame
                                    .iter()
                                    .map(|&s| s as f32 / i16::MAX as f32)
                                    .sum::<f32>()
                                    / channels as f32
                            })
                            .collect();
                        cb(&mono);
                    },
                    err_fn,
                    None,
                )
            }
            format => {
                return Err(CoreError::AudioCapture {
                    code: oneshim_core::error_codes::AudioCode::CaptureFailed,
                    message: format!("unsupported sample format: {format:?}"),
                });
            }
        }
        .map_err(|e| CoreError::AudioCapture {
            code: oneshim_core::error_codes::AudioCode::CaptureFailed,
            message: format!("build stream: {e}"),
        })?;

        stream.play().map_err(|e| CoreError::AudioCapture {
            code: oneshim_core::error_codes::AudioCode::CaptureFailed,
            message: format!("play stream: {e}"),
        })?;

        self.vad_active.store(true, Ordering::SeqCst);
        *self.stream.lock() = Some(stream);
        Ok(())
    }

    /// Stop VAD listening mode.
    pub fn stop_vad(&self) -> Result<(), CoreError> {
        self.vad_active.store(false, Ordering::SeqCst);
        if let Some(stream) = self.stream.lock().take() {
            drop(stream);
        }
        self.speech_buffer.lock().clear();
        debug!("VAD listening stopped");
        Ok(())
    }

    /// Whether VAD listening is currently active.
    pub fn is_vad_active(&self) -> bool {
        self.vad_active.load(Ordering::SeqCst)
    }

    /// Drain the speech buffer and return resampled 16kHz audio.
    /// Called by the receiver task after on_speech_signal fires.
    pub fn drain_speech_buffer(&self) -> Result<AudioBuffer, CoreError> {
        let raw_samples: Vec<f32> = std::mem::take(&mut *self.speech_buffer.lock());
        let native_rate = (*self.native_rate.lock()).unwrap_or(TARGET_SAMPLE_RATE);

        debug!(
            samples = raw_samples.len(),
            native_rate, "draining VAD speech buffer"
        );

        if raw_samples.is_empty() {
            return Ok(AudioBuffer::new(Vec::new()));
        }

        let samples_16k = if native_rate != TARGET_SAMPLE_RATE {
            resample(&raw_samples, native_rate, TARGET_SAMPLE_RATE)?
        } else {
            raw_samples
        };

        Ok(AudioBuffer::new(samples_16k))
    }
}

impl Drop for AudioCapture {
    fn drop(&mut self) {
        self.capturing.store(false, Ordering::SeqCst);
        self.vad_active.store(false, Ordering::SeqCst);
        // Stream is dropped automatically by Mutex<Option<Stream>>
    }
}

impl oneshim_core::ports::audio_capture::AudioCapturePort for AudioCapture {
    fn start(&self) -> Result<(), CoreError> {
        AudioCapture::start(self)
    }

    fn stop(&self) -> Result<AudioBuffer, CoreError> {
        AudioCapture::stop(self)
    }

    fn is_capturing(&self) -> bool {
        AudioCapture::is_capturing(self)
    }

    fn start_vad(
        &self,
        config: VadConfig,
        on_speech_signal: std::sync::Arc<dyn Fn() + Send + Sync>,
    ) -> Result<(), CoreError> {
        AudioCapture::start_vad(self, config, on_speech_signal)
    }

    fn stop_vad(&self) -> Result<(), CoreError> {
        AudioCapture::stop_vad(self)
    }

    fn is_vad_active(&self) -> bool {
        AudioCapture::is_vad_active(self)
    }

    fn drain_speech_buffer(&self) -> Result<AudioBuffer, CoreError> {
        AudioCapture::drain_speech_buffer(self)
    }
}

/// Resample mono f32 audio from `from_rate` to `to_rate` using rubato SincFixedIn.
fn resample(input: &[f32], from_rate: u32, to_rate: u32) -> Result<Vec<f32>, CoreError> {
    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 256,
        window: WindowFunction::BlackmanHarris2,
    };

    let chunk_size = 1024;
    let mut resampler = SincFixedIn::<f32>::new(
        to_rate as f64 / from_rate as f64,
        1.0,
        params,
        chunk_size,
        1, // mono
    )
    .map_err(|e| CoreError::AudioCapture {
        code: oneshim_core::error_codes::AudioCode::CaptureFailed,
        message: format!("resampler init: {e}"),
    })?;

    let mut output = Vec::with_capacity(
        (input.len() as f64 * to_rate as f64 / from_rate as f64) as usize + chunk_size,
    );

    // Process in chunks of chunk_size
    let mut pos = 0;
    while pos + chunk_size <= input.len() {
        let chunk = &input[pos..pos + chunk_size];
        let resampled = resampler
            .process(&[chunk], None)
            .map_err(|e| CoreError::AudioCapture {
                code: oneshim_core::error_codes::AudioCode::CaptureFailed,
                message: format!("resample: {e}"),
            })?;
        output.extend_from_slice(&resampled[0]);
        pos += chunk_size;
    }

    // Handle remaining samples by zero-padding to chunk_size
    if pos < input.len() {
        let mut last_chunk = vec![0.0f32; chunk_size];
        let remaining = input.len() - pos;
        last_chunk[..remaining].copy_from_slice(&input[pos..]);
        let resampled =
            resampler
                .process(&[last_chunk], None)
                .map_err(|e| CoreError::AudioCapture {
                    code: oneshim_core::error_codes::AudioCode::CaptureFailed,
                    message: format!("resample tail: {e}"),
                })?;
        let expected = (remaining as f64 * to_rate as f64 / from_rate as f64).ceil() as usize;
        let take = expected.min(resampled[0].len());
        output.extend_from_slice(&resampled[0][..take]);
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_capture_not_capturing() {
        let capture = AudioCapture::new();
        assert!(!capture.is_capturing());
    }

    #[test]
    fn stop_without_start_returns_empty_buffer() {
        let capture = AudioCapture::new();
        let buffer = capture.stop().unwrap();
        assert!(buffer.is_empty());
        assert_eq!(buffer.sample_rate, 16000);
    }

    #[test]
    fn resample_identity_when_same_rate() {
        // Inject samples at 16kHz — should bypass resampling entirely.
        let capture = AudioCapture::new();
        let input: Vec<f32> = (0..1600).map(|i| (i as f32 * 0.001).sin()).collect();
        capture.buffer.lock().extend_from_slice(&input);
        *capture.native_rate.lock() = Some(16000);
        capture.capturing.store(true, Ordering::SeqCst);
        let buf = capture.stop().unwrap();
        assert_eq!(buf.samples.len(), 1600);
        assert_eq!(buf.sample_rate, 16000);
        // Samples should be identical (no resampling).
        assert_eq!(buf.samples, input);
    }

    #[test]
    fn start_fails_if_vad_active() {
        let capture = AudioCapture::new();
        // Simulate VAD being active
        capture.vad_active.store(true, Ordering::SeqCst);
        let result = capture.start();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("cannot start PTT while VAD is active"));
    }

    #[test]
    fn start_vad_fails_if_ptt_capturing() {
        let capture = AudioCapture::new();
        // Simulate PTT capturing being active
        capture.capturing.store(true, Ordering::SeqCst);
        let config = oneshim_core::models::audio::VadConfig {
            threshold: 0.02,
            silence_ms: 800,
            min_speech_ms: 300,
        };
        let signal = std::sync::Arc::new(|| {});
        let result = capture.start_vad(config, signal);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("cannot start VAD while PTT is active"));
    }

    #[test]
    fn drain_speech_buffer_empty() {
        let capture = AudioCapture::new();
        *capture.native_rate.lock() = Some(16000);
        let buffer = capture.drain_speech_buffer().unwrap();
        assert!(buffer.is_empty());
    }

    #[test]
    fn drain_speech_buffer_with_data() {
        let capture = AudioCapture::new();
        *capture.native_rate.lock() = Some(16000);
        // Add some samples to the speech buffer
        let samples: Vec<f32> = (0..1600).map(|i| (i as f32 * 0.001).sin()).collect();
        capture.speech_buffer.lock().extend_from_slice(&samples);

        let buffer = capture.drain_speech_buffer().unwrap();
        assert_eq!(buffer.samples.len(), 1600);
        assert_eq!(buffer.sample_rate, 16000);

        // Buffer should be drained
        assert!(capture.speech_buffer.lock().is_empty());
    }

    #[test]
    fn resample_48k_to_16k() {
        // 48000 Hz sine wave → 16000 Hz: output should be ~1/3 the length
        let input: Vec<f32> = (0..4800)
            .map(|i| (i as f32 * 2.0 * std::f32::consts::PI * 440.0 / 48000.0).sin())
            .collect();
        let output = resample(&input, 48000, 16000).unwrap();
        // Expected ~1600 samples (4800 * 16000/48000)
        assert!((output.len() as i32 - 1600).unsigned_abs() < 50);
    }
}
