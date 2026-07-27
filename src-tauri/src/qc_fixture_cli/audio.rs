use anyhow::{bail, Context, Result};
use maekon_core::config::{AppConfig, CloudSttPolicy, MicInputMode, SttProviderKind};
use maekon_core::config_manager::ConfigManager;
use maekon_core::consent::{ConsentManager, ConsentPermissions};

use super::types::AudioSeedReport;
use super::{require_exact_gate, require_isolated_profile, AUDIO_GATE_ENV, AUDIO_STATE_FILE};

pub(crate) fn run_audio_from_env() -> Result<AudioSeedReport> {
    require_isolated_profile()?;
    require_exact_gate(AUDIO_GATE_ENV)?;
    if !cfg!(feature = "audio") {
        bail!("the isolated QC app must be built with the `audio` feature")
    }

    let config = ConfigManager::new()
        .context("initialize isolated config")?
        .update_with(|config| {
            configure_audio_fixture(config);
            Ok(())
        })
        .context("persist isolated QC audio config")?;

    let data_dir = ConfigManager::data_dir().context("resolve isolated data directory")?;
    std::fs::create_dir_all(&data_dir).context("create isolated data directory")?;
    let state_path = data_dir.join(AUDIO_STATE_FILE);
    let stale_state_removed = if state_path.exists() {
        std::fs::remove_file(&state_path).context("remove stale QC audio fixture state")?;
        true
    } else {
        false
    };

    let consent = ConsentManager::new(data_dir.join("consent.json"));
    consent
        .grant_consent(
            ConsentPermissions {
                microphone: true,
                ..ConsentPermissions::default()
            },
            config.storage.retention_days,
        )
        .context("grant isolated microphone-only consent")?;

    Ok(AudioSeedReport {
        data_dir: data_dir.display().to_string(),
        microphone_consent: true,
        synthetic_capture: true,
        cloud_stt_disabled: true,
        stale_state_removed,
    })
}

pub(super) fn configure_audio_fixture(config: &mut AppConfig) {
    config.vision.capture_enabled = false;
    config.audio.enabled = true;
    config.audio.mic_input_mode = MicInputMode::VoiceActivity;
    config.audio.stt_provider = SttProviderKind::Local;
    config.audio.cloud_api_key.clear();
    config.audio.cloud_stt_policy = CloudSttPolicy::Disabled;
    config.sync.enabled = false;
    config.integration.enabled = false;
    config.telemetry.enabled = false;
    config.telemetry.crash_reports = false;
    config.telemetry.usage_analytics = false;
    config.telemetry.performance_metrics = false;
    config.web.allow_external = false;
    config.external_grpc.enabled = false;
    config.automation.enabled = false;
}
