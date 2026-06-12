use crate::runtime_state::{AudioContext, AudioRuntimeState};
use maekon_core::ports::consent_manager::ConsentManagerPort;
use std::sync::Arc;
use tauri::{AppHandle, Manager};

pub(super) fn build_audio_runtime_state(
    app_handle: &AppHandle,
    config_manager: maekon_core::config_manager::ConfigManager,
    config: &maekon_core::config::AppConfig,
    capture_consent_manager: Arc<dyn ConsentManagerPort>,
    capture_paused: Arc<std::sync::atomic::AtomicBool>,
) -> AudioRuntimeState {
    let model_dir: std::path::PathBuf = app_handle
        .path()
        .app_data_dir()
        .map(|d| d.join("models"))
        .unwrap_or_else(|_| std::path::PathBuf::from("models"));

    let audio_capture: Option<Arc<dyn maekon_core::ports::audio_capture::AudioCapturePort>> = {
        #[cfg(feature = "audio")]
        {
            if config.audio.enabled {
                Some(Arc::new(maekon_audio::AudioCapture::new()))
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

    let stt_engine: Option<Arc<dyn maekon_core::ports::stt_provider::SttProvider>> = {
        use maekon_core::config::SttProviderKind;

        if !config.audio.enabled {
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
                    if model_path.exists() {
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
            stt_engine: Arc::new(tokio::sync::RwLock::new(stt_engine)),
            model_downloader,
            model_dir,
            downloading: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            download_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            vad_state: Arc::new(parking_lot::Mutex::new("idle".into())),
        },
    )
}
