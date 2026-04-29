//! ONNX-based GUI element classifier.
//!
//! Loads a trained `.onnx` model and classifies image crops into
//! `GuiElementType` variants with confidence scores.
//!
//! When no model file is present, `is_ready()` returns `false` and callers
//! fall back to heuristic inference (Phase 1 scored rules).

mod preprocess;

use std::path::Path;
use std::sync::Mutex;

use async_trait::async_trait;
use oneshim_core::error::CoreError;
use oneshim_core::models::gui_interaction::GuiElementType;
use oneshim_core::ports::gui_element_classifier::GuiElementClassifier;
use tracing::{debug, info, warn};

/// Minimum confidence threshold: predictions below this are returned as None.
const MIN_CONFIDENCE: f32 = 0.3;

/// Label ordering must match the model's output layer.
const LABELS: [GuiElementType; 12] = [
    GuiElementType::Button,
    GuiElementType::TextInput,
    GuiElementType::Link,
    GuiElementType::MenuItem,
    GuiElementType::TabLabel,
    GuiElementType::StatusBar,
    GuiElementType::TitleBar,
    GuiElementType::ToolbarIcon,
    GuiElementType::TreeItem,
    GuiElementType::ScrollBar,
    GuiElementType::TextRegion,
    GuiElementType::Unknown,
];

/// ONNX Runtime-backed GUI element classifier.
///
/// Wraps `ort::Session` in a `Mutex` because `Session::run()` requires
/// `&mut self`. The `Mutex` is acceptable since classification is infrequent
/// (once per click, not per frame).
pub struct OnnxGuiClassifier {
    session: Mutex<ort::session::Session>,
    model_path: std::path::PathBuf,
    loaded_mtime: Mutex<Option<std::time::SystemTime>>,
}

// Safety: ort::Session is Send but not Sync by default.
// We wrap in Mutex to guarantee exclusive access during run().
unsafe impl Sync for OnnxGuiClassifier {}

impl OnnxGuiClassifier {
    /// Load an ONNX model from the given path.
    ///
    /// Returns `Ok(None)` if the model file does not exist (graceful skip).
    /// Returns `Err` only on actual loading failures (corrupt model, etc.).
    pub fn load(model_path: &Path) -> Result<Option<Self>, CoreError> {
        if !model_path.exists() {
            info!(
                ?model_path,
                "GUI classifier model not found — ML classification disabled"
            );
            return Ok(None);
        }

        let session = ort::session::Session::builder()
            .map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("ort session builder: {e}"),
            })?
            .commit_from_file(model_path)
            .map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("ort model load: {e}"),
            })?;

        info!(?model_path, "GUI classifier model loaded");
        debug!(
            inputs = ?session.inputs().iter().map(|i| i.name()).collect::<Vec<_>>(),
            outputs = ?session.outputs().iter().map(|o| o.name()).collect::<Vec<_>>(),
            "model I/O"
        );

        let mtime = std::fs::metadata(model_path)
            .ok()
            .and_then(|m| m.modified().ok());

        Ok(Some(Self {
            session: Mutex::new(session),
            model_path: model_path.to_path_buf(),
            loaded_mtime: Mutex::new(mtime),
        }))
    }

    /// Run inference on a preprocessed input tensor.
    fn run_inference(&self, input: Vec<f32>) -> Result<Vec<f32>, CoreError> {
        use ort::value::TensorRef;

        let input_tensor = TensorRef::from_array_view(([1usize, 3, 64, 64], input.as_slice()))
            .map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("ort tensor: {e}"),
            })?;

        let mut session = self.session.lock().map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("session lock poisoned: {e}"),
        })?;

        let outputs = session
            .run([input_tensor.into()])
            .map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("ort inference: {e}"),
            })?;

        let output = &outputs[0usize];
        let (_shape, data) =
            output
                .try_extract_tensor::<f32>()
                .map_err(|e| CoreError::Internal {
                    code: oneshim_core::error_codes::InternalCode::Generic,
                    message: format!("ort output extract: {e}"),
                })?;

        Ok(data.to_vec())
    }

    /// Check if the model file has been modified since last load.
    /// If so, reload the ONNX session with the new model.
    /// Returns `true` if a reload occurred.
    pub fn reload_if_changed(&self) -> bool {
        let current_mtime = std::fs::metadata(&self.model_path)
            .ok()
            .and_then(|m| m.modified().ok());

        let mut cached = self.loaded_mtime.lock().unwrap_or_else(|e| {
            warn!("ml classifier mtime lock poisoned — recovering inner data");
            e.into_inner()
        });

        if current_mtime == *cached {
            return false;
        }

        match ort::session::Session::builder().and_then(|b| b.commit_from_file(&self.model_path)) {
            Ok(new_session) => {
                let mut session = self.session.lock().unwrap_or_else(|e| {
                    warn!("ml classifier session lock poisoned — recovering inner data");
                    e.into_inner()
                });
                *session = new_session;
                *cached = current_mtime;
                info!(
                    path = ?self.model_path,
                    "GUI classifier model hot-reloaded"
                );
                true
            }
            Err(e) => {
                warn!(
                    path = ?self.model_path,
                    error = %e,
                    "GUI classifier model reload failed — keeping previous model"
                );
                false
            }
        }
    }
}

#[async_trait]
impl GuiElementClassifier for OnnxGuiClassifier {
    async fn classify_crop(
        &self,
        crop_rgba: &[u8],
        width: u32,
        height: u32,
    ) -> Result<Option<(GuiElementType, f32)>, CoreError> {
        let input = preprocess::prepare_input(crop_rgba, width, height)?;

        let probabilities = tokio::task::block_in_place(|| self.run_inference(input))?;

        if probabilities.len() != LABELS.len() {
            return Err(CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!(
                    "model output size mismatch: expected {}, got {}",
                    LABELS.len(),
                    probabilities.len()
                ),
            });
        }

        // Find argmax
        let (max_idx, max_prob) = probabilities
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap_or((LABELS.len() - 1, &0.0));

        if *max_prob < MIN_CONFIDENCE {
            debug!(
                max_prob,
                max_class = ?LABELS[max_idx],
                "ML classification below threshold"
            );
            return Ok(None);
        }

        Ok(Some((LABELS[max_idx].clone(), *max_prob)))
    }

    fn is_ready(&self) -> bool {
        true // If constructed, the model is loaded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn load_missing_model_returns_none() {
        let result = OnnxGuiClassifier::load(&PathBuf::from("/nonexistent/model.onnx"));
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn label_count_matches_gui_element_type_variants() {
        // 12 variants in GuiElementType
        assert_eq!(LABELS.len(), 12);
    }

    #[test]
    fn preprocess_produces_correct_size() {
        // 64x64 RGB = 12288 floats
        let rgba = vec![128u8; 4 * 10 * 10]; // 10x10 RGBA
        let input = preprocess::prepare_input(&rgba, 10, 10).unwrap();
        assert_eq!(input.len(), 3 * 64 * 64);
    }
}
