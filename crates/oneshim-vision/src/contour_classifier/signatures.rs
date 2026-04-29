//! Element type signatures and scoring for visual feature matching.

use oneshim_core::models::gui_interaction::GuiElementType;

use super::features::VisualFeatures;

/// Defines expected visual feature ranges for a GUI element type.
struct ElementSignature {
    element_type: GuiElementType,
    border_contrast: (f32, f32),
    fill_uniformity: (f32, f32),
    aspect_ratio: (f32, f32),
    needs_distinct_border: Option<bool>,
    needs_background: Option<bool>,
}

const SIGNATURES: &[ElementSignature] = &[
    // Button: clear border, filled background, compact shape
    ElementSignature {
        element_type: GuiElementType::Button,
        border_contrast: (0.15, 1.0),
        fill_uniformity: (0.6, 1.0),
        aspect_ratio: (1.5, 6.0),
        needs_distinct_border: Some(true),
        needs_background: Some(true),
    },
    // TextInput: clear border, mostly empty, wide
    ElementSignature {
        element_type: GuiElementType::TextInput,
        border_contrast: (0.15, 1.0),
        fill_uniformity: (0.8, 1.0),
        aspect_ratio: (3.0, 25.0),
        needs_distinct_border: Some(true),
        needs_background: None,
    },
    // Link: no border, no fill, text-like
    ElementSignature {
        element_type: GuiElementType::Link,
        border_contrast: (0.0, 0.1),
        fill_uniformity: (0.5, 1.0),
        aspect_ratio: (2.0, 30.0),
        needs_distinct_border: Some(false),
        needs_background: Some(false),
    },
    // MenuItem: minimal border, uniform, stacked
    ElementSignature {
        element_type: GuiElementType::MenuItem,
        border_contrast: (0.0, 0.15),
        fill_uniformity: (0.6, 1.0),
        aspect_ratio: (3.0, 20.0),
        needs_distinct_border: None,
        needs_background: None,
    },
    // TabLabel: medium border, compact
    ElementSignature {
        element_type: GuiElementType::TabLabel,
        border_contrast: (0.05, 0.4),
        fill_uniformity: (0.5, 1.0),
        aspect_ratio: (1.5, 5.0),
        needs_distinct_border: None,
        needs_background: None,
    },
    // StatusBar: no border, uniform, very wide
    ElementSignature {
        element_type: GuiElementType::StatusBar,
        border_contrast: (0.0, 0.1),
        fill_uniformity: (0.5, 1.0),
        aspect_ratio: (10.0, 100.0),
        needs_distinct_border: Some(false),
        needs_background: None,
    },
    // TitleBar: subtle border, uniform, very wide
    ElementSignature {
        element_type: GuiElementType::TitleBar,
        border_contrast: (0.0, 0.15),
        fill_uniformity: (0.6, 1.0),
        aspect_ratio: (10.0, 100.0),
        needs_distinct_border: None,
        needs_background: None,
    },
    // ToolbarIcon: small and squarish
    ElementSignature {
        element_type: GuiElementType::ToolbarIcon,
        border_contrast: (0.0, 0.5),
        fill_uniformity: (0.3, 1.0),
        aspect_ratio: (0.5, 2.0),
        needs_distinct_border: None,
        needs_background: None,
    },
    // TreeItem: no border, text-like
    ElementSignature {
        element_type: GuiElementType::TreeItem,
        border_contrast: (0.0, 0.1),
        fill_uniformity: (0.5, 1.0),
        aspect_ratio: (2.0, 15.0),
        needs_distinct_border: Some(false),
        needs_background: None,
    },
    // ScrollBar: narrow or short bar
    ElementSignature {
        element_type: GuiElementType::ScrollBar,
        border_contrast: (0.05, 0.3),
        fill_uniformity: (0.7, 1.0),
        aspect_ratio: (0.05, 0.3),
        needs_distinct_border: None,
        needs_background: None,
    },
    // TextRegion: no border, low uniformity (text variance), wide
    ElementSignature {
        element_type: GuiElementType::TextRegion,
        border_contrast: (0.0, 0.1),
        fill_uniformity: (0.2, 0.7),
        aspect_ratio: (2.0, 30.0),
        needs_distinct_border: Some(false),
        needs_background: None,
    },
    // Unknown: catch-all with lowest specificity
    ElementSignature {
        element_type: GuiElementType::Unknown,
        border_contrast: (0.0, 1.0),
        fill_uniformity: (0.0, 1.0),
        aspect_ratio: (0.0, 100.0),
        needs_distinct_border: None,
        needs_background: None,
    },
];

