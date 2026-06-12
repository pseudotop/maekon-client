//! Shared privacy helpers for dashboard gRPC payloads.

use maekon_core::config::PiiFilterLevel;
use maekon_core::ports::pii_sanitizer::PiiSanitizer;

pub(crate) fn sanitize_dashboard_text(
    raw: String,
    pii_sanitizer: Option<&dyn PiiSanitizer>,
) -> String {
    match pii_sanitizer {
        Some(sanitizer) => sanitizer.sanitize_text(&raw, PiiFilterLevel::Standard),
        None => raw,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MarkerSanitizer;

    impl PiiSanitizer for MarkerSanitizer {
        fn sanitize_text(&self, text: &str, _level: PiiFilterLevel) -> String {
            text.replace("secret@example.com", "[EMAIL]")
                .replace("Acme Roadmap", "[TITLE]")
        }
    }

    #[test]
    fn sanitize_dashboard_text_uses_configured_sanitizer() {
        let sanitized = sanitize_dashboard_text(
            "Acme Roadmap - secret@example.com".to_string(),
            Some(&MarkerSanitizer),
        );

        assert_eq!(sanitized, "[TITLE] - [EMAIL]");
    }

    #[test]
    fn sanitize_dashboard_text_preserves_text_without_sanitizer() {
        let raw = "Notes - local fixture".to_string();

        assert_eq!(sanitize_dashboard_text(raw.clone(), None), raw);
    }
}
