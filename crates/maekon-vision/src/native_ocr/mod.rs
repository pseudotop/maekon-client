//! Platform-native OCR provider using OS-level text recognition APIs.
//!
//! On macOS, this uses Vision.framework's `VNRecognizeTextRequest` via raw
//! objc2 FFI. On Windows, this uses WinRT `Windows.Media.Ocr.OcrEngine`.
//! On other platforms, `create_native_ocr()` returns `None`.

use maekon_core::ports::ocr_provider::OcrProvider;
use std::sync::Arc;

pub mod benchmark;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "windows")]
mod windows;

/// Create platform-native OCR provider.
///
/// Returns `Some(Arc<dyn OcrProvider>)` on macOS (Vision.framework) and
/// Windows (WinRT Media.Ocr), `None` on all other platforms.
pub fn create_native_ocr() -> Option<Arc<dyn OcrProvider>> {
    #[cfg(target_os = "macos")]
    {
        Some(Arc::new(macos::MacOsNativeOcr))
    }

    #[cfg(target_os = "windows")]
    {
        Some(Arc::new(windows::WindowsNativeOcr))
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        None
    }
}

#[cfg(test)]
mod benchmark_tests {
    use maekon_core::config::PiiFilterLevel;
    use maekon_core::error::CoreError;
    use maekon_core::ports::ocr_provider::OcrResult;

    use super::benchmark::{
        classify_windows_native_ocr_failure, summarize_windows_native_ocr_sample,
        WindowsNativeOcrFailureClass, WindowsNativeOcrSampleInput,
    };

    #[test]
    fn benchmark_summary_masks_text_and_normalizes_scaled_bounds() {
        let results = vec![OcrResult {
            text: "Settings user@example.com".to_string(),
            x: 100,
            y: 60,
            width: 300,
            height: 40,
            confidence: 1.0,
        }];

        let summary = summarize_windows_native_ocr_sample(
            WindowsNativeOcrSampleInput {
                sample_label: "maekon-settings-crop",
                image_width: 1200,
                image_height: 400,
                image_format: "png",
                display_scale: 2.0,
                latency_ms: 42,
            },
            Ok(&results),
            PiiFilterLevel::Strict,
        );

        assert!(summary.ok);
        assert_eq!(summary.extraction_count, 1);
        assert_eq!(summary.image_width, 1200);
        assert_eq!(summary.image_height, 400);
        assert_eq!(summary.display_scale, 2.0);
        assert_eq!(summary.logical_bounds_samples[0].x, 50);
        assert_eq!(summary.logical_bounds_samples[0].width, 150);
        assert!(summary.sanitized_text_samples[0].contains("[EMAIL]"));

        let serialized = serde_json::to_string(&summary).unwrap();
        assert!(!serialized.contains("user@example.com"));
        assert!(!serialized.contains("Settings user"));
    }

    #[test]
    fn benchmark_failure_class_distinguishes_windows_capability() {
        let error = CoreError::OcrError {
            code: maekon_core::error_codes::ProviderCode::OcrFailed,
            message: "OcrEngine creation failed: language pack unavailable".to_string(),
        };

        assert_eq!(
            classify_windows_native_ocr_failure(&error),
            WindowsNativeOcrFailureClass::WindowsOcrCapabilityMissing
        );
    }

    #[test]
    fn benchmark_failure_summary_masks_user_paths() {
        let error = CoreError::OcrError {
            code: maekon_core::error_codes::ProviderCode::OcrFailed,
            message: r"image read failed at C:\Users\Alice\AppData\Local\Temp\crop.png".to_string(),
        };

        let summary = summarize_windows_native_ocr_sample(
            WindowsNativeOcrSampleInput {
                sample_label: "external-reference-app-crop",
                image_width: 1200,
                image_height: 360,
                image_format: "png",
                display_scale: 1.5,
                latency_ms: 0,
            },
            Err(&error),
            PiiFilterLevel::Strict,
        );
        let serialized = serde_json::to_string(&summary).unwrap();

        assert!(serialized.contains(r"C:\\Users\\[USER]"));
        assert!(!serialized.contains("Alice"));
    }
}
