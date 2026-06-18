//! Cfg-free PII-redaction core for accessibility focused-element extraction.
//!
//! The macOS and Windows extractors both produce a [`FocusedElementInfo`] and
//! apply the *same* per-`PiiFilterLevel` field gating to it; that gating is the
//! security boundary that decides what element text leaves the device. It used
//! to be re-implemented inline in each platform extractor **and** copied again
//! inside each platform's test module (so the tests validated a copy, not the
//! shipped code). This module is the single source of truth: each platform
//! resolves its own raw fields (label, value) then calls [`apply_pii_level`],
//! and the tests exercise that exact function — cross-platform, no Windows or
//! macOS runner required (#5120).
//!
//! Linux is intentionally NOT a caller: it emits a different shape
//! (`AccessibilityElement` = role/label/bounds, strict-suppress-label only),
//! not the four-level `FocusedElementInfo` redaction, so unifying it would be a
//! behavior change rather than an extraction.

// Called by both the macOS and Windows extractors, plus the cross-platform
// tests below. It is non-test-dead only on Linux/other targets, which emit a
// different element shape (see the module doc) and deliberately do not call it.
#![allow(dead_code)]

use maekon_core::config::PiiFilterLevel;
use maekon_core::models::focused_element::{ElementRect, FocusedElementInfo};

use crate::privacy::sanitize_title_with_level;

