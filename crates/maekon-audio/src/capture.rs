//! Cross-platform microphone capture with automatic resampling to 16kHz mono.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use parking_lot::Mutex;
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use tracing::{debug, warn};
use zeroize::Zeroize;

use maekon_core::error::CoreError;
use maekon_core::models::audio::{AudioBuffer, VadConfig};

use crate::vad::{VadDetector, VadEvent};

const TARGET_SAMPLE_RATE: u32 = 16000;
/// Hard cap: 120s at 48kHz mono = 5.76M samples (~23MB). Prevents unbounded growth.
const MAX_BUFFER_SAMPLES: usize = 120 * 48_000;

/// Capture-mode discriminants for the single shared `mode` atomic.
///
/// PTT (`start`) and VAD (`start_vad`) are mutually exclusive: only one can own
/// the input device at a time. Coordinating both modes through a SINGLE atomic
/// (rather than two independent bools) lets each entry point claim the device
/// with one `compare_exchange(MODE_IDLE, ...)`, closing the cross-mode TOCTOU
/// window where PTT and VAD could each pass an independent check and both build
/// a live cpal stream (#6102-25).
const MODE_IDLE: u8 = 0;
const MODE_PTT: u8 = 1;
const MODE_VAD: u8 = 2;

/// Resets the shared `mode` atomic back to [`MODE_IDLE`] on drop unless
/// [`disarm`](ClaimGuard::disarm)ed.
///
/// `start`/`start_vad` claim the device with a single `compare_exchange` BEFORE
/// device/stream init (closing the TOCTOU window, #6102-24/#6102-25). If any
/// fallible setup then fails — including via `?` — this guard resets the mode to
/// `MODE_IDLE` so the device stays retriable; on success the caller `disarm`s it
/// so the claimed mode remains set.
struct ClaimGuard<'a> {
    mode: &'a AtomicU8,
    armed: bool,
}

impl<'a> ClaimGuard<'a> {
    fn new(mode: &'a AtomicU8) -> Self {
        Self { mode, armed: true }
    }

    /// Keep the claim (do not reset on drop). Consumes the guard.
    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for ClaimGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.mode.store(MODE_IDLE, Ordering::SeqCst);
        }
    }
}

pub struct AudioCapture {
    buffer: Arc<Mutex<Vec<f32>>>,
    /// Single shared capture-mode atomic ([`MODE_IDLE`]/[`MODE_PTT`]/[`MODE_VAD`]).
    /// PTT and VAD are mutually exclusive and both claim this one atomic, so the
    /// two modes can never concurrently own the input device (#6102-25).
    mode: Arc<AtomicU8>,
    stream: Mutex<Option<cpal::Stream>>,
    /// Native sample rate of the input device (set during start).
    native_rate: Mutex<Option<u32>>,
    /// Accumulated speech samples during VAD mode (raw, native rate).
    speech_buffer: Arc<Mutex<Vec<f32>>>,
    /// Configured maximum recording duration (seconds). Bounds the PTT/VAD
    /// capture buffers to `max_recording_secs * native_rate` samples (#6342),
    /// clamped to `[native_rate, MAX_BUFFER_SAMPLES]`. Previously the configured
    /// limit was ignored and only the hardcoded `MAX_BUFFER_SAMPLES` ceiling
    /// (120s @ 48kHz) applied.
    max_recording_secs: u32,
    /// Serializes start/stop lifecycle transitions (#7078). The shared `mode`
    /// atomic closes start-vs-start and PTT-vs-VAD races, but NOT start-vs-stop:
    /// `start`/`start_vad` write the built stream into `self.stream` only AFTER
    /// the device is playing, while `stop`/`stop_vad` unconditionally set
    /// `MODE_IDLE` and `take()` the stream slot with no mode check. Interleaved,
    /// a `stop()` could null a still-empty slot while a concurrent `start()` is
    /// mid-build, after which `start()` stores its now-playing stream — leaving
    /// the microphone physically open (OS recording indicator lit) while
    /// `mode == MODE_IDLE`, until the next lifecycle call. Holding this mutex
    /// across the entire start/stop critical section makes the transitions
    /// mutually exclusive so that interleave can never happen. The realtime audio
    /// callback never touches this mutex, so it cannot stall the audio thread or
    /// deadlock it; the methods never re-enter each other, so the non-reentrant
    /// lock is safe.
    lifecycle: Mutex<()>,
}

impl Default for AudioCapture {
    fn default() -> Self {
        // 60s matches `AudioConfig::default().max_recording_secs`.
        Self::new(60)
    }
}

/// Effective sample cap for a recording buffer: `max_recording_secs * native_rate`,
/// clamped so it never drops below one second of audio nor exceeds the hard
/// `MAX_BUFFER_SAMPLES` memory ceiling (#6342).
fn max_buffer_samples(max_recording_secs: u32, native_rate: u32) -> usize {
    (max_recording_secs as usize)
        .saturating_mul(native_rate as usize)
        .clamp(native_rate as usize, MAX_BUFFER_SAMPLES)
}

