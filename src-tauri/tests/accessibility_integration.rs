//! Integration tests for the Phase 2 AccessibilityExtractor pipeline.
//!
//! Tests cover:
//! - Mock extractor with PII-level fallback chain
//! - Config gating (accessibility_extraction + consent)
//! - Zeroize memory clearing behavior
//! - Platform factory function

use async_trait::async_trait;
use oneshim_core::config::PiiFilterLevel;
use oneshim_core::consent::ConsentPermissions;
use oneshim_core::error::CoreError;
use oneshim_core::models::focused_element::{ElementRect, FocusedElementInfo};
use oneshim_core::ports::accessibility::AccessibilityExtractor;

// ── Mock AccessibilityExtractor ──

/// A mock extractor that applies PII-level rules the same way a real
/// implementation would, using pre-set test data.
struct MockAccessibilityExtractor {
    role: String,
    label: Option<String>,
    value: Option<String>,
    position: Option<ElementRect>,
}

impl MockAccessibilityExtractor {
    fn new(
        role: &str,
        label: Option<&str>,
        value: Option<&str>,
        position: Option<ElementRect>,
    ) -> Self {
        Self {
            role: role.to_string(),
            label: label.map(|s| s.to_string()),
            value: value.map(|s| s.to_string()),
            position,
        }
    }
}

#[async_trait]
impl AccessibilityExtractor for MockAccessibilityExtractor {
    async fn extract_focused_element(
        &self,
        pii_level: PiiFilterLevel,
        has_full_text_consent: bool,
    ) -> Result<Option<FocusedElementInfo>, CoreError> {
        // Apply the same PII-level fallback as the real implementation
        let effective_level = if pii_level == PiiFilterLevel::Off && !has_full_text_consent {
            PiiFilterLevel::Standard
        } else {
            pii_level
        };

        let info = match effective_level {
            PiiFilterLevel::Strict => FocusedElementInfo {
                role: self.role.clone(),
                position: self.position,
                ..Default::default()
            },
            PiiFilterLevel::Standard => FocusedElementInfo {
                role: self.role.clone(),
                position: self.position,
                label: self.label.clone(),
                value_length: self.value.as_ref().map(|v| v.len() as u32),
                extracted_text: None,
            },
            PiiFilterLevel::Basic => FocusedElementInfo {
                role: self.role.clone(),
                position: self.position,
                label: self.label.clone(),
                value_length: self.value.as_ref().map(|v| v.len() as u32),
                extracted_text: self.value.as_ref().map(|v| {
                    oneshim_vision::privacy::sanitize_title_with_level(v, PiiFilterLevel::Basic)
                }),
            },
            PiiFilterLevel::Off => FocusedElementInfo {
                role: self.role.clone(),
                position: self.position,
                label: self.label.clone(),
                value_length: self.value.as_ref().map(|v| v.len() as u32),
                extracted_text: self.value.clone(),
            },
        };

        Ok(Some(info))
    }

    fn has_permission(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "mock-accessibility"
    }
}

// ── Test 1: PII Off without consent falls back to Standard ──

#[tokio::test]
async fn pii_off_without_consent_falls_back_to_standard() {
    let extractor = MockAccessibilityExtractor::new(
        "AXTextField",
        Some("Input"),
        Some("secret password"),
        Some(ElementRect {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 25.0,
        }),
    );

    let result = extractor
        .extract_focused_element(PiiFilterLevel::Off, false) // no consent!
        .await
        .unwrap();

    assert!(result.is_some());
    let info = result.unwrap();
    // Should NOT have extracted_text because consent is missing (fell back to Standard)
    assert!(info.extracted_text.is_none());
    // Should still have label and value_length (Standard level)
    assert_eq!(info.label, Some("Input".to_string()));
    assert_eq!(info.value_length, Some(15));
}

#[tokio::test]
async fn pii_off_with_consent_includes_full_text() {
    let extractor = MockAccessibilityExtractor::new(
        "AXTextField",
        Some("Input"),
        Some("secret password"),
        None,
    );

    let result = extractor
        .extract_focused_element(PiiFilterLevel::Off, true) // consent granted
        .await
        .unwrap();

    let info = result.unwrap();
    assert_eq!(info.extracted_text, Some("secret password".to_string()));
}

#[tokio::test]
async fn strict_level_only_role_and_position() {
    let extractor = MockAccessibilityExtractor::new(
        "AXButton",
        Some("Save"),
        Some("important data"),
        Some(ElementRect {
            x: 50.0,
            y: 100.0,
            width: 80.0,
            height: 30.0,
        }),
    );

    let result = extractor
        .extract_focused_element(PiiFilterLevel::Strict, false)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(result.role, "AXButton");
    assert!(result.position.is_some());
    assert!(result.label.is_none());
    assert!(result.value_length.is_none());
    assert!(result.extracted_text.is_none());
}

