//! Screen capture and frame processing ports — defines contracts for
//! deciding when to capture (`CaptureTrigger`) and how to process frames
//! (`FrameProcessor`: full, delta, thumbnail, or metadata-only).
//! Implemented by `SmartCaptureTrigger` and `EdgeFrameProcessor` in `oneshim-vision`.

use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::context::WindowBounds;
use crate::models::event::ContextEvent;
use crate::models::frame::ProcessedFrame;

pub trait CaptureTrigger: Send + Sync {
    fn should_capture(&self, event: &ContextEvent) -> Option<CaptureRequest>;
}

#[derive(Debug, Clone)]
pub struct CaptureRequest {
    pub trigger_type: String,
    pub importance: f32,
    pub app_name: String,
    pub window_title: String,
    /// Active window bounds for multi-monitor capture targeting.
    /// When set, the frame processor captures the monitor containing
    /// the window instead of always using the primary monitor.
    pub window_bounds: Option<WindowBounds>,
}

/// Captures and processes screen frames based on importance level.
///
/// # Errors
/// Methods return `CoreError::PermissionDenied` (wire:
/// `permission.permission_denied`) when screen-capture permission is
/// missing — emitted by the accessibility adapter before the
/// FrameProcessor is called. `CoreError::Internal` (wire:
/// `internal.generic`) on intra-process failures such as mutex
/// poisoning or image-buffer allocation errors.
/// `CoreError::OcrError` (wire: `provider.ocr_failed`) on OCR extraction
/// failure.
#[async_trait]
pub trait FrameProcessor: Send + Sync {
    async fn capture_and_process(
        &self,
        capture_request: &CaptureRequest,
    ) -> Result<ProcessedFrame, CoreError>;

    /// Capture a lightweight thumbnail for ring buffer use.
    /// Returns raw WebP bytes at low quality. Default returns unsupported error.
    async fn capture_thumbnail(&self) -> Result<Vec<u8>, CoreError> {
        Err(CoreError::Internal {
            code: crate::error_codes::InternalCode::Generic,
            message: "thumbnail capture not supported".to_string(),
        })
    }
}
