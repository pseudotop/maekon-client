//! PII (Personally Identifiable Information) sanitization port.
//! Implemented by `VisionPiiSanitizer` in `oneshim-vision`.

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
