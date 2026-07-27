//! Shared secret (credential) redaction helper — single source of truth (SSOT).
//!
//! Detects and masks **prefix-less** arbitrary high-entropy secrets (random
//! passwords, DB-connection-string passwords, JWTs, Google/Slack tokens, etc.)
//! using known credential-token prefixes (`sk-`, `ghp_`, `AKIA`, `xoxb-` …) plus
//! a per-character Shannon-entropy heuristic.
//!
//! The pattern-based PII sanitizer (email/phone/card/SSN …) cannot recognize a
//! prefix-less arbitrary high-entropy secret, so this helper closes that gap.
//!
//! This module lives in `maekon-core` so the clipboard preview
//! (`maekon-monitor`) and the OCR/window-title redaction pipeline
//! (`maekon-vision`) can **share the exact same implementation**.
//! `maekon-monitor` cannot depend on `maekon-vision` under the hexagonal rule,
//! so — for the same reason as `path_redaction` (user-path masking) — the
//! shared implementation lives in `maekon-core`. Previously this heuristic only
//! existed on the clipboard path and had never been ported to the redaction
//! pipeline protecting OCR/AX text. (#8041)

/// Known credential token prefixes (case-sensitive).
///
/// A whitespace-delimited token starting with one of these (and long enough)
/// is treated as a secret. The cased entries (`AKIA`, `ghp_`, `xoxb-`, …) are
/// matched case-sensitively, mirroring the real token formats so ordinary
/// words like "skill" or "apiserver" are not falsely flagged.
pub const SECRET_TOKEN_PREFIXES: [&str; 17] = [
    "sk-",
    "pk-",
    "sk_",
    "pk_",
    "api_",
    "key_",
    "token_",
    "secret_",
    "AKIA",
    "ghp_",
    "gho_",
    "ghs_",
    "github_pat_",
    "xoxb-",
    "xoxp-",
    "xoxa-",
    "xoxr-",
];

/// Minimum token length shared by the prefix and high-entropy heuristics.
/// Avoids masking short false positives (a bare `sk-`, `api_`, or a short word).
pub const MIN_SECRET_LEN: usize = 12;

/// Minimum Shannon entropy (bits/char) for the high-entropy password
/// heuristic. A 12-char string of distinct characters has ~3.58 bits/char;
/// ordinary words fall well below this once combined with the character-class
/// requirement.
pub const MIN_SECRET_ENTROPY_BITS: f64 = 3.0;

/// Minimum distinct character classes (lowercase / uppercase / digit / symbol)
/// for the high-entropy heuristic. Ordinary prose words rarely mix three or
/// more classes, which keeps the false-positive rate low.
pub const MIN_SECRET_CHAR_CLASSES: u32 = 3;

/// Replace whitespace-delimited tokens in `text` that are classified as
/// secrets with `marker`.
///
/// Whitespace (and newlines) are preserved so the downstream pattern
/// sanitizer still sees the original structure of the remaining text.
/// `marker` is caller-specified (the clipboard preview uses
/// `[REDACTED_SECRET]`; OCR/title redaction uses `[API_KEY]`).
#[must_use]
pub fn redact_secrets_with_marker(text: &str, marker: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut token = String::new();
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !token.is_empty() {
                out.push_str(&redact_secret_token(&token, marker));
                token.clear();
            }
            out.push(ch);
        } else {
            token.push(ch);
        }
    }
    if !token.is_empty() {
        out.push_str(&redact_secret_token(&token, marker));
    }
    out
}

/// Classify a single whitespace-delimited token: returns `marker` when it
/// looks like a secret, otherwise the original token.
#[must_use]
pub fn redact_secret_token(token: &str, marker: &str) -> String {
    if token.chars().count() >= MIN_SECRET_LEN
        && SECRET_TOKEN_PREFIXES
            .iter()
            .any(|prefix| token.starts_with(prefix))
    {
        return marker.to_string();
    }
    if looks_like_high_entropy_secret(token) {
        return marker.to_string();
    }
    token.to_string()
}

