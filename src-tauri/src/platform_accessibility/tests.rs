#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use super::super::types;
    use super::super::*;
    use maekon_core::config::PiiFilterLevel;

    // ──────────────────────────────────────────────────────────────────────────────
    // F-QA-C23-03: async tests for the find_element / analyze_scene spawn_blocking path
    // ──────────────────────────────────────────────────────────────────────────────

    /// Wrap the filtering + conversion logic inside find_element in spawn_blocking
    /// and verify the happy path (returns a single matching element) asynchronously.
    ///
    /// Does not call the real OS accessibility APIs (osascript/powershell/xdotool);
    /// builds controlled nodes via parse_accessibility_lines to test the filter
    /// logic directly.
    #[tokio::test]
    async fn find_element_filter_logic_via_spawn_blocking() {
        // Synthetic accessibility node raw data
        let raw = "Notes|||AXButton|||Save|||100|||200|||80|||30\n\
                   Notes|||AXTextField|||Email|||200|||300|||150|||25\n";

        // Isolate synchronous parsing via spawn_blocking (same as the real find_element pattern)
        let nodes = tokio::task::spawn_blocking(move || types::parse_accessibility_lines(raw))
            .await
            .expect("spawn_blocking join should succeed")
            .expect("parse_accessibility_lines should succeed");

        assert_eq!(nodes.len(), 2, "two nodes should be parsed");

        // text="Save" filter — exactly one match
        let text_filter = Some("Save".to_string());
        let filtered: Vec<maekon_core::models::intent::UiElement> = nodes
            .into_iter()
            .filter(|node| {
                text_filter
                    .as_deref()
                    .map(|value| contains_ignore_case(&node.label, value))
                    .unwrap_or(true)
            })
            .map(|node| maekon_core::models::intent::UiElement {
                text: node.label,
                bounds: node.bounds,
                role: node.role,
                confidence: node.confidence,
                source: maekon_core::models::intent::FinderSource::Accessibility,
            })
            .collect();

        assert_eq!(filtered.len(), 1, "text filter 'Save' should yield 1 match");
        assert_eq!(filtered[0].text, "Save");
    }

    /// Wrap the accessibility_nodes_to_scene_elements conversion logic inside
    /// analyze_scene in spawn_blocking and verify it asynchronously.
    #[tokio::test]
    async fn analyze_scene_node_to_element_conversion_via_spawn_blocking() {
        let raw = "Code|||AXButton|||Run|||50|||60|||120|||40\n\
                   Code|||AXWindow|||Editor|||0|||0|||1920|||1080\n";

        let nodes = tokio::task::spawn_blocking(move || types::parse_accessibility_lines(raw))
            .await
            .expect("spawn_blocking join should succeed")
            .expect("parse_accessibility_lines should succeed");

        assert!(!nodes.is_empty(), "at least one node required");

        let (width, height) = estimate_screen_size(&nodes);
        let elements =
            accessibility_nodes_to_scene_elements(nodes, width, height, PiiFilterLevel::Off);

        assert_eq!(elements.len(), 2, "two scene elements should be converted");
        // Verify element_id pattern
        assert!(
            elements[0].element_id.starts_with("ax_"),
            "element_id should have the ax_ prefix"
        );
        // Verify bbox_norm range (0.0 ~ 1.0)
        let e = &elements[0];
        assert!(
            e.bbox_norm.x >= 0.0 && e.bbox_norm.x <= 1.0,
            "bbox_norm.x should be in the [0,1] range"
        );
        assert!(
            e.bbox_norm.y >= 0.0 && e.bbox_norm.y <= 1.0,
            "bbox_norm.y should be in the [0,1] range"
        );
    }

    /// analyze_scene: verify it returns CoreError::ElementNotFound when there are no nodes.
    #[tokio::test]
    async fn analyze_scene_empty_nodes_returns_element_not_found() {
        let raw = "";
        let result = tokio::task::spawn_blocking(move || types::parse_accessibility_lines(raw))
            .await
            .expect("spawn_blocking join should succeed");

        assert!(
            matches!(
                result,
                Err(maekon_core::error::CoreError::ElementNotFound { .. })
            ),
            "empty raw data should yield CoreError::ElementNotFound"
        );
    }

    #[test]
    fn parse_accessibility_lines_parses_expected_fields() {
        let raw = "Code|||button|||Save|||100|||200|||80|||30\n";
        let nodes = types::parse_accessibility_lines(raw).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].app_name.as_deref(), Some("Code"));
        assert_eq!(nodes[0].role.as_deref(), Some("button"));
        assert_eq!(nodes[0].label, "Save");
        assert_eq!(nodes[0].bounds.x, 100);
    }

    #[test]
    fn parse_accessibility_lines_keeps_unlabelled_structural_node() {
        let raw = "|||ControlType.Pane||||||0|||0|||1920|||1080\n";
        let nodes = types::parse_accessibility_lines(raw).unwrap();

        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].role.as_deref(), Some("ControlType.Pane"));
        assert!(nodes[0].label.is_empty());
        assert_eq!(nodes[0].bounds.width, 1920);
        assert_eq!(nodes[0].bounds.height, 1080);
    }

    #[test]
    fn parse_accessibility_lines_rejects_node_without_role_or_label() {
        let raw = "|||||||||0|||0|||1920|||1080\n";

        assert!(matches!(
            types::parse_accessibility_lines(raw),
            Err(maekon_core::error::CoreError::ElementNotFound { .. })
        ));
    }

    #[test]
    fn intersects_detects_overlap() {
        let a = maekon_core::models::intent::ElementBounds {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        let b = maekon_core::models::intent::ElementBounds {
            x: 5,
            y: 5,
            width: 10,
            height: 10,
        };
        assert!(intersects(&a, &b));
    }

    #[test]
    fn crt_prv_axtree_006_masks_ax_text_before_scene_storage() {
        let nodes = vec![types::AccessibilityNode {
            app_name: Some("Notes".to_string()),
            role: Some("AXTextField".to_string()),
            label: "email user@example.com phone 010-1234-5678; IBAN DE89370400440532013000; token sk-abc123def456ghi789jkl0".to_string(),
            bounds: maekon_core::models::intent::ElementBounds {
                x: 10,
                y: 20,
                width: 300,
                height: 40,
            },
            confidence: 0.98,
        }];

        let elements =
            accessibility_nodes_to_scene_elements(nodes, 1920, 1080, PiiFilterLevel::Strict);
        let element = &elements[0];

        assert_eq!(element.role.as_deref(), Some("AXTextField"));
        assert!(element.label.contains("[EMAIL]"));
        assert!(element.label.contains("[PHONE]"));
        assert!(element.label.contains("[IBAN]"));
        assert!(
            element.label.contains("[API_KEY]"),
            "masked label: {}",
            element.label
        );
        assert_eq!(element.text_masked.as_deref(), Some(element.label.as_str()));

        let serialized = serde_json::to_string(element).unwrap();
        assert!(!serialized.contains("user@example.com"));
        assert!(!serialized.contains("010-1234-5678"));
        assert!(!serialized.contains("0532013000"));
        assert!(!serialized.contains("sk-abc123def456ghi789jkl0"));
    }

    #[test]
    fn windows_uia_benchmark_summary_compares_paths_without_raw_labels() {
        use maekon_core::models::focused_element::{
            AccessibilityElement, ElementRect, FocusedElementInfo,
        };
        use maekon_core::models::intent::ElementBounds;
        use maekon_core::models::ui_scene::{NormalizedBounds, UiScene, UiSceneElement};

        let focused = FocusedElementInfo {
            role: "Button".to_string(),
            position: Some(ElementRect {
                x: 100.0,
                y: 200.0,
                width: 80.0,
                height: 30.0,
            }),
            label: Some("Save user@example.com".to_string()),
            value_length: Some(21),
            extracted_text: None,
        };
        let com_tree = vec![
            AccessibilityElement {
                role: "Window".to_string(),
                label: "Maekon".to_string(),
                bounds: Some(ElementRect {
                    x: 90.0,
                    y: 190.0,
                    width: 300.0,
                    height: 220.0,
                }),
            },
            AccessibilityElement {
                role: "Button".to_string(),
                label: "Save user@example.com".to_string(),
                bounds: Some(ElementRect {
                    x: 100.0,
                    y: 200.0,
                    width: 80.0,
                    height: 30.0,
                }),
            },
        ];
        let platform_scene = UiScene {
            schema_version: "ui_scene.v1".to_string(),
            scene_id: "ax_scene_test".to_string(),
            app_name: Some("Maekon".to_string()),
            screen_id: Some("primary".to_string()),
            captured_at: chrono::Utc::now(),
            screen_width: 1920,
            screen_height: 1080,
            elements: vec![UiSceneElement {
                element_id: "ax_0".to_string(),
                bbox_abs: ElementBounds {
                    x: 102,
                    y: 201,
                    width: 79,
                    height: 31,
                },
                bbox_norm: NormalizedBounds::new(
                    102.0 / 1920.0,
                    201.0 / 1080.0,
                    79.0 / 1920.0,
                    31.0 / 1080.0,
                ),
                label: "Save [EMAIL]".to_string(),
                role: Some("ControlType.Button".to_string()),
                intent: None,
                state: None,
                confidence: 0.98,
                text_masked: Some("Save [EMAIL]".to_string()),
                parent_id: None,
            }],
        };

        let summary = benchmark::summarize_windows_uia_observations(
            Some(&focused),
            &com_tree,
            Some(&platform_scene),
        );

        assert!(summary.com_focused.success);
        assert_eq!(summary.com_focused.role.as_deref(), Some("Button"));
        assert!(summary.com_focused.bounds_sane);
        assert!(summary.com_focused.label_observed);
        assert!(summary.com_focused.value_length_observed);
        assert!(!summary.com_focused.extracted_text_exposed);
        assert_eq!(summary.com_tree.element_count, 2);
        assert_eq!(summary.com_tree.first_role.as_deref(), Some("window"));
        assert_eq!(summary.com_tree.role_counts.get("window"), Some(&1));
        assert_eq!(summary.com_tree.role_counts.get("button"), Some(&1));
        assert_eq!(summary.platform_scene.element_count, 1);
        assert!(summary.convergence.focused_role_seen_in_platform_scene);
        assert!(
            summary
                .convergence
                .focused_bounds_overlap_platform_candidate
        );

        let json = serde_json::to_string(&summary).unwrap();
        assert!(!json.contains("user@example.com"));
        assert!(!json.contains("Save user"));
    }
}
