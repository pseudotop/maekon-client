//! CV-based GUI element classifier using visual feature analysis.
//!
//! Classifies GUI element crops by analyzing border contrast, fill uniformity,
//! and aspect ratio — no ML model, GPU, or training data required.
//! Always ready, works out of the box on all platforms.

pub mod features;
pub mod feedback;
mod signatures;

use async_trait::async_trait;
use oneshim_core::error::CoreError;
use oneshim_core::models::gui_interaction::GuiElementType;
use oneshim_core::ports::gui_element_classifier::GuiElementClassifier;

/// Visual feature-based GUI element classifier.
///
/// Analyzes the visual properties of a cropped GUI element image
/// (border contrast, fill uniformity, aspect ratio) and matches
/// against element type signatures. No model file needed.
pub struct ContourGuiClassifier;

impl ContourGuiClassifier {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ContourGuiClassifier {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl GuiElementClassifier for ContourGuiClassifier {
    async fn classify_crop(
        &self,
        crop_rgba: &[u8],
        width: u32,
        height: u32,
    ) -> Result<Option<(GuiElementType, f32)>, CoreError> {
        let visual = features::extract_visual_features(crop_rgba, width, height);
        Ok(signatures::match_signatures(&visual))
    }

    fn is_ready(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn classify_crop_returns_result() {
        let classifier = ContourGuiClassifier::new();
        // 20x10 gray crop
        let rgba = vec![128u8; 20 * 10 * 4];
        let result = classifier.classify_crop(&rgba, 20, 10).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_some());
    }

    #[test]
    fn is_always_ready() {
        let classifier = ContourGuiClassifier::new();
        assert!(classifier.is_ready());
    }

    #[tokio::test]
    async fn classify_button_like_crop() {
        let classifier = ContourGuiClassifier::new();
        // Create a bordered crop (dark border, light interior)
        let mut rgba = Vec::with_capacity(60 * 30 * 4);
        for y in 0..30u32 {
            for x in 0..60u32 {
                let c = if !(2..58).contains(&x) || !(2..28).contains(&y) {
                    40 // dark border
                } else {
                    180 // light interior
                };
                rgba.extend_from_slice(&[c, c, c, 255]);
            }
        }
        let result = classifier.classify_crop(&rgba, 60, 30).await.unwrap();
        assert!(result.is_some());
        let (etype, conf) = result.unwrap();
        assert_eq!(etype, GuiElementType::Button);
        assert!(conf > 0.5);
    }

    #[tokio::test]
    async fn classify_tiny_crop_no_crash() {
        let classifier = ContourGuiClassifier::new();
        let rgba = vec![128u8; 3 * 3 * 4];
        let result = classifier.classify_crop(&rgba, 3, 3).await;
        assert!(result.is_ok());
    }
}
