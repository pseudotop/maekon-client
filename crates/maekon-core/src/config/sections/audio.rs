// 오디오 캡처 및 음성-텍스트 변환 설정
use super::super::enums::MicInputMode;
use super::super::enums::SttLanguage;
use super::super::enums::SttProviderKind;
use super::super::enums::WhisperModelSize;
use serde::{Deserialize, Serialize};

fn default_max_recording_secs() -> u32 {
    60
}

fn default_cloud_stt_endpoint() -> String {
    "https://api.openai.com/v1/audio/transcriptions".into()
}

fn default_cloud_timeout_secs() -> u32 {
    10
}

fn default_vad_threshold() -> f32 {
    0.02
}

fn default_vad_silence_ms() -> u32 {
    800
}

fn default_vad_min_speech_ms() -> u32 {
    300
}

/// Audio capture and speech-to-text configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConfig {
    /// Enable audio capture and STT features (opt-in).
    #[serde(default)]
    pub enabled: bool,
    /// Path to Whisper GGML model file. Empty = auto-detect bundled model.
    #[serde(default)]
    pub whisper_model_path: String,
    /// Language hint for Whisper transcription.
    #[serde(default)]
    pub language: SttLanguage,
    /// Maximum recording duration in seconds.
    #[serde(default = "default_max_recording_secs")]
    pub max_recording_secs: u32,
    /// Whisper model size selection.
    #[serde(default)]
    pub model_size: WhisperModelSize,
    /// STT provider selection (local or cloud).
    #[serde(default)]
    pub stt_provider: SttProviderKind,
    /// API key for cloud STT provider (e.g. OpenAI). Stored locally.
    #[serde(default)]
    pub cloud_api_key: String,
    /// Cloud STT endpoint URL.
    #[serde(default = "default_cloud_stt_endpoint")]
    pub cloud_stt_endpoint: String,
    /// Timeout in seconds for cloud STT requests.
    #[serde(default = "default_cloud_timeout_secs")]
    pub cloud_timeout_secs: u32,
    /// Mic input mode — Push-to-Talk (default) or Voice Activity Detection.
    #[serde(default)]
    pub mic_input_mode: MicInputMode,
    /// RMS energy threshold for VAD speech detection (0.0–1.0).
    #[serde(default = "default_vad_threshold")]
    pub vad_threshold: f32,
    /// Silence duration in ms to end an utterance.
    #[serde(default = "default_vad_silence_ms")]
    pub vad_silence_ms: u32,
    /// Minimum speech duration in ms to trigger transcription.
    #[serde(default = "default_vad_min_speech_ms")]
    pub vad_min_speech_ms: u32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            whisper_model_path: String::new(),
            language: SttLanguage::Auto,
            max_recording_secs: default_max_recording_secs(),
            model_size: WhisperModelSize::default(),
            stt_provider: SttProviderKind::default(),
            cloud_api_key: String::new(),
            cloud_stt_endpoint: default_cloud_stt_endpoint(),
            cloud_timeout_secs: default_cloud_timeout_secs(),
            mic_input_mode: MicInputMode::default(),
            vad_threshold: default_vad_threshold(),
            vad_silence_ms: default_vad_silence_ms(),
            vad_min_speech_ms: default_vad_min_speech_ms(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_config_serde_round_trip() {
        let config = AudioConfig {
            enabled: true,
            whisper_model_path: "/tmp/model.bin".into(),
            language: SttLanguage::Ko,
            max_recording_secs: 30,
            model_size: WhisperModelSize::Small,
            ..Default::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let restored: AudioConfig = serde_json::from_str(&json).unwrap();
        assert!(restored.enabled);
        assert_eq!(restored.whisper_model_path, "/tmp/model.bin");
        assert_eq!(restored.max_recording_secs, 30);
    }

    #[test]
    fn stt_language_serde_variants() {
        assert_eq!(
            serde_json::to_string(&SttLanguage::Auto).unwrap(),
            "\"auto\""
        );
        assert_eq!(serde_json::to_string(&SttLanguage::En).unwrap(), "\"en\"");
        assert_eq!(serde_json::to_string(&SttLanguage::Ko).unwrap(), "\"ko\"");

        let restored: SttLanguage = serde_json::from_str("\"ko\"").unwrap();
        assert_eq!(restored, SttLanguage::Ko);
    }

    #[test]
    fn audio_config_missing_model_size_uses_default() {
        let json = r#"{"enabled": true, "language": "ko"}"#;
        let config: AudioConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.model_size, WhisperModelSize::Base);
        assert!(config.enabled);
    }

    #[test]
    fn audio_config_missing_cloud_fields_uses_defaults() {
        let json = r#"{"enabled": true}"#;
        let config: AudioConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.stt_provider, SttProviderKind::Local);
        assert!(config.cloud_api_key.is_empty());
        assert!(config.cloud_stt_endpoint.contains("openai.com"));
        assert_eq!(config.cloud_timeout_secs, 10);
    }

    #[test]
    fn stt_provider_kind_serde() {
        assert_eq!(
            serde_json::to_string(&SttProviderKind::Cloud).unwrap(),
            "\"cloud\""
        );
        assert_eq!(
            serde_json::to_string(&SttProviderKind::Local).unwrap(),
            "\"local\""
        );
    }

    #[test]
    fn mic_input_mode_serde_round_trip() {
        assert_eq!(
            serde_json::to_string(&MicInputMode::PushToTalk).unwrap(),
            "\"push_to_talk\""
        );
        assert_eq!(
            serde_json::to_string(&MicInputMode::VoiceActivity).unwrap(),
            "\"voice_activity\""
        );
        let restored: MicInputMode = serde_json::from_str("\"voice_activity\"").unwrap();
        assert_eq!(restored, MicInputMode::VoiceActivity);
    }

    #[test]
    fn mic_input_mode_default_is_ptt() {
        assert_eq!(MicInputMode::default(), MicInputMode::PushToTalk);
    }

    #[test]
    fn audio_config_missing_vad_fields_uses_defaults() {
        let json = r#"{"enabled": true, "language": "ko"}"#;
        let config: AudioConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.mic_input_mode, MicInputMode::PushToTalk);
        assert!((config.vad_threshold - 0.02).abs() < f32::EPSILON);
        assert_eq!(config.vad_silence_ms, 800);
        assert_eq!(config.vad_min_speech_ms, 300);
    }
}
