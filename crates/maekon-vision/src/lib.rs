// Cast safety: pixel coordinates, image dimensions, confidence scores — precision loss acceptable.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
// P2 PR-C: `missing_const_for_fn` accepted crate-wide.
// Rationale: const-viral cascade + nursery false-positive rate outweigh the value.
#![allow(clippy::missing_const_for_fn)]
// P2 remaining-nursery-lints: stylistic/cosmetic nursery lints accepted crate-wide.
#![allow(
    clippy::use_self,
    clippy::option_if_let_else,
    clippy::redundant_pub_crate
)]
// P2 PR-A nursery-hardening.
// (Enforced workspace-wide via `[workspace.lints.clippy]`, #7719.)
#![cfg_attr(test, allow(clippy::significant_drop_tightening))]
// P2 nursery-hardening (PR-B): derive Eq alongside PartialEq when possible.
// (Enforced workspace-wide via `[workspace.lints.clippy]`, #7719.)

//! # maekon-vision

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
pub mod pointer_context;
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

pub mod ocr_geometry;

#[cfg(feature = "ml-detect")]
pub mod ml_classifier;

/// Whether this binary can run local OCR through ANY compiled engine:
/// platform-native (macOS Vision.framework / Windows WinRT Media.Ocr — behind
/// the `native-vision` cargo feature, see [`native_ocr::native_ocr_platform_supported`])
/// OR the leptess/Tesseract fallback (behind the `ocr` cargo feature, see
/// `local_ocr_provider`).
///
/// `false` when neither is compiled in — the case for every shipped release
/// build on Linux today: native OCR is macOS/Windows only, and `src-tauri`
/// never enables the `ocr` feature (leptess requires a system Tesseract
/// install and is opt-in only). See `local_ocr_provider.rs` and the #7602
/// capture-pipeline warn.
///
/// `FeatureCapabilitySnapshot.ocr_available` (src-tauri) is the single
/// consumer of this predicate — capability descriptors should call this
/// instead of re-deriving the OR by hand.
#[must_use]
pub fn local_ocr_engine_available() -> bool {
    #[cfg(feature = "native-vision")]
    let native = native_ocr::native_ocr_platform_supported();
    #[cfg(not(feature = "native-vision"))]
    let native = false;

    native || cfg!(feature = "ocr")
}

#[cfg(test)]
mod capability_tests {
    use super::local_ocr_engine_available;

    /// Cross-checks against the documented platform allow-list + `ocr` feature
    /// so a future edit to either half of the OR drifts loudly instead of
    /// silently (mirrors `native_ocr::capability_tests`, #7602 audit ①).
    #[test]
    fn matches_documented_native_platform_allow_list_or_leptess_feature() {
        let expected_native = cfg!(all(
            feature = "native-vision",
            any(target_os = "macos", target_os = "windows")
        ));
        assert_eq!(
            local_ocr_engine_available(),
            expected_native || cfg!(feature = "ocr")
        );
    }

    /// On a default-feature Linux build (native-vision on, `ocr` off — the
    /// shipped release configuration, #7678), local OCR must be honestly
    /// reported as unavailable, not silently `true`.
    #[cfg(all(target_os = "linux", not(feature = "ocr")))]
    #[test]
    fn linux_without_leptess_reports_no_local_ocr_engine() {
        assert!(!local_ocr_engine_available());
    }
}
