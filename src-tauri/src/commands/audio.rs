//! Tauri IPC commands for audio capture and speech-to-text.

use std::sync::Arc;

use tauri::{command, Emitter};
use tokio::sync::mpsc;

use maekon_core::models::audio::TranscriptionResult;
use maekon_core::ports::consent_manager::ConsentManagerPort;

use crate::ipc_error::IpcError;
use crate::runtime_state::AudioRuntimeState;

/// Canonical "audio capture not available" error — audio capture adapter missing.
fn audio_capture_not_available() -> IpcError {
    IpcError::new("service.unavailable", "audio capture not available")
}

/// Fire the VAD re-gate signal so any running VAD listener re-evaluates the
/// audio privacy gate immediately (e.g. right after a pause toggle) instead of
/// waiting for the ≤2 s backstop tick. No-op if no VAD listener is running
/// (the stored Notify permit is consumed harmlessly on the next start). Call
/// from EVERY capture-pause entry point.
///
/// Generic over `R: tauri::Runtime` so it accepts both the concrete
/// `AppHandle` (shortcut / IPC command sites) and the tray's generic
/// `AppHandle<R>` — single-sourcing the re-gate concern so a future fourth
/// pause site cannot silently skip it (ADR-075 P-3 "Sibling-Endpoint Blind
/// Spot").
pub(crate) fn signal_vad_regate<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    use tauri::Manager;
    if let Some(audio) = app.try_state::<crate::runtime_state::AudioRuntimeState>() {
        audio.audio_regate().notify_one();
    }
}

/// Re-arm decision: restart VAD on unpause ONLY when a resume was pending (VAD
/// was running at pause) AND the user is in Voice-Activity mode (not PTT).
fn should_rearm_vad(resume_pending: bool, mode: MicInputMode) -> bool {
    resume_pending && mode == MicInputMode::VoiceActivity
}

/// Single entry point every capture-pause toggle site must call. Fires the
/// immediate VAD re-gate (pause → stop; unpause → no-op), AND:
///  - at the PAUSE edge, remembers whether VAD was running (read BEFORE the
///    regate signal, since a multi-threaded runtime may poll the receiver task
///    the instant the Notify fires);
///  - at the UNPAUSE edge, consumes that flag and auto-restarts VAD when in
///    Voice-Activity mode. The rearm's `ensure_capture_permitted` is the safety
///    net against re-arming into a still-closed gate (battery / hours / schedule).
pub(crate) fn on_capture_pause_toggled<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    new_paused: bool,
) {
    use tauri::Manager;

    // PAUSE edge: capture resume intent BEFORE signaling the stop.
    if new_paused {
        if let Some(audio) = app.try_state::<crate::runtime_state::AudioRuntimeState>() {
            let was_active = audio
                .audio()
                .capture
                .as_ref()
                .is_some_and(|c| c.is_vad_active());
            audio.set_vad_resume_pending(was_active);
        }
    }

    signal_vad_regate(app);

    // UNPAUSE edge: consume the flag + auto-rearm in VAD mode.
    if !new_paused {
        if let Some(audio) = app.try_state::<crate::runtime_state::AudioRuntimeState>() {
            let resume = audio.take_vad_resume_pending();
            let mode = audio.config_manager().get().audio.mic_input_mode;
            if should_rearm_vad(resume, mode) {
                let app_clone = app.clone();
                tauri::async_runtime::spawn(async move {
                    let audio = app_clone.state::<crate::runtime_state::AudioRuntimeState>();
                    if let Err(e) = start_vad_listening_inner(&app_clone, &audio).await {
                        debug!("VAD auto-rearm after unpause skipped: {e:?}");
                    }
                });
            }
        }
    }
}

/// Shared microphone privacy gate for EVERY audio-capture entry point.
///
/// CONS-PC04 / D13: the 4-term composite gate — consent AND active_hours AND
/// tracking_schedule AND NOT capture_paused. Both `start_audio_capture` (PTT)
/// and `start_vad_listening` (continuous auto-listen) MUST pass this before any
/// microphone access. It lives in one shared function — not duplicated per
/// command — so a future third entry point cannot silently skip it (ADR-075 P-3
/// "Sibling-Endpoint Blind Spot"). Uses `snapshot()` (O(1) Arc-clone) per
/// CONS-PI13; NOT `get()` (deep-clone of 37 sections).
fn ensure_capture_permitted(state: &AudioRuntimeState) -> Result<(), IpcError> {
    use std::sync::atomic::Ordering;
    // effective_permissions()은 Valid 상태일 때만 권한을 반환한다 — Expired/UpdateRequired는
    // all-false를 반환하므로 스테일 동의 레코드도 fail-closed 처리된다 (Task 3).
    let consent = state
        .consent_manager()
        .map(|cm| cm.effective_permissions())
        .unwrap_or_default();
    let paused = state.capture_paused().load(Ordering::Relaxed);
    let permitted = crate::scheduler::audio_capture_permitted_now(
        &state.config_manager().snapshot(),
        &consent,
        paused,
    );
    if !permitted {
        return Err(IpcError::new(
            "validation.invalid_arguments",
            "Audio capture unavailable — privacy gate active (consent/hours/schedule/pause).",
        ));
    }
    Ok(())
}

/// Start microphone capture (Push-to-Talk begin).
#[command]
pub async fn start_audio_capture(
    state: tauri::State<'_, AudioRuntimeState>,
) -> Result<(), IpcError> {
    // CONS-PC04 / D13: 4-term composite privacy gate (shared with VAD auto-listen).
    ensure_capture_permitted(&state)?;

    let capture = state
        .audio()
        .capture
        .as_ref()
        .ok_or_else(audio_capture_not_available)?;
    capture.start().map_err(IpcError::from)
}

/// Stop capture and transcribe the recorded audio.
#[command]
pub async fn stop_and_transcribe(
    state: tauri::State<'_, AudioRuntimeState>,
) -> Result<TranscriptionResult, IpcError> {
    stop_and_transcribe_inner(&state).await
}

/// PTT stop+transcribe with the microphone re-gate (F-MIC-1, #4568).
///
/// Ordering is privacy-critical:
///  1. `capture.stop()` runs FIRST — it drains the recorded buffer AND releases
///     the held cpal stream. This MUST happen even when the gate is closed so the
///     microphone is never left open (drain-then-discard, mirroring the VAD rx-arm).
///  2. THEN `ensure_capture_permitted(&state)?` — if the `microphone` consent was
///     revoked (or any gate term is false), this returns Err and the drained
///     buffer is dropped here, never reaching `stt.transcribe` (no cloud egress).
///  3. Only after the gate passes do we fetch the STT engine and transcribe.
///
/// The STT fetch (which can early-return `service.unavailable`) is deliberately
/// placed AFTER the `stop()` drain so it cannot pre-empt the cpal release on a
/// closed gate.
pub(crate) async fn stop_and_transcribe_inner(
    state: &AudioRuntimeState,
) -> Result<TranscriptionResult, IpcError> {
    let capture = state
        .audio()
        .capture
        .as_ref()
        .ok_or_else(audio_capture_not_available)?;

    // (1) Drain + release the cpal stream FIRST — even on a closed gate.
    let buffer = capture.stop().map_err(IpcError::from)?;

    // (2) Re-gate AFTER stop(): a revoked `microphone` consent discards the
    //     drained buffer without transcribing (no cloud/local egress).
    ensure_capture_permitted(state)?;

    // (3) Gate open — fetch the STT engine and transcribe.
    let stt = {
        let guard = state.audio().stt_engine.read().await;
        guard.as_ref().map(Arc::clone).ok_or_else(|| {
            IpcError::new(
                "service.unavailable",
                "STT engine not available (model may not be loaded)",
            )
        })?
    };

    if buffer.is_empty() {
        return Ok(TranscriptionResult {
            text: String::new(),
            language: None,
            duration_secs: 0.0,
            processing_secs: 0.0,
        });
    }

    stt.transcribe(buffer).await.map_err(IpcError::from)
}

use std::sync::atomic::Ordering;

use maekon_core::config::{MicInputMode, WhisperModelSize};
use maekon_core::models::audio::{AudioStatus, ModelDownloadStatus, VadConfig};
use tracing::debug;

type VadSpeechSignalSender = mpsc::Sender<()>;
type VadSpeechSignalReceiver = mpsc::Receiver<()>;

const VAD_SPEECH_SIGNAL_QUEUE_CAPACITY: usize = 1;

