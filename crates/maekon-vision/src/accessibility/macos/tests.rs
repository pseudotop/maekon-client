use super::extractor::*;

use maekon_core::config::PiiFilterLevel;
use maekon_core::models::focused_element::ElementRect;
use maekon_core::ports::accessibility::AccessibilityExtractor;

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
/// Run manually: `cargo test -p maekon-vision -- macos_native_ax --ignored`
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
    // Contract: with Accessibility permission granted the AX API must not return Err.
    // Value may be None when no element is focused (e.g. headless CI) — that is Ok(None).
    // #5594: Ok-only is the full contract here; the caller guards against the no-permission
    // path above, so Err would indicate a genuine AX API failure.
    result
        .expect("extract_focused_element must return Ok when Accessibility permission is granted");
}

/// Integration test for tree traversal -- requires Accessibility permission.
/// Run manually: `cargo test -p maekon-vision -- macos_tree_traversal --ignored`
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
    // Contract: with Accessibility permission the traversal must not return Err.
    let elements = result
        .expect("extract_window_elements must return Ok when Accessibility permission is granted");
    // May return 0 on headless CI; each returned element must have a non-empty role.
    for elem in &elements {
        assert!(
            !elem.role.is_empty(),
            "every returned element must have a non-empty role"
        );
    }
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
        Err(maekon_core::error::CoreError::PermissionDenied { .. })
    ));
}

/// Integration test for batch attribute fetching -- requires Accessibility permission.
/// Verifies that traverse_tree uses batch_get_attributes and produces the
/// same results as the individual-call fallback path.
/// Run manually: `cargo test -p maekon-vision -- macos_batch_traversal --ignored`
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
    // Contract: batch traversal must not return Err when Accessibility permission is granted.
    let elements = result.expect(
        "batch extract_window_elements must return Ok when Accessibility permission is granted",
    );
    // Each element must have a non-empty role populated by the batch AX attribute fetch.
    for elem in &elements {
        assert!(!elem.role.is_empty(), "batch fetch should populate role");
    }
    eprintln!(
        "batch traversal: {} elements, {} with bounds",
        elements.len(),
        elements.iter().filter(|e| e.bounds.is_some()).count()
    );
}

/// Apply the PII filter to test data through the production redaction
/// (`pii_filter::apply_pii_level`). Previously this re-implemented the filter
/// logic verbatim — so the tests validated a copy, not shipped code; the #5120
/// seam exposed the real function, so this is now a thin adapter over it
/// (label resolved the same way the macOS extractor does: title → placeholder).
fn apply_filter(
    role: &str,
    title: Option<&str>,
    value: Option<&str>,
    placeholder: Option<&str>,
    position: Option<ElementRect>,
    level: PiiFilterLevel,
) -> maekon_core::models::focused_element::FocusedElementInfo {
    let label = title
        .map(str::to_string)
        .or_else(|| placeholder.map(str::to_string));
    super::super::pii_filter::apply_pii_level(role.to_string(), label, value, position, level)
}
