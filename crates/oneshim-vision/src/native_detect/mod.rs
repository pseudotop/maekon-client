use oneshim_core::ports::rectangle_detector::RectangleDetector;
use std::sync::Arc;

mod fallback;
#[cfg(target_os = "macos")]
mod macos;

pub fn create_rectangle_detector() -> Option<Arc<dyn RectangleDetector>> {
    #[cfg(target_os = "macos")]
    {
        Some(Arc::new(macos::MacOsRectangleDetector))
    }

    #[cfg(not(target_os = "macos"))]
    {
        Some(Arc::new(fallback::OcrBboxFallback))
    }
}
