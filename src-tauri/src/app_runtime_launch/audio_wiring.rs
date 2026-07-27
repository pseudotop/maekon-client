use crate::runtime_state::{AudioContext, AudioRuntimeState};
use maekon_core::ports::consent_manager::ConsentManagerPort;
use maekon_core::ports::transcript_storage::TranscriptStoragePort;
use std::sync::Arc;
use tauri::{AppHandle, Manager};

#[cfg(feature = "audio")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AudioCaptureSelection {
    Real,
    Synthetic,
    Disabled,
}

/// A fixture request may never fall back to the OS microphone. Debug builds
/// select the gated synthetic adapter; release builds fail closed.
#[cfg(feature = "audio")]
fn select_audio_capture(
    fixture_variable_present: bool,
    debug_build: bool,
) -> AudioCaptureSelection {
    match (fixture_variable_present, debug_build) {
        (false, _) => AudioCaptureSelection::Real,
        (true, true) => AudioCaptureSelection::Synthetic,
        (true, false) => AudioCaptureSelection::Disabled,
    }
}

pub(super) fn build_audio_runtime_state(
    app_handle: &AppHandle,
    config_manager: maekon_core::config_manager::ConfigManager,
    config: &maekon_core::config::AppConfig,
    capture_consent_manager: Arc<dyn ConsentManagerPort>,
    capture_paused: Arc<std::sync::atomic::AtomicBool>,
    // #8059: local transcript persistence (`Arc<SqliteStorage>` coerced to the
    // port). A successful transcription is saved best-effort so it survives
    // restart and is reachable from keyword search.
    transcript_storage: Option<Arc<dyn TranscriptStoragePort>>,
) -> AudioRuntimeState {
    let app_data_dir: std::path::PathBuf = app_handle
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."));
    let model_dir = app_data_dir.join("models");
    #[cfg(all(feature = "audio", debug_assertions))]
    let fixture_data_dir = maekon_core::config_manager::ConfigManager::data_dir()
        .unwrap_or_else(|_| app_data_dir.clone());

    #[cfg(feature = "audio")]
    let audio_capture_selection = select_audio_capture(
        std::env::var_os("MAEKON_DEBUG_QC_AUDIO_FIXTURE").is_some(),
        cfg!(debug_assertions),
    );

    let audio_capture: Option<Arc<dyn maekon_core::ports::audio_capture::AudioCapturePort>> = {
        #[cfg(feature = "audio")]
        {
            if config.audio.enabled {
                match audio_capture_selection {
                    AudioCaptureSelection::Real => {
                        // #6342: thread the configured max recording duration into the
                        // capture buffers (was silently ignored).
                        Some(Arc::new(maekon_audio::AudioCapture::new(
                            config.audio.max_recording_secs,
                        )))
                    }
                    AudioCaptureSelection::Synthetic => {
                        #[cfg(debug_assertions)]
                        {
                            match crate::qc_audio_fixture::build_from_env(&fixture_data_dir) {
                                Ok(Some(capture)) => {
                                    tracing::info!(
                                        "isolated synthetic QC audio capture enabled; OS microphone remains closed"
                                    );
                                    Some(capture)
                                }
                                Ok(None) => {
                                    tracing::error!(
                                        "synthetic QC audio fixture was selected without its exact gate; audio capture disabled"
                                    );
                                    None
                                }
                                Err(error) => {
                                    tracing::error!(
                                        "synthetic QC audio fixture rejected: {error}; audio capture disabled"
                                    );
                                    None
                                }
                            }
                        }
                        #[cfg(not(debug_assertions))]
                        {
                            None
                        }
                    }
                    AudioCaptureSelection::Disabled => {
                        tracing::error!(
                            "synthetic QC audio fixture requested in a release build; audio capture disabled"
                        );
                        None
                    }
                }
            } else {
                tracing::debug!("audio capture disabled by config");
                None
            }
        }
        #[cfg(not(feature = "audio"))]
        {
            None
        }
    };

    #[cfg(feature = "audio")]
    let synthetic_fixture = matches!(audio_capture_selection, AudioCaptureSelection::Synthetic)
        && audio_capture.is_some();
    #[cfg(not(feature = "audio"))]
    let synthetic_fixture = false;

    let stt_engine: Option<Arc<dyn maekon_core::ports::stt_provider::SttProvider>> = {
        use maekon_core::config::SttProviderKind;

        if synthetic_fixture {
            #[cfg(all(feature = "audio", debug_assertions))]
            {
                Some(crate::qc_audio_fixture::synthetic_stt_provider())
            }
            #[cfg(not(all(feature = "audio", debug_assertions)))]
            {
                None
            }
        } else if !config.audio.enabled {
            None
        } else {
            let local_provider: Option<Arc<dyn maekon_core::ports::stt_provider::SttProvider>> = {
                #[cfg(feature = "stt")]
                {
                    let model_path = if config.audio.whisper_model_path.is_empty() {
                        #[cfg(feature = "download")]
                        {
                            model_dir.join(maekon_audio::model_downloader::model_filename(
                                config.audio.model_size,
                            ))
                        }
                        #[cfg(not(feature = "download"))]
                        {
                            app_handle
                                .path()
                                .resource_dir()
                                .map(|d| d.join("ggml-base.bin"))
                                .unwrap_or_default()
                        }
                    } else {
                        std::path::PathBuf::from(&config.audio.whisper_model_path)
                    };
                    // #6344: for the download-managed default model, gate on a
                    // verify-on-load SHA check (not bare existence), so a model
                    // corrupted/tampered on disk after a verified download is not
                    // loaded. A user-supplied path has no pinned SHA, so it stays
                    // existence-gated by design.
                    let model_loadable = if config.audio.whisper_model_path.is_empty() {
                        #[cfg(feature = "download")]
                        {
                            maekon_audio::model_downloader::managed_model_verified_on_disk(
                                config.audio.model_size,
                                &model_dir,
                            )
                        }
                        #[cfg(not(feature = "download"))]
                        {
                            model_path.exists()
                        }
                    } else {
                        model_path.exists()
                    };
                    if model_loadable {
                        match maekon_audio::WhisperSttProvider::new(
                            &model_path,
                            config.audio.language,
                        ) {
                            Ok(provider) => {
                                let stt_pii_sanitizer: Arc<
                                    dyn maekon_core::ports::pii_sanitizer::PiiSanitizer,
                                > = Arc::new(maekon_vision::privacy::VisionPiiSanitizer);
                                let provider = provider.with_pii_sanitizer(
                                    stt_pii_sanitizer,
                                    config.privacy.pii_filter_level,
                                );
                                tracing::info!(
                                    "Whisper STT loaded with PII sanitizer: {}",
                                    model_path.display()
                                );
                                Some(Arc::new(provider) as _)
                            }
                            Err(e) => {
                                tracing::warn!("Failed to load Whisper model: {e}");
                                None
                            }
                        }
                    } else {
                        tracing::info!(
                            "Whisper model not found at {}; local STT unavailable",
                            model_path.display()
                        );
                        None
                    }
                }
                #[cfg(not(feature = "stt"))]
                {
                    None
                }
            };

            let cloud_provider: Option<Arc<dyn maekon_core::ports::stt_provider::SttProvider>> = {
                #[cfg(feature = "cloud-stt")]
                {
                    use maekon_core::config::CloudSttBuild;
                    // Enterprise managed-policy gate (#4685): the build decision
                    // (policy fail-safe + API-key check) is single-sourced and
                    // unit-tested in AudioConfig::cloud_stt_build_decision so both
                    // wiring paths enforce identically.
                    match config.audio.cloud_stt_build_decision() {
                        CloudSttBuild::SkipPolicyBlocked => {
                            tracing::warn!(
                                "Cloud STT blocked by managed policy ({}) at startup; raw audio will not egress — using local STT if available",
                                config.audio.cloud_stt_policy
                            );
                            None
                        }
                        CloudSttBuild::SkipNoKey => None,
                        CloudSttBuild::Build => match maekon_audio::CloudSttProvider::new(
                            config.audio.cloud_api_key.clone(),
                            config.audio.cloud_stt_endpoint.clone(),
                            config.audio.language,
                            config.audio.cloud_timeout_secs,
                        ) {
                            Ok(p) => {
                                let stt_pii_sanitizer: Arc<
                                    dyn maekon_core::ports::pii_sanitizer::PiiSanitizer,
                                > = Arc::new(maekon_vision::privacy::VisionPiiSanitizer);
                                let p = p.with_pii_sanitizer(
                                    stt_pii_sanitizer,
                                    config.privacy.pii_filter_level,
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

            match config.audio.stt_provider {
                SttProviderKind::Cloud => match (cloud_provider, local_provider) {
                    (Some(cloud), Some(local)) => {
                        tracing::info!("STT startup: Cloud with local fallback");
                        Some(
                            Arc::new(crate::fallback_stt::FallbackSttProvider::new(cloud, local))
                                as _,
                        )
                    }
                    (Some(cloud), None) => {
                        tracing::info!("STT startup: Cloud only (no local model)");
                        Some(cloud)
                    }
                    (None, Some(local)) => {
                        tracing::warn!("Cloud STT unavailable at startup, falling back to local");
                        Some(local)
                    }
                    (None, None) => None,
                },
                SttProviderKind::Local => {
                    if local_provider.is_some() {
                        tracing::info!("STT startup: Local provider");
                    }
                    local_provider
                }
            }
        }
    };

    let model_downloader: Option<Arc<dyn maekon_core::ports::model_downloader::ModelDownloader>> = {
        #[cfg(feature = "download")]
        {
            Some(Arc::new(maekon_audio::WhisperModelDownloader::new()))
        }
        #[cfg(not(feature = "download"))]
        {
            None
        }
    };

    AudioRuntimeState::new(
        config_manager,
        Some(capture_consent_manager),
        capture_paused,
        AudioContext {
            capture: audio_capture,
            synthetic_fixture,
            stt_engine: Arc::new(tokio::sync::RwLock::new(stt_engine)),
            model_downloader,
            model_dir,
            downloading: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            download_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            vad_state: Arc::new(parking_lot::Mutex::new("idle".into())),
        },
        transcript_storage,
    )
}

#[cfg(all(test, feature = "audio"))]
mod tests {
    use super::*;

    #[test]
    fn normal_runtime_selects_real_capture() {
        assert_eq!(
            select_audio_capture(false, true),
            AudioCaptureSelection::Real
        );
        assert_eq!(
            select_audio_capture(false, false),
            AudioCaptureSelection::Real
        );
    }

    #[test]
    fn fixture_request_is_synthetic_in_debug_and_disabled_in_release() {
        assert_eq!(
            select_audio_capture(true, true),
            AudioCaptureSelection::Synthetic
        );
        assert_eq!(
            select_audio_capture(true, false),
            AudioCaptureSelection::Disabled
        );
    }
}
