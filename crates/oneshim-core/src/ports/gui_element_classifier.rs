use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::gui_interaction::GuiElementType;

/// ML-based GUI element classifier.
///
/// Classifies a cropped image region into a `GuiElementType` with a confidence
/// score. When no trained model is available, `is_ready()` returns false and
/// callers should fall back to heuristic inference.
///
/// # Errors
/// - `CoreError::Internal` (wire: `internal.generic`) — model inference
///   failure, tensor-shape mismatch, runtime panic from the ML backend
///   (onnx/tract/candle).
/// - `CoreError::Io` (wire: `internal.io`) — image decode failure for
///   adapters that accept encoded bytes instead of pre-decoded RGBA.
/// - Low-confidence classification is `Ok(None)`, not Err — callers
///   check `is_ready()` to decide whether to fall back to heuristics
///   before invoking `classify_crop`.
#[async_trait]
pub trait GuiElementClassifier: Send + Sync {
    /// Classify a GUI element from an image crop (RGBA, row-major).
    /// Returns `(element_type, confidence)` or `None` if classification is
    /// inconclusive (e.g., confidence below threshold).
    async fn classify_crop(
        &self,
        crop_rgba: &[u8],
        width: u32,
        height: u32,
    ) -> Result<Option<(GuiElementType, f32)>, CoreError>;

    /// Returns true if a trained model is loaded and ready for inference.
    fn is_ready(&self) -> bool;
}