fn new_vad_speech_signal_channel() -> (VadSpeechSignalSender, VadSpeechSignalReceiver) {
    mpsc::channel(VAD_SPEECH_SIGNAL_QUEUE_CAPACITY)
}

fn try_enqueue_vad_speech_signal(tx: &VadSpeechSignalSender) -> bool {
    tx.try_send(()).is_ok()
}

/// Get combined audio subsystem status (reads live config via config_manager).
#[command]
pub async fn get_audio_status(
    state: tauri::State<'_, AudioRuntimeState>,
) -> Result<AudioStatus, IpcError> {
    let live_config = state.config_manager().get();
    let audio_cfg = &live_config.audio;
    // F-PF-C28-03: model_status 는 내부적으로 std::fs::metadata 를 호출하므로
    // spawn_blocking 으로 감싸 async executor 스레드 블로킹을 방지.
    let model_status = match state.audio().model_downloader.clone() {
        Some(dl) => {
            let model_size = audio_cfg.model_size;
            let model_dir = state.audio().model_dir.clone();
            tokio::task::spawn_blocking(move || dl.model_status(model_size, &model_dir))
                .await
                .unwrap_or(ModelDownloadStatus::NotInstalled)
        }
        None => ModelDownloadStatus::NotInstalled,
    };
    let stt_loaded = state.audio().stt_engine.read().await.is_some();
    let vad_state = state.audio().vad_state.lock().clone();
    Ok(AudioStatus {
        enabled: audio_cfg.enabled,
        selected_model: audio_cfg.model_size,
        model_status,
        stt_provider_loaded: stt_loaded,
        stt_provider: format!("{:?}", audio_cfg.stt_provider).to_lowercase(),
        mic_input_mode: format!("{:?}", audio_cfg.mic_input_mode).to_lowercase(),
        vad_state,
    })
}

/// Start downloading a Whisper model with progress events.
#[command]
pub async fn download_whisper_model(
    app: tauri::AppHandle,
    state: tauri::State<'_, AudioRuntimeState>,
    model_size: WhisperModelSize,
) -> Result<(), IpcError> {
    // Guard: reject if already downloading
    if state.audio().downloading.swap(true, Ordering::SeqCst) {
        return Err(IpcError::new(
            "service.unavailable",
            "a download is already in progress",
        ));
    }
    // Reset cancel flag
    state.audio().download_cancel.store(false, Ordering::SeqCst);

    let downloader = match state.audio().model_downloader.as_ref() {
        Some(dl) => dl.clone(),
        None => {
            state.audio().downloading.store(false, Ordering::SeqCst);
            return Err(IpcError::new(
                "service.unavailable",
                "model downloader not available",
            ));
        }
    };
    let model_dir = state.audio().model_dir.clone();
    let cancel = state.audio().download_cancel.clone();
    let downloading = state.audio().downloading.clone();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    // Bridge progress channel -> Tauri events
    let app_handle = app.clone();
    tokio::spawn(async move {
        while let Some(progress) = rx.recv().await {
            if let Err(e) = app_handle.emit("audio-model-progress", &progress) {
                debug!("emit audio-model-progress failed: {e}");
            }
        }
    });

    // Spawn download task
    let app_clone = app.clone();
    tokio::spawn(async move {
        let result = downloader
            .download(model_size, &model_dir, tx, cancel)
            .await;
        downloading.store(false, Ordering::SeqCst);
        match result {
            Ok(path) => {
                // F-RC-10: use tokio::fs::metadata in async context
                let size_bytes = tokio::fs::metadata(&path)
                    .await
                    .map(|m| m.len())
                    .unwrap_or(0);
                let _ = app_clone.emit(
                    "audio-model-complete",
                    serde_json::json!({
                        "path": path.to_string_lossy(),
                        "model_size": model_size,
                        "size_bytes": size_bytes,
                    }),
                );
            }
            Err(e) => {
                let _ = app_clone.emit(
                    "audio-model-error",
                    serde_json::json!({ "message": e.to_string() }),
                );
            }
        }
    });

    Ok(())
}

/// Cancel an active model download.
#[command]
pub async fn cancel_model_download(
    state: tauri::State<'_, AudioRuntimeState>,
) -> Result<(), IpcError> {
    state.audio().download_cancel.store(true, Ordering::SeqCst);
    Ok(())
}

/// Delete a downloaded Whisper model.
#[command]
pub async fn delete_whisper_model(
    state: tauri::State<'_, AudioRuntimeState>,
    model_size: WhisperModelSize,
) -> Result<(), IpcError> {
    let dl =
        state.audio().model_downloader.clone().ok_or_else(|| {
            IpcError::new("service.unavailable", "model downloader not available")
        })?;
    // F-PF-C28-03: delete_model 은 std::fs::remove_file 을 호출하므로
    // spawn_blocking 으로 감싸 async executor 스레드 블로킹을 방지.
    let model_dir = state.audio().model_dir.clone();
    tokio::task::spawn_blocking(move || dl.delete_model(model_size, &model_dir))
        .await
        .map_err(|e| IpcError::new("service.unavailable", e.to_string()))?
        .map_err(IpcError::from)
}

/// Stop the MICROPHONE (cpal stream teardown + buffer clear via `stop_vad`) and
/// emit the distinguishable privacy-stop event. Used by every involuntary stop
/// path so the `privacy_gate_closed` reason is attributed uniformly.
fn close_for_privacy<F: FnMut(&str, serde_json::Value)>(
    capture: &Arc<dyn maekon_core::ports::audio_capture::AudioCapturePort>,
    vad_state: &Arc<parking_lot::Mutex<String>>,
    emit: &mut F,
) {
    if let Err(e) = capture.stop_vad() {
        tracing::warn!("VAD privacy-stop: stop_vad failed: {e}");
    }
    *vad_state.lock() = "idle".into();
    emit(
        "vad-state-changed",
        serde_json::json!({"state": "idle", "reason": "privacy_gate_closed"}),
    );
}