/// Match visual features against element type signatures.
///
/// Returns the best-matching type and a confidence score [0.5, 1.0].
/// Returns `None` only if no signatures match at all (shouldn't happen
/// because Unknown is a catch-all).
pub fn match_signatures(features: &VisualFeatures) -> Option<(GuiElementType, f32)> {
    let mut scores: Vec<(GuiElementType, f32)> = Vec::with_capacity(SIGNATURES.len());

    for sig in SIGNATURES {
        // Boolean hard filters
        if let Some(needs) = sig.needs_distinct_border {
            if features.has_distinct_border != needs {
                continue;
            }
        }
        if let Some(needs) = sig.needs_background {
            if features.has_background_fill != needs {
                continue;
            }
        }

        // Numeric dimension scoring
        let dims = [
            range_score(features.border_contrast, sig.border_contrast),
            range_score(features.fill_uniformity, sig.fill_uniformity),
            range_score(features.aspect_ratio, sig.aspect_ratio),
        ];

        let dim_avg: f32 = dims.iter().sum::<f32>() / dims.len() as f32;

        // Specificity bonus: narrower ranges are more specific
        let range_product = (sig.border_contrast.1 - sig.border_contrast.0)
            * (sig.fill_uniformity.1 - sig.fill_uniformity.0)
            * (sig.aspect_ratio.1 - sig.aspect_ratio.0);
        let specificity = 1.0 / (1.0 + range_product);

        let score = dim_avg * (1.0 + specificity * 0.5);
        if score > 0.0 {
            scores.push((sig.element_type.clone(), score));
        }
    }

    if scores.is_empty() {
        return None;
    }

    scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let winner = scores[0].1;
    let runner_up = scores.get(1).map_or(0.0, |s| s.1);
    let confidence = if winner + runner_up > 0.0 {
        (winner / (winner + runner_up)).max(0.5)
    } else {
        0.5
    };

    Some((scores[0].0.clone(), confidence))
}

/// Score how well a feature value fits within a range.
/// Returns 1.0 at the midpoint, 0.0 at or outside the edges.
fn range_score(value: f32, (min, max): (f32, f32)) -> f32 {
    if value < min || value > max {
        return 0.0;
    }
    let midpoint = (min + max) / 2.0;
    let half_range = (max - min) / 2.0;
    if half_range <= 0.0 {
        return 1.0;
    }
    1.0 - (value - midpoint).abs() / half_range
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_features_match_button() {
        let f = VisualFeatures {
            border_contrast: 0.4,
            fill_uniformity: 0.85,
            has_distinct_border: true,
            has_background_fill: true,
            aspect_ratio: 3.0,
        };
        let result = match_signatures(&f);
        assert!(result.is_some());
        let (etype, conf) = result.unwrap();
        assert_eq!(
            etype,
            GuiElementType::Button,
            "button features should match Button"
        );
        assert!(conf > 0.5);
    }

    #[test]
    fn text_input_features_match() {
        let f = VisualFeatures {
            border_contrast: 0.3,
            fill_uniformity: 0.95,
            has_distinct_border: true,
            has_background_fill: false,
            aspect_ratio: 8.0,
        };
        let result = match_signatures(&f).unwrap();
        assert_eq!(result.0, GuiElementType::TextInput);
    }

    #[test]
    fn toolbar_icon_square_features() {
        let f = VisualFeatures {
            border_contrast: 0.1,
            fill_uniformity: 0.6,
            has_distinct_border: false,
            has_background_fill: true,
            aspect_ratio: 1.0,
        };
        let result = match_signatures(&f).unwrap();
        assert_eq!(result.0, GuiElementType::ToolbarIcon);
    }

    #[test]
    fn unknown_catches_ambiguous() {
        // Features that don't strongly match anything specific
        let f = VisualFeatures {
            border_contrast: 0.5,
            fill_uniformity: 0.5,
            has_distinct_border: true,
            has_background_fill: true,
            aspect_ratio: 50.0, // very wide, doesn't match Button
        };
        let result = match_signatures(&f);
        assert!(result.is_some(), "should always match at least Unknown");
    }

    #[test]
    fn range_score_midpoint_is_one() {
        assert!((range_score(0.5, (0.0, 1.0)) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn range_score_edge_is_zero() {
        assert!(range_score(0.0, (0.0, 1.0)).abs() < f32::EPSILON);
        assert!(range_score(1.0, (0.0, 1.0)).abs() < f32::EPSILON);
    }

    #[test]
    fn range_score_outside_is_zero() {
        assert_eq!(range_score(2.0, (0.0, 1.0)), 0.0);
        assert_eq!(range_score(-1.0, (0.0, 1.0)), 0.0);
    }
}
