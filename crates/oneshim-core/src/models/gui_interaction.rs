use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::frame::BoundingBox;

/// A GUI element inferred from OCR + input correlation.
///
/// Part of the **observation layer**: used by `GuiElementDetector` (formerly
/// `InputOcrCorrelator`) to track what the user interacts with. This is
/// passive observation only — recording clicks, keystrokes, and screen regions.
///
/// Compare with `UiElement` / `UiSceneElement` in the **automation layer**,
/// which are used by `ElementFinder` to locate targets for automated agent
/// actions. Both models share similar spatial data but serve different purposes
/// and lifecycles: observation tracks user behavior, automation executes agent
/// actions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GuiElement {
    /// Text content of the element.
    pub text: String,
    /// Spatial position on screen.
    pub bbox: BoundingBox,
    /// Inferred element type.
    pub element_type: GuiElementType,
    /// OCR confidence.
    pub confidence: f32,
    /// Classification confidence: how certain the inferred element type is (0.0–1.0).
    #[serde(default = "default_type_confidence")]
    pub type_confidence: f32,
}

fn default_type_confidence() -> f32 {
    1.0
}

/// Inferred type of a GUI element based on OCR text and position heuristics.
///
/// Part of the **observation layer**. These variants are inferred from OCR text
/// content and screen position heuristics, not from accessibility APIs.
///
/// Compare with `UiElementKind` in the automation layer, which represents
/// platform-native control types (e.g., from macOS AXUIElement or Windows
/// UIAutomation). Both coexist by design.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GuiElementType {
    Button,
    TextInput,
    Link,
    MenuItem,
    TabLabel,
    StatusBar,
    TitleBar,
    // Phase 2 variants
    ToolbarIcon,
    TreeItem,
    ScrollBar,
    TextRegion,
    Unknown,
}

/// Record of a user interacting with a GUI element.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiInteractionEvent {
    pub timestamp: DateTime<Utc>,
    pub element: GuiElement,
    pub interaction_type: GuiInteractionType,
    pub app_name: String,
    /// Window title at the time of the interaction (Phase 2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_title: Option<String>,
    /// Absolute screen position of the interaction (Phase 2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen_position: Option<(u32, u32)>,
    /// Structured interaction details (Phase 2). Coexists with
    /// `interaction_type` for backward compatibility.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction: Option<InteractionType>,
}

/// Type of user interaction with a GUI element (Phase 1, simple enum).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GuiInteractionType {
    Click,
    DoubleClick,
    RightClick,
    Type,
    Hover,
}

/// Structured interaction details with payloads (Phase 2).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum InteractionType {
    Click {
        button: ClickButton,
    },
    KeyboardShortcut {
        keys: String,
    },
    TextEntry {
        char_count: u32,
        duration_ms: u64,
    },
    Scroll {
        direction: ScrollDirection,
        amount: f32,
    },
    DragDrop {
        start: (u32, u32),
        end: (u32, u32),
    },
}

/// Mouse button for click interactions.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ClickButton {
    Left,
    Right,
    Middle,
}

/// Scroll direction.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ScrollDirection {
    Up,
    Down,
    Left,
    Right,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gui_element_serde_roundtrip() {
        let element = GuiElement {
            text: "Save".to_string(),
            bbox: BoundingBox {
                x: 100,
                y: 200,
                width: 80,
                height: 30,
            },
            element_type: GuiElementType::Button,
            confidence: 0.92,
            type_confidence: 1.0,
        };
        let json = serde_json::to_string(&element).unwrap();
        let parsed: GuiElement = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.text, "Save");
        assert_eq!(parsed.element_type, GuiElementType::Button);
    }

    #[test]
    fn gui_element_type_serde_screaming_case() {
        let json = serde_json::to_string(&GuiElementType::TextInput).unwrap();
        assert_eq!(json, "\"TEXT_INPUT\"");

        let parsed: GuiElementType = serde_json::from_str("\"TITLE_BAR\"").unwrap();
        assert_eq!(parsed, GuiElementType::TitleBar);
    }

    #[test]
    fn gui_interaction_type_serde_screaming_case() {
        let json = serde_json::to_string(&GuiInteractionType::DoubleClick).unwrap();
        assert_eq!(json, "\"DOUBLE_CLICK\"");

        let parsed: GuiInteractionType = serde_json::from_str("\"RIGHT_CLICK\"").unwrap();
        assert_eq!(parsed, GuiInteractionType::RightClick);
    }

    #[test]
    fn gui_interaction_event_serde_roundtrip() {
        let event = GuiInteractionEvent {
            timestamp: Utc::now(),
            element: GuiElement {
                text: "File".to_string(),
                bbox: BoundingBox {
                    x: 0,
                    y: 0,
                    width: 40,
                    height: 20,
                },
                element_type: GuiElementType::MenuItem,
                confidence: 0.88,
                type_confidence: 1.0,
            },
            interaction_type: GuiInteractionType::Click,
            app_name: "Firefox".to_string(),
            window_title: Some("Mozilla Firefox".to_string()),
            screen_position: Some((120, 30)),
            interaction: Some(InteractionType::Click {
                button: ClickButton::Left,
            }),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: GuiInteractionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.app_name, "Firefox");
        assert_eq!(parsed.interaction_type, GuiInteractionType::Click);
        assert_eq!(parsed.element.element_type, GuiElementType::MenuItem);
        assert_eq!(parsed.window_title, Some("Mozilla Firefox".to_string()));
        assert_eq!(parsed.screen_position, Some((120, 30)));
    }

    #[test]
    fn gui_interaction_event_phase1_compat() {
        // Phase 1 events without Phase 2 fields should still deserialize
        let json = r#"{
            "timestamp": "2026-03-19T00:00:00Z",
            "element": {
                "text": "OK",
                "bbox": {"x": 0, "y": 0, "width": 40, "height": 20},
                "element_type": "BUTTON",
                "confidence": 0.9
            },
            "interaction_type": "CLICK",
            "app_name": "TestApp"
        }"#;
        let parsed: GuiInteractionEvent = serde_json::from_str(json).unwrap();
        assert!(parsed.window_title.is_none());
        assert!(parsed.screen_position.is_none());
        assert!(parsed.interaction.is_none());
    }

    #[test]
    fn phase2_element_type_serde() {
        let json = serde_json::to_string(&GuiElementType::ToolbarIcon).unwrap();
        assert_eq!(json, "\"TOOLBAR_ICON\"");

        let parsed: GuiElementType = serde_json::from_str("\"TREE_ITEM\"").unwrap();
        assert_eq!(parsed, GuiElementType::TreeItem);

        let parsed: GuiElementType = serde_json::from_str("\"SCROLL_BAR\"").unwrap();
        assert_eq!(parsed, GuiElementType::ScrollBar);

        let parsed: GuiElementType = serde_json::from_str("\"TEXT_REGION\"").unwrap();
        assert_eq!(parsed, GuiElementType::TextRegion);
    }

    #[test]
    fn interaction_type_serde_roundtrip() {
        let scroll = InteractionType::Scroll {
            direction: ScrollDirection::Down,
            amount: 3.5,
        };
        let json = serde_json::to_string(&scroll).unwrap();
        let parsed: InteractionType = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, scroll);

        let drag = InteractionType::DragDrop {
            start: (10, 20),
            end: (100, 200),
        };
        let json = serde_json::to_string(&drag).unwrap();
        let parsed: InteractionType = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, drag);
    }
}
