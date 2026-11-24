use maekon_core::ports::rectangle_detector::RectangleDetector;
use std::sync::Arc;

mod fallback;
pub use fallback::PixelRegionDetector;
#[cfg(target_os = "macos")]
mod macos;

pub fn create_rectangle_detector() -> Option<Arc<dyn RectangleDetector>> {
    #[cfg(target_os = "macos")]
    {
        Some(Arc::new(macos::MacOsRectangleDetector))
    }

    #[cfg(not(target_os = "macos"))]
    {
        Some(Arc::new(PixelRegionDetector))
    }
}

#[cfg(test)]
mod tests {
    use super::create_rectangle_detector;

    #[test]
    fn factory_connects_the_platform_rectangle_detector() {
        let detector = create_rectangle_detector().expect("native vision must provide a detector");
        let expected = if cfg!(target_os = "macos") {
            "macos-vision-rect"
        } else {
            "pixel-contours"
        };
        assert_eq!(detector.provider_name(), expected);
    }
}
