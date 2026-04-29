//! GUI element detection via OCR-input correlation.
//!
//! Phase 1 foundation with Phase 2 upgrades: word grouping pre-pass,
//! proximity fallback, PII filtering, and resolution-aware thresholds.
//!
//! Integration into the monitor loop pipeline via `gui_pipeline.rs`
//! (see docs/superpowers/specs/2026-03-19-gui-activity-intelligence-design.md).

mod correlation;
mod inference;

use std::sync::Arc;

use oneshim_core::config::PiiFilterLevel;
use oneshim_core::models::frame::{BoundingBox, OcrRegion};
use oneshim_core::models::gui_interaction::GuiElement;
use oneshim_core::ports::gui_element_classifier::GuiElementClassifier;

use crate::privacy::sanitize_title_with_level;

const TAB_LABEL_MAX_LEN: usize = 30;

/// Default proximity threshold in pixels for fallback matching.
const DEFAULT_PROXIMITY_THRESHOLD_PX: u32 = 40;

/// When OCR region count exceeds this threshold, use R-tree spatial index.
const SPATIAL_INDEX_THRESHOLD: usize = 400;

/// R-tree wrapper for an OCR region reference.
const WORD_GROUP_Y_TOLERANCE: u32 = 5;

/// Horizontal gap multiplier for word grouping: merge when gap < factor * avg_char_width.
const WORD_GROUP_GAP_FACTOR: f32 = 1.5;

/// Detects GUI elements from OCR regions and input coordinates.
///
/// Upgraded from Phase 1 `InputOcrCorrelator` with:
/// - Word grouping pre-pass to reduce OCR fragmentation
/// - Proximity fallback when no direct hit is found
/// - PII filtering on element text
/// - Resolution-aware position thresholds
pub struct GuiElementDetector {
    screen_resolution: (u32, u32),
    pii_filter_level: PiiFilterLevel,
    proximity_threshold_px: u32,
    ml_classifier: Option<Arc<dyn GuiElementClassifier>>,
}

/// Backward-compatible type alias for Phase 1 callers.
pub type InputOcrCorrelator = GuiElementDetector;

impl GuiElementDetector {
    /// Create a new detector with screen resolution and PII filter level.
    pub fn new(screen_resolution: (u32, u32), pii_filter_level: PiiFilterLevel) -> Self {
        Self {
            screen_resolution,
            pii_filter_level,
            proximity_threshold_px: DEFAULT_PROXIMITY_THRESHOLD_PX,
            ml_classifier: None,
        }
    }

    /// Override the default proximity threshold.
    pub fn with_proximity_threshold(mut self, px: u32) -> Self {
        self.proximity_threshold_px = px;
        self
    }

    /// Attach an ML classifier for element type refinement.
    pub fn with_ml_classifier(mut self, classifier: Arc<dyn GuiElementClassifier>) -> Self {
        self.ml_classifier = Some(classifier);
        self
    }

    /// Returns a reference to the attached ML classifier (if any).
    pub fn ml_classifier(&self) -> Option<&Arc<dyn GuiElementClassifier>> {
        self.ml_classifier.as_ref()
    }