/// Heuristic for a randomly-generated credential: long enough, mixing several
/// character classes, and with high per-character Shannon entropy.
///
/// Structural non-secret tokens are excluded up front to avoid false
/// positives:
///   - Paths/URLs (`/`, `\`) and already-redacted tokens (marker `[`) are
///     handled by a dedicated masker (e.g. `mask_user_paths`); this entropy
///     fallback runs LAST in the pipeline and must not overwrite their output
///     (`[USER]`/`[IP]`/…).
///   - Tokens containing `:` (IPv6, `IP:port`, URLs, `HH:MM:SS`,
///     `key:value`) are structural identifiers and are excluded.
///
/// Leading/trailing punctuation is also trimmed before classification so a
/// label-shaped token (`Authorization:`, `TODO-`) does not get a false
/// positive third character class purely from a trailing symbol.
#[must_use]
pub fn looks_like_high_entropy_secret(token: &str) -> bool {
    if token.contains('/') || token.contains('\\') || token.contains('[') || token.contains(':') {
        return false;
    }
    let core = token.trim_matches(|c: char| !c.is_alphanumeric());
    core.chars().count() >= MIN_SECRET_LEN
        && character_class_count(core) >= MIN_SECRET_CHAR_CLASSES
        && shannon_entropy_bits(core) >= MIN_SECRET_ENTROPY_BITS
}

/// Count the distinct character classes present in `token`: lowercase,
/// uppercase, digit, and other (symbol / punctuation).
fn character_class_count(token: &str) -> u32 {
    let mut lower = false;
    let mut upper = false;
    let mut digit = false;
    let mut symbol = false;
    for ch in token.chars() {
        if ch.is_lowercase() {
            lower = true;
        } else if ch.is_uppercase() {
            upper = true;
        } else if ch.is_numeric() {
            digit = true;
        } else {
            symbol = true;
        }
    }
    u32::from(lower) + u32::from(upper) + u32::from(digit) + u32::from(symbol)
}

/// Shannon entropy of `token` in bits per character.
fn shannon_entropy_bits(token: &str) -> f64 {
    let chars: Vec<char> = token.chars().collect();
    let len = chars.len();
    if len == 0 {
        return 0.0;
    }
    let mut counts: std::collections::HashMap<char, usize> = std::collections::HashMap::new();
    for ch in &chars {
        *counts.entry(*ch).or_insert(0) += 1;
    }
    let len_f = len as f64;
    counts
        .values()
        .map(|&count| {
            let p = count as f64 / len_f;
            -p * p.log2()
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    const M: &str = "[REDACTED_SECRET]";

    #[test]
    fn prefix_path_redacts_known_token_even_when_low_entropy() {
        // A `ghp_` token of repeated characters has low entropy and only two
        // character classes, so the entropy path skips it — the prefix path
        // must still catch it.
        assert_eq!(redact_secret_token("ghp_aaaaaaaaaaaaaaaaaa", M), M);
        assert_eq!(redact_secret_token("AKIAIOSFODNN7EXAMPLE", M), M);
    }

    #[test]
    fn entropy_path_flags_random_mixed_class_token_only() {
        assert!(looks_like_high_entropy_secret("xQ9!mB2#vL8z"));
        // A long single-class word is not a secret (too few classes).
        assert!(!looks_like_high_entropy_secret("internationalization"));
        // A short token is not a secret regardless of class mix.
        assert!(!looks_like_high_entropy_secret("aB3$"));
    }

    #[test]
    fn marker_is_caller_specified() {
        // The same logic must be reusable with different markers (SSOT).
        assert_eq!(
            redact_secret_token("xQ9!mB2#vL8z", "[API_KEY]"),
            "[API_KEY]"
        );
        assert_eq!(
            redact_secret_token("xQ9!mB2#vL8z", "[REDACTED_SECRET]"),
            "[REDACTED_SECRET]"
        );
    }

    #[test]
    fn redact_secrets_preserves_ordinary_text_and_whitespace() {
        let input = "Meeting at noon tomorrow with the team";
        assert_eq!(redact_secrets_with_marker(input, M), input);
    }

    #[test]
    fn redact_secrets_masks_prefixless_secret_in_context() {
        // A prefix-less high-entropy token must be masked in context, while
        // the surrounding prose and whitespace are preserved.
        let out = redact_secrets_with_marker("db pw is Xy9$Kw2mQ8vT next", M);
        assert!(!out.contains("Xy9$Kw2mQ8vT"), "secret leaked: {out}");
        assert!(out.contains(M), "marker missing: {out}");
        assert!(out.starts_with("db pw is "), "prose lost: {out}");
        assert!(out.ends_with(" next"), "prose lost: {out}");
    }
}
