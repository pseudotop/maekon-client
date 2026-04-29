// Cast safety: pixel coordinates, image dimensions, confidence scores — precision loss acceptable.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
// P2 PR-C: `missing_const_for_fn` accepted crate-wide. See
// docs/reviews/2026-04-21-p2-missing-const-for-fn-decision.md.
#![allow(clippy::missing_const_for_fn)]
// P2 remaining-nursery-lints: see decision doc.
#![allow(
    clippy::use_self,
    clippy::option_if_let_else,
    clippy::redundant_pub_crate
)]
// P2 PR-A nursery-hardening.
#![deny(clippy::significant_drop_tightening)]
#![cfg_attr(test, allow(clippy::significant_drop_tightening))]
// P2 nursery-hardening (PR-B): derive Eq alongside PartialEq when possible.
#![deny(clippy::derive_partial_eq_without_eq)]

//! # oneshim-vision

pub mod error;
pub use error::VisionError;

pub mod accessibility;
pub mod capture;
pub mod contour_classifier;
pub mod delta;
pub mod element_finder;
pub mod encoder;
pub mod gui_detector;
/// Backward-compatible re-export for Phase 1 callers.
pub use gui_detector as input_correlator;
pub mod local_ocr_provider;
#[cfg(feature = "ocr")]
pub mod ocr;
pub mod privacy;
pub mod privacy_gateway;
pub mod processor;
pub mod ring_buffer;
pub mod thumbnail;
pub mod timeline;
pub mod trigger;
pub mod work_classifier;

#[cfg(feature = "native-vision")]
pub mod native_ocr;

#[cfg(feature = "native-vision")]
pub mod native_detect;

#[cfg(feature = "ml-detect")]
pub mod ml_classifier;
