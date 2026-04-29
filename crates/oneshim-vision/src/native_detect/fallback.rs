use oneshim_core::error::CoreError;
use oneshim_core::ports::rectangle_detector::{DetectedRectangle, RectangleDetector};

#[allow(dead_code)] // used on non-macOS platforms only
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