/// Securely wipe a captured-PCM buffer before its contents become unreachable
/// (#7077).
///
/// `Vec::clear()` only sets the length to 0, and `std::mem::take` moves the Vec
/// out to be freed — neither overwrites the backing allocation, so raw microphone
/// PCM (recorded human speech) lingers in freed/spare heap until something else
/// happens to reuse it (recoverable via a core dump, swap/hibernation file, or a
/// privileged memory scraper). `Vec::zeroize` overwrites every initialized sample
/// AND the spare capacity with zeros (best effort) and then truncates the length,
/// matching the `Zeroizing` treatment the (less sensitive) cloud STT API key
/// already receives (`cloud_stt.rs`). Call this on every stop/drain/Drop path
/// that retires raw audio.
fn wipe_samples(buf: &mut Vec<f32>) {
    buf.zeroize();
}

/// Lend a raw-PCM buffer to `consume`, then zeroize it before it drops, returning
/// whatever `consume` produced.
///
/// Several short-lived `Vec<f32>` buffers in this crate hold raw microphone PCM
/// that a consumer only borrows: the per-callback `mono` mixdown buffer in the
/// realtime audio callback (#7098), and the owned source samples the local
/// Whisper provider lends to inference (#7115, `whisper.rs`). Once the consumer
/// (PTT/VAD accumulation, or Whisper inference) has read what it needs, the local
/// buffer would otherwise be freed by `Vec::drop` WITHOUT overwriting its backing
/// allocation, leaving recorded speech recoverable in spare heap until reuse.
/// Wiping it here extends the #7077 stop/drain/Drop capture-buffer treatment to
/// every borrow-then-drop PCM consumer. The buffers are at most one utterance of
/// audio, so the extra zeroize is negligible. The consumer's return value is
/// forwarded so an inference call's `Result` can be `?`-propagated by the caller
/// after the wipe has already run.
pub(crate) fn consume_then_wipe<T>(buf: &mut Vec<f32>, consume: impl FnOnce(&[f32]) -> T) -> T {
    let out = consume(buf.as_slice());
    wipe_samples(buf);
    out
}

/// Copy up to `take` samples from a freshly-resampled chunk into `output`, then
/// zeroize the whole chunk.
///
/// rubato's `process` allocates a NEW output `Vec` per call, so `chunk` is a
/// resampled-PCM copy this crate owns. After the needed samples are appended to
/// `output`, the per-chunk copy would drop un-overwritten; wiping it keeps the
/// resampled speech from lingering in the freed intermediate allocation (#7098).
/// (The final `output` buffer is moved out to the caller as an `AudioBuffer`, so
/// it cannot be wiped here — see `resample`.)
fn drain_resampled_chunk(output: &mut Vec<f32>, chunk: &mut Vec<f32>, take: usize) {
    let take = take.min(chunk.len());
    output.extend_from_slice(&chunk[..take]);
    wipe_samples(chunk);
}

/// Validate the device-reported input channel count before building a stream.
///
/// A 0-channel device is degenerate: the mono downmix in the audio callback does
/// `data.chunks(channels)`, and `slice::chunks(0)` panics. Rejecting it here keeps
/// the panic out of the realtime audio thread. Extracted as a pure function so the
/// guard is unit-testable without a live input device.
fn validate_channels(channels: usize) -> Result<(), CoreError> {
    if channels == 0 {
        return Err(CoreError::AudioCapture {
            code: maekon_core::error_codes::AudioCode::CaptureFailed,
            message: "device reported 0 input channels".into(),
        });
    }
    Ok(())
}

/// Minimum acceptable device sample rate. The capture path always resamples
/// `native_rate -> 16000`, and the resampler's output (and its eager capacity
/// hint) scales with `16000 / native_rate`. A 0 Hz rate gives an infinite ratio
/// (process abort); a degenerate *tiny* nonzero rate (e.g. 1 Hz) gives a ~16000×
/// blow-up that, against a maxed PTT buffer, still requests a hundreds-of-GB
/// allocation and aborts the process (review4 re-verify). No real capture device
/// is sub-telephony, so flooring at 8 kHz bounds the ratio to ≤2 without rejecting
/// any real hardware (cpal default rates are 8k–192k).
const MIN_SAMPLE_RATE: u32 = 8_000;

/// Reject a degenerate device sample rate (0 Hz or implausibly low) BEFORE the
/// stream is built, so a fake/broken device stays retriable instead of crashing
/// the realtime path in `resample`. Symmetric to `validate_channels`; extracted as
/// a pure function so the guard is unit-testable without a live device.
fn validate_sample_rate(rate: u32) -> Result<(), CoreError> {
    if rate < MIN_SAMPLE_RATE {
        return Err(CoreError::AudioCapture {
            code: maekon_core::error_codes::AudioCode::CaptureFailed,
            message: format!(
                "device reported an implausible {rate} Hz sample rate (minimum {MIN_SAMPLE_RATE} Hz)"
            ),
        });
    }
    Ok(())
}

impl AudioCapture {
    pub fn new(max_recording_secs: u32) -> Self {
        Self {
            buffer: Arc::new(Mutex::new(Vec::new())),
            mode: Arc::new(AtomicU8::new(MODE_IDLE)),
            stream: Mutex::new(None),
            native_rate: Mutex::new(None),
            speech_buffer: Arc::new(Mutex::new(Vec::new())),
            max_recording_secs,
            lifecycle: Mutex::new(()),
        }
    }

