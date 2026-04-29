//! LLM feedback loop types and parsing for GUI element classification.

use oneshim_core::models::gui_interaction::GuiElementType;
use serde::{Deserialize, Serialize};

use super::features::VisualFeatures;

/// System prompt for LLM feedback on uncertain GUI element classifications.
pub const CONTOUR_FEEDBACK_PROMPT: &str = r#"You are a GUI element classifier. You receive uncertain GUI element classifications with visual feature data. Respond with corrections.

Element types (use ONLY these exact values):
Button, TextInput, Link, MenuItem, TabLabel, StatusBar, TitleBar, ToolbarIcon, TreeItem, ScrollBar, TextRegion, Unknown

Respond in JSON matching this schema exactly:
{
  "corrections": [
    {"index": 0, "correct_type": "TabLabel", "confidence": 0.9}
  ]
}

Rules:
- correct_type MUST be one of the listed types exactly as written
- confidence is 0.0-1.0
- Only include corrections where you are reasonably confident
- If uncertain, omit the element from corrections"#;

/// An element queued for LLM feedback due to low classification confidence.
#[derive(Debug, Clone, Serialize)]
pub struct UncertainElement {
    pub app_name: String,
    pub text: String,
    pub current_type: String,
    pub confidence: f32,
    pub features: FeatureSummary,
}

/// Serializable subset of VisualFeatures for LLM context.
#[derive(Debug, Clone, Serialize)]
pub struct FeatureSummary {
    pub border_contrast: f32,
    pub fill_uniformity: f32,
    pub has_distinct_border: bool,
    pub has_background_fill: bool,
    pub aspect_ratio: f32,
}

impl FeatureSummary {
    /// Fallback when no frame crop data is available.
    pub fn from_aspect_ratio(aspect_ratio: f32) -> Self {
        Self {
            border_contrast: 0.0,
            fill_uniformity: 0.0,
            has_distinct_border: false,
            has_background_fill: false,
            aspect_ratio,
        }
    }
}

impl From<&VisualFeatures> for FeatureSummary {
    fn from(v: &VisualFeatures) -> Self {
        Self {
            border_contrast: v.border_contrast,
            fill_uniformity: v.fill_uniformity,
            has_distinct_border: v.has_distinct_border,
            has_background_fill: v.has_background_fill,
            aspect_ratio: v.aspect_ratio,
        }
    }
}

/// Feedback request sent to LLM.
#[derive(Debug, Serialize)]
pub struct FeedbackRequest {
    pub uncertain_elements: Vec<UncertainElement>,
}

/// Single correction from LLM.
#[derive(Debug, Clone, Deserialize)]
pub struct Correction {
    pub index: usize,
    pub correct_type: String,
    #[serde(default = "default_confidence")]
    pub confidence: f32,
}

fn default_confidence() -> f32 {
    0.8
}

/// Parsed feedback response from LLM.
#[derive(Debug, Deserialize)]
pub struct FeedbackResponse {
    #[serde(default)]
    pub corrections: Vec<Correction>,
}

/// Parse and validate LLM feedback response.
///
/// Discards corrections with invalid element types.
pub fn parse_feedback_response(raw: &str) -> Result<FeedbackResponse, String> {
    // Try to extract JSON from the response (LLM may include preamble)
    let json_str = extract_json_object(raw).unwrap_or(raw);

    let response: FeedbackResponse =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {e}"))?;

    Ok(response)
}

/// Validate that a type string matches a known GuiElementType.
pub fn validate_element_type(type_str: &str) -> Option<GuiElementType> {
    match type_str {
        "Button" => Some(GuiElementType::Button),
        "TextInput" => Some(GuiElementType::TextInput),
        "Link" => Some(GuiElementType::Link),
        "MenuItem" => Some(GuiElementType::MenuItem),
        "TabLabel" => Some(GuiElementType::TabLabel),
        "StatusBar" => Some(GuiElementType::StatusBar),
        "TitleBar" => Some(GuiElementType::TitleBar),
        "ToolbarIcon" => Some(GuiElementType::ToolbarIcon),
        "TreeItem" => Some(GuiElementType::TreeItem),
        "ScrollBar" => Some(GuiElementType::ScrollBar),
        "TextRegion" => Some(GuiElementType::TextRegion),
        "Unknown" => Some(GuiElementType::Unknown),
        _ => None,
    }
}

/// Extract the first JSON object `{...}` from a string that may contain
/// surrounding text (LLM preamble/explanation).
fn extract_json_object(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let mut depth = 0;
    for (i, ch) in s[start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[start..start + i + 1]);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_response() {
        let json =
            r#"{"corrections": [{"index": 0, "correct_type": "TabLabel", "confidence": 0.9}]}"#;
        let resp = parse_feedback_response(json).unwrap();
        assert_eq!(resp.corrections.len(), 1);
        assert_eq!(resp.corrections[0].correct_type, "TabLabel");
        assert!((resp.corrections[0].confidence - 0.9).abs() < f32::EPSILON);
    }

    #[test]
    fn parse_response_with_preamble() {
        let raw = r#"Here's my analysis:
{"corrections": [{"index": 0, "correct_type": "Button", "confidence": 0.85}]}
Hope this helps!"#;
        let resp = parse_feedback_response(raw).unwrap();
        assert_eq!(resp.corrections.len(), 1);
        assert_eq!(resp.corrections[0].correct_type, "Button");
    }

    #[test]
    fn parse_empty_corrections() {
        let json = r#"{"corrections": []}"#;
        let resp = parse_feedback_response(json).unwrap();
        assert!(resp.corrections.is_empty());
    }

    #[test]
    fn parse_invalid_json_returns_error() {
        let result = parse_feedback_response("not json at all");
        assert!(result.is_err());
    }

    #[test]
    fn validate_known_types() {
        assert_eq!(
            validate_element_type("Button"),
            Some(GuiElementType::Button)
        );
        assert_eq!(
            validate_element_type("TabLabel"),
            Some(GuiElementType::TabLabel)
        );
        assert_eq!(
            validate_element_type("Unknown"),
            Some(GuiElementType::Unknown)
        );
    }

    #[test]
    fn validate_unknown_type_returns_none() {
        assert_eq!(validate_element_type("Breadcrumb"), None);
        assert_eq!(validate_element_type("button"), None); // case-sensitive
    }

    #[test]
    fn extract_json_nested() {
        let s = r#"text {"a": {"b": 1}} more"#;
        assert_eq!(extract_json_object(s), Some(r#"{"a": {"b": 1}}"#));
    }

    #[test]
    fn default_confidence_applied() {
        let json = r#"{"corrections": [{"index": 0, "correct_type": "Link"}]}"#;
        let resp = parse_feedback_response(json).unwrap();
        assert!((resp.corrections[0].confidence - 0.8).abs() < f32::EPSILON);
    }
}