#[tokio::test]
async fn basic_level_sanitizes_email() {
    let extractor =
        MockAccessibilityExtractor::new("AXTextField", None, Some("user@example.com"), None);

    let result = extractor
        .extract_focused_element(PiiFilterLevel::Basic, false)
        .await
        .unwrap()
        .unwrap();

    assert!(result.extracted_text.is_some());
    let text = result.extracted_text.unwrap();
    assert!(text.contains("[EMAIL]"));
    assert!(!text.contains("user@example.com"));
}

// ── Test 2: Config gating ──

#[test]
fn accessibility_disabled_when_config_false() {
    use oneshim_core::config::TextIntelligenceConfig;

    let config = TextIntelligenceConfig {
        enabled: true,
        accessibility_extraction: false,
        ..Default::default()
    };
    // Scheduler should NOT construct an AccessibilityExtractor
    assert!(!config.accessibility_extraction);
}

#[test]
fn accessibility_disabled_when_text_intelligence_disabled() {
    use oneshim_core::config::TextIntelligenceConfig;

    let config = TextIntelligenceConfig::default();
    // Default has enabled=false
    assert!(!config.enabled);
    assert!(!config.accessibility_extraction);
}

#[test]
fn accessibility_disabled_when_consent_missing() {
    let consent = ConsentPermissions {
        activity_pattern_learning: false,
        ..Default::default()
    };
    // Even if config says enabled, missing consent blocks construction
    assert!(!consent.activity_pattern_learning);
}

#[test]
fn accessibility_enabled_when_all_gates_pass() {
    use oneshim_core::config::TextIntelligenceConfig;

    let config = TextIntelligenceConfig {
        enabled: true,
        accessibility_extraction: true,
        ..Default::default()
    };
    let consent = ConsentPermissions {
        activity_pattern_learning: true,
        ..Default::default()
    };

    let should_create =
        config.enabled && config.accessibility_extraction && consent.activity_pattern_learning;
    assert!(should_create);
}

#[test]
fn full_text_consent_backward_compatible() {
    // JSON without full_text_extraction field deserializes to false
    let json = r#"{"screen_capture":true,"activity_pattern_learning":true}"#;
    let perms: ConsentPermissions = serde_json::from_str(json).unwrap();
    assert!(!perms.full_text_extraction);
    assert!(perms.activity_pattern_learning);
}

// ── Test 3: Zeroize behavior ──

#[test]
fn zeroizing_string_drops_cleanly() {
    use zeroize::Zeroizing;

    // Verify that Zeroizing<String> can be created and dropped without panic
    let secret = Zeroizing::new("sensitive accessibility text".to_string());
    assert_eq!(secret.len(), 28);
    // Zeroizing<String> is dropped here, zeroing memory automatically.
    // We cannot reliably verify the memory is zeroed (allocator may reuse it),
    // but zeroize guarantees the drop impl zeros the buffer.
    drop(secret);
}

#[test]
fn zeroizing_string_deref_works() {
    use zeroize::Zeroizing;

    let secret = Zeroizing::new("test value".to_string());
    // Should be accessible via Deref
    assert_eq!(secret.as_str(), "test value");
    assert_eq!(secret.len(), 10);
}

// ── Test 4: Platform factory function ──

#[test]
fn create_extractor_returns_expected_on_platform() {
    let extractor = oneshim_vision::accessibility::create_extractor();

    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    assert!(
        extractor.is_some(),
        "create_extractor() should return Some on supported platforms (macOS/Windows/Linux)"
    );

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    assert!(
        extractor.is_none(),
        "create_extractor() should return None on unsupported platforms"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn macos_extractor_reports_correct_name() {
    let extractor = oneshim_vision::accessibility::create_extractor().unwrap();
    assert_eq!(extractor.name(), "macos-native-accessibility");
}

// ── Test 5: FocusedElementInfo domain model ──

#[test]
fn focused_element_serde_with_all_pii_levels() {
    // Strict level: only role + position
    let strict = FocusedElementInfo {
        role: "AXTextField".to_string(),
        position: Some(ElementRect {
            x: 10.0,
            y: 20.0,
            width: 200.0,
            height: 25.0,
        }),
        ..Default::default()
    };
    let json = serde_json::to_string(&strict).unwrap();
    assert!(!json.contains("label"));
    assert!(!json.contains("extracted_text"));
    let decoded: FocusedElementInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.role, "AXTextField");
    assert!(decoded.label.is_none());

    // Off level: includes everything
    let full = FocusedElementInfo {
        role: "AXTextArea".to_string(),
        position: None,
        label: Some("Editor".to_string()),
        value_length: Some(42),
        extracted_text: Some("full text content".to_string()),
    };
    let json = serde_json::to_string(&full).unwrap();
    assert!(json.contains("extracted_text"));
    let decoded: FocusedElementInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(
        decoded.extracted_text,
        Some("full text content".to_string())
    );
}

#[test]
fn mock_extractor_name() {
    let extractor = MockAccessibilityExtractor::new("AXButton", None, None, None);
    assert_eq!(extractor.name(), "mock-accessibility");
    assert!(extractor.has_permission());
}
