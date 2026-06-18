//! Detection overlay payload builders — convert `UiScene` into
//! `DetectionScenePayload` with visibility filtering and confidence-cap logic.

use tracing::warn;

use super::types::{DetectionElementPayload, DetectionScenePayload};

const DETECTION_ELEMENT_LIMIT: usize = 200;

/// Convert a `UiScene` into a capped, sorted `DetectionScenePayload`.
///
/// Returns `None` when the scene has no visible elements or invalid dimensions,
/// so the caller can keep the overlay in click-through mode.
pub(super) fn build_detection_payload(
    scene: &maekon_core::models::ui_scene::UiScene,
) -> Option<DetectionScenePayload> {
    if scene.screen_width == 0 || scene.screen_height == 0 {
        return None;
    }

    let mut elements: Vec<DetectionElementPayload> = scene
        .elements
        .iter()
        .filter_map(|el| {
            let confidence = el.confidence;
            if !confidence.is_finite() {
                return None;
            }

            let payload = DetectionElementPayload {
                element_id: el.element_id.clone(),
                x: el.bbox_abs.x,
                y: el.bbox_abs.y,
                width: el.bbox_abs.width,
                height: el.bbox_abs.height,
                // Overlay IPC egresses to the WebView; surface only the masked
                // copy, never the raw `label` (which carries unredacted OCR text).
                label: el.text_masked.clone().unwrap_or_default(),
                role: el.role.clone(),
                confidence: confidence.clamp(0.0, 1.0),
                source: "composite".to_string(),
            };

            if is_visible(&payload, scene.screen_width, scene.screen_height) {
                Some(payload)
            } else {
                None
            }
        })
        .collect();

    if elements.is_empty() {
        return None;
    }

    // Sort highest-confidence first so the cap retains the most valuable
    // detections rather than silently dropping them.
    elements.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let total = elements.len();
    if total > DETECTION_ELEMENT_LIMIT {
        warn!(
            total = total,
            limit = DETECTION_ELEMENT_LIMIT,
            "detection scene truncated — showing top {DETECTION_ELEMENT_LIMIT} of {total} elements by confidence",
        );
        elements.truncate(DETECTION_ELEMENT_LIMIT);
    }

    Some(DetectionScenePayload {
        scene_id: scene.scene_id.clone(),
        app_name: scene.app_name.clone(),
        screen_width: scene.screen_width,
        screen_height: scene.screen_height,
        element_count: elements.len(),
        elements,
    })
}

fn is_visible(el: &DetectionElementPayload, screen_width: u32, screen_height: u32) -> bool {
    if el.width == 0 || el.height == 0 {
        return false;
    }

    let left = el.x as i64;
    let top = el.y as i64;
    let right = left + el.width as i64;
    let bottom = top + el.height as i64;
    let screen_right = screen_width as i64;
    let screen_bottom = screen_height as i64;

    right > 0 && bottom > 0 && left < screen_right && top < screen_bottom
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_detection_scene_payload_is_suppressed() {
        let scene = maekon_core::models::ui_scene::UiScene {
            schema_version: maekon_core::models::ui_scene::UI_SCENE_SCHEMA_VERSION.to_string(),
            scene_id: "scene-empty".to_string(),
            app_name: Some("Finder".to_string()),
            screen_id: Some("main".to_string()),
            captured_at: chrono::Utc::now(),
            screen_width: 1920,
            screen_height: 1080,
            elements: vec![],
        };

        assert!(build_detection_payload(&scene).is_none());
    }

    #[test]
    fn detection_payload_drops_non_visible_elements() {
        use maekon_core::models::intent::ElementBounds;
        use maekon_core::models::ui_scene::{NormalizedBounds, UiScene, UiSceneElement};

        let scene = UiScene {
            schema_version: maekon_core::models::ui_scene::UI_SCENE_SCHEMA_VERSION.to_string(),
            scene_id: "scene-filter".to_string(),
            app_name: Some("Finder".to_string()),
            screen_id: Some("main".to_string()),
            captured_at: chrono::Utc::now(),
            screen_width: 1920,
            screen_height: 1080,
            elements: vec![
                UiSceneElement {
                    element_id: "zero-width".to_string(),
                    bbox_abs: ElementBounds {
                        x: 10,
                        y: 10,
                        width: 0,
                        height: 20,
                    },
                    bbox_norm: NormalizedBounds::new(0.0, 0.0, 0.0, 0.1),
                    label: "hidden".to_string(),
                    role: Some("button".to_string()),
                    intent: None,
                    state: None,
                    confidence: 0.9,
                    text_masked: None,
                    parent_id: None,
                },
                UiSceneElement {
                    element_id: "offscreen".to_string(),
                    bbox_abs: ElementBounds {
                        x: 3000,
                        y: 10,
                        width: 20,
                        height: 20,
                    },
                    bbox_norm: NormalizedBounds::new(1.0, 0.0, 0.1, 0.1),
                    label: "offscreen".to_string(),
                    role: Some("button".to_string()),
                    intent: None,
                    state: None,
                    confidence: 0.9,
                    text_masked: None,
                    parent_id: None,
                },
                UiSceneElement {
                    element_id: "visible".to_string(),
                    bbox_abs: ElementBounds {
                        x: 100,
                        y: 120,
                        width: 80,
                        height: 30,
                    },
                    bbox_norm: NormalizedBounds::new(0.05, 0.1, 0.04, 0.03),
                    label: "Open".to_string(),
                    role: Some("button".to_string()),
                    intent: None,
                    state: None,
                    confidence: 0.8,
                    text_masked: None,
                    parent_id: None,
                },
            ],
        };

        let payload = build_detection_payload(&scene).expect("visible element should keep payload");
        assert_eq!(payload.element_count, 1);
        assert_eq!(payload.elements[0].element_id, "visible");
    }
}