/// VAD speech-ended receiver loop with continuous privacy re-gating. Testable
/// without a Tauri `AppHandle` (events go through `emit`).
///
/// Re-evaluates the audio privacy gate (audio.enabled + consent + active hours +
/// tracking schedule + pause + battery) on three triggers: an immediate
/// `regate` signal (pause/revoke gesture), a mandatory ≤2 s backstop tick (the
/// only trigger that fires during silence), and immediately before each
/// transcription. On a closed gate the task stops the mic and exits. `biased;`
/// prioritizes the privacy arms over transcription.
#[allow(clippy::too_many_arguments)]
async fn run_vad_receiver(
    mut rx: VadSpeechSignalReceiver,
    capture: Arc<dyn maekon_core::ports::audio_capture::AudioCapturePort>,
    stt_engine: Arc<
        tokio::sync::RwLock<Option<Arc<dyn maekon_core::ports::stt_provider::SttProvider>>>,
    >,
    vad_state: Arc<parking_lot::Mutex<String>>,
    config_manager: maekon_core::config_manager::ConfigManager,
    consent_manager: Option<Arc<dyn ConsentManagerPort>>,
    capture_paused: Arc<std::sync::atomic::AtomicBool>,
    regate: Arc<tokio::sync::Notify>,
    mut emit: impl FnMut(&str, serde_json::Value) + Send + 'static,
) {
    use std::sync::atomic::Ordering;

    // Privacy backstop: re-gate at least every 2 s even during silence. Plain
    // interval with Delay (NOT a Skip/coalescing behavior) so a runtime stall
    // cannot stretch the window. NOTE: interval's FIRST tick fires immediately
    // (t=0) — re-gating right at startup is harmless (the gate was just checked
    // by `start_vad_listening`), and the every-2 s cadence holds thereafter.
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(2));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    // Re-evaluates the composite audio gate. Takes its inputs by reference as
    // explicit params (NOT captured) so it can be invoked from multiple select!
    // arms without holding a long-lived borrow across the owned locals.
    let gate_open = |cfg_mgr: &maekon_core::config_manager::ConfigManager,
                     consent_mgr: &Option<Arc<dyn ConsentManagerPort>>,
                     paused: &std::sync::atomic::AtomicBool| {
        // effective_permissions()은 Valid 상태일 때만 권한을 반환한다 — Expired/UpdateRequired는
        // all-false를 반환하므로 스테일 동의 레코드도 fail-closed 처리된다 (Task 3).
        let consent = consent_mgr
            .as_ref()
            .map(|cm| cm.effective_permissions())
            .unwrap_or_default();
        crate::scheduler::audio_capture_permitted_now(
            &cfg_mgr.snapshot(),
            &consent,
            paused.load(Ordering::Relaxed),
        )
    };

    loop {
        tokio::select! {
            biased;

            // Privacy arm 1: immediate revoke/pause gesture.
            _ = regate.notified() => {
                if !gate_open(&config_manager, &consent_manager, &capture_paused) {
                    close_for_privacy(&capture, &vad_state, &mut emit);
                    break;
                }
            }
            // Privacy arm 2: ≤2 s backstop tick — the ONLY trigger that fires
            // during silence (when rx never produces a speech-ended signal).
            _ = tick.tick() => {
                if !gate_open(&config_manager, &consent_manager, &capture_paused) {
                    close_for_privacy(&capture, &vad_state, &mut emit);
                    break;
                }
            }
            // Transcribe arm: a speech-ended signal arrived, or the sender was
            // dropped (external stop_vad_listening → end task).
            signal = rx.recv() => {
                if signal.is_none() {
                    break; // sender dropped (external stop_vad_listening) → end task
                }
                // Re-gate before transcription: if the gate closed between the
                // speech-ended signal and now, drop the buffer (no transcription,
                // no cloud egress) and stop.
                if !gate_open(&config_manager, &consent_manager, &capture_paused) {
                    let _ = capture.drain_speech_buffer(); // drain + discard
                    close_for_privacy(&capture, &vad_state, &mut emit);
                    break;
                }

                *vad_state.lock() = "transcribing".into();
                emit(
                    "vad-state-changed",
                    serde_json::json!({"state": "transcribing"}),
                );

                let start = std::time::Instant::now();
                let result: Result<maekon_core::models::audio::TranscriptionResult, String> = async {
                    let buffer = capture.drain_speech_buffer().map_err(|e| e.to_string())?;
                    if buffer.is_empty() {
                        return Ok(maekon_core::models::audio::TranscriptionResult {
                            text: String::new(),
                            language: None,
                            duration_secs: 0.0,
                            processing_secs: 0.0,
                        });
                    }
                    let stt = {
                        let guard = stt_engine.read().await;
                        guard
                            .as_ref()
                            .map(Arc::clone)
                            .ok_or_else(|| "STT engine not available".to_string())?
                    };
                    stt.transcribe(buffer).await.map_err(|e| e.to_string())
                }
                .await;

                let processing_secs = start.elapsed().as_secs_f64();
                match result {
                    Ok(tr) => emit(
                        "vad-transcription-result",
                        serde_json::json!({
                            "text": tr.text,
                            "duration_secs": tr.duration_secs,
                            "processing_secs": processing_secs,
                        }),
                    ),
                    Err(e) => {
                        tracing::warn!("VAD transcription failed: {e}");
                        emit(
                            "vad-transcription-result",
                            serde_json::json!({
                                "text": "",
                                "duration_secs": 0.0,
                                "processing_secs": processing_secs,
                                "error": e,
                            }),
                        );
                    }
                }

                if capture.is_vad_active() {
                    *vad_state.lock() = "listening".into();
                    emit(
                        "vad-state-changed",
                        serde_json::json!({"state": "listening"}),
                    );
                } else {
                    *vad_state.lock() = "idle".into();
                    emit("vad-state-changed", serde_json::json!({"state": "idle"}));
                    break;
                }
            }
        }
    }
}

/// Start VAD continuous listening. Extracted from the `#[command]` so the
/// pause→unpause auto-rearm path can invoke it with just an `AppHandle` + state.
/// Generic over the runtime so tray (`AppHandle<R>`) and the Wry command both call it.
pub(crate) async fn start_vad_listening_inner<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    state: &AudioRuntimeState,
) -> Result<(), IpcError> {
    // CONS-PC04 / D13: VAD auto-listen is CONTINUOUS microphone capture — gate it
    // with the SAME 4-term privacy gate as PTT. This was previously missing, so
    // VAD could record (and auto-transcribe, incl. cloud STT egress) despite
    // revoked consent, a tray pause, or being outside active hours / schedule.
    ensure_capture_permitted(state)?;

    let capture = state
        .audio()
        .capture
        .as_ref()
        .ok_or_else(audio_capture_not_available)?;

    // Double-start guard: if a VAD stream is already live (e.g. a pause→unpause
    // race delivered the rearm before the prior receiver task observed the stop),
    // do nothing rather than double-init the cpal stream.
    // Covered by code review + integration path; no unit test (AppHandle not
    // constructable in tests).
    if capture.is_vad_active() {
        return Ok(());
    }

    let live_cfg = state.config_manager().get();
    let config = VadConfig {
        threshold: live_cfg.audio.vad_threshold,
        silence_ms: live_cfg.audio.vad_silence_ms,
        min_speech_ms: live_cfg.audio.vad_min_speech_ms,
    };

    let (tx, rx) = new_vad_speech_signal_channel();

    // Signal callback — called on audio thread when speech ends.
    // Lightweight: queue at most one pending utterance-end signal.
    let on_speech_signal = Arc::new(move || {
        if !try_enqueue_vad_speech_signal(&tx) {
            debug!("VAD speech-ended signal coalesced or receiver closed");
        }
    });

    capture
        .start_vad(config, on_speech_signal)
        .map_err(IpcError::from)?;

    // Update VAD state to "listening"
    *state.audio().vad_state.lock() = "listening".into();
    let _ = app.emit(
        "vad-state-changed",
        serde_json::json!({"state": "listening"}),
    );

    // Spawn receiver task to handle speech-ended signals + continuous re-gating.
    let capture_clone = Arc::clone(capture);
    let stt_engine = state.audio().stt_engine.clone();
    let vad_state = state.audio().vad_state.clone();
    let app_clone = app.clone();
    let config_manager = state.config_manager().clone();
    let consent_manager = state.consent_manager().cloned();
    let capture_paused = state.capture_paused().clone();
    let regate = state.audio_regate().clone();

    tokio::spawn(run_vad_receiver(
        rx,
        capture_clone,
        stt_engine,
        vad_state,
        config_manager,
        consent_manager,
        capture_paused,
        regate,
        move |event, payload| {
            let _ = app_clone.emit(event, payload);
        },
    ));

    Ok(())
}

/// Start VAD listening mode — automatically detects speech start/end.
#[command]
pub async fn start_vad_listening(
    app: tauri::AppHandle,
    state: tauri::State<'_, AudioRuntimeState>,
) -> Result<(), IpcError> {
    start_vad_listening_inner(&app, &state).await
}

/// Stop VAD listening mode.
#[command]
pub async fn stop_vad_listening(
    app: tauri::AppHandle,
    state: tauri::State<'_, AudioRuntimeState>,
) -> Result<(), IpcError> {
    let capture = state
        .audio()
        .capture
        .as_ref()
        .ok_or_else(audio_capture_not_available)?;

    capture.stop_vad().map_err(IpcError::from)?;
    *state.audio().vad_state.lock() = "idle".into();
    if let Err(e) = app.emit("vad-state-changed", serde_json::json!({"state": "idle"})) {
        debug!("emit vad-state-changed failed: {e}");
    }
    Ok(())
}