    /// Start capturing audio from the default input device (PTT mode).
    pub fn start(&self) -> Result<(), CoreError> {
        // #7078: serialize against any concurrent stop()/stop_vad()/start_vad()
        // for the whole critical section. Held until this method returns so a
        // stop() cannot null the stream slot while this start() is mid-build.
        let _lifecycle = self.lifecycle.lock();

        // #6102-24/#6102-25: claim the device ATOMICALLY for PTT mode with a
        // single compare_exchange (MODE_IDLE -> MODE_PTT). Because PTT and VAD
        // share this one atomic, a concurrent start()/start_vad() pair can never
        // both pass their check and both build a cpal stream — the loser sees a
        // non-Idle mode and bails out here. A failure means another mode (or a
        // concurrent same-mode start) already owns the device.
        if self
            .mode
            .compare_exchange(MODE_IDLE, MODE_PTT, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(CoreError::AudioCapture {
                code: maekon_core::error_codes::AudioCode::CaptureFailed,
                message: "already capturing".into(),
            });
        }
        // Release the claim on any early-return below (device/config/build/play
        // failure, incl. `?`) so a failed start stays retriable; disarmed on success.
        let claim = ClaimGuard::new(&self.mode);

        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| CoreError::AudioCapture {
                code: maekon_core::error_codes::AudioCode::CaptureFailed,
                message: "no input device available".into(),
            })?;

        let config = device
            .default_input_config()
            .map_err(|e| CoreError::AudioCapture {
                code: maekon_core::error_codes::AudioCode::CaptureFailed,
                message: format!("input config error: {e}"),
            })?;

        let native_rate = config.sample_rate();
        let channels = config.channels() as usize;

        // Reject a degenerate 0-channel device BEFORE building the stream (the
        // callback downmix `data.chunks(channels)` panics on `chunks(0)`). `claim`
        // resets the mode on this early return so the device stays retriable.
        // Mirrors the unsupported-sample-format guard below.
        validate_channels(channels)?;
        validate_sample_rate(native_rate)?;

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

        // #6342: honor the configured max recording duration (was ignored — only
        // the hardcoded MAX_BUFFER_SAMPLES ceiling applied).
        let max_samples = max_buffer_samples(self.max_recording_secs, native_rate);

        // Wipe (not just clear) the buffer for the new recording so any residual
        // PCM from a prior session is overwritten rather than left in the reused
        // backing allocation (#7077).
        wipe_samples(&mut self.buffer.lock());

