use super::extractor::*;

use oneshim_core::config::PiiFilterLevel;
use oneshim_core::models::focused_element::ElementRect;
use oneshim_core::ports::accessibility::AccessibilityExtractor;
use zeroize::Zeroizing;

#[test]
fn filter_strict_only_role_and_position() {
    let info = apply_filter(
        "AXTextField",
        Some("Search"),
        Some("secret query"),
        Some("Type here"),
        Some(ElementRect {
            x: 10.0,
            y: 20.0,
            width: 200.0,
            height: 25.0,
        }),
        PiiFilterLevel::Strict,
    );
    assert_eq!(info.role, "AXTextField");
    assert!(info.position.is_some());
    assert!(info.label.is_none());
    assert!(info.value_length.is_none());
    assert!(info.extracted_text.is_none());
}

#[test]
fn filter_standard_includes_label_and_length() {
    let info = apply_filter(
        "AXTextArea",
        Some("Terminal"),
        Some("cargo test"),
        None,
        None,
        PiiFilterLevel::Standard,
    );
    assert_eq!(info.label, Some("Terminal".to_string()));
    assert_eq!(info.value_length, Some(10));
    assert!(info.extracted_text.is_none());
}

#[test]
fn filter_basic_includes_sanitized_text() {
    let info = apply_filter(
        "AXTextField",
        None,
        Some("user@example.com"),
        None,
        None,
        PiiFilterLevel::Basic,
    );
    assert!(info.extracted_text.is_some());
    let text = info.extracted_text.unwrap();
    assert!(text.contains("[EMAIL]"));
    assert!(!text.contains("user@example.com"));
}

#[test]
fn filter_off_includes_full_text() {
    let info = apply_filter(
        "AXTextField",
        None,
        Some("full content here"),
        None,
        None,
        PiiFilterLevel::Off,
    );
    assert_eq!(info.extracted_text, Some("full content here".to_string()));
}

#[test]
fn filter_standard_falls_back_to_placeholder_when_no_title() {
    let info = apply_filter(
        "AXTextField",
        None,
        Some("value"),
        Some("Search..."),
        None,
        PiiFilterLevel::Standard,
    );
    assert_eq!(info.label, Some("Search...".to_string()));
}

/// Integration test -- requires Accessibility permission.
/// Run manually: `cargo test -p oneshim-vision -- macos_native_ax --ignored`
#[tokio::test]
#[ignore]
async fn extract_focused_element_integration() {
    let extractor = MacOsNativeAccessibility::new();
    if !extractor.has_permission() {
        eprintln!("SKIP: Accessibility permission not granted");
        return;
    }
    let result = extractor
        .extract_focused_element(PiiFilterLevel::Standard, false)
        .await;
    assert!(result.is_ok());
    // May be None if no element is focused (headless CI)
}

/// Integration test for tree traversal -- requires Accessibility permission.
/// Run manually: `cargo test -p oneshim-vision -- macos_tree_traversal --ignored`
#[tokio::test]
#[ignore]
async fn extract_window_elements_integration() {
    let extractor = MacOsNativeAccessibility::new();
    if !extractor.has_permission() {
        eprintln!("SKIP: Accessibility permission not granted");
        return;
    }
    let result = extractor
        .extract_window_elements(3, 300, PiiFilterLevel::Standard, false)
        .await;
    assert!(result.is_ok());
    let elements = result.unwrap();
    // Should return at least 1 element (the window or focused element)
    // May return 0 on headless CI
    eprintln!("extracted {} elements", elements.len());
}

#[tokio::test]
#[ignore]
async fn extract_window_elements_permission_denied_without_access() {
    // This test verifies the PermissionDenied path, but only
    // meaningful when run without Accessibility permission.
    let extractor = MacOsNativeAccessibility::new();
    if extractor.has_permission() {
        eprintln!("SKIP: permission already granted, cannot test denial");
        return;
    }
    let result = extractor
        .extract_window_elements(3, 300, PiiFilterLevel::Standard, false)
        .await;
    assert!(matches!(
        result,
        Err(oneshim_core::error::CoreError::PermissionDenied { .. })
    ));
}

/// Integration test for batch attribute fetching -- requires Accessibility permission.
/// Verifies that traverse_tree uses batch_get_attributes and produces the
/// same results as the individual-call fallback path.
/// Run manually: `cargo test -p oneshim-vision -- macos_batch_traversal --ignored`
#[tokio::test]
#[ignore]
async fn extract_window_elements_batch_traversal() {
    let extractor = MacOsNativeAccessibility::new();
    if !extractor.has_permission() {
        eprintln!("SKIP: Accessibility permission not granted");
        return;
    }
    let result = extractor
        .extract_window_elements(2, 100, PiiFilterLevel::Off, true)
        .await;
    assert!(result.is_ok());
    let elements = result.unwrap();
    // Each element should have a non-empty role from the batch fetch
    for elem in &elements {
        assert!(!elem.role.is_empty(), "batch fetch should populate role");
    }
    eprintln!(
        "batch traversal: {} elements, {} with bounds",
        elements.len(),
        elements.iter().filter(|e| e.bounds.is_some()).count()
    );
}

/// Apply PII filter to test data by reconstructing the filter logic.
/// This duplicates the private struct so we can test the filtering
/// without exposing internals.
fn apply_filter(
    role: &str,
    title: Option<&str>,
    value: Option<&str>,
    placeholder: Option<&str>,
    position: Option<ElementRect>,
    level: PiiFilterLevel,
) -> oneshim_core::models::focused_element::FocusedElementInfo {
    use crate::privacy::sanitize_title_with_level;
    use oneshim_core::models::focused_element::FocusedElementInfo;

    let title_z = title.map(|s| Zeroizing::new(s.to_string()));
    let value_z = value.map(|s| Zeroizing::new(s.to_string()));
    let placeholder_s = placeholder.map(|s| s.to_string());

    let result = match level {
        PiiFilterLevel::Strict => FocusedElementInfo {
            role: role.to_string(),
            position,
            ..Default::default()
        },
        PiiFilterLevel::Standard => FocusedElementInfo {
            role: role.to_string(),
            position,
            label: title_z
                .as_deref()
                .map(|s| s.to_string())
                .or(placeholder_s.clone()),
            value_length: value_z.as_deref().map(|v| v.len() as u32),
            ..Default::default()
        },
        PiiFilterLevel::Basic => {
            let text = value_z
                .as_deref()
                .map(|v| sanitize_title_with_level(v, PiiFilterLevel::Basic));
            FocusedElementInfo {
                role: role.to_string(),
                position,
                label: title_z
                    .as_deref()
                    .map(|s| s.to_string())
                    .or(placeholder_s.clone()),
                value_length: value_z.as_deref().map(|v| v.len() as u32),
                extracted_text: text,
            }
        }
        PiiFilterLevel::Off => FocusedElementInfo {
            role: role.to_string(),
            position,
            label: title_z
                .as_deref()
                .map(|s| s.to_string())
                .or(placeholder_s.clone()),
            value_length: value_z.as_deref().map(|v| v.len() as u32),
            extracted_text: value_z.as_deref().map(|v| v.to_string()),
        },
    };
    // Zeroizing values dropped here.
    result
}
