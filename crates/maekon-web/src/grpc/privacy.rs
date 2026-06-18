//! Shared privacy helpers for dashboard gRPC payloads.

use maekon_core::config::PiiFilterLevel;
use maekon_core::ports::pii_sanitizer::PiiSanitizer;

pub(crate) fn sanitize_dashboard_text(
    raw: String,
    pii_sanitizer: Option<&dyn PiiSanitizer>,
) -> String {
    match pii_sanitizer {
        Some(sanitizer) => sanitizer.sanitize_text(&raw, PiiFilterLevel::Standard),
        // #6421: fail CLOSED. `None` means no PII sanitizer is wired (NOT "the user
        // disabled filtering" — that is expressed through the sanitizer's own level).
        // Dashboard app/window text can carry PII, so redact rather than emit raw,
        // matching the fail-closed posture of the REST export/search paths.
        None => "[redacted: PII sanitizer unavailable]".to_string(),
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
    fn sanitize_dashboard_text_redacts_without_sanitizer() {
        // #6421: fail closed — an absent sanitizer must NOT pass raw text through.
        let raw = "Notes - local fixture".to_string();
        let out = sanitize_dashboard_text(raw.clone(), None);
        assert_ne!(out, raw);
        assert!(out.contains("redacted"));
    }
}