/// Reload STT engine with current config — creates Local, Cloud, or Fallback provider.
#[command]
pub async fn reload_stt_engine(
    state: tauri::State<'_, AudioRuntimeState>,
) -> Result<bool, IpcError> {
    use maekon_core::config::SttProviderKind;

    let live_config = state.config_manager().get();
    let config = &live_config.audio;

    // Build local provider (if model available)
    let local_provider: Option<Arc<dyn maekon_core::ports::stt_provider::SttProvider>> = {
        #[cfg(feature = "stt")]
        {
            #[cfg(feature = "download")]
            let model_path =
                state
                    .audio()
                    .model_dir
                    .join(maekon_audio::model_downloader::model_filename(
                        config.model_size,
                    ));
            #[cfg(not(feature = "download"))]
            let model_path = std::path::PathBuf::from(&config.whisper_model_path);

            if model_path.exists() {
                match maekon_audio::WhisperSttProvider::new(&model_path, config.language) {
                    Ok(p) => {
                        // P1-1: wire PII sanitizer so reload path also sanitizes transcripts.
                        let stt_pii_sanitizer: Arc<
                            dyn maekon_core::ports::pii_sanitizer::PiiSanitizer,
                        > = Arc::new(maekon_vision::privacy::VisionPiiSanitizer);
                        let p = p.with_pii_sanitizer(
                            stt_pii_sanitizer,
                            live_config.privacy.pii_filter_level,
                        );
                        Some(Arc::new(p) as _)
                    }
                    Err(e) => {
                        tracing::warn!("Failed to load local Whisper: {e}");
                        None
                    }
                }
            } else {
                None
            }
        }
        #[cfg(not(feature = "stt"))]
        {
            None
        }
    };

    // Build cloud provider (if key configured)
    let cloud_provider: Option<Arc<dyn maekon_core::ports::stt_provider::SttProvider>> = {
        #[cfg(feature = "cloud-stt")]
        {
            use maekon_core::config::CloudSttBuild;
            // Enterprise managed-policy gate (#4685): single-sourced, unit-tested
            // build decision (policy fail-safe + API-key check). Mirrors the startup
            // path in audio_wiring.rs so both wiring paths enforce identically.
            match config.cloud_stt_build_decision() {
                CloudSttBuild::SkipPolicyBlocked => {
                    tracing::warn!(
                        "Cloud STT blocked by managed policy ({}) on reload; raw audio will not egress — using local STT if available",
                        config.cloud_stt_policy
                    );
                    None
                }
                CloudSttBuild::SkipNoKey => None,
                CloudSttBuild::Build => match maekon_audio::CloudSttProvider::new(
                    config.cloud_api_key.clone(),
                    config.cloud_stt_endpoint.clone(),
                    config.language,
                    config.cloud_timeout_secs,
                ) {
                    Ok(p) => {
                        // P1-1: wire PII sanitizer for cloud reload path.
                        let stt_pii_sanitizer: Arc<
                            dyn maekon_core::ports::pii_sanitizer::PiiSanitizer,
                        > = Arc::new(maekon_vision::privacy::VisionPiiSanitizer);
                        let p = p.with_pii_sanitizer(
                            stt_pii_sanitizer,
                            live_config.privacy.pii_filter_level,
                        );
                        Some(Arc::new(p) as _)
                    }
                    Err(e) => {
                        tracing::warn!("Failed to create cloud STT: {e}");
                        None
                    }
                },
            }
        }
        #[cfg(not(feature = "cloud-stt"))]
        {
            None
        }
    };

    // Assemble final provider based on config preference
    let provider: Option<Arc<dyn maekon_core::ports::stt_provider::SttProvider>> =
        match config.stt_provider {
            SttProviderKind::Cloud => match (cloud_provider, local_provider) {
                (Some(cloud), Some(local)) => {
                    Some(Arc::new(crate::fallback_stt::FallbackSttProvider::new(cloud, local)) as _)
                }
                (Some(cloud), None) => Some(cloud),
                (None, Some(local)) => {
                    tracing::warn!("Cloud STT unavailable, using local");
                    Some(local)
                }
                (None, None) => None,
            },
            SttProviderKind::Local => local_provider,
        };

    let loaded = provider.is_some();
    let mut guard = state.audio().stt_engine.write().await;
    *guard = provider;

    if loaded {
        tracing::info!("STT engine reloaded (provider: {:?})", config.stt_provider);
    }
    Ok(loaded)
}

#[cfg(test)]
mod tests {
    // M2 (Task 4 review): single `use` for the atomic imports — both the bare
    // `AtomicBool` (gate_state / disabled-config test) and the `StdAtomicBool`
    // alias (MockVadCapture + the privacy re-gate tests) name the same type.
    use std::sync::atomic::{
        AtomicBool, AtomicBool as StdAtomicBool, AtomicUsize, Ordering as AtomicOrdering,
    };
    use std::sync::Arc;

    use maekon_core::config_manager::ConfigManager;
    use maekon_core::consent::{ConsentManager, ConsentPermissions};
    use maekon_core::error::CoreError;
    use maekon_core::models::audio::{AudioBuffer, VadConfig};
    use maekon_core::ports::audio_capture::AudioCapturePort;
    use maekon_core::ports::consent_manager::ConsentManagerPort;
    use serial_test::serial;
    use tokio::sync::mpsc::error::TryRecvError;

    use crate::runtime_state::{AudioContext, AudioRuntimeState};

    /// Test double for the VAD receiver: records `stop_vad` calls and exposes a
    /// controllable `vad_active` flag. `drain_speech_buffer` returns a buffer
    /// built from `drain_samples` (empty for `listening()`, non-empty for
    /// `listening_with_buffer(..)`) so the rx/transcribe path can be exercised
    /// with a real (mock) utterance.
    struct MockVadCapture {
        vad_active: StdAtomicBool,
        stop_vad_calls: AtomicUsize,
        drain_samples: Vec<f32>,
    }
    impl MockVadCapture {
        fn listening() -> Arc<Self> {
            Arc::new(Self {
                vad_active: StdAtomicBool::new(true),
                stop_vad_calls: AtomicUsize::new(0),
                drain_samples: Vec::new(),
            })
        }
        /// Like `listening()` but `drain_speech_buffer` yields a NON-empty
        /// buffer — required to reach the `stt.transcribe` call in the rx arm
        /// (an empty drain short-circuits before transcription).
        fn listening_with_buffer(samples: Vec<f32>) -> Arc<Self> {
            Arc::new(Self {
                vad_active: StdAtomicBool::new(true),
                stop_vad_calls: AtomicUsize::new(0),
                drain_samples: samples,
            })
        }
        fn stop_vad_count(&self) -> usize {
            self.stop_vad_calls.load(AtomicOrdering::SeqCst)
        }
    }
    impl AudioCapturePort for MockVadCapture {
        fn start(&self) -> Result<(), CoreError> {
            Ok(())
        }
        fn stop(&self) -> Result<AudioBuffer, CoreError> {
            Ok(AudioBuffer::new(Vec::new()))
        }
        fn is_capturing(&self) -> bool {
            false
        }
        fn start_vad(
            &self,
            _config: VadConfig,
            _on_speech_signal: Arc<dyn Fn() + Send + Sync>,
        ) -> Result<(), CoreError> {
            self.vad_active.store(true, AtomicOrdering::SeqCst);
            Ok(())
        }
        fn stop_vad(&self) -> Result<(), CoreError> {
            self.vad_active.store(false, AtomicOrdering::SeqCst);
            self.stop_vad_calls.fetch_add(1, AtomicOrdering::SeqCst);
            Ok(())
        }
        fn is_vad_active(&self) -> bool {
            self.vad_active.load(AtomicOrdering::SeqCst)
        }
        fn drain_speech_buffer(&self) -> Result<AudioBuffer, CoreError> {
            Ok(AudioBuffer::new(self.drain_samples.clone()))
        }
    }

