//! ADR-022 (client) prefix+ULID entity ID generation.
//!
//! The client ID generation convention is canonically defined by ADR-022 in the
//! maekon-client registry, and the prefix+ULID shape is identical to server
//! ADR-055.

use thiserror::Error;

/// Reason a `generate_id_checked` prefix validation failed.
///
/// Provided so callers handling dynamic (runtime) prefixes can deal with the
/// error without panicking. Existing callers that only use static literal
/// prefixes keep using [`generate_id`] as-is (#4344).
#[derive(Debug, Error, PartialEq, Eq)]
pub enum IdError {
    /// The ADR-022 id prefix violates the rules (empty / longer than 63 bytes /
    /// does not start with a lowercase ASCII letter / contains characters
    /// outside `a-z`, `0-9`, `_`).
    #[error("invalid ADR-022 id prefix `{prefix}`")]
    InvalidPrefix { prefix: String },
}

/// Generates a `{prefix}_{ULID}` identifier.
///
/// The Rust client keeps the ADR-022 prefix+ULID shape (identical to the
/// server-side ADR-055 shape) while allowing client-local prefixes that are
/// not part of the server domain registry.
///
/// # Static-prefix only (#4344)
///
/// This function is **only for static literal prefixes known at compile time**.
/// Every caller passes a verified static literal such as `"sug"` or `"req"`, so
/// there is no real possibility of a panic. If you need to handle a dynamic
/// prefix decided at runtime, use the non-panicking [`generate_id_checked`].
///
/// # Panics
///
/// Panics when `prefix` is empty, longer than 63 bytes, does not start with a
/// lowercase ASCII letter, or contains characters outside `a-z`, `0-9`, and
/// `_`.
#[must_use]
pub fn generate_id(prefix: &str) -> String {
    assert!(
        is_valid_prefix(prefix),
        "invalid ADR-022 id prefix `{prefix}`"
    );
    format!("{prefix}_{}", ulid::Ulid::generate())
}

/// Non-panicking variant of [`generate_id`] (#4344).
///
/// Provided for future callers that handle dynamic prefixes decided at runtime.
/// If the prefix violates the ADR-022 rules, it returns
/// [`IdError::InvalidPrefix`] instead of panicking.
///
/// # Errors
///
/// Returns [`IdError::InvalidPrefix`] if `prefix` is empty, longer than 63
/// bytes, does not start with a lowercase ASCII letter, or contains characters
/// outside `a-z`/`0-9`/`_`.
pub fn generate_id_checked(prefix: &str) -> Result<String, IdError> {
    if is_valid_prefix(prefix) {
        Ok(format!("{prefix}_{}", ulid::Ulid::generate()))
    } else {
        Err(IdError::InvalidPrefix {
            prefix: prefix.to_owned(),
        })
    }
}

/// ADR-022 prefix rule validation (pure function shared by both the panic and
/// Result paths).
fn is_valid_prefix(prefix: &str) -> bool {
    let starts_with_lowercase = prefix
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_lowercase);
    let chars_are_supported = prefix
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');

    starts_with_lowercase && prefix.len() <= 63 && chars_are_supported
}

#[cfg(test)]
mod tests {
    use super::{generate_id, generate_id_checked, IdError};

    /// The complete list of static literal prefixes actually passed to
    /// `generate_id(prefix)` across the entire workspace (#4344).
    ///
    /// The set of registered/used prefixes extracted via
    /// `grep -rhoE 'generate_id\("[^"]+"\)'`. When introducing a new prefix,
    /// update this list too so the registry does not drift into a panic.
    const USED_PREFIXES: &[&str] = &[
        "ann",
        "aud",
        "audit",
        "cch",
        "clip",
        "clm",
        "consent",
        "ctx",
        "edg",
        "env",
        "erasure",
        "evt",
        "fa",
        "flow",
        "hl",
        "input",
        "notification_activation",
        "ovr",
        "pomo",
        "proc",
        "ptr",
        "q",
        "rcpt",
        "rect",
        "req",
        "scene",
        "ses",
        "skact",
        "sug",
        "tcand",
        "tkt",
        "tmut",
        "todo",
        "wctx",
        "win",
    ];

    #[test]
    fn generate_id_uses_prefix_and_ulid_shape() {
        let id = generate_id("sug");

        assert!(id.starts_with("sug_"));
        assert_eq!(id.len(), "sug_".len() + 26);
        assert!(id["sug_".len()..]
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase()));
    }

    #[test]
    fn generate_id_uniqueness() {
        let a = generate_id("req");
        let b = generate_id("req");
        assert_ne!(
            a, b,
            "consecutive generate_id calls must produce different IDs"
        );
    }

    #[test]
    #[should_panic(expected = "invalid ADR-022 id prefix")]
    fn generate_id_rejects_uppercase_prefixes() {
        let _ = generate_id("Sug");
    }

    /// #4344 regression guard: every static prefix actually in use must not
    /// panic in `generate_id` (blocks the registry from drifting into a panic).
    #[test]
    fn generate_id_does_not_panic_on_any_used_prefix() {
        for &prefix in USED_PREFIXES {
            let id = generate_id(prefix);
            assert!(
                id.starts_with(&format!("{prefix}_")),
                "generate_id({prefix:?}) must yield a `{prefix}_`-prefixed id"
            );
            assert_eq!(
                id.len(),
                prefix.len() + 1 + 26,
                "generate_id({prefix:?}) must be `{{prefix}}_{{26-char ULID}}`"
            );
        }
    }

    /// #4344: `generate_id_checked` must return Ok for every prefix in use and
    /// produce an id of the same shape as `generate_id`.
    #[test]
    fn generate_id_checked_accepts_all_used_prefixes() {
        for &prefix in USED_PREFIXES {
            let id = generate_id_checked(prefix)
                .unwrap_or_else(|e| panic!("checked variant rejected used prefix {prefix:?}: {e}"));
            assert!(id.starts_with(&format!("{prefix}_")));
            assert_eq!(id.len(), prefix.len() + 1 + 26);
        }
    }

    /// #4344: `generate_id_checked` does not panic on a bad prefix and returns
    /// [`IdError::InvalidPrefix`] instead.
    #[test]
    fn generate_id_checked_rejects_invalid_prefixes_without_panic() {
        for bad in [
            "Sug",
            "",
            "1abc",
            "has space",
            "tooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooo_long",
        ] {
            let err = generate_id_checked(bad).expect_err("invalid prefix must error");
            assert_eq!(
                err,
                IdError::InvalidPrefix {
                    prefix: bad.to_owned()
                }
            );
        }
    }

    /// #4344: asserts that for a prefix that passes validation the panic variant
    /// and the checked variant behave identically (both share the same
    /// `is_valid_prefix`).
    #[test]
    fn generate_id_checked_uniqueness() {
        let a = generate_id_checked("req").expect("valid prefix");
        let b = generate_id_checked("req").expect("valid prefix");
        assert_ne!(a, b, "consecutive generate_id_checked calls must differ");
    }
}
