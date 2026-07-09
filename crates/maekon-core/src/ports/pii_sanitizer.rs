//! PII (Personally Identifiable Information) sanitization port.
//! Implemented by `VisionPiiSanitizer` in `maekon-vision`.

use crate::config::PiiFilterLevel;

/// Sanitizes text by replacing PII patterns with redaction markers.
///
/// # Errors
/// **Infallible.** `sanitize_text` returns `String` directly, not
/// `Result<_, _>`. Malformed input (invalid UTF-8 is already impossible
/// at the &str type level) or pattern-compilation failures inside the
/// impl are pre-baked via `OnceLock`/`once_cell` at construction, so
/// they cannot surface here. A filter level that recognizes nothing
/// simply returns the input unchanged.
pub trait PiiSanitizer: Send + Sync {
    /// Replace PII in `text` according to the given filter level.
    /// Returns sanitized text with markers like `[EMAIL]`, `[PHONE]`, `[USER]`.
    fn sanitize_text(&self, text: &str, level: PiiFilterLevel) -> String;
}

/// Canonical `PiiSanitizer` test double (#7729 ctd-W2 G2).
///
/// Before this, 6 workspace crates each hand-rolled their own `MockSanitizer`
/// with a DIFFERENT hardcoded replace list — meaning "the sanitizer redacted
/// PII" was asserted against 6 different, silently-diverging fixture sets. This
/// fake stays intentionally free of any hardcoded domain fixture strings (no
/// `maekon-core` type should know about a specific test's `alice@example.com`)
/// and instead exposes a builder so each call site declares exactly the
/// find/replace pairs its own assertions depend on — same ergonomics as the 6
/// hand-rolled copies, minus the duplicated boilerplate struct + impl block.
///
/// ```
/// use maekon_core::ports::pii_sanitizer::{FakePiiSanitizer, PiiSanitizer};
/// use maekon_core::config::PiiFilterLevel;
///
/// let sanitizer = FakePiiSanitizer::new()
///     .with_email("alice@example.com")
///     .with_phone("010-1234-5678");
/// assert_eq!(
///     sanitizer.sanitize_text("contact alice@example.com", PiiFilterLevel::Standard),
///     "contact [EMAIL]"
/// );
/// ```
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Clone, Default)]
pub struct FakePiiSanitizer {
    replacements: Vec<(String, String)>,
}

#[cfg(any(test, feature = "test-support"))]
impl FakePiiSanitizer {
    /// Identity sanitizer — no replacements registered. Useful for tests that
    /// only assert a sanitizer was *wired* (e.g. `.is_some()`), not its output.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a literal find/replace pair, applied in registration order.
    /// Chainable so a call site can declare its exact fixture set in one
    /// expression.
    pub fn with_replacement(mut self, find: impl Into<String>, replace: impl Into<String>) -> Self {
        self.replacements.push((find.into(), replace.into()));
        self
    }

    /// Convenience for the common "this email literal → `[EMAIL]`" case.
    pub fn with_email(self, email: impl Into<String>) -> Self {
        self.with_replacement(email, "[EMAIL]")
    }

    /// Convenience for the common "this phone literal → `[PHONE]`" case.
    pub fn with_phone(self, phone: impl Into<String>) -> Self {
        self.with_replacement(phone, "[PHONE]")
    }
}

#[cfg(any(test, feature = "test-support"))]
impl PiiSanitizer for FakePiiSanitizer {
    fn sanitize_text(&self, text: &str, _level: PiiFilterLevel) -> String {
        let mut sanitized = text.to_string();
        for (find, replace) in &self.replacements {
            sanitized = sanitized.replace(find.as_str(), replace.as_str());
        }
        sanitized
    }
}
