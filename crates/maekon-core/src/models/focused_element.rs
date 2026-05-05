//! Focused UI element information from OS accessibility APIs.
//!
//! Domain model consumed by the analysis pipeline to provide element-level
//! context for text-heavy applications. PII filtering is applied before
//! these structs are persisted or transmitted.

use serde::{Deserialize, Serialize};

/// Screen rectangle for an accessibility element.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ElementRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Information about the currently focused UI element, extracted via
/// OS accessibility API. All text fields are PII-filtered before storage.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FocusedElementInfo {
    /// Accessibility role (e.g., "AXTextField", "AXTextArea", "AXButton",
    /// "AXStaticText", "edit", "document").
    pub role: String,

    /// Position and size of the element on screen.
    pub position: Option<ElementRect>,

    /// Accessibility label (e.g., "Search", "Terminal", "Message input").
    /// Filtered by PII level. None at Strict level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,

    /// Length of the element's text value in characters (not the content itself).
    /// Useful for distinguishing empty fields from filled ones.
    /// Available at Standard and Basic levels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_length: Option<u32>,

    /// Extracted text content from the element.
    /// Only available at Basic level (with email/phone masking) or Off level
    /// (full text, requires additional consent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extracted_text: Option<String>,
}

/// A single element from the accessibility tree snapshot.
/// Used for dashcam tagging and overlay highlights.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AccessibilityElement {
    /// Accessibility role (e.g., "AXButton", "Edit", "push_button").
    pub role: String,
    /// Accessibility label/name.
    pub label: String,
    /// Bounding rectangle (x, y, width, height) in screen coordinates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounds: Option<ElementRect>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focused_element_info_default() {
        let info = FocusedElementInfo::default();
        assert_eq!(info.role, "");
        assert!(info.position.is_none());
        assert!(info.label.is_none());
        assert!(info.value_length.is_none());
        assert!(info.extracted_text.is_none());
    }

    #[test]
    fn serde_roundtrip_full() {
        let info = FocusedElementInfo {
            role: "AXTextField".to_string(),
            position: Some(ElementRect {
                x: 100.0,
                y: 200.0,
                width: 300.0,
                height: 25.0,
            }),
            label: Some("Search".to_string()),
            value_length: Some(42),
            extracted_text: Some("cargo test --workspace".to_string()),
        };
        let json = serde_json::to_string(&info).unwrap();
        let decoded: FocusedElementInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(info, decoded);
    }

    #[test]
    fn serde_roundtrip_minimal() {
        let info = FocusedElementInfo {
            role: "AXButton".to_string(),
            position: Some(ElementRect {
                x: 10.0,
                y: 20.0,
                width: 80.0,
                height: 30.0,
            }),
            ..Default::default()
        };
        let json = serde_json::to_string(&info).unwrap();
        // None fields are skipped
        assert!(!json.contains("label"));
        assert!(!json.contains("value_length"));
        assert!(!json.contains("extracted_text"));
        let decoded: FocusedElementInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(info, decoded);
    }

    #[test]
    fn backward_compat_missing_fields() {
        // Old JSON without focused_element fields deserializes to defaults
        let json = r#"{"role":"AXGroup"}"#;
        let info: FocusedElementInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.role, "AXGroup");
        assert!(info.position.is_none());
    }

    #[test]
    fn accessibility_element_serde_roundtrip() {
        let elem = AccessibilityElement {
            role: "AXButton".to_string(),
            label: "Save".to_string(),
            bounds: Some(ElementRect {
                x: 10.0,
                y: 20.0,
                width: 80.0,
                height: 30.0,
            }),
        };
        let json = serde_json::to_string(&elem).unwrap();
        let decoded: AccessibilityElement = serde_json::from_str(&json).unwrap();
        assert_eq!(elem, decoded);
    }

    #[test]
    fn accessibility_element_default_has_empty_fields() {
        let elem = AccessibilityElement::default();
        assert_eq!(elem.role, "");
        assert_eq!(elem.label, "");
        assert!(elem.bounds.is_none());
    }
}
