//! Audio capture and speech-to-text adapter crate.
//!
//! - `AudioCapture`: microphone input via cpal + rubato resampling to 16kHz
//! - `VadDetector`: energy-based voice activity detection
//! - `WhisperSttProvider`: local Whisper STT (feature-gated behind `whisper`)

// P2 PR-C: `missing_const_for_fn` accepted crate-wide. See
// docs/reviews/2026-04-21-p2-missing-const-for-fn-decision.md.
#![allow(clippy::missing_const_for_fn)]
// P2 remaining-nursery-lints: see decision doc.
#![allow(
    clippy::use_self,
    clippy::option_if_let_else,
    clippy::redundant_pub_crate
)]
// P2 PR-A nursery-hardening: mutex guards must not be held across I/O or
// long-running work unless intentionally kept (use function-level #[allow]
// with reason). See docs/reviews/2026-04-21-p2-significant-drop-tightening-spec.md.
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
