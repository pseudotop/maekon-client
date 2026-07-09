use maekon_core::error::CoreError;
use maekon_core::ports::rectangle_detector::{DetectedRectangle, RectangleDetector};

// Only constructed by `create_rectangle_detector()`'s
// `#[cfg(not(target_os = "macos"))]` arm — dead on macOS builds, where the
// native `macos::MacOsRectangleDetector` is used instead.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub struct OcrBboxFallback;

impl RectangleDetector for OcrBboxFallback {
    fn detect_rectangles(
        &self,
        _image: &[u8],
        _image_width: u32,
        _image_height: u32,
        _min_size: f32,
        _max_results: usize,
    ) -> Result<Vec<DetectedRectangle>, CoreError> {
        Ok(Vec::new())
    }

    fn provider_name(&self) -> &str {
        "ocr-bbox-fallback"
    }
}
