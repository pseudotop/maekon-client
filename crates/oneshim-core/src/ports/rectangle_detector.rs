//! Rectangle detection port — defines the contract for detecting rectangular
//! UI element boundaries in screen images. Implemented by platform-specific
//! adapters (macOS Vision.framework, future Core ML models).

use crate::error::CoreError;
use crate::models::intent::ElementBounds;
use crate::models::ui_scene::NormalizedBounds;

/// A rectangle detected by a vision framework or ML model.
#[derive(Debug, Clone)]
pub struct DetectedRectangle {
    pub bounds: ElementBounds,
    pub bounds_normalized: NormalizedBounds,
    pub confidence: f64,
    pub classification: Option<String>,
}

/// Synchronous rectangle detection from image data.
///
/// # Errors
/// - `CoreError::Internal` (wire: `internal.generic`) — Vision framework
///   call failure (macOS), Core ML prediction error, image dimension
///   mismatch, zero-length image buffer.
/// - `CoreError::Io` (wire: `internal.io`) — image decode failure for
///   adapters that accept encoded bytes instead of raw pixels.
/// - `CoreError::ServiceUnavailable` (wire: `service.unavailable`) —
///   unsupported platform (no Vision framework on non-macOS builds) or
///   ML backend not linked at compile time.
/// - Empty result (no rectangles detected above confidence/size
///   thresholds) is `Ok(Vec::new())`, not Err.
pub trait RectangleDetector: Send + Sync {
    fn detect_rectangles(
        &self,
        image: &[u8],
        image_width: u32,
        image_height: u32,
        min_size: f32,
        max_results: usize,
    ) -> Result<Vec<DetectedRectangle>, CoreError>;

    fn provider_name(&self) -> &str;
}
