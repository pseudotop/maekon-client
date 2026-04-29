//! PII-sanitizing `Display` wrapper.
//!
//! Wraps any `T: Display` value so its formatted output is routed through a
//! [`PiiSanitizer`] port before being written to the underlying `Formatter`.
//! This is the canonical pattern for scrubbing `CoreError` bodies (and other
//! user-text-carrying `Display` types) in tracing/log sinks without modifying
//! the derived `thiserror` `Display` impls.
//!
//! # Why a wrapper instead of changing `CoreError`'s `Display`?
//!
//! `CoreError` derives `Display` via `thiserror` across 38 variants. Replacing
//! those with manually sanitized impls would require either a macro (heavy),
//! a runtime sanitizer registry (global state), or a fallback policy when no
//! sanitizer is present. The wrapper sidesteps all three: the sanitizer is
//! passed explicitly at the formatting site, errors remain fully expressive
//! internally, and sanitization is opt-in per log site.
//!
//! # Usage
//!
//! ```ignore
//! use oneshim_core::config::PiiFilterLevel;
//! use oneshim_core::ports::PiiSanitizer;
//! use oneshim_core::sanitized;
//!
//! fn log_failure(err: &dyn std::error::Error, san: &dyn PiiSanitizer) {
//!     tracing::warn!(
//!         err.code = "internal.io",
//!         "task failed: {}",
//!         sanitized(err, san, PiiFilterLevel::Standard),
//!     );
//! }
//! ```
//!
//! # Fast path
//!
//! When `level == PiiFilterLevel::Off`, the wrapper forwards directly to the
//! inner `Display` and skips the `to_string()`/`sanitize_text()` round trip.

use core::fmt::{self, Display};

use crate::config::PiiFilterLevel;
use crate::ports::pii_sanitizer::PiiSanitizer;

/// `Display` wrapper that sanitizes the inner value's formatted output.
///
/// Construct with [`SanitizedDisplay::new`] or the [`sanitized`] helper.
pub struct SanitizedDisplay<'a, T: Display + ?Sized> {
    inner: &'a T,
    sanitizer: &'a dyn PiiSanitizer,
    level: PiiFilterLevel,
}

impl<'a, T: Display + ?Sized> SanitizedDisplay<'a, T> {
    /// Wrap `inner` so its `Display` output is sanitized by `sanitizer` at the
    /// given `level`.
    #[must_use]
    pub fn new(inner: &'a T, sanitizer: &'a dyn PiiSanitizer, level: PiiFilterLevel) -> Self {
        Self {
            inner,
            sanitizer,
            level,
        }
    }
}

impl<T: Display + ?Sized> Display for SanitizedDisplay<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Level-Off fast path: no sanitizer allocation, direct forwarding.
        if self.level == PiiFilterLevel::Off {
            return Display::fmt(self.inner, f);
        }
        let raw = self.inner.to_string();
        let scrubbed = self.sanitizer.sanitize_text(&raw, self.level);
        f.write_str(&scrubbed)
    }
}

/// Ergonomic constructor: `sanitized(&err, sanitizer, level)`.
///
/// Equivalent to [`SanitizedDisplay::new`].
#[must_use]
pub fn sanitized<'a, T: Display + ?Sized>(
    value: &'a T,
    sanitizer: &'a dyn PiiSanitizer,
    level: PiiFilterLevel,
) -> SanitizedDisplay<'a, T> {
    SanitizedDisplay::new(value, sanitizer, level)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test double: replaces any occurrence of the sensitive token `"PII"`
    /// with `"[REDACTED]"` when level is non-Off. This is intentionally
    /// simpler than `VisionPiiSanitizer` so tests focus on the wrapper's
    /// contract, not regex coverage.
    struct TokenReplacingSanitizer {
        sensitive: &'static str,
        replacement: &'static str,
    }

    impl PiiSanitizer for TokenReplacingSanitizer {
        fn sanitize_text(&self, text: &str, level: PiiFilterLevel) -> String {
            if level == PiiFilterLevel::Off {
                return text.to_string();
            }
            text.replace(self.sensitive, self.replacement)
        }
    }

    fn sanitizer() -> TokenReplacingSanitizer {
        TokenReplacingSanitizer {
            sensitive: "PII",
            replacement: "[REDACTED]",
        }
    }

    #[test]
    fn off_level_is_passthrough() {
        let san = sanitizer();
        let raw = "contains PII leak";
        let wrapped = sanitized(&raw, &san, PiiFilterLevel::Off);
        assert_eq!(wrapped.to_string(), "contains PII leak");
    }

    #[test]
    fn standard_level_invokes_sanitizer() {
        let san = sanitizer();
        let raw = "contains PII leak";
        let wrapped = sanitized(&raw, &san, PiiFilterLevel::Standard);
        assert_eq!(wrapped.to_string(), "contains [REDACTED] leak");
    }

    #[test]
    fn strict_level_invokes_sanitizer() {
        let san = sanitizer();
        let raw = "PII at start";
        let wrapped = sanitized(&raw, &san, PiiFilterLevel::Strict);
        assert_eq!(wrapped.to_string(), "[REDACTED] at start");
    }

    #[test]
    fn basic_level_invokes_sanitizer() {
        let san = sanitizer();
        let raw = "trailing PII";
        let wrapped = sanitized(&raw, &san, PiiFilterLevel::Basic);
        assert_eq!(wrapped.to_string(), "trailing [REDACTED]");
    }

    #[test]
    fn works_with_core_error_display() {
        use crate::error::CoreError;
        use crate::error_codes::ConfigCode;

        let err = CoreError::Config {
            code: ConfigCode::Invalid,
            message: "bad PII value".into(),
        };
        let san = sanitizer();
        let wrapped = sanitized(&err, &san, PiiFilterLevel::Standard);
        let out = wrapped.to_string();
        assert!(
            out.contains("[config.invalid]"),
            "wire code preserved: {out}"
        );
        assert!(out.contains("[REDACTED]"), "message sanitized: {out}");
        assert!(!out.contains("PII"), "raw PII leaked through: {out}");
    }

    #[test]
    fn works_with_format_macro() {
        let san = sanitizer();
        let raw = "PII goes here";
        let formatted = format!("log: {}", sanitized(&raw, &san, PiiFilterLevel::Standard));
        assert_eq!(formatted, "log: [REDACTED] goes here");
    }

    #[test]
    fn new_and_sanitized_helper_are_equivalent() {
        let san = sanitizer();
        let raw = "PII content";
        let via_new = SanitizedDisplay::new(&raw, &san, PiiFilterLevel::Standard).to_string();
        let via_helper = sanitized(&raw, &san, PiiFilterLevel::Standard).to_string();
        assert_eq!(via_new, via_helper);
    }

    #[test]
    fn works_with_dyn_error() {
        let san = sanitizer();
        let err: Box<dyn std::error::Error> = Box::new(std::io::Error::other("PII in io message"));
        // Display is implemented for dyn Error, so the wrapper accepts it.
        let wrapped = sanitized(err.as_ref(), &san, PiiFilterLevel::Standard);
        let out = wrapped.to_string();
        assert_eq!(out, "[REDACTED] in io message");
    }
}
