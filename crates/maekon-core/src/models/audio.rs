//! Audio capture and speech-to-text data types.

use serde::{Deserialize, Serialize};

/// Raw 16kHz mono f32 PCM audio buffer.
#[derive(Debug, Clone)]
pub struct AudioBuffer {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub duration_secs: f32,
}

impl AudioBuffer {
    /// Create a new buffer from 16kHz mono samples.
    pub fn new(samples: Vec<f32>) -> Self {
        let duration_secs = samples.len() as f32 / 16000.0;
        Self {
            samples,
            sample_rate: 16000,
            duration_secs,
        }
    }

    /// Whether the buffer contains no audio data.
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Encode PCM samples as a WAV byte buffer (16-bit, 16kHz, mono).
    /// Used for uploading to cloud STT APIs that accept WAV format.
    pub fn to_wav_bytes(&self) -> Vec<u8> {
        let num_samples = self.samples.len();
        let data_size = (num_samples * 2) as u32; // 16-bit = 2 bytes per sample
        let file_size = 36 + data_size;

        let mut buf = Vec::with_capacity(44 + data_size as usize);

        // RIFF header
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&file_size.to_le_bytes());
        buf.extend_from_slice(b"WAVE");

        // fmt sub-chunk
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes()); // sub-chunk size
        buf.extend_from_slice(&1u16.to_le_bytes()); // PCM format
        buf.extend_from_slice(&1u16.to_le_bytes()); // mono
        buf.extend_from_slice(&16000u32.to_le_bytes()); // sample rate
        buf.extend_from_slice(&32000u32.to_le_bytes()); // byte rate (16000 * 1 * 2)
        buf.extend_from_slice(&2u16.to_le_bytes()); // block align (1 * 2)
        buf.extend_from_slice(&16u16.to_le_bytes()); // bits per sample

        // data sub-chunk
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&data_size.to_le_bytes());
        for &sample in &self.samples {
            let clamped = (sample.clamp(-1.0, 1.0) * 32767.0) as i16;
            buf.extend_from_slice(&clamped.to_le_bytes());
        }

        buf
    }
}

/// Configuration for Voice Activity Detection (passed from config to AudioCapture).
#[derive(Debug, Clone)]
pub struct VadConfig {
    pub threshold: f32,
    pub silence_ms: u32,
    pub min_speech_ms: u32,
}

/// STT transcription result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionResult {
    pub text: String,
    pub language: Option<String>,
    pub duration_secs: f32,
    pub processing_secs: f32,
}

use crate::config::WhisperModelSize;

/// Download progress event sent via channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadProgress {
    pub progress_pct: Option<u8>,
    pub bytes_downloaded: u64,
    pub total_bytes: Option<u64>,
}

/// Model download/install status.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum ModelDownloadStatus {
    NotInstalled,
    Downloading {
        progress_pct: Option<u8>,
        bytes_downloaded: u64,
        total_bytes: Option<u64>,
    },
    Ready {
        path: String,
        size_bytes: u64,
    },
    Error {
        message: String,
    },
}

