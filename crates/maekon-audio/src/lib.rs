//! Audio capture and speech-to-text adapter crate.
//!
//! - `AudioCapture`: microphone input via cpal + rubato resampling to 16kHz
//! - `VadDetector`: energy-based voice activity detection
//! - `WhisperSttProvider`: local Whisper STT (feature-gated behind `whisper`)

// P2 PR-C: `missing_const_for_fn` accepted crate-wide.
// Rationale: const-viral cascade + nursery false-positive rate outweigh the value.
#![allow(clippy::missing_const_for_fn)]
// P2 remaining-nursery-lints: stylistic/cosmetic nursery lints accepted crate-wide.
#![allow(
    clippy::use_self,
    clippy::option_if_let_else,
    clippy::redundant_pub_crate
)]
// P2 PR-A nursery-hardening: mutex guards must not be held across I/O or
// long-running work unless intentionally kept (use function-level #[allow]
// with reason).
#![deny(clippy::significant_drop_tightening)]
#![cfg_attr(test, allow(clippy::significant_drop_tightening))]

mod capture;
pub mod vad;

#[cfg(feature = "whisper")]
mod whisper;

#[cfg(feature = "download")]
pub mod model_downloader;

#[cfg(feature = "cloud-stt")]
mod cloud_stt;

pub use capture::AudioCapture;
pub use vad::{VadDetector, VadEvent, VadState};

#[cfg(feature = "whisper")]
pub use whisper::WhisperSttProvider;

#[cfg(feature = "download")]
pub use model_downloader::WhisperModelDownloader;

#[cfg(feature = "cloud-stt")]
pub use cloud_stt::CloudSttProvider;
