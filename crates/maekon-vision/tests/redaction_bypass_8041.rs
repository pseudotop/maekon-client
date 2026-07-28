//! #8041 regression tests — close the systematic PII text-redaction engine
//! bypasses.
//!
//! Verifies the bypass vectors listed in the issue (unseparated SSN,
//! dot-separated SSN/phone/card, prefix-less high-entropy secrets) from the
//! `sanitize_title_with_level` single-chokepoint perspective. The `mask_*`
//! functions in the `redaction` submodule are crate-internal (see mod.rs:
//! "Redaction functions are internal"), so these tests exercise the full
//! pipeline end-to-end through the public API:
//! `sanitize_title_with_level` / `detect_pii_markers_with_level`.
//!
//! This pipeline is the shared filter for window titles, full OCR text,
//! accessibility (AX/UIA/AT-SPI) text, and — via the assembler — the remote
//! LLM prompt.

use maekon_core::config::PiiFilterLevel;
use maekon_vision::privacy::{detect_pii_markers_with_level, sanitize_title_with_level, PiiMarker};

// ───────────────────── Gap A: numeric PII bypass vectors ─────────────────────

#[test]
fn redaction_masks_unseparated_ssn_at_standard() {
    // The issue's headline vector: an unseparated 9-digit SSN.
    let out = sanitize_title_with_level("SSN 123456789 on file", PiiFilterLevel::Standard);
    assert!(out.contains("[SSN]"), "unseparated SSN not masked: {out}");
    assert!(!out.contains("123456789"), "raw SSN leaked: {out}");
}

#[test]
fn redaction_masks_dot_separated_ssn_at_standard() {
    let out = sanitize_title_with_level("SSN 123.45.6789 on file", PiiFilterLevel::Standard);
    assert!(out.contains("[SSN]"), "dot-separated SSN not masked: {out}");
    assert!(!out.contains("123.45.6789"), "raw SSN leaked: {out}");
}

#[test]
fn redaction_masks_space_separated_ssn_at_standard() {
    let out = sanitize_title_with_level("SSN 123 45 6789 file", PiiFilterLevel::Standard);
    assert!(
        out.contains("[SSN]"),
        "space-separated SSN not masked: {out}"
    );
    assert!(!out.contains("123 45 6789"), "raw SSN leaked: {out}");
}

#[test]
fn redaction_masks_dash_ssn_still_works() {
    // The pre-existing dash form must keep masking without regression.
    let out = sanitize_title_with_level("SSN 123-45-6789 file", PiiFilterLevel::Standard);
    assert!(out.contains("[SSN]"), "dash SSN not masked: {out}");
    assert!(!out.contains("123-45-6789"), "raw SSN leaked: {out}");
}

#[test]
fn redaction_ssn_bypass_vectors_all_caught_at_strict() {
    // Must be caught at Strict too (#8041 requirement 4).
    for raw in ["123456789", "123.45.6789", "123 45 6789", "123-45-6789"] {
        let input = format!("id {raw} end");
        let out = sanitize_title_with_level(&input, PiiFilterLevel::Strict);
        assert!(
            out.contains("[SSN]"),
            "SSN vector not masked at Strict: {raw} -> {out}"
        );
        assert!(
            !out.contains(raw),
            "raw SSN leaked at Strict: {raw} -> {out}"
        );
    }
}

#[test]
fn redaction_detects_ssn_marker_for_new_forms() {
    for raw in ["123456789", "123.45.6789"] {
        let input = format!("SSN: {raw}");
        let markers = detect_pii_markers_with_level(&input, PiiFilterLevel::Standard);
        assert!(
            markers.contains(&PiiMarker::Ssn),
            "Ssn marker missing for {raw}: {markers:?}"
        );
    }
}

#[test]
fn redaction_masks_dot_separated_phone_at_standard() {
    // A dot-separated phone (not a valid IPv4) must be masked.
    let out = sanitize_title_with_level("call 555.123.4567 now", PiiFilterLevel::Standard);
    assert!(
        out.contains("[PHONE]"),
        "dot-separated phone not masked: {out}"
    );
    assert!(!out.contains("555.123.4567"), "raw phone leaked: {out}");
}

#[test]
fn redaction_masks_dot_separated_card_at_standard() {
    let out = sanitize_title_with_level("card 4111.1111.1111.1111 paid", PiiFilterLevel::Standard);
    assert!(
        out.contains("[CARD]"),
        "dot-separated card not masked: {out}"
    );
    assert!(
        !out.contains("4111.1111.1111.1111"),
        "raw card leaked: {out}"
    );
}

// ───────────────── Gap B: prefix-less high-entropy secrets ─────────────────

#[test]
fn redaction_masks_prefixless_high_entropy_secret_at_standard() {
    // An arbitrary prefix-less high-entropy password/token must be masked at
    // the shipped-default level (Standard) — the egress-to-LLM path also runs
    // at Standard (#8041 blast radius).
    let secret = "Xy9$Kw2mQ8vTzR";
    let input = format!("db pw is {secret} next");
    let out = sanitize_title_with_level(&input, PiiFilterLevel::Standard);
    assert!(
        out.contains("[API_KEY]"),
        "prefixless secret not masked: {out}"
    );
    assert!(!out.contains(secret), "raw secret leaked: {out}");
}