/// Combined audio subsystem status for the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioStatus {
    pub enabled: bool,
    pub selected_model: WhisperModelSize,
    pub model_status: ModelDownloadStatus,
    pub stt_provider_loaded: bool,
    #[serde(default)]
    pub stt_provider: String,
    /// Current mic input mode: "push_to_talk" or "voice_activity".
    #[serde(default)]
    pub mic_input_mode: String,
    /// VAD state: "idle", "listening", "speech", "transcribing".
    #[serde(default)]
    pub vad_state: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_calculates_duration() {
        let buf = AudioBuffer::new(vec![0.0; 16000]);
        assert_eq!(buf.duration_secs, 1.0);
        assert_eq!(buf.sample_rate, 16000);
        assert!(!buf.is_empty());
    }

    #[test]
    fn new_half_second() {
        let buf = AudioBuffer::new(vec![0.0; 8000]);
        assert!((buf.duration_secs - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn empty_buffer() {
        let buf = AudioBuffer::new(vec![]);
        assert!(buf.is_empty());
        assert_eq!(buf.duration_secs, 0.0);
    }

    #[test]
    fn whisper_model_size_serde_round_trip() {
        let size = crate::config::WhisperModelSize::Small;
        let json = serde_json::to_string(&size).unwrap();
        assert_eq!(json, "\"small\"");
        let restored: crate::config::WhisperModelSize = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, size);
    }

    #[test]
    fn whisper_model_size_default_is_base() {
        assert_eq!(
            crate::config::WhisperModelSize::default(),
            crate::config::WhisperModelSize::Base
        );
    }

    #[test]
    fn model_download_status_serde_tagged() {
        let ready = ModelDownloadStatus::Ready {
            path: "/tmp/model.bin".into(),
            size_bytes: 142_000_000,
        };
        let json = serde_json::to_string(&ready).unwrap();
        assert!(json.contains("\"state\":\"ready\""));
        let restored: ModelDownloadStatus = serde_json::from_str(&json).unwrap();
        assert!(matches!(restored, ModelDownloadStatus::Ready { .. }));
    }

    #[test]
    fn model_download_status_not_installed_serde() {
        let status = ModelDownloadStatus::NotInstalled;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "{\"state\":\"not_installed\"}");
    }

    #[test]
    fn download_progress_with_none_pct() {
        let p = DownloadProgress {
            progress_pct: None,
            bytes_downloaded: 1024,
            total_bytes: None,
        };
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"progress_pct\":null"));
    }

    #[test]
    fn audio_status_serde() {
        let status = AudioStatus {
            enabled: true,
            selected_model: crate::config::WhisperModelSize::Base,
            model_status: ModelDownloadStatus::NotInstalled,
            stt_provider_loaded: false,
            stt_provider: String::new(),
            mic_input_mode: "push_to_talk".into(),
            vad_state: "idle".into(),
        };
        let json = serde_json::to_string(&status).unwrap();
        let restored: AudioStatus = serde_json::from_str(&json).unwrap();
        assert!(restored.enabled);
        assert!(!restored.stt_provider_loaded);
        assert_eq!(restored.mic_input_mode, "push_to_talk");
        assert_eq!(restored.vad_state, "idle");
    }

    #[test]
    fn audio_status_missing_vad_fields_uses_defaults() {
        let json = r#"{"enabled":true,"selected_model":"base","model_status":{"state":"not_installed"},"stt_provider_loaded":false,"stt_provider":""}"#;
        let status: AudioStatus = serde_json::from_str(json).unwrap();
        assert_eq!(status.mic_input_mode, "");
        assert_eq!(status.vad_state, "");
    }

    #[test]
    fn vad_config_creation() {
        let cfg = VadConfig {
            threshold: 0.02,
            silence_ms: 800,
            min_speech_ms: 300,
        };
        assert!((cfg.threshold - 0.02).abs() < f32::EPSILON);
        assert_eq!(cfg.silence_ms, 800);
        assert_eq!(cfg.min_speech_ms, 300);
    }

    #[test]
    fn to_wav_bytes_valid_header() {
        let buf = AudioBuffer::new(vec![0.0; 16000]); // 1 second
        let wav = buf.to_wav_bytes();
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");
        // Total: 44 header + 32000 data = 32044
        assert_eq!(wav.len(), 44 + 32000);
    }

    #[test]
    fn to_wav_bytes_pcm16_conversion() {
        let buf = AudioBuffer::new(vec![1.0, -1.0, 0.0]);
        let wav = buf.to_wav_bytes();
        // PCM16 data starts at offset 44
        let s1 = i16::from_le_bytes([wav[44], wav[45]]);
        let s2 = i16::from_le_bytes([wav[46], wav[47]]);
        let s3 = i16::from_le_bytes([wav[48], wav[49]]);
        assert_eq!(s1, 32767); // 1.0 * 32767
        assert_eq!(s2, -32767); // -1.0 * 32767
        assert_eq!(s3, 0);
    }

    #[test]
    fn to_wav_bytes_clamps_out_of_range() {
        let buf = AudioBuffer::new(vec![2.0, -2.0]); // beyond [-1, 1]
        let wav = buf.to_wav_bytes();
        let s1 = i16::from_le_bytes([wav[44], wav[45]]);
        let s2 = i16::from_le_bytes([wav[46], wav[47]]);
        assert_eq!(s1, 32767); // clamped to 1.0
        assert_eq!(s2, -32767); // clamped to -1.0
    }

    #[test]
    fn to_wav_bytes_empty() {
        let buf = AudioBuffer::new(vec![]);
        let wav = buf.to_wav_bytes();
        assert_eq!(wav.len(), 44); // header only
    }
}