    /// Spy `SttProvider` for the no-egress test: counts how many times
    /// `transcribe` is called (i.e. how many times audio would have left for a
    /// cloud/local STT engine). Under a closed privacy gate the count MUST stay
    /// zero. Returns an empty transcription so the rx arm completes cleanly.
    struct SpyStt {
        transcribe_calls: AtomicUsize,
    }
    impl SpyStt {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                transcribe_calls: AtomicUsize::new(0),
            })
        }
        fn transcribe_calls(&self) -> usize {
            self.transcribe_calls.load(AtomicOrdering::SeqCst)
        }
    }
    #[async_trait::async_trait]
    impl maekon_core::ports::stt_provider::SttProvider for SpyStt {
        async fn transcribe(
            &self,
            _audio: AudioBuffer,
        ) -> Result<maekon_core::models::audio::TranscriptionResult, CoreError> {
            self.transcribe_calls.fetch_add(1, AtomicOrdering::SeqCst);
            Ok(maekon_core::models::audio::TranscriptionResult {
                text: String::new(),
                language: None,
                duration_secs: 0.0,
                processing_secs: 0.0,
            })
        }
        fn provider_name(&self) -> &str {
            "spy"
        }
    }

    /// Shared emit recorder for receiver tests.
    type EmitLog = Arc<parking_lot::Mutex<Vec<(String, serde_json::Value)>>>;
    fn emit_recorder() -> (
        EmitLog,
        impl FnMut(&str, serde_json::Value) + Send + 'static,
    ) {
        let log: EmitLog = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let sink = log.clone();
        (log, move |event: &str, payload: serde_json::Value| {
            sink.lock().push((event.to_string(), payload));
        })
    }

    #[test]
    fn should_rearm_vad_only_when_pending_and_voice_activity() {
        use maekon_core::config::MicInputMode;
        // Re-arm ONLY when a resume is pending AND the user is in Voice-Activity mode.
        assert!(
            super::should_rearm_vad(true, MicInputMode::VoiceActivity),
            "pending + VAD → must rearm"
        );
        assert!(
            !super::should_rearm_vad(false, MicInputMode::VoiceActivity),
            "no pending → no rearm"
        );
        assert!(
            !super::should_rearm_vad(true, MicInputMode::PushToTalk),
            "PTT mode → no rearm"
        );
        assert!(
            !super::should_rearm_vad(false, MicInputMode::PushToTalk),
            "no pending + PTT → no rearm"
        );
    }

    #[tokio::test]
    async fn run_vad_receiver_ends_when_signal_channel_closes() {
        // Behavior-preserving baseline: when the speech-signal sender is dropped and
        // the mock reports vad inactive, the receiver loop terminates cleanly via
        // the transcribe→idle path. The gate is held OPEN (open_gate_inputs) so the
        // continuous-regate select! arms do not pre-empt this path: the immediate
        // t=0 tick re-gates (open → no-op) and the rx signal then drives the drain.
        let temp = tempfile::TempDir::new().unwrap();
        let (cfg, consent) = open_gate_inputs(temp.path());
        let capture = MockVadCapture::listening();
        capture.vad_active.store(false, AtomicOrdering::SeqCst); // not active → ends after one drain
        let stt: Arc<
            tokio::sync::RwLock<Option<Arc<dyn maekon_core::ports::stt_provider::SttProvider>>>,
        > = Arc::new(tokio::sync::RwLock::new(None));
        let vad_state = Arc::new(parking_lot::Mutex::new("listening".to_string()));
        let capture_paused = Arc::new(StdAtomicBool::new(false));
        let regate = Arc::new(tokio::sync::Notify::new());
        let (log, emit) = emit_recorder();

        let (tx, rx) = super::new_vad_speech_signal_channel();
        let handle = tokio::spawn(super::run_vad_receiver(
            rx,
            capture.clone(),
            stt,
            vad_state.clone(),
            cfg,
            consent,
            capture_paused,
            regate,
            emit,
        ));
        // One speech-ended signal, then close the channel.
        tx.try_send(()).unwrap();
        drop(tx);
        handle.await.unwrap();

        assert_eq!(*vad_state.lock(), "idle");
        assert!(
            log.lock()
                .iter()
                .any(|(e, p)| e == "vad-state-changed" && p["state"] == "idle"),
            "receiver must emit idle when vad becomes inactive"
        );
    }

    /// Build an `AudioRuntimeState` for gate tests. `consent` None = no record;
    /// `paused` toggles the tray-pause veto. Config enables audio so the audio
    /// gate's `audio.enabled` term is satisfied and the NAMED cause under test
    /// (consent / pause) is the sole false term.
    fn gate_state(
        dir: &std::path::Path,
        consent: Option<Arc<ConsentManager>>,
        paused: bool,
    ) -> AudioRuntimeState {
        let cfg = ConfigManager::with_path(dir.join("config.json")).expect("config manager");
        cfg.update_with(|c| {
            c.audio.enabled = true;
            Ok(())
        })
        .expect("enable audio for gate test");
        let consent: Option<Arc<dyn ConsentManagerPort>> =
            consent.map(|c| c as Arc<dyn ConsentManagerPort>);
        AudioRuntimeState::new(
            cfg,
            consent,
            Arc::new(AtomicBool::new(paused)),
            AudioContext::disabled(dir.join("models")),
        )
    }

    #[test]
    fn capture_gate_denies_without_consent() {
        // No consent record → microphone=false → gate denies regardless of the
        // wall clock. Both PTT and VAD route through this shared (audio) gate.
        let temp = tempfile::TempDir::new().unwrap();
        let state = gate_state(temp.path(), None, false);
        let gate_err = super::ensure_capture_permitted(&state).unwrap_err();
        assert!(
            gate_err.code.contains("validation"),
            "mic capture must be denied when consent is absent"
        );
    }

    #[test]
    fn capture_gate_denies_when_paused_even_with_consent() {
        // The tray-pause veto must reach the audio gate: full consent granted but
        // capture_paused=true → still denied (time-independent). This is the
        // scenario the VAD path previously failed to honor.
        let temp = tempfile::TempDir::new().unwrap();
        let cm = ConsentManager::new(temp.path().join("consent.json"));
        cm.grant_consent(
            ConsentPermissions {
                microphone: true,
                ..Default::default()
            },
            30,
        )
        .unwrap();
        let state = gate_state(temp.path(), Some(Arc::new(cm)), true);
        let gate_err = super::ensure_capture_permitted(&state).unwrap_err();
        assert!(
            gate_err.code.contains("validation"),
            "mic capture must be denied while paused, even with consent granted"
        );
    }

    #[test]
    fn capture_gate_denies_when_audio_disabled() {
        // audio.enabled=false alone must deny — even with consent granted and not
        // paused. This is the enable-flag fix: audio is gated on audio.enabled, and
        // audio.enabled defaults false (opt-in). NOTE: this does NOT use gate_state
        // (which force-enables audio); it builds a default-config state on purpose.
        let temp = tempfile::TempDir::new().unwrap();
        let cm = ConsentManager::new(temp.path().join("consent.json"));
        cm.grant_consent(
            ConsentPermissions {
                microphone: true,
                ..Default::default()
            },
            30,
        )
        .unwrap();
        let cfg =
            ConfigManager::with_path(temp.path().join("config.json")).expect("config manager");
        // audio.enabled defaults false — intentionally do NOT enable it.
        let state = AudioRuntimeState::new(
            cfg,
            Some(Arc::new(cm)),
            Arc::new(AtomicBool::new(false)),
            AudioContext::disabled(temp.path().join("models")),
        );
        let gate_err = super::ensure_capture_permitted(&state).unwrap_err();
        assert!(gate_err.code.contains("validation"), "audio capture must be denied when audio.enabled=false even with consent and not paused");
    }

    /// Task 3: `effective_permissions` 마이그레이션 후 합성 게이트 동작 검증.
    ///
    /// Expired 동의 레코드(microphone=true이지만 만료됨)를 파일에 직접 기록하고
    /// `ConsentManager::new`로 로드한 뒤 `ensure_capture_permitted`가 거부하는지 확인한다.
    /// `is_permitted`가 아닌 `effective_permissions`를 사용하면 Valid가 아닌 동의는
    /// all-false를 반환하므로, 불리언이 true여도 게이트가 닫힌다.
    #[test]
    fn capture_gate_denies_when_consent_expired_even_if_microphone_true() {
        use chrono::Utc;
        use maekon_core::consent::{ConsentPermissions, ConsentRecord, CURRENT_POLICY_VERSION};

        let temp = tempfile::TempDir::new().unwrap();
        let consent_path = temp.path().join("consent.json");

        // microphone=true 이지만 이미 만료된 레코드를 파일에 직접 기록한다
        // (grant_consent는 expires_at을 None으로 고정하므로 직접 작성한다).
        let expired = ConsentRecord {
            consent_id: "test-expired".into(),
            version: CURRENT_POLICY_VERSION.to_string(),
            granted_at: Utc::now() - chrono::Duration::days(2),
            expires_at: Some(Utc::now() - chrono::Duration::days(1)),
            revoked_at: None,
            data_deletion_requested: false,
            erasure_nonce: None,
            permissions: ConsentPermissions {
                microphone: true,
                ..Default::default()
            },
            data_retention_days: 30,
        };
        std::fs::write(&consent_path, serde_json::to_string(&expired).unwrap()).unwrap();

        // ConsentManager는 로드 시 Expired 판정을 내린다.
        let cm = ConsentManager::new(consent_path);
        assert_eq!(
            cm.check_consent(),
            maekon_core::consent::ConsentStatus::Expired,
            "precondition: consent must be Expired"
        );
        // effective_permissions()은 Valid가 아니면 all-false를 반환한다.
        assert!(
            !cm.effective_permissions().microphone,
            "Expired consent must yield microphone=false via effective_permissions"
        );

        // 합성 게이트: Expired 동의로 구성된 AudioRuntimeState는 gate_state 처럼 audio.enabled=true 이지만
        // 만료 동의이므로 ensure_capture_permitted가 거부해야 한다.
        let state = gate_state(temp.path(), Some(Arc::new(cm)), false);
        let gate_err = super::ensure_capture_permitted(&state).unwrap_err();
        assert!(gate_err.code.contains("validation"), "ensure_capture_permitted must deny when consent is Expired, even with microphone=true in the record");
    }

    #[test]
    fn audio_config_defaults_to_opt_in() {
        // The enable-flag swap is privacy-neutral BECAUSE audio is opt-in (default
        // off). If this ever flips to true, the swap would silently enable mic capture.
        assert!(
            !maekon_core::config::AppConfig::default_config()
                .audio
                .enabled,
            "AudioConfig.enabled must default to false (opt-in)"
        );
    }

    /// Composed migration-regression (#4568): a legacy `consent.json` written
    /// before the `microphone` field existed (screen_capture=true, NO microphone
    /// key) loads with `microphone` defaulting false. Once loaded, the SCREEN gate
    /// stays OPEN (screen_capture=true) while the AUDIO gate is CLOSED — proving a
    /// pre-Tier-8 screen-consenting user does NOT get the mic silently authorized
    /// by the upgrade. Active hours are disabled (always-on) so the time term is
    /// deterministic regardless of wall clock.
    #[test]
    fn legacy_consent_without_microphone_denies_audio_but_permits_screen() {
        let temp = tempfile::TempDir::new().unwrap();
        let consent_path = temp.path().join("consent.json");

        // Pre-migration record: screen_capture granted, NO `microphone` key.
        // Written as a Valid (non-expired, non-revoked) PascalCase ConsentRecord.
        let granted_at = chrono::Utc::now();
        let legacy_json = format!(
            r#"{{
                "consent_id": "legacy-pre-mic",
                "version": "{ver}",
                "granted_at": "{ts}",
                "expires_at": null,
                "revoked_at": null,
                "data_deletion_requested": false,
                "permissions": {{
                    "screen_capture": true,
                    "ocr_processing": false,
                    "telemetry": false,
                    "process_monitoring": false,
                    "input_activity": false,
                    "window_title_collection": false,
                    "app_usage_analytics": false,
                    "clipboard_monitoring": false,
                    "file_access_monitoring": false,
                    "activity_pattern_learning": false,
                    "cross_device_sync": false,
                    "full_text_extraction": false,
                    "memory_graph_enrichment": false
                }},
                "data_retention_days": 30
            }}"#,
            ver = maekon_core::consent::CURRENT_POLICY_VERSION,
            ts = granted_at.to_rfc3339(),
        );
        std::fs::write(&consent_path, legacy_json).unwrap();

        let cm = ConsentManager::new(consent_path);
        assert_eq!(
            cm.check_consent(),
            maekon_core::consent::ConsentStatus::Valid,
            "precondition: legacy record must load as Valid"
        );
        let perms = cm.effective_permissions();
        assert!(
            perms.screen_capture,
            "legacy record must retain screen_capture=true"
        );
        assert!(
            !perms.microphone,
            "missing microphone field must default false (fail-closed on upgrade)"
        );

        // Default config: vision.capture_enabled=true, active_hours disabled
        // (time term always true), not battery-saver. Audio enabled so the ONLY
        // difference between the two gates is the consent term.
        let mut cfg = maekon_core::config::AppConfig::default_config();
        cfg.audio.enabled = true;
        assert!(
            crate::scheduler::capture_permitted_now(&cfg, &perms, false),
            "screen gate must stay OPEN for the legacy screen-consenting user"
        );
        assert!(
            !crate::scheduler::audio_capture_permitted_now(&cfg, &perms, false),
            "audio gate must be CLOSED — screen consent must NOT authorize the mic (#4568)"
        );
    }

    /// PTT capture spy for the F-MIC-1 re-gate test. Counts `stop()` calls (the
    /// cpal teardown / drain that MUST run even on a closed gate) and returns a
    /// configurable NON-empty buffer so the positive-control path reaches
    /// `stt.transcribe`. `start` is a no-op.
    struct StopSpyCapture {
        stop_calls: AtomicUsize,
        buffer_samples: Vec<f32>,
    }
    impl StopSpyCapture {
        fn with_buffer(samples: Vec<f32>) -> Arc<Self> {
            Arc::new(Self {
                stop_calls: AtomicUsize::new(0),
                buffer_samples: samples,
            })
        }
        fn stop_calls(&self) -> usize {
            self.stop_calls.load(AtomicOrdering::SeqCst)
        }
    }
    impl AudioCapturePort for StopSpyCapture {
        fn start(&self) -> Result<(), CoreError> {
            Ok(())
        }
        fn stop(&self) -> Result<AudioBuffer, CoreError> {
            self.stop_calls.fetch_add(1, AtomicOrdering::SeqCst);
            Ok(AudioBuffer::new(self.buffer_samples.clone()))
        }
        fn is_capturing(&self) -> bool {
            false
        }
    }

    /// Build an `AudioRuntimeState` with a POPULATED capture + STT engine (unlike
    /// `gate_state`/`open_gate_inputs`, which build `AudioContext::disabled`). The
    /// audio gate is satisfied except for the `microphone` consent term, which the
    /// caller drives via `mic_granted` — so the gate is the SOLE thing standing
    /// between the drained PTT buffer and `stt.transcribe` (cloud egress).
    fn ptt_state(
        dir: &std::path::Path,
        mic_granted: bool,
        capture: Arc<StopSpyCapture>,
        stt: Arc<SpyStt>,
    ) -> AudioRuntimeState {
        let cfg = ConfigManager::with_path(dir.join("config.json")).expect("config manager");
        cfg.update_with(|c| {
            c.audio.enabled = true;
            Ok(())
        })
        .expect("enable audio for ptt test");
        let cm = ConsentManager::new(dir.join("consent.json"));
        cm.grant_consent(
            ConsentPermissions {
                microphone: mic_granted,
                ..Default::default()
            },
            30,
        )
        .unwrap();
        let audio = AudioContext {
            capture: Some(capture as Arc<dyn AudioCapturePort>),
            stt_engine: Arc::new(tokio::sync::RwLock::new(Some(
                stt as Arc<dyn maekon_core::ports::stt_provider::SttProvider>,
            ))),
            model_downloader: None,
            model_dir: dir.join("models"),
            downloading: Arc::new(StdAtomicBool::new(false)),
            download_cancel: Arc::new(StdAtomicBool::new(false)),
            vad_state: Arc::new(parking_lot::Mutex::new("idle".into())),
        };
        AudioRuntimeState::new(
            cfg,
            Some(Arc::new(cm)),
            Arc::new(StdAtomicBool::new(false)),
            audio,
        )
    }

    /// F-MIC-1: `stop_and_transcribe` must NOT transcribe (cloud egress) the PTT
    /// buffer when the microphone consent is revoked — yet it MUST still drain +
    /// release the cpal stream (`capture.stop()`), never leaking the mic open.
    #[tokio::test]
    async fn stop_and_transcribe_discards_buffer_on_closed_gate() {
        let temp = tempfile::TempDir::new().unwrap();
        let capture = StopSpyCapture::with_buffer(vec![0.1, 0.2, 0.3]); // NON-empty
        let stt = SpyStt::new();
        // microphone consent NOT granted → gate closed.
        let state = ptt_state(temp.path(), false, capture.clone(), stt.clone());

        let result = super::stop_and_transcribe_inner(&state).await;

        let gate_err = result.unwrap_err();
        assert!(
            gate_err.code.contains("validation"),
            "stop_and_transcribe must error (privacy gate) when microphone consent is revoked"
        );
        assert_eq!(
            stt.transcribe_calls(),
            0,
            "no audio may reach STT (no cloud egress) on a closed gate"
        );
        assert_eq!(
            capture.stop_calls(),
            1,
            "capture.stop() must run exactly once even on a closed gate (drain + release cpal, never leak the mic open)"
        );
    }

    /// F-MIC-1 positive control (anti-vacuous): with microphone consent granted
    /// the same path DOES transcribe the drained buffer exactly once.
    #[tokio::test]
    async fn stop_and_transcribe_transcribes_on_open_gate() {
        let temp = tempfile::TempDir::new().unwrap();
        let capture = StopSpyCapture::with_buffer(vec![0.1, 0.2, 0.3]);
        let stt = SpyStt::new();
        // microphone consent granted → gate open.
        let state = ptt_state(temp.path(), true, capture.clone(), stt.clone());

        let result = super::stop_and_transcribe_inner(&state).await;

        // stop_and_transcribe_inner returns Result<()>; the only contract value
        // is unit — the observable effects (transcribe_calls, stop_calls) are
        // the real pins below (#5594).
        result.expect("stop_and_transcribe must return Ok when microphone consent is granted");
        assert_eq!(
            stt.transcribe_calls(),
            1,
            "open gate must transcribe the drained buffer exactly once"
        );
        assert_eq!(capture.stop_calls(), 1, "capture.stop() runs once");
    }

    #[tokio::test]
    async fn vad_speech_end_signal_queue_coalesces_when_receiver_busy() {
        let (tx, mut rx) = super::new_vad_speech_signal_channel();

        assert!(super::try_enqueue_vad_speech_signal(&tx));
        assert!(!super::try_enqueue_vad_speech_signal(&tx));

        assert_eq!(rx.recv().await, Some(()));
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));

        assert!(super::try_enqueue_vad_speech_signal(&tx));
        assert_eq!(rx.recv().await, Some(()));
    }

    /// Build (config_manager, consent_manager) with audio enabled + consent
    /// granted → the audio gate is OPEN unless a term is flipped (pause/battery).
    fn open_gate_inputs(
        dir: &std::path::Path,
    ) -> (
        maekon_core::config_manager::ConfigManager,
        Option<Arc<dyn ConsentManagerPort>>,
    ) {
        let cfg = ConfigManager::with_path(dir.join("config.json")).expect("config manager");
        // `update_with` takes `FnOnce(&mut AppConfig) -> Result<(), String>`, so
        // the closure MUST return Ok — a bare assignment would not compile.
        cfg.update_with(|c| {
            c.audio.enabled = true;
            Ok(())
        })
        .expect("enable audio");
        let cm = ConsentManager::new(dir.join("consent.json"));
        cm.grant_consent(
            ConsentPermissions {
                microphone: true,
                ..Default::default()
            },
            30,
        )
        .unwrap();
        (cfg, Some(Arc::new(cm)))
    }

    #[tokio::test(start_paused = true)]
    async fn vad_receiver_stops_mic_on_silent_pause_within_tick() {
        // Privacy-critical: user pauses while SILENT (no speech-ended signal). The
        // ≤2 s backstop tick must re-gate, stop the MIC (cpal teardown via
        // stop_vad), and emit privacy_gate_closed — even though rx never fires.
        //
        // NOTE on test rigor: `tokio::time::interval`'s FIRST tick fires
        // immediately (t=0). A naive test that closes the gate BEFORE the first
        // advance would stop on that startup tick and pass vacuously, proving
        // nothing about the backstop. So this test instead: (1) runs with the gate
        // OPEN across the first (immediate) tick and asserts the mic is STILL on
        // (count==0, task alive) — consuming the startup tick harmlessly; then
        // (2) closes the gate mid-silence; then (3) advances one 2 s window and
        // asserts the stop NOW happens. The stop is therefore attributable to the
        // post-closure backstop tick, not the startup tick.
        let temp = tempfile::TempDir::new().unwrap();
        let (cfg, consent) = open_gate_inputs(temp.path());
        let capture = MockVadCapture::listening();
        let stt = Arc::new(tokio::sync::RwLock::new(None));
        let vad_state = Arc::new(parking_lot::Mutex::new("listening".to_string()));
        let capture_paused = Arc::new(StdAtomicBool::new(false));
        let regate = Arc::new(tokio::sync::Notify::new());
        let (log, emit) = emit_recorder();

        // Keep tx alive so rx.recv() stays pending — forces the tick path (silence).
        let (_tx, rx) = super::new_vad_speech_signal_channel();
        let handle = tokio::spawn(super::run_vad_receiver(
            rx,
            capture.clone(),
            stt,
            vad_state.clone(),
            cfg,
            consent,
            capture_paused.clone(),
            regate,
            emit,
        ));

        // Phase 1: gate OPEN across the first (immediate) tick + a full 2 s window.
        // The mic must remain on and the task must stay alive — this rules out a
        // vacuous pass on the startup tick.
        tokio::time::advance(std::time::Duration::from_secs(2)).await;
        assert_eq!(
            capture.stop_vad_count(),
            0,
            "mic must stay on while the gate is open (startup tick must NOT stop it)"
        );
        assert!(
            !handle.is_finished(),
            "task must still be running while the gate is open"
        );
        assert_eq!(*vad_state.lock(), "listening");

        // Phase 2: pause while silent, then let the NEXT 2 s backstop tick fire.
        capture_paused.store(true, AtomicOrdering::SeqCst);
        tokio::time::advance(std::time::Duration::from_secs(2)).await;
        handle.await.unwrap();

        assert_eq!(
            capture.stop_vad_count(),
            1,
            "mic must be stopped on closed gate during silence (≤2 s backstop tick)"
        );
        assert_eq!(*vad_state.lock(), "idle");
        assert!(
            log.lock()
                .iter()
                .any(|(e, p)| e == "vad-state-changed" && p["reason"] == "privacy_gate_closed"),
            "must emit a distinguishable privacy_gate_closed event"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn vad_receiver_stops_mic_immediately_on_regate_signal() {
        // The pause-gesture fast path: regate.notify_one() stops the mic at once,
        // without waiting for the tick.
        let temp = tempfile::TempDir::new().unwrap();
        let (cfg, consent) = open_gate_inputs(temp.path());
        let capture = MockVadCapture::listening();
        let stt = Arc::new(tokio::sync::RwLock::new(None));
        let vad_state = Arc::new(parking_lot::Mutex::new("listening".to_string()));
        let capture_paused = Arc::new(StdAtomicBool::new(false));
        let regate = Arc::new(tokio::sync::Notify::new());
        let (log, emit) = emit_recorder();

        let (_tx, rx) = super::new_vad_speech_signal_channel();
        let handle = tokio::spawn(super::run_vad_receiver(
            rx,
            capture.clone(),
            stt,
            vad_state.clone(),
            cfg,
            consent,
            capture_paused.clone(),
            regate.clone(),
            emit,
        ));

        capture_paused.store(true, AtomicOrdering::SeqCst);
        regate.notify_one();
        handle.await.unwrap();

        assert_eq!(capture.stop_vad_count(), 1);
        assert!(log
            .lock()
            .iter()
            .any(|(e, p)| e == "vad-state-changed" && p["reason"] == "privacy_gate_closed"));
    }

    #[tokio::test(start_paused = true)]
    async fn vad_receiver_stops_mic_on_microphone_consent_revoke_mid_listen() {
        // F-MIC-2 (receiver level): revoking the `microphone` consent mid-listen
        // (NOT a pause — `capture_paused` stays false) and firing the regate signal
        // must tear the mic down at once via the immediate privacy arm. This is the
        // receiver-side proof that the consent-write→signal_vad_regate wiring
        // (set_consent/withdraw_consent, tested separately) actually stops capture.
        let temp = tempfile::TempDir::new().unwrap();
        let (cfg, consent) = open_gate_inputs(temp.path());
        // Loop-2 note #3: clone the Arc<dyn ConsentManagerPort> BEFORE moving it into the
        // receiver — `cm_handle` revokes mid-listen; the clone is moved in.
        let cm_handle = consent
            .clone()
            .expect("open_gate_inputs grants consent → Some");
        let capture = MockVadCapture::listening();
        let stt = Arc::new(tokio::sync::RwLock::new(None));
        let vad_state = Arc::new(parking_lot::Mutex::new("listening".to_string()));
        // capture_paused stays FALSE for the whole test — the ONLY thing that
        // closes the gate is the microphone-consent revoke.
        let capture_paused = Arc::new(StdAtomicBool::new(false));
        let regate = Arc::new(tokio::sync::Notify::new());
        let (log, emit) = emit_recorder();

        let (_tx, rx) = super::new_vad_speech_signal_channel();
        let handle = tokio::spawn(super::run_vad_receiver(
            rx,
            capture.clone(),
            stt,
            vad_state.clone(),
            cfg,
            consent,
            capture_paused.clone(),
            regate.clone(),
            emit,
        ));

        // Revoke microphone consent mid-listen, THEN fire the regate signal. Both
        // happen before the task is first polled (start_paused), so when it polls,
        // the biased immediate arm (regate, already notified) evaluates the gate,
        // sees microphone revoked (effective_permissions → all-false), and stops.
        cm_handle.revoke_consent().expect("revoke must persist");
        assert!(
            !capture_paused.load(AtomicOrdering::SeqCst),
            "precondition: pause must NOT be the cause — revoke alone closes the gate"
        );
        regate.notify_one();
        handle.await.unwrap();

        assert_eq!(
            capture.stop_vad_count(),
            1,
            "microphone-consent revoke + regate must stop the mic exactly once"
        );
        assert!(
            log.lock()
                .iter()
                .any(|(e, p)| e == "vad-state-changed" && p["reason"] == "privacy_gate_closed"),
            "must emit privacy_gate_closed on consent-revoke teardown"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn vad_receiver_no_stt_egress_when_gate_closed_on_rx_arm() {
        // STRONGEST privacy guarantee: a speech-ended signal arrives but the gate
        // has closed in the meantime → the rx arm MUST drain+discard the buffer
        // and stop WITHOUT calling `stt.transcribe` (no cloud/local STT egress).
        //
        // This test specifically exercises the RX ARM (not a privacy arm). The
        // select! is `biased` (regate, tick, rx) and `interval`'s first tick is
        // Ready at t=0, so we must keep the tick arm from pre-empting:
        //   Phase 1 — gate OPEN: drive the task to its first park (via yields, NOT
        //     time advance — see below) so it consumes the immediate t=0 tick as a
        //     no-op and re-parks with the next tick deadline at t=2 s, STRICTLY in
        //     the future. Assert the mic is still on / task alive (rules out a
        //     vacuous pass).
        //   Phase 2 — WITHOUT advancing time (so the t=2 s tick stays strictly
        //     pending → the tick arm cannot fire), close the gate and send a
        //     speech-ended signal. With arm 1 (regate) and arm 2 (tick) both
        //     pending, the only ready arm is rx → the rx-arm re-gate sees the
        //     closed gate → drain + discard + stop, never calling transcribe.
        //     (If the rx-arm re-gate were absent, transcribe WOULD run here before
        //     auto-advance later fires the tick — verified: deleting that block
        //     makes this test FAIL with transcribe_calls == 1.)
        let temp = tempfile::TempDir::new().unwrap();
        let (cfg, consent) = open_gate_inputs(temp.path());
        // Non-empty drain buffer so, absent the rx-arm gate, transcribe WOULD run.
        let capture = MockVadCapture::listening_with_buffer(vec![0.1_f32; 16_000]);
        let spy = SpyStt::new();
        let stt: Arc<
            tokio::sync::RwLock<Option<Arc<dyn maekon_core::ports::stt_provider::SttProvider>>>,
        > = Arc::new(tokio::sync::RwLock::new(Some(
            spy.clone() as Arc<dyn maekon_core::ports::stt_provider::SttProvider>
        )));
        let vad_state = Arc::new(parking_lot::Mutex::new("listening".to_string()));
        let capture_paused = Arc::new(StdAtomicBool::new(false));
        let regate = Arc::new(tokio::sync::Notify::new());
        let (log, emit) = emit_recorder();

        let (tx, rx) = super::new_vad_speech_signal_channel();
        let handle = tokio::spawn(super::run_vad_receiver(
            rx,
            capture.clone(),
            stt,
            vad_state.clone(),
            cfg,
            consent,
            capture_paused.clone(),
            regate,
            emit,
        ));

        // Phase 1: gate OPEN. Drive the task to its first park WITHOUT advancing
        // virtual time, so it consumes the immediate t=0 interval tick (gate open
        // → no-op) and re-parks awaiting the next tick at t=2 s, STRICTLY in the
        // future. `yield_now` (not `advance`) is used deliberately: under
        // `start_paused`, a freshly spawned task is not polled until the scheduler
        // runs it, and `advance` does NOT drive that first poll here — it would
        // leave the t=0 tick unconsumed so the biased tick arm would steal phase 2
        // from the rx arm (masking a dropped rx-arm re-gate). Several yields cover
        // the multi-step start-up (poll → consume t=0 → re-enter select → park).
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            capture.stop_vad_count(),
            0,
            "mic must stay on while the gate is open (startup tick must NOT stop it)"
        );
        assert!(
            !handle.is_finished(),
            "task must still be running while the gate is open"
        );
        assert_eq!(
            spy.transcribe_calls(),
            0,
            "no speech signal yet → no transcribe"
        );

        // Phase 2: close the gate, then deliver a speech-ended signal WITHOUT
        // advancing virtual time. The t=2 s tick is still pending, so the biased
        // select! resolves the rx arm — exercising the rx-arm re-gate.
        capture_paused.store(true, AtomicOrdering::SeqCst);
        tx.try_send(()).expect("speech-ended signal must enqueue");
        handle.await.unwrap();

        assert_eq!(
            spy.transcribe_calls(),
            0,
            "closed-gate rx arm must NOT call transcribe — no cloud-STT egress"
        );
        assert_eq!(
            capture.stop_vad_count(),
            1,
            "closed-gate rx arm must stop the mic"
        );
        assert_eq!(*vad_state.lock(), "idle");
        assert!(
            log.lock()
                .iter()
                .any(|(e, p)| e == "vad-state-changed" && p["reason"] == "privacy_gate_closed"),
            "must emit a distinguishable privacy_gate_closed event"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn vad_receiver_transcribes_on_rx_arm_when_gate_open() {
        // Positive control for the no-egress test: with the gate OPEN, the SAME
        // rx arm DOES call transcribe exactly once (proving the no-egress result
        // is attributable to the closed gate, not to the rx arm being unreachable).
        let temp = tempfile::TempDir::new().unwrap();
        let (cfg, consent) = open_gate_inputs(temp.path());
        let capture = MockVadCapture::listening_with_buffer(vec![0.1_f32; 16_000]);
        let spy = SpyStt::new();
        let stt: Arc<
            tokio::sync::RwLock<Option<Arc<dyn maekon_core::ports::stt_provider::SttProvider>>>,
        > = Arc::new(tokio::sync::RwLock::new(Some(
            spy.clone() as Arc<dyn maekon_core::ports::stt_provider::SttProvider>
        )));
        let vad_state = Arc::new(parking_lot::Mutex::new("listening".to_string()));
        let capture_paused = Arc::new(StdAtomicBool::new(false));
        let regate = Arc::new(tokio::sync::Notify::new());
        let (_log, emit) = emit_recorder();

        let (tx, rx) = super::new_vad_speech_signal_channel();
        let handle = tokio::spawn(super::run_vad_receiver(
            rx,
            capture.clone(),
            stt,
            vad_state.clone(),
            cfg,
            consent,
            capture_paused.clone(),
            regate,
            emit,
        ));

        // Drive the task to its first park (yields, not time advance — same
        // start-paused first-poll reason as the no-egress test) so it consumes the
        // t=0 tick with the gate OPEN and re-parks awaiting the t=2 s tick. Then
        // deliver a speech-ended signal WITHOUT advancing time (next tick pending →
        // rx arm wins). Gate is open → transcribe runs once. The mock reports vad
        // still active, so after transcription the loop returns to listening; drop
        // tx so the subsequent rx.recv() yields None and the task terminates.
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        tx.try_send(()).expect("speech-ended signal must enqueue");
        drop(tx);
        handle.await.unwrap();

        assert_eq!(
            spy.transcribe_calls(),
            1,
            "open-gate rx arm must transcribe exactly once"
        );
    }

    #[tokio::test(start_paused = true)]
    #[serial]
    async fn vad_receiver_stops_mic_on_battery_saver_tick() {
        // Battery-saver is an event-less term → only the tick catches it.
        let temp = tempfile::TempDir::new().unwrap();
        let (cfg, consent) = open_gate_inputs(temp.path());
        cfg.update_with(|c| {
            c.schedule.pause_on_battery_saver = true;
            Ok(())
        })
        .unwrap();
        let capture = MockVadCapture::listening();
        let stt = Arc::new(tokio::sync::RwLock::new(None));
        let vad_state = Arc::new(parking_lot::Mutex::new("listening".to_string()));
        let capture_paused = Arc::new(StdAtomicBool::new(false));
        let regate = Arc::new(tokio::sync::Notify::new());
        let (_log, emit) = emit_recorder();

        let (_tx, rx) = super::new_vad_speech_signal_channel();
        // Enter battery-saver (global flag consumed by the scheduler wrapper).
        crate::scheduler::set_battery_saver_active_for_scheduler(true);
        let handle = tokio::spawn(super::run_vad_receiver(
            rx,
            capture.clone(),
            stt,
            vad_state.clone(),
            cfg,
            consent,
            capture_paused,
            regate,
            emit,
        ));
        tokio::time::advance(std::time::Duration::from_secs(2)).await;
        let result = handle.await;
        // Reset the global FIRST (even on the assert path) so it can't pollute
        // other tests that share this process-global flag.
        crate::scheduler::set_battery_saver_active_for_scheduler(false);
        result.unwrap();

        assert_eq!(
            capture.stop_vad_count(),
            1,
            "battery-saver must stop the mic via the tick"
        );
    }
}