    /// Update the screen resolution used for proportional thresholds.
    ///
    /// Called when `WindowLayoutTracker` reports a new resolution so that
    /// element type inference remains accurate across monitor changes.
    /// Ignores zero-dimension values to protect against invalid inputs.
    pub fn update_resolution(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.screen_resolution = (width, height);
        }
    }

    /// Return the current screen resolution.
    pub fn resolution(&self) -> (u32, u32) {
        self.screen_resolution
    }

    /// Build a `GuiElement` from an OCR region using heuristic inference.
    ///
    /// For ML-enhanced classification, use `build_gui_element_with_frame()` which
    /// consults the ML classifier when available and falls back to heuristics.
    pub(super) fn build_gui_element(&self, region: &OcrRegion) -> GuiElement {
        let filtered_text = sanitize_title_with_level(&region.text, self.pii_filter_level);
        let (element_type, type_confidence) =
            self.infer_element_type_scored(&region.text, &region.bbox);
        GuiElement {
            text: filtered_text,
            bbox: region.bbox.clone(),
            element_type,
            confidence: region.confidence,
            type_confidence,
        }
    }

    /// Async variant of `build_gui_element` that consults the ML classifier
    /// when available and the frame image is provided.
    ///
    /// If the ML classifier is attached, ready, and returns a confident result
    /// (confidence > 0.7), its prediction overrides the heuristic inference.
    /// Otherwise falls back to the standard heuristic scoring.
    pub async fn build_gui_element_with_frame(
        &self,
        region: &OcrRegion,
        frame_rgba: Option<&[u8]>,
        frame_width: u32,
        frame_height: u32,
    ) -> GuiElement {
        let filtered_text = sanitize_title_with_level(&region.text, self.pii_filter_level);

        // Try ML classifier if available and frame data is provided
        if let (Some(classifier), Some(frame)) = (&self.ml_classifier, frame_rgba) {
            if classifier.is_ready() {
                if let Some(crop) =
                    Self::crop_region_rgba(frame, frame_width, frame_height, &region.bbox)
                {
                    match classifier
                        .classify_crop(&crop, region.bbox.width, region.bbox.height)
                        .await
                    {
                        Ok(Some((ml_type, ml_conf))) if ml_conf > 0.7 => {
                            return GuiElement {
                                text: filtered_text,
                                bbox: region.bbox.clone(),
                                element_type: ml_type,
                                confidence: region.confidence,
                                type_confidence: ml_conf,
                            };
                        }
                        Ok(_) => {
                            // ML inconclusive or low confidence — fall through to heuristic
                        }
                        Err(e) => {
                            tracing::debug!("ML classifier error, falling back to heuristic: {e}");
                        }
                    }
                }
            }
        }

        // Fallback: heuristic inference
        let (element_type, type_confidence) =
            self.infer_element_type_scored(&region.text, &region.bbox);
        GuiElement {
            text: filtered_text,
            bbox: region.bbox.clone(),
            element_type,
            confidence: region.confidence,
            type_confidence,
        }
    }

    /// Crop a bounding box region from an RGBA frame buffer.
    /// Returns None if the bbox extends outside the frame bounds.
    pub fn crop_region_rgba(
        frame_rgba: &[u8],
        frame_width: u32,
        frame_height: u32,
        bbox: &BoundingBox,
    ) -> Option<Vec<u8>> {
        // Validate total buffer size
        let expected_len = (frame_width as usize)
            .checked_mul(frame_height as usize)?
            .checked_mul(4)?;
        if frame_rgba.len() < expected_len {
            return None;
        }

        if bbox.x + bbox.width > frame_width || bbox.y + bbox.height > frame_height {
            return None;
        }

        let stride = (frame_width as usize) * 4;
        let crop_stride = (bbox.width as usize) * 4;
        let mut crop = Vec::with_capacity((bbox.width as usize) * (bbox.height as usize) * 4);

        for row in 0..bbox.height as usize {
            let y_offset = (bbox.y as usize).checked_add(row)?;
            let src_offset = y_offset
                .checked_mul(stride)?
                .checked_add((bbox.x as usize) * 4)?;
            let src_end = src_offset.checked_add(crop_stride)?;
            if src_end <= frame_rgba.len() {
                crop.extend_from_slice(&frame_rgba[src_offset..src_end]);
            } else {
                return None;
            }
        }
        Some(crop)
    }
}

// Note: Per-word OCR confidence is not available — `leptess` does not expose
// `TessBaseAPIGetIterator` + `RIL_WORD`. `OcrRegion.confidence` uses page-level
// `mean_text_conf()`. Upstream contribution or raw Tesseract FFI would be needed.

#[cfg(test)]
mod tests;
