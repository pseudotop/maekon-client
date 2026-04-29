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
        assert_eq!(deserialized.elements[0].label, "Save");
    }
}