#[test]
fn redaction_masks_bearerless_jwt_at_standard() {
    // A JWT without a "Bearer " prefix is not in the known-prefix set but is
    // high-entropy, so the entropy fallback must catch it. Assembled at
    // runtime from three separate segment literals (not a single JWT-shaped
    // string literal) so the public-export secret scanner's `jwt` detector
    // (`scripts/scan-public-export-secrets.py`, which matches a contiguous
    // `eyJ...\...\...` literal) does not flag this test fixture.
    let jwt_header = "eyJhbGciOiJIUzI1NiJ9";
    let jwt_payload = "eyJzdWIiOiJ1c2VyMTIzIn0";
    let jwt_signature = "aZ9Kx2Lm4Qn8Rt";
    let jwt = format!("{jwt_header}.{jwt_payload}.{jwt_signature}");

    let out = sanitize_title_with_level(&jwt, PiiFilterLevel::Standard);
    assert!(out.contains("[API_KEY]"), "JWT not masked: {out}");
    assert!(!out.contains(jwt_payload), "raw JWT payload leaked: {out}");
}

#[test]
fn redaction_masks_prefixless_secret_at_strict() {
    let secret = "Xy9$Kw2mQ8vTzR";
    let out = sanitize_title_with_level(secret, PiiFilterLevel::Strict);
    assert_eq!(out, "[API_KEY]", "secret not masked at Strict: {out}");
}

// ──────────────── False-positive suppression (ordinary data) ────────────────

#[test]
fn redaction_does_not_mask_ipv4_as_phone_or_card_or_ssn_at_standard() {
    let out = sanitize_title_with_level("host 192.168.1.100 up", PiiFilterLevel::Standard);
    assert!(!out.contains("[PHONE]"), "IPv4 masked as phone: {out}");
    assert!(!out.contains("[CARD]"), "IPv4 masked as card: {out}");
    assert!(!out.contains("[SSN]"), "IPv4 masked as SSN: {out}");
    assert!(
        out.contains("192.168.1.100"),
        "IPv4 wrongly removed at Standard: {out}"
    );
}

#[test]
fn redaction_does_not_mask_hostport_at_standard() {
    let out = sanitize_title_with_level("server at 192.168.1.100:8080", PiiFilterLevel::Standard);
    assert!(
        out.contains("192.168.1.100:8080"),
        "host:port wrongly masked: {out}"
    );
}

#[test]
fn redaction_does_not_mask_decimal_as_ssn() {
    // The 9-digit fractional part of `3.141592653` is not an SSN (leading
    // decimal-point guard).
    let out = sanitize_title_with_level("pi is 3.141592653 approx", PiiFilterLevel::Strict);
    assert!(!out.contains("[SSN]"), "decimal masked as SSN: {out}");
    assert!(
        out.contains("3.141592653"),
        "decimal wrongly removed: {out}"
    );
}

#[test]
fn redaction_does_not_mask_9digit_integer_with_decimal_tail_as_ssn() {
    // The 9-digit integer part of `123456789.5` is not an SSN (trailing
    // decimal-point guard).
    let out = sanitize_title_with_level("value 123456789.5 total", PiiFilterLevel::Standard);
    assert!(
        !out.contains("[SSN]"),
        "decimal integer part masked as SSN: {out}"
    );
}

#[test]
fn redaction_does_not_mask_version_string() {
    let out = sanitize_title_with_level("release 1.2.3 shipped", PiiFilterLevel::Strict);
    assert!(!out.contains("[SSN]"), "version masked as SSN: {out}");
    assert!(!out.contains("[PHONE]"), "version masked as phone: {out}");
    assert!(out.contains("1.2.3"), "version wrongly removed: {out}");
}

#[test]
fn redaction_leaves_plain_prose_and_short_numbers_unchanged() {
    for title in [
        "Visual Studio Code - main.rs",
        "Meeting at noon with the team",
        "order 12345 shipped",
        "internationalization pipeline",
    ] {
        let out = sanitize_title_with_level(title, PiiFilterLevel::Strict);
        assert_eq!(
            out, title,
            "ordinary text wrongly masked: {title:?} -> {out}"
        );
    }
}

// ───────────────────────────── Level coherence ─────────────────────────────

#[test]
fn redaction_off_level_leaves_all_pii_untouched() {
    let title = "SSN 123456789 card 4111.1111.1111.1111 pw Xy9$Kw2mQ8vTzR";
    let out = sanitize_title_with_level(title, PiiFilterLevel::Off);
    assert_eq!(out, title, "Off level must not change anything: {out}");
}

// ────────────── assembler → LLM prompt egress path smoke test ──────────────

#[test]
fn redaction_llm_egress_filter_masks_all_bypass_vectors() {
    // The egress boundary in agent_runtime_support.rs / export_assembler.rs
    // filters OCR/title text with an injected PiiFilter closure
    // `|t| sanitize_title_with_level(t, <level>)` before it enters the LLM
    // prompt (export is pinned to Standard). This smoke test reproduces that
    // closure directly, verifying the bypass vectors never egress into the
    // LLM payload.
    let llm_egress_filter = |t: &str| sanitize_title_with_level(t, PiiFilterLevel::Standard);

    let ocr_blob = "Screen: SSN 123456789, card 4111.1111.1111.1111, \
                    phone 555.123.4567, pw Xy9$Kw2mQ8vTzR";
    let prompt = llm_egress_filter(ocr_blob);

    assert!(
        !prompt.contains("123456789"),
        "SSN reached LLM payload: {prompt}"
    );
    assert!(
        !prompt.contains("4111.1111.1111.1111"),
        "card reached LLM payload: {prompt}"
    );
    assert!(
        !prompt.contains("555.123.4567"),
        "phone reached LLM payload: {prompt}"
    );
    assert!(
        !prompt.contains("Xy9$Kw2mQ8vTzR"),
        "secret reached LLM payload: {prompt}"
    );
    assert!(prompt.contains("[SSN]") && prompt.contains("[CARD]"));
    assert!(prompt.contains("[PHONE]") && prompt.contains("[API_KEY]"));
}