        let buffer = self.buffer.clone();
        let mode = self.mode.clone();

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
                    if mode.load(Ordering::SeqCst) != MODE_PTT {
                        return;
                    }
                    let mut mono: Vec<f32> = data
                        .chunks(channels)
                        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
                        .collect();
                    // #7098: wipe the transient mono PCM once it has been accumulated.
                    consume_then_wipe(&mut mono, |m| {
                        let mut buf = buffer.lock();
                        if buf.len() < max_samples {
                            buf.extend_from_slice(m);
                        }
                    });
                },
                err_fn,
                None,
            ),
            SampleFormat::I16 => {
                let buffer = self.buffer.clone();
                let mode = self.mode.clone();
                device.build_input_stream(
                    &config.into(),
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        if mode.load(Ordering::SeqCst) != MODE_PTT {
                            return;
                        }
                        let mut mono: Vec<f32> = data
                            .chunks(channels)
                            .map(|frame| {
                                frame
                                    .iter()
                                    .map(|&s| s as f32 / i16::MAX as f32)
                                    .sum::<f32>()
                                    / channels as f32
                            })
                            .collect();
                        // #7098: wipe the transient mono PCM once it has been accumulated.
                        consume_then_wipe(&mut mono, |m| {
                            let mut buf = buffer.lock();
                            if buf.len() < max_samples {
                                buf.extend_from_slice(m);
                            }
                        });
                    },
                    err_fn,
                    None,
                )
            }
            format => {
                return Err(CoreError::AudioCapture {
                    code: maekon_core::error_codes::AudioCode::CaptureFailed,
                    message: format!("unsupported sample format: {format:?}"),
                });
            }
        }
        .map_err(|e| CoreError::AudioCapture {
            code: maekon_core::error_codes::AudioCode::CaptureFailed,
            message: format!("build stream: {e}"),
        })?;

        stream.play().map_err(|e| CoreError::AudioCapture {
            code: maekon_core::error_codes::AudioCode::CaptureFailed,
            message: format!("play stream: {e}"),
        })?;

        // Setup succeeded — keep the claim (mode stays MODE_PTT). If build or
        // play() had failed above, `claim` would have reset mode to MODE_IDLE on
        // drop so start() stays retriable (no permanent bricking).
        claim.disarm();
        *self.stream.lock() = Some(stream);
        Ok(())
    }

    /// Stop capturing and return the accumulated audio buffer resampled to 16kHz.
    pub fn stop(&self) -> Result<AudioBuffer, CoreError> {
        // #7078: serialize against any concurrent start()/start_vad() so this
        // stop() cannot null the stream slot a start() is about to populate.
        let _lifecycle = self.lifecycle.lock();

        // Release the device claim back to Idle so a subsequent PTT/VAD start can
        // re-acquire it. The callback's `mode != MODE_PTT` check also short-circuits.
        self.mode.store(MODE_IDLE, Ordering::SeqCst);

        // Drop the stream to release the device
        if let Some(stream) = self.stream.lock().take() {
            drop(stream);
        }

        let mut raw_samples: Vec<f32> = std::mem::take(&mut *self.buffer.lock());
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
            // `mem::take` moved the raw native-rate PCM out of the shared buffer
            // into `raw_samples`; wipe it once it has been resampled so the
            // recorded speech is not left recoverable in freed heap when
            // `raw_samples` drops (#7077). Wipe on both success and error.
            let resampled = resample(&raw_samples, native_rate, TARGET_SAMPLE_RATE);
            wipe_samples(&mut raw_samples);
            resampled?
        } else {
            // Identity rate: `raw_samples` IS the returned payload, so it is moved
            // into the result rather than wiped here.
            raw_samples
        };

        Ok(AudioBuffer::new(samples_16k))
    }

    /// Whether currently capturing (PTT mode).
    pub fn is_capturing(&self) -> bool {
        self.mode.load(Ordering::SeqCst) == MODE_PTT
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
        // #7078: serialize against any concurrent stop()/stop_vad()/start() for
        // the whole critical section so a stop cannot null the stream slot while
        // this start_vad() is mid-build.
        let _lifecycle = self.lifecycle.lock();

        // #6102-24/#6102-25: claim the device ATOMICALLY for VAD mode with a
        // single compare_exchange (MODE_IDLE -> MODE_VAD). Sharing this one atomic
        // with start() means a concurrent PTT start can never co-own the device:
        // whichever entry point loses the CAS sees a non-Idle mode and bails out.
        if self
            .mode
            .compare_exchange(MODE_IDLE, MODE_VAD, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(CoreError::AudioCapture {
                code: maekon_core::error_codes::AudioCode::CaptureFailed,
                message: "VAD already active".into(),
            });
        }
        // Release the claim on any early-return below (incl. `?`); disarmed on success.
        let claim = ClaimGuard::new(&self.mode);

        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| CoreError::AudioCapture {
                code: maekon_core::error_codes::AudioCode::CaptureFailed,
                message: "no input device available".into(),
            })?;

        let stream_config = device
            .default_input_config()
            .map_err(|e| CoreError::AudioCapture {
                code: maekon_core::error_codes::AudioCode::CaptureFailed,
                message: format!("input config error: {e}"),
            })?;

        let native_rate = stream_config.sample_rate();
        let channels = stream_config.channels() as usize;

        // Reject a degenerate 0-channel device BEFORE building the stream (the
        // callback downmix `data.chunks(channels)` panics on `chunks(0)`). `claim`
        // resets the mode on this early return so the device stays retriable.
        validate_channels(channels)?;
        validate_sample_rate(native_rate)?;

        debug!(
            sample_rate = native_rate,
            channels, "starting VAD audio capture"
        );

        *self.native_rate.lock() = Some(native_rate);
        // Wipe (not just clear) residual speech PCM from a prior session before
        // reusing the backing allocation (#7077).
        wipe_samples(&mut self.speech_buffer.lock());

        // #6342: honor the configured max recording duration for the VAD buffer.
        let max_samples = max_buffer_samples(self.max_recording_secs, native_rate);

        let speech_buffer = self.speech_buffer.clone();
        let mode = self.mode.clone();

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
                                  mode: Arc<AtomicU8>,
                                  on_signal: Arc<dyn Fn() + Send + Sync>|
         -> Box<dyn FnMut(&[f32]) + Send> {
            let mut vad =
                VadDetector::new(config.threshold, config.silence_ms, config.min_speech_ms);
            let mut pre_buf: std::collections::VecDeque<f32> =
                std::collections::VecDeque::with_capacity(pre_buffer_samples);
            let mut speech_active = false;

            Box::new(move |mono: &[f32]| {
                if mode.load(Ordering::SeqCst) != MODE_VAD {
                    return;
                }

                let event = vad.process_chunk(mono);
                match event {
                    VadEvent::SpeechStarted => {
                        speech_active = true;
                        // Flush pre-buffer into speech_buffer (captures onset audio)
                        let mut buf = speech_buffer.lock();
                        if buf.len() < max_samples {
                            buf.extend(pre_buf.iter());
                            buf.extend_from_slice(mono);
                        }
                        pre_buf.clear();
                    }
                    VadEvent::SpeechContinuing => {
                        let mut buf = speech_buffer.lock();
                        if buf.len() < max_samples {
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
                    mode.clone(),
                    on_speech_signal.clone(),
                );
                device.build_input_stream(
                    &stream_config.into(),
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        let mut mono: Vec<f32> = data
                            .chunks(channels)
                            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
                            .collect();
                        // #7098: wipe the transient mono PCM after the VAD consumes it.
                        consume_then_wipe(&mut mono, |m| cb(m));
                    },
                    err_fn,
                    None,
                )
            }
            SampleFormat::I16 => {
                let mut cb = build_vad_callback(
                    self.speech_buffer.clone(),
                    self.mode.clone(),
                    on_speech_signal.clone(),
                );
                device.build_input_stream(
                    &stream_config.into(),
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        let mut mono: Vec<f32> = data
                            .chunks(channels)
                            .map(|frame| {
                                frame
                                    .iter()
                                    .map(|&s| s as f32 / i16::MAX as f32)
                                    .sum::<f32>()
                                    / channels as f32
                            })
                            .collect();
                        // #7098: wipe the transient mono PCM after the VAD consumes it.
                        consume_then_wipe(&mut mono, |m| cb(m));
                    },
                    err_fn,
                    None,
                )
            }
            format => {
                return Err(CoreError::AudioCapture {
                    code: maekon_core::error_codes::AudioCode::CaptureFailed,
                    message: format!("unsupported sample format: {format:?}"),
                });
            }
        }
        .map_err(|e| CoreError::AudioCapture {
            code: maekon_core::error_codes::AudioCode::CaptureFailed,
            message: format!("build stream: {e}"),
        })?;

        stream.play().map_err(|e| CoreError::AudioCapture {
            code: maekon_core::error_codes::AudioCode::CaptureFailed,
            message: format!("play stream: {e}"),
        })?;

        // Setup succeeded — keep the claim (mode stays MODE_VAD).
        claim.disarm();
        *self.stream.lock() = Some(stream);
        Ok(())
    }

    /// Stop VAD listening mode.
    pub fn stop_vad(&self) -> Result<(), CoreError> {
        // #7078: serialize against any concurrent start()/start_vad() so this
        // stop cannot null the stream slot a start is about to populate.
        let _lifecycle = self.lifecycle.lock();

        // Release the device claim back to Idle so a subsequent PTT/VAD start can
        // re-acquire it. The VAD callback's `mode != MODE_VAD` check also short-circuits.
        self.mode.store(MODE_IDLE, Ordering::SeqCst);
        if let Some(stream) = self.stream.lock().take() {
            drop(stream);
        }
        // Wipe (not just clear) the accumulated speech PCM so it is not left
        // recoverable in the retained backing allocation (#7077).
        wipe_samples(&mut self.speech_buffer.lock());
        debug!("VAD listening stopped");
        Ok(())
    }

    /// Whether VAD listening is currently active.
    pub fn is_vad_active(&self) -> bool {
        self.mode.load(Ordering::SeqCst) == MODE_VAD
    }

    /// Drain the speech buffer and return resampled 16kHz audio.
    /// Called by the receiver task after on_speech_signal fires.
    pub fn drain_speech_buffer(&self) -> Result<AudioBuffer, CoreError> {
        let mut raw_samples: Vec<f32> = std::mem::take(&mut *self.speech_buffer.lock());
        let native_rate = (*self.native_rate.lock()).unwrap_or(TARGET_SAMPLE_RATE);

        debug!(
            samples = raw_samples.len(),
            native_rate, "draining VAD speech buffer"
        );

        if raw_samples.is_empty() {
            return Ok(AudioBuffer::new(Vec::new()));
        }

        let samples_16k = if native_rate != TARGET_SAMPLE_RATE {
            // `mem::take` moved the raw native-rate speech PCM out of the shared
            // buffer; wipe it once resampled so it is not left recoverable in
            // freed heap when `raw_samples` drops (#7077). Wipe on success/error.
            let resampled = resample(&raw_samples, native_rate, TARGET_SAMPLE_RATE);
            wipe_samples(&mut raw_samples);
            resampled?
        } else {
            // Identity rate: `raw_samples` IS the returned payload.
            raw_samples
        };

        Ok(AudioBuffer::new(samples_16k))
    }
}

