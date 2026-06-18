use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::models::intent::ElementBounds;

pub const UI_SCENE_SCHEMA_VERSION: &str = "ui_scene.v1";

fn default_ui_scene_schema_version() -> String {
    UI_SCENE_SCHEMA_VERSION.to_string()
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct NormalizedBounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl NormalizedBounds {
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x: x.clamp(0.0, 1.0),
            y: y.clamp(0.0, 1.0),
            width: width.clamp(0.0, 1.0),
            height: height.clamp(0.0, 1.0),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiSceneElement {
    pub element_id: String,
    pub bbox_abs: ElementBounds,
    pub bbox_norm: NormalizedBounds,
    /// Raw, unredacted OCR/accessibility text.
    ///
    /// Kept in-process for click targeting (text matching, ranking, and Strict
    /// re-sanitization). It is never serialized so it cannot egress via overlay
    /// IPC or the localhost REST surface — egress consumers must read the masked
    /// `text_masked` copy instead. On deserialization it defaults to empty.
    #[serde(skip_serializing, default)]
    pub label: String,
    pub role: Option<String>,
    pub intent: Option<String>,
    pub state: Option<String>,
    pub confidence: f64,
    pub text_masked: Option<String>,
    pub parent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiScene {
    #[serde(default = "default_ui_scene_schema_version")]
    pub schema_version: String,
    pub scene_id: String,
    pub app_name: Option<String>,
    pub screen_id: Option<String>,
    pub captured_at: DateTime<Utc>,
    pub screen_width: u32,
    pub screen_height: u32,
    pub elements: Vec<UiSceneElement>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_bounds_clamps_to_zero_one() {
        let bounds = NormalizedBounds::new(-0.2, 0.4, 1.8, -0.1);
        assert_eq!(bounds.x, 0.0);
        assert_eq!(bounds.y, 0.4);
        assert_eq!(bounds.width, 1.0);
        assert_eq!(bounds.height, 0.0);
    }

    #[test]
    fn ui_scene_serde_roundtrip() {
        let scene = UiScene {
            schema_version: UI_SCENE_SCHEMA_VERSION.to_string(),
            scene_id: "scene-1".to_string(),
            app_name: Some("VSCode".to_string()),
            screen_id: Some("screen-main".to_string()),
            captured_at: Utc::now(),
            screen_width: 1920,
            screen_height: 1080,
            elements: vec![UiSceneElement {
                element_id: "el-1".to_string(),
                bbox_abs: ElementBounds {
                    x: 100,
                    y: 80,
                    width: 240,
                    height: 48,
                },
                bbox_norm: NormalizedBounds::new(0.05, 0.07, 0.12, 0.04),
                label: "Save".to_string(),
                role: Some("button".to_string()),
                intent: Some("execute".to_string()),
                state: Some("enabled".to_string()),
                confidence: 0.91,
                text_masked: Some("Save".to_string()),
                parent_id: None,
            }],
        };

        let json = serde_json::to_string(&scene).unwrap();
        let deserialized: UiScene = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.scene_id, "scene-1");
        assert_eq!(deserialized.elements.len(), 1);
        // The masked copy survives serialization; it is what egress consumers see.
        assert_eq!(
            deserialized.elements[0].text_masked.as_deref(),
            Some("Save")
        );
    }

    #[test]
    fn ui_scene_element_label_never_serializes() {
        let element = UiSceneElement {
            element_id: "el-pii".to_string(),
            bbox_abs: ElementBounds {
                x: 0,
                y: 0,
                width: 10,
                height: 10,
            },
            bbox_norm: NormalizedBounds::new(0.0, 0.0, 0.01, 0.01),
            // Simulate raw OCR text containing PII.
            label: "alice@example.com".to_string(),
            role: None,
            intent: None,
            state: None,
            confidence: 0.5,
            text_masked: Some("[EMAIL]".to_string()),
            parent_id: None,
        };

        let json = serde_json::to_string(&element).unwrap();
        // Raw label must never reach the wire (overlay IPC / localhost REST).
        assert!(
            !json.contains("alice@example.com"),
            "raw label leaked into serialized output: {json}"
        );
        assert!(
            !json.contains("\"label\""),
            "label field must be skipped on serialization: {json}"
        );
        assert!(json.contains("[EMAIL]"), "masked copy should serialize");

        // It still deserializes (defaulting to empty) so embedding DTOs round-trip.
        let decoded: UiSceneElement = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.label, "");
        assert_eq!(decoded.text_masked.as_deref(), Some("[EMAIL]"));
    }
}
