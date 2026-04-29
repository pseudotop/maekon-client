//! Tauri IPC commands for audio capture and speech-to-text.

use std::sync::Arc;

use tauri::{command, Emitter};

use oneshim_core::models::audio::TranscriptionResult;

use crate::ipc_error::IpcError;
use crate::runtime_state::AudioRuntimeState;

/// Canonical "audio capture not available" error — audio capture adapter missing.
fn audio_capture_not_available() -> IpcError {
    IpcError::new("service.unavailable", "audio capture not available")
}

/// Start microphone capture (Push-to-Talk begin).
#[command]
pub async fn start_audio_capture(
    state: tauri::State<'_, AudioRuntimeState>,
) -> Result<(), IpcError> {
    // CONS-PC04 / D13: 4-term composite gate — reject when capture is not permitted.
    // Uses snapshot() (O(1) Arc-clone) per CONS-PI13; NOT get() (deep-clone 37 sections).
    {
        use std::sync::atomic::Ordering;
        let consent = state
            .consent_manager()
            .and_then(|cm| cm.current_consent().map(|r| r.permissions.clone()))
            .unwrap_or_default();
        let paused = state.capture_paused().load(Ordering::Relaxed);
        let permitted = crate::scheduler::capture_permitted_now(
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
    }

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
    let capture = state
        .audio()
        .capture
        .as_ref()
        .ok_or_else(audio_capture_not_available)?;

    let stt = {
        let guard = state.audio().stt_engine.read().await;
        guard.as_ref().map(Arc::clone).ok_or_else(|| {
            IpcError::new(
                "service.unavailable",
                "STT engine not available (model may not be loaded)",
            )
        })?
    };

    let buffer = capture.stop().map_err(IpcError::from)?;

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

use oneshim_core::config::WhisperModelSize;
use oneshim_core::models::audio::{AudioStatus, ModelDownloadStatus, VadConfig};
use tracing::debug;

/// Get combined audio subsystem status (reads live config via config_manager).
#[command]
pub async fn get_audio_status(
    state: tauri::State<'_, AudioRuntimeState>,
) -> Result<AudioStatus, IpcError> {
    let live_config = state.config_manager().get();
    let audio_cfg = &live_config.audio;
    let model_status = match &state.audio().model_downloader {
        Some(dl) => dl.model_status(audio_cfg.model_size, &state.audio().model_dir),
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
                let _ = app_clone.emit(
                    "audio-model-complete",
                    serde_json::json!({
                        "path": path.to_string_lossy(),
                        "model_size": model_size,
                        "size_bytes": std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
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
        state.audio().model_downloader.as_ref().ok_or_else(|| {
            IpcError::new("service.unavailable", "model downloader not available")
        })?;
    dl.delete_model(model_size, &state.audio().model_dir)
        .map_err(IpcError::from)
}

/// Start VAD listening mode — automatically detects speech start/end.
#[command]
pub async fn start_vad_listening(
    app: tauri::AppHandle,
    state: tauri::State<'_, AudioRuntimeState>,
) -> Result<(), IpcError> {
    let capture = state
        .audio()
        .capture
        .as_ref()
        .ok_or_else(audio_capture_not_available)?;

    let live_cfg = state.config_manager().get();
    let config = VadConfig {
        threshold: live_cfg.audio.vad_threshold,
        silence_ms: live_cfg.audio.vad_silence_ms,
        min_speech_ms: live_cfg.audio.vad_min_speech_ms,
    };

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();

    // Signal callback — called on audio thread when speech ends.
    // Lightweight: just sends () to the channel.
    let on_speech_signal = Arc::new(move || {
        if let Err(e) = tx.send(()) {
            debug!("channel send failed: {e}");
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

    // Spawn receiver task to handle speech-ended signals
    let capture_clone = Arc::clone(capture);
    let stt_engine = state.audio().stt_engine.clone();
    let vad_state = state.audio().vad_state.clone();
    let app_clone = app.clone();

    tokio::spawn(async move {
        while rx.recv().await.is_some() {
            // Set state to "transcribing"
            *vad_state.lock() = "transcribing".into();
            let _ = app_clone.emit(
                "vad-state-changed",
                serde_json::json!({"state": "transcribing"}),
            );

            let start = std::time::Instant::now();

            // Drain speech buffer and transcribe
            let result: Result<oneshim_core::models::audio::TranscriptionResult, String> = async {
                let buffer = capture_clone
                    .drain_speech_buffer()
                    .map_err(|e| e.to_string())?;

                if buffer.is_empty() {
                    return Ok(oneshim_core::models::audio::TranscriptionResult {
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
                Ok(tr) => {
                    let _ = app_clone.emit(
                        "vad-transcription-result",
                        serde_json::json!({
                            "text": tr.text,
                            "duration_secs": tr.duration_secs,
                            "processing_secs": processing_secs,
                        }),
                    );
                }
                Err(e) => {
                    tracing::warn!("VAD transcription failed: {e}");
                    let _ = app_clone.emit(
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

            // Return to "listening" state if still active
            if capture_clone.is_vad_active() {
                *vad_state.lock() = "listening".into();
                let _ = app_clone.emit(
                    "vad-state-changed",
                    serde_json::json!({"state": "listening"}),
                );
            } else {
                *vad_state.lock() = "idle".into();
                if let Err(e) =
                    app_clone.emit("vad-state-changed", serde_json::json!({"state": "idle"}))
                {
                    debug!("emit vad-state-changed failed: {e}");
                }
                break;
            }
        }
    });

    Ok(())
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
    use oneshim_core::config::SttProviderKind;

    let live_config = state.config_manager().get();
    let config = &live_config.audio;

    // Build local provider (if model available)
    let local_provider: Option<Arc<dyn oneshim_core::ports::stt_provider::SttProvider>> = {
        #[cfg(feature = "stt")]
        {
            #[cfg(feature = "download")]
            let model_path =
                state
                    .audio()
                    .model_dir
                    .join(oneshim_audio::model_downloader::model_filename(
                        config.model_size,
                    ));
            #[cfg(not(feature = "download"))]
            let model_path = std::path::PathBuf::from(&config.whisper_model_path);

            if model_path.exists() {
                match oneshim_audio::WhisperSttProvider::new(&model_path, config.language) {
                    Ok(p) => Some(Arc::new(p) as _),
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
    let cloud_provider: Option<Arc<dyn oneshim_core::ports::stt_provider::SttProvider>> = {
        #[cfg(feature = "cloud-stt")]
        {
            if !config.cloud_api_key.is_empty() {
                match oneshim_audio::CloudSttProvider::new(
                    config.cloud_api_key.clone(),
                    config.cloud_stt_endpoint.clone(),
                    config.language,
                    config.cloud_timeout_secs,
                ) {
                    Ok(p) => Some(Arc::new(p) as _),
                    Err(e) => {
                        tracing::warn!("Failed to create cloud STT: {e}");
                        None
                    }
                }
            } else {
                None
            }
        }
        #[cfg(not(feature = "cloud-stt"))]
        {
            None
        }
    };

    // Assemble final provider based on config preference
    let provider: Option<Arc<dyn oneshim_core::ports::stt_provider::SttProvider>> =
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