impl Drop for AudioCapture {
    fn drop(&mut self) {
        // Reset the shared mode atomic so any in-flight callback short-circuits.
        self.mode.store(MODE_IDLE, Ordering::SeqCst);
        // Stop the device FIRST so no audio callback can append samples after the
        // wipe below (otherwise it could re-fill the buffer we just zeroized).
        if let Some(stream) = self.stream.lock().take() {
            drop(stream);
        }
        // Wipe any residual raw PCM so recorded speech is not left recoverable in
        // freed heap after this capture is dropped (#7077). `&mut self` guarantees
        // exclusive access, so these locks are uncontended.
        wipe_samples(&mut self.buffer.lock());
        wipe_samples(&mut self.speech_buffer.lock());
    }
}

impl maekon_core::ports::audio_capture::AudioCapturePort for AudioCapture {
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
    // Defense-in-depth (review4): a 0 Hz rate yields an infinite/zero ratio that
    // rubato does not reject and that overflows the output-capacity calc below to
    // usize::MAX, aborting the process. start()/start_vad() already reject 0 Hz
    // devices; this guards any other call path.
    if from_rate == 0 || to_rate == 0 {
        return Err(CoreError::AudioCapture {
            code: maekon_core::error_codes::AudioCode::CaptureFailed,
            message: format!("invalid resample rate (from={from_rate}, to={to_rate})"),
        });
    }
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
        code: maekon_core::error_codes::AudioCode::CaptureFailed,
        message: format!("resampler init: {e}"),
    })?;

    let mut output = Vec::with_capacity(
        (input.len() as f64 * to_rate as f64 / from_rate as f64) as usize + chunk_size,
    );

    // Process in chunks of chunk_size
    let mut pos = 0;
    while pos + chunk_size <= input.len() {
        let chunk = &input[pos..pos + chunk_size];
        let mut resampled =
            resampler
                .process(&[chunk], None)
                .map_err(|e| CoreError::AudioCapture {
                    code: maekon_core::error_codes::AudioCode::CaptureFailed,
                    message: format!("resample: {e}"),
                })?;
        // #7098: copy the full resampled chunk out, then wipe the per-chunk copy.
        let produced = resampled[0].len();
        drain_resampled_chunk(&mut output, &mut resampled[0], produced);
        pos += chunk_size;
    }

    // Handle remaining samples by zero-padding to chunk_size
    if pos < input.len() {
        let mut last_chunk = vec![0.0f32; chunk_size];
        let remaining = input.len() - pos;
        last_chunk[..remaining].copy_from_slice(&input[pos..]);
        // Borrow (not move) `last_chunk` into `process` so this function keeps
        // ownership and can wipe the zero-padded input-tail copy afterwards (#7098).
        let mut resampled = resampler
            .process(&[last_chunk.as_slice()], None)
            .map_err(|e| CoreError::AudioCapture {
                code: maekon_core::error_codes::AudioCode::CaptureFailed,
                message: format!("resample tail: {e}"),
            })?;
        let expected = (remaining as f64 * to_rate as f64 / from_rate as f64).ceil() as usize;
        // #7098: append only the expected tail samples, then wipe the per-chunk
        // resampled copy and the zero-padded input-tail copy this function owns.
        drain_resampled_chunk(&mut output, &mut resampled[0], expected);
        wipe_samples(&mut last_chunk);
    }

    // `output` holds the resampled PCM and is moved out to the caller as an
    // `AudioBuffer` (a `maekon-core` type); its final owner lives outside this crate,
    // so it cannot be wiped here. The intermediate per-chunk copies this function
    // owns are wiped above (#7098); the native-rate source is wiped by the
    // `stop`/`drain` caller (#7077).
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_capture_not_capturing() {
        let capture = AudioCapture::new(60);
        assert!(!capture.is_capturing());
    }

    #[test]
    fn stop_without_start_returns_empty_buffer() {
        let capture = AudioCapture::new(60);
        let buffer = capture.stop().unwrap();
        assert!(buffer.is_empty());
        assert_eq!(buffer.sample_rate, 16000);
    }

    #[test]
    fn resample_identity_when_same_rate() {
        // Inject samples at 16kHz — should bypass resampling entirely.
        let capture = AudioCapture::new(60);
        let input: Vec<f32> = (0..1600).map(|i| (i as f32 * 0.001).sin()).collect();
        capture.buffer.lock().extend_from_slice(&input);
        *capture.native_rate.lock() = Some(16000);
        capture.mode.store(MODE_PTT, Ordering::SeqCst);
        let buf = capture.stop().unwrap();
        assert_eq!(buf.samples.len(), 1600);
        assert_eq!(buf.sample_rate, 16000);
        // Samples should be identical (no resampling).
        assert_eq!(buf.samples, input);
    }

    fn test_vad_config() -> maekon_core::models::audio::VadConfig {
        maekon_core::models::audio::VadConfig {
            threshold: 0.02,
            silence_ms: 800,
            min_speech_ms: 300,
        }
    }

    #[test]
    fn start_fails_if_vad_active() {
        // VAD-then-PTT: the second (PTT) claim must be rejected because both
        // modes share a single `mode` atomic. Simulate VAD already owning the
        // device by setting the shared mode to MODE_VAD.
        let capture = AudioCapture::new(60);
        capture.mode.store(MODE_VAD, Ordering::SeqCst);
        let err = capture.start().unwrap_err().to_string();
        assert!(
            err.contains("already capturing"),
            "PTT start while VAD owns the device must be rejected; got: {err}"
        );
        // The losing claim must NOT have clobbered the existing VAD ownership.
        assert_eq!(capture.mode.load(Ordering::SeqCst), MODE_VAD);
    }

    #[test]
    fn start_vad_fails_if_ptt_capturing() {
        // PTT-then-VAD: the second (VAD) claim must be rejected. Simulate PTT
        // already owning the device by setting the shared mode to MODE_PTT.
        let capture = AudioCapture::new(60);
        capture.mode.store(MODE_PTT, Ordering::SeqCst);
        let signal = std::sync::Arc::new(|| {});
        let err = capture
            .start_vad(test_vad_config(), signal)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("VAD already active"),
            "VAD start while PTT owns the device must be rejected; got: {err}"
        );
        // The losing claim must NOT have clobbered the existing PTT ownership.
        assert_eq!(capture.mode.load(Ordering::SeqCst), MODE_PTT);
    }

    #[test]
    fn second_ptt_claim_rejected_when_already_capturing() {
        // Two concurrent PTT starts: only one wins the single CAS.
        let capture = AudioCapture::new(60);
        capture.mode.store(MODE_PTT, Ordering::SeqCst);
        let err = capture.start().unwrap_err().to_string();
        assert!(
            err.contains("already capturing"),
            "second PTT claim must be rejected; got: {err}"
        );
    }

    #[test]
    fn second_vad_claim_rejected_when_already_active() {
        // Two concurrent VAD starts: only one wins the single CAS.
        let capture = AudioCapture::new(60);
        capture.mode.store(MODE_VAD, Ordering::SeqCst);
        let signal = std::sync::Arc::new(|| {});
        let err = capture
            .start_vad(test_vad_config(), signal)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("VAD already active"),
            "second VAD claim must be rejected; got: {err}"
        );
    }

    #[test]
    fn zero_channel_device_rejected() {
        // A 0-channel device must be rejected before stream build so the callback
        // never executes `data.chunks(0)` (which panics). `validate_channels` is
        // the exact guard both start() and start_vad() invoke after reading the
        // device channel count.
        let err = validate_channels(0).unwrap_err().to_string();
        assert!(
            err.contains("0 input channels"),
            "0-channel device must be rejected; got: {err}"
        );
        // A normal channel count passes (mono + stereo).
        for channels in [1usize, 2] {
            validate_channels(channels)
                .unwrap_or_else(|e| panic!("{channels}-channel device must pass; got: {e:?}"));
        }
    }

    #[test]
    fn degenerate_sample_rate_device_rejected() {
        // 0 Hz and implausibly-low rates must be rejected before stream build
        // (review4): resample() scales output by 16000/native_rate, so 0 → infinite
        // ratio and a tiny nonzero rate → a hundreds-of-GB allocation, both aborting
        // the process. validate_sample_rate floors at MIN_SAMPLE_RATE; the guard is
        // invoked by both start() and start_vad() after reading the device rate.
        for rate in [0u32, 1, 7999] {
            let err = validate_sample_rate(rate).expect_err(&format!("{rate} Hz must be rejected"));
            assert!(
                matches!(
                    err,
                    CoreError::AudioCapture {
                        code: maekon_core::error_codes::AudioCode::CaptureFailed,
                        ..
                    }
                ),
                "{rate} Hz must yield AudioCapture/CaptureFailed; got: {err:?}"
            );
            // The message must name the rejected rate so logs are diagnosable.
            assert!(
                err.to_string().contains(&rate.to_string()),
                "rejection message must mention {rate}; got: {err}"
            );
        }
        // Real capture rates pass.
        for rate in [8000u32, 16000, 48000, 192_000] {
            validate_sample_rate(rate)
                .unwrap_or_else(|e| panic!("real rate {rate} Hz must pass; got: {e:?}"));
        }
    }

    #[test]
    fn resample_rejects_zero_rate_without_panic() {
        // Defense-in-depth: resample must error (not abort) on a 0 Hz rate, and
        // the error must be the typed AudioCapture/CaptureFailed variant (not a
        // resampler-init failure) so the 0-rate guard, not rubato, is what fired.
        for (from, to) in [(0u32, 16000u32), (16000, 0)] {
            let err = resample(&[0.1, 0.2, 0.3], from, to)
                .expect_err(&format!("resample({from} -> {to}) must error, not abort"));
            assert!(
                matches!(
                    err,
                    CoreError::AudioCapture {
                        code: maekon_core::error_codes::AudioCode::CaptureFailed,
                        ..
                    }
                ),
                "0 Hz resample must yield AudioCapture/CaptureFailed; got: {err:?}"
            );
            assert!(
                err.to_string().contains("invalid resample rate"),
                "error must come from the 0-rate guard; got: {err}"
            );
        }
    }

    #[test]
    fn stop_resets_mode_to_idle() {
        // stop()/stop_vad() must release the claim so a later start can re-acquire.
        let capture = AudioCapture::new(60);
        capture.mode.store(MODE_PTT, Ordering::SeqCst);
        *capture.native_rate.lock() = Some(16000);
        let _ = capture.stop().unwrap();
        assert_eq!(capture.mode.load(Ordering::SeqCst), MODE_IDLE);

        capture.mode.store(MODE_VAD, Ordering::SeqCst);
        capture.stop_vad().unwrap();
        assert_eq!(capture.mode.load(Ordering::SeqCst), MODE_IDLE);
    }

    #[test]
    fn drain_speech_buffer_empty() {
        let capture = AudioCapture::new(60);
        *capture.native_rate.lock() = Some(16000);
        let buffer = capture.drain_speech_buffer().unwrap();
        assert!(buffer.is_empty());
    }

    #[test]
    fn drain_speech_buffer_with_data() {
        let capture = AudioCapture::new(60);
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

    // ── #7077: raw-audio buffers are wiped, not merely cleared/taken ──

    #[test]
    fn wipe_samples_zeroizes_backing_allocation() {
        // `wipe_samples` is the shared primitive every stop/drain/Drop path uses
        // to retire raw PCM. It must overwrite the backing allocation, not just
        // set the length to 0 (which `Vec::clear()` does, leaving voice residue).
        const N: usize = 256;
        let mut buf: Vec<f32> = Vec::with_capacity(N);
        for i in 0..N {
            buf.push(i as f32 + 1.0); // all strictly non-zero
        }
        let ptr = buf.as_ptr();

        wipe_samples(&mut buf);

        assert!(buf.is_empty(), "wipe must truncate the length to 0");
        assert_eq!(
            buf.as_ptr(),
            ptr,
            "wipe must reuse the same allocation (no realloc), so the assertion below inspects the wiped heap"
        );
        // SAFETY: same still-allocated buffer; the first N f32 slots were written
        // (1.0..=N) and then overwritten with 0.0 by `zeroize`, so reading exactly
        // N initialized elements is sound. A `clear()`-only body would leave the
        // original non-zero PCM here, failing the assertion.
        let backing = unsafe { std::slice::from_raw_parts(ptr, N) };
        assert!(
            backing.iter().all(|&s| s == 0.0),
            "wipe_samples must zero the backing allocation, not just the length (#7077)"
        );
    }

    #[test]
    fn stop_vad_wipes_speech_buffer_backing_allocation() {
        // Public-path proof for the in-place clear path: stop_vad() must zeroize
        // the speech buffer's retained backing allocation, not just clear() it.
        const N: usize = 512;
        let capture = AudioCapture::new(60);
        {
            let mut buf = capture.speech_buffer.lock();
            buf.reserve_exact(N);
            for i in 0..N {
                buf.push(i as f32 + 1.0); // all strictly non-zero
            }
        }
        let ptr = capture.speech_buffer.lock().as_ptr();

        capture.stop_vad().unwrap();

        let buf = capture.speech_buffer.lock();
        assert!(buf.is_empty(), "stop_vad must clear the speech buffer");
        assert_eq!(
            buf.as_ptr(),
            ptr,
            "wipe must reuse the same allocation (no realloc)"
        );
        // SAFETY: same still-allocated buffer; the first N slots were written then
        // overwritten with 0.0 by the wipe, so reading N initialized elements is
        // sound. A clear()-only stop_vad() would leave the recorded PCM here.
        let backing = unsafe { std::slice::from_raw_parts(ptr, N) };
        assert!(
            backing.iter().all(|&s| s == 0.0),
            "stop_vad must zeroize the speech-buffer backing allocation (#7077)"
        );
    }

    // ── #7078: start/stop lifecycle transitions are serialized ──

    #[test]
    fn lifecycle_serializes_stop_against_in_progress_start() {
        use std::sync::mpsc;
        use std::time::Duration;

        let capture = Arc::new(AudioCapture::new(60));

        // Simulate a start()/start_vad() that has entered its critical section and
        // is mid-build by holding the same lifecycle lock those methods hold.
        let held = capture.lifecycle.lock();

        let c2 = Arc::clone(&capture);
        let (tx, rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            // stop() must acquire the lifecycle lock first; it blocks until the
            // simulated in-progress start() releases it.
            let _ = c2.stop();
            let _ = tx.send(());
        });

        // While the simulated start() owns the lock, stop() cannot make progress —
        // without serialization (the pre-fix code) it would complete immediately.
        // A Timeout (not Disconnected) confirms the stop() thread is still alive and
        // blocked on the lifecycle lock rather than having returned early.
        let blocked = rx.recv_timeout(Duration::from_millis(300)).unwrap_err();
        assert_eq!(
            blocked,
            mpsc::RecvTimeoutError::Timeout,
            "stop() must block while a start() holds the lifecycle lock (#7078)"
        );

        // Releasing the lock lets the serialized stop() run to completion.
        drop(held);
        rx.recv_timeout(Duration::from_secs(5))
            .expect("stop() must complete once the lifecycle lock is released (#7078)");
        handle.join().unwrap();
    }

    // ── #7098: remaining raw-PCM allocations are wiped on their retire paths ──

    #[test]
    fn consume_then_wipe_passes_pcm_then_zeroizes() {
        // sub-part 1: the per-callback `mono` mixdown buffer must reach the consumer
        // intact and then be zeroized before it drops. Inspect both halves on a
        // buffer the test owns (the real callback's `mono` is a closure local).
        let mut mono = vec![0.5f32; 8];
        let mut seen: Vec<f32> = Vec::new();
        consume_then_wipe(&mut mono, |m| seen.extend_from_slice(m));
        assert_eq!(
            seen,
            vec![0.5f32; 8],
            "consumer must observe the mono PCM before it is wiped"
        );
        // `Vec::zeroize` overwrites the samples and truncates the length to 0.
        assert!(
            mono.is_empty(),
            "callback mono buffer must be zeroized after the consumer copies it (#7098)"
        );
    }

    #[test]
    fn consume_then_wipe_forwards_consumer_value_then_zeroizes() {
        // #7115: the helper must forward the consumer's return value (so a Whisper
        // inference `Result` can be `?`-propagated AFTER the wipe), while still
        // zeroizing the borrowed PCM. Use a `Result`-returning consumer that reads
        // the samples, mirroring `whisper.rs::transcribe`'s borrow-then-wipe.
        let mut pcm = vec![0.5f32, -0.25, 0.75, -1.0];
        let observed: Result<usize, ()> = consume_then_wipe(&mut pcm, |s| {
            assert_eq!(s, [0.5f32, -0.25, 0.75, -1.0], "consumer must see real PCM");
            Ok(s.len())
        });
        assert_eq!(
            observed.expect("consumer value must be forwarded"),
            4,
            "consume_then_wipe must return what the consumer produced"
        );
        assert!(
            pcm.is_empty(),
            "borrowed PCM must be zeroized after the consumer reads it (#7115)"
        );
    }

    #[test]
    fn drain_resampled_chunk_copies_take_then_zeroizes() {
        // sub-part 2: the per-chunk resampler output (a copy this crate owns) must
        // have its needed samples copied into `output` and then be zeroized so the
        // resampled PCM is not left in the freed intermediate allocation.
        let mut output = vec![1.0f32, 2.0];
        let mut chunk = vec![0.3f32, 0.4, 0.5, 0.6];
        drain_resampled_chunk(&mut output, &mut chunk, 3);
        assert_eq!(
            output,
            vec![1.0, 2.0, 0.3, 0.4, 0.5],
            "only the first `take` samples must be appended to output"
        );
        assert!(
            chunk.is_empty(),
            "the per-chunk resampled copy must be zeroized after draining (#7098)"
        );
    }
}