/// Apply the consent/PII-level field gating to one focused accessibility
/// element, producing the [`FocusedElementInfo`] that is allowed to leave the
/// device at `level`.
///
/// The caller pre-resolves `label` (e.g. macOS `title.or(placeholder)`, Windows
/// `name`) and passes the element's raw `value` text by reference (the caller
/// keeps ownership of the zeroizing buffer, which it drops right after). The
/// gating, from most to least restrictive:
///
/// - **Strict** — role + position ONLY. No label, no value length, no text.
/// - **Standard** — adds `label` and `value_length` (the value's UTF-8 **byte**
///   length, never its contents).
/// - **Basic** — additionally exposes `extracted_text`, run through
///   [`sanitize_title_with_level`] at `Basic` (PII patterns masked).
/// - **Off** — exposes `extracted_text` verbatim (only reachable with explicit
///   full-text consent; the caller is responsible for that gate).
pub(crate) fn apply_pii_level(
    role: String,
    label: Option<String>,
    value: Option<&str>,
    position: Option<ElementRect>,
    level: PiiFilterLevel,
) -> FocusedElementInfo {
    match level {
        PiiFilterLevel::Strict => FocusedElementInfo {
            role,
            position,
            ..Default::default()
        },
        PiiFilterLevel::Standard => FocusedElementInfo {
            role,
            position,
            // The accessibility name/title frequently equals user content (a mail
            // row's name is sender+subject; a contact row's is a phone/email), so
            // mask it at the configured level — it was previously passed verbatim
            // while `value`/extracted_text was masked, an in-function asymmetry that
            // contradicts the model contract. (review4 V15)
            label: label.map(|l| sanitize_title_with_level(&l, level)),
            value_length: value.map(|v| v.len() as u32),
            ..Default::default()
        },
        PiiFilterLevel::Basic => FocusedElementInfo {
            role,
            position,
            label: label.map(|l| sanitize_title_with_level(&l, level)),
            value_length: value.map(|v| v.len() as u32),
            extracted_text: value.map(|v| sanitize_title_with_level(v, PiiFilterLevel::Basic)),
        },
        PiiFilterLevel::Off => FocusedElementInfo {
            role,
            position,
            label,
            value_length: value.map(|v| v.len() as u32),
            extracted_text: value.map(|v| v.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> Option<ElementRect> {
        Some(ElementRect {
            x: 1.0,
            y: 2.0,
            width: 3.0,
            height: 4.0,
        })
    }

    // The core security contract: Strict leaks nothing but role + position.
    #[test]
    fn strict_exposes_only_role_and_position() {
        let info = apply_pii_level(
            "Edit".into(),
            Some("Search".into()),
            Some("secret@example.com"),
            rect(),
            PiiFilterLevel::Strict,
        );
        assert_eq!(info.role, "Edit");
        assert!(info.position.is_some());
        assert!(info.label.is_none(), "Strict must not leak the label");
        assert!(
            info.value_length.is_none(),
            "Strict must not leak even the value length"
        );
        assert!(
            info.extracted_text.is_none(),
            "Strict must never expose value text"
        );
    }

    #[test]
    fn standard_adds_label_and_length_but_no_text() {
        let info = apply_pii_level(
            "Edit".into(),
            Some("Search".into()),
            Some("hello"),
            rect(),
            PiiFilterLevel::Standard,
        );
        assert_eq!(info.label.as_deref(), Some("Search"));
        assert_eq!(info.value_length, Some(5));
        assert!(
            info.extracted_text.is_none(),
            "Standard must not expose value text"
        );
    }

    #[test]
    fn standard_masks_pii_in_label() {
        // review4 V15: the label (accessibility name) must be PII-masked at the
        // configured level, not passed through verbatim.
        let info = apply_pii_level(
            "Row".into(),
            Some("Re: invoice for client@example.com".into()),
            Some("x"),
            rect(),
            PiiFilterLevel::Standard,
        );
        let label = info.label.expect("Standard exposes label");
        assert!(
            !label.contains("client@example.com"),
            "label email must be masked: {label}"
        );
        assert!(
            label.contains("[EMAIL]"),
            "label must carry the [EMAIL] marker: {label}"
        );
    }

    #[test]
    fn basic_exposes_sanitized_text() {
        let info = apply_pii_level(
            "Edit".into(),
            Some("Search".into()),
            Some("plain text"),
            rect(),
            PiiFilterLevel::Basic,
        );
        assert_eq!(info.value_length, Some(10));
        assert!(
            info.extracted_text.is_some(),
            "Basic must expose (sanitized) text"
        );
    }

    #[test]
    fn basic_sanitizes_pii_so_email_is_masked() {
        // A value carrying an email must be MASKED at Basic, not merely altered:
        // assert the email is gone and the `[EMAIL]` marker is present, so a
        // regression that masked nothing (or only trimmed whitespace) fails.
        let raw = "contact me at jane.doe@example.com now";
        let info = apply_pii_level("Edit".into(), None, Some(raw), None, PiiFilterLevel::Basic);
        let text = info.extracted_text.expect("Basic exposes text");
        assert!(
            !text.contains("jane.doe@example.com"),
            "the email must NOT survive at Basic: {text}"
        );
        assert!(
            text.contains("[EMAIL]"),
            "the email must be masked to its [EMAIL] marker: {text}"
        );
    }

    #[test]
    fn role_and_position_survive_every_exposing_level() {
        // role + position are always emitted; a regression dropping position at
        // a non-Strict level would otherwise go uncaught (only Strict asserts it).
        for level in [
            PiiFilterLevel::Standard,
            PiiFilterLevel::Basic,
            PiiFilterLevel::Off,
        ] {
            let info = apply_pii_level("Edit".into(), Some("L".into()), Some("v"), rect(), level);
            assert_eq!(info.role, "Edit", "role must survive at {level:?}");
            assert!(
                info.position.is_some(),
                "position must survive at {level:?}"
            );
        }
    }

    #[test]
    fn off_exposes_value_verbatim() {
        let raw = "jane.doe@example.com";
        let info = apply_pii_level(
            "Edit".into(),
            Some("Email".into()),
            Some(raw),
            rect(),
            PiiFilterLevel::Off,
        );
        assert_eq!(
            info.extracted_text.as_deref(),
            Some(raw),
            "Off exposes the raw value (full-text consent already gated by caller)"
        );
        assert_eq!(info.value_length, Some(raw.len() as u32));
    }

    #[test]
    fn none_value_yields_no_length_or_text_at_any_exposing_level() {
        for level in [
            PiiFilterLevel::Standard,
            PiiFilterLevel::Basic,
            PiiFilterLevel::Off,
        ] {
            let info = apply_pii_level("Button".into(), Some("OK".into()), None, None, level);
            assert!(
                info.value_length.is_none(),
                "no value ⇒ no length ({level:?})"
            );
            assert!(
                info.extracted_text.is_none(),
                "no value ⇒ no text ({level:?})"
            );
            assert_eq!(info.label.as_deref(), Some("OK"));
        }
    }

    #[test]
    fn absent_label_stays_absent() {
        // A caller that resolved no label (no title/placeholder/name) must not
        // gain one from the filter.
        let info = apply_pii_level(
            "Text".into(),
            None,
            Some("x"),
            None,
            PiiFilterLevel::Standard,
        );
        assert!(info.label.is_none());
    }

    #[test]
    fn value_length_counts_utf8_bytes() {
        // len() is byte length; a 3-byte char (U+D55C, written as a \u escape to
        // keep this source ASCII) reports 3.
        let info = apply_pii_level(
            "Edit".into(),
            None,
            Some("\u{D55C}"),
            None,
            PiiFilterLevel::Standard,
        );
        assert_eq!(info.value_length, Some(3));
    }
}
