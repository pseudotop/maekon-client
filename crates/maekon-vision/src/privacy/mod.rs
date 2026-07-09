// ADR-013: privacy module split (was 1243 lines)
// Responsibilities:
//   detection.rs — PiiMarker enum, sanitize_title_*, detect_pii_markers_*, should_exclude, sensitive app keywords
//   redaction.rs — all mask_* and try_parse_* implementation functions
//   adapter.rs   — VisionPiiSanitizer (PiiSanitizer port impl)

mod adapter;
mod detection;
mod redaction;

// Public surface — all callers use `maekon_vision::privacy::{...}`
pub use adapter::VisionPiiSanitizer;
pub use detection::{
    detect_pii_markers_with_level, is_sensitive_app, is_sensitive_segment_with_level,
    matches_exclusion_pattern, sanitize_title, sanitize_title_with_level, should_exclude,
    should_exclude_by_policy, PiiMarker, SENSITIVE_APP_KEYWORDS, SENSITIVE_TITLE_SUBSTRINGS,
};
// Redaction functions are internal; only the detection layer exposes the public API.
// Individual mask_* functions used within detection.rs are pub within the crate.

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::config::PiiFilterLevel;
    use std::time::Instant;

    fn perf_gates_enabled() -> bool {
        std::env::var("MAEKON_ENABLE_PERF_GATES")
            .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    }

    fn perf_budget_ms(env_key: &str, default_ms: u128) -> u128 {
        std::env::var(env_key)
            .ok()
            .and_then(|value| value.parse::<u128>().ok())
            .unwrap_or(default_ms)
    }

    #[test]
    fn sanitize_email() {
        let result = sanitize_title("Login - user@example.com");
        assert!(result.contains("[EMAIL]"));
        assert!(!result.contains("user@example.com"));
    }

    #[test]
    fn sanitize_macos_path() {
        let result = sanitize_title("File: /Users/johndoe/Documents/secret.txt");
        assert!(result.contains("[USER]"));
        assert!(!result.contains("johndoe"));
    }

    #[test]
    fn sanitize_windows_path() {
        let result = sanitize_title(r"File: C:\Users\johndoe\Documents\secret.txt");
        assert!(result.contains("[USER]"));
    }

    #[test]
    fn no_pii_unchanged() {
        let title = "Visual Studio Code - main.rs";
        assert_eq!(sanitize_title(title), title);
    }

    #[test]
    fn level_off_no_filter() {
        let title = "user@example.com - 010-1234-5678";
        let result = sanitize_title_with_level(title, PiiFilterLevel::Off);
        assert_eq!(result, title);
    }

    #[test]
    fn level_basic_email_and_phone() {
        let result =
            sanitize_title_with_level("user@example.com 010-1234-5678", PiiFilterLevel::Basic);
        assert!(result.contains("[EMAIL]"));
        assert!(result.contains("[PHONE]"));
    }

    #[test]
    fn level_strict_masks_api_keys() {
        let result =
            sanitize_title_with_level("key: sk-abc123def456ghi789jkl0", PiiFilterLevel::Strict);
        assert!(result.contains("[API_KEY]"));
        assert!(!result.contains("sk-abc123"));
    }

    #[test]
    fn level_standard_masks_api_keys() {
        // #7100: API tokens must be masked at the shipped-default Standard level,
        // not only at Strict. (Before the fix this leaked the raw token.)
        let result =
            sanitize_title_with_level("key: sk-abc123def456ghi789jkl0", PiiFilterLevel::Standard);
        assert!(result.contains("[API_KEY]"), "got: {result}");
        assert!(
            !result.contains("sk-abc123"),
            "raw API token leaked: {result}"
        );
    }

    #[test]
    fn level_basic_does_not_mask_api_keys() {
        // #7100 guard: the API-key mask is scoped to Standard and stricter. Basic
        // (email + phone only) must not start masking tokens.
        let result =
            sanitize_title_with_level("key: sk-abc123def456ghi789jkl0", PiiFilterLevel::Basic);
        assert!(!result.contains("[API_KEY]"), "got: {result}");
        assert!(result.contains("sk-abc123def456ghi789jkl0"));
    }

    #[test]
    fn level_standard_does_not_mask_ip_or_passport() {
        // #7100 guard: only mask_api_keys moves to Standard. IP and passport
        // remain Strict-only.
        let result =
            sanitize_title_with_level("server at 192.168.1.100:8080", PiiFilterLevel::Standard);
        assert!(
            !result.contains("[IP]"),
            "IP wrongly masked at Standard: {result}"
        );
        assert!(result.contains("192.168.1.100"));
    }

    #[test]
    fn level_strict_masks_ip() {
        let result =
            sanitize_title_with_level("server at 192.168.1.100:8080", PiiFilterLevel::Strict);
        assert!(result.contains("[IP]"));
        assert!(!result.contains("192.168.1.100"));
    }

    #[test]
    fn detect_sensitive_apps() {
        assert!(is_sensitive_app("1Password"));
        assert!(is_sensitive_app("Bitwarden"));
        assert!(is_sensitive_app("KB Banking"));
        assert!(is_sensitive_app("Google Authenticator"));
        assert!(!is_sensitive_app("Visual Studio Code"));
        assert!(!is_sensitive_app("Chrome"));
    }

    #[test]
    fn exclusion_pattern_glob() {
        let patterns = vec!["*bank*".to_string(), "Discord".to_string()];
        assert!(matches_exclusion_pattern("KB Banking", &patterns));
        assert!(matches_exclusion_pattern("Discord", &patterns));
        assert!(!matches_exclusion_pattern("Chrome", &patterns));
    }

    #[test]
    fn should_exclude_comprehensive() {
        assert!(should_exclude(
            "1Password",
            "Unlock",
            &[],
            &[],
            &[],
            true, // auto_exclude_sensitive
        ));
        assert!(!should_exclude("Chrome", "Google", &[], &[], &[], true,));
        assert!(should_exclude(
            "Slack",
            "General",
            &["Slack".to_string()],
            &[],
            &[],
            false,
        ));
        assert!(should_exclude(
            "Chrome",
            "My Password Manager",
            &[],
            &[],
            &["*password*".to_string()],
            false,
        ));
    }

    /// #7909: the `PrivacyConfig` wrapper must wire every policy field into
    /// [`should_exclude`] — list, patterns, and the auto-sensitive flag.
    #[test]
    fn should_exclude_by_policy_wires_all_config_fields() {
        use maekon_core::config::PrivacyConfig;

        let listed = PrivacyConfig {
            excluded_apps: vec!["Slack".to_string()],
            ..Default::default()
        };
        assert!(should_exclude_by_policy(&listed, "Slack", "General"));
        assert!(!should_exclude_by_policy(&listed, "Chrome", "Google"));

        // auto_exclude_sensitive defaults on and flows through the wrapper…
        assert!(should_exclude_by_policy(
            &PrivacyConfig::default(),
            "1Password",
            "Unlock"
        ));
        // …and opting out disables the auto-detection.
        let opted_out = PrivacyConfig {
            auto_exclude_sensitive: false,
            ..Default::default()
        };
        assert!(!should_exclude_by_policy(&opted_out, "1Password", "Unlock"));

        let title_patterns = PrivacyConfig {
            excluded_title_patterns: vec!["*password*".to_string()],
            auto_exclude_sensitive: false,
            ..Default::default()
        };
        assert!(should_exclude_by_policy(
            &title_patterns,
            "Chrome",
            "My Password Manager"
        ));

        let app_patterns = PrivacyConfig {
            excluded_app_patterns: vec!["*bank*".to_string()],
            auto_exclude_sensitive: false,
            ..Default::default()
        };
        assert!(should_exclude_by_policy(&app_patterns, "KB Banking", ""));
    }

    #[test]
    fn mask_korean_phone() {
        let result = redaction::mask_phone_numbers("call 010-1234-5678 now");
        assert!(result.contains("[PHONE]"));
        assert!(!result.contains("010-1234-5678"));
    }

    #[test]
    fn mask_international_phone() {
        let result = redaction::mask_phone_numbers("reach +82-10-1234-5678");
        assert!(result.contains("[PHONE]"));
    }

    #[test]
    fn mask_ipv4_basic() {
        let result = redaction::mask_ip_addresses("connecting to 10.0.0.1 on port 80");
        assert!(result.contains("[IP]"));
        assert!(!result.contains("10.0.0.1"));
    }

    #[test]
    fn mask_ipv4_preserves_non_ip() {
        let result = redaction::mask_ip_addresses("version 1.2.3");
        assert!(!result.contains("[IP]"));
    }

    #[test]
    fn mask_ipv6_full() {
        // Mask a fully-written IPv6 address
        let result =
            redaction::mask_ip_addresses("host 2001:0db8:85a3:0000:0000:8a2e:0370:7334 up");
        assert!(result.contains("[IP]"));
        assert!(!result.contains("2001:0db8"));
    }

    #[test]
    fn mask_ipv6_compressed() {
        // Mask a `::`-compressed IPv6 address
        let result = redaction::mask_ip_addresses("connecting to 2001:db8::1 now");
        assert!(result.contains("[IP]"));
        assert!(!result.contains("2001:db8::1"));
    }

    #[test]
    fn mask_ipv6_loopback() {
        // Mask the loopback address (`::1`)
        let result = redaction::mask_ip_addresses("loopback ::1 alive");
        assert!(result.contains("[IP]"));
    }

    #[test]
    fn mask_ipv6_strict_level() {
        // Verify that IPv6 addresses are masked at the Strict level (Strict contract)
        let result =
            sanitize_title_with_level("server at fe80::a00:27ff:fe4e:66a1", PiiFilterLevel::Strict);
        assert!(result.contains("[IP]"));
        assert!(!result.contains("fe80::a00"));
    }

    #[test]
    fn mask_ipv6_preserves_non_ip() {
        // A single-colon token (e.g. a time-of-day notation) is not IPv6, so preserve it
        let result = redaction::mask_ip_addresses("time 12:30 done");
        assert!(!result.contains("[IP]"));
    }

    #[test]
    fn mask_linux_home_path() {
        let result = redaction::mask_user_paths("/home/devuser/projects/secret.txt");
        assert!(result.contains("[USER]"));
        assert!(!result.contains("devuser"));
    }

    #[test]
    fn mask_user_paths_no_trailing_separator() {
        assert_eq!(redaction::mask_user_paths("/Users/alice"), "/Users/[USER]");
        assert_eq!(redaction::mask_user_paths("/home/bob"), "/home/[USER]");
        assert_eq!(
            redaction::mask_user_paths(r"C:\Users\carol"),
            r"C:\Users\[USER]"
        );
    }

    #[test]
    fn mask_user_paths_followed_by_space() {
        assert_eq!(
            redaction::mask_user_paths("path /Users/alice next"),
            "path /Users/[USER] next"
        );
    }

    #[test]
    fn mask_user_paths_multiple_users() {
        let input = "/Users/alice/docs and /Users/bob/file";
        let result = redaction::mask_user_paths(input);
        assert_eq!(result, "/Users/[USER]/docs and /Users/[USER]/file");
    }

    #[test]
    fn mask_user_paths_idempotent_on_already_masked() {
        let input = "/Users/[USER]/already-masked";
        let result = redaction::mask_user_paths(input);
        assert_eq!(result, "/Users/[USER]/already-masked");
    }

    #[test]
    fn detect_pii_markers_in_email() {
        let markers =
            detect_pii_markers_with_level("contact: user@example.com", PiiFilterLevel::Standard);
        assert!(markers.contains(&PiiMarker::Email));
        assert!(is_sensitive_segment_with_level(
            "contact: user@example.com",
            PiiFilterLevel::Standard
        ));
    }

    #[test]
    fn no_markers_when_filter_off() {
        let markers =
            detect_pii_markers_with_level("contact: user@example.com", PiiFilterLevel::Off);
        assert!(markers.is_empty());
        assert!(!is_sensitive_segment_with_level(
            "contact: user@example.com",
            PiiFilterLevel::Off
        ));
    }

    #[test]
    fn perf_budget_privacy_marker_scan() {
        if !perf_gates_enabled() {
            return;
        }

        let samples = [
            "contact user@example.com for access",
            "call +82-10-1234-5678 for approval",
            "server 10.0.0.24 has alert",
            "token sk-abc123def456ghi789jkl0 detected",
            "normal productivity dashboard event",
            "file path /Users/alice/Documents/client-plan.md",
        ];

        let iterations = 3_000usize;
        let budget_ms = perf_budget_ms("MAEKON_PERF_BUDGET_PRIVACY_SCAN_MS", 1_200);
        let started = Instant::now();
        let mut hits = 0usize;
        for _ in 0..iterations {
            for sample in samples {
                if is_sensitive_segment_with_level(sample, PiiFilterLevel::Strict) {
                    hits += 1;
                }
            }
        }
        let elapsed_ms = started.elapsed().as_millis();

        assert!(
            hits >= iterations * 5,
            "unexpected marker scan hit ratio: hits={} iterations={}",
            hits,
            iterations
        );
        assert!(
            elapsed_ms <= budget_ms,
            "privacy marker scan perf budget exceeded: elapsed={}ms budget={}ms",
            elapsed_ms,
            budget_ms
        );
    }

    #[test]
    fn mask_bearer_single_token() {
        let input = "Authorization: Bearer eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9";
        let result = sanitize_title_with_level(input, PiiFilterLevel::Strict);
        assert!(
            result.contains("Bearer [API_KEY]"),
            "single bearer not masked"
        );
        assert!(!result.contains("eyJhbGci"), "raw token still present");
    }

    #[test]
    fn mask_bearer_multiple_tokens() {
        // Verifies that all bearer tokens in a string are masked, not just the first.
        let input = "first: Bearer eyJhbGciOiJSUzI1NiJ9 second: bearer zyxwvutsrqponmlkjih end";
        let result = sanitize_title_with_level(input, PiiFilterLevel::Strict);
        let count = result.matches("[API_KEY]").count();
        assert_eq!(count, 2, "expected 2 masked tokens, got {count}");
        assert!(
            !result.contains("eyJhbGci"),
            "first raw token still present"
        );
        assert!(
            !result.contains("zyxwvuts"),
            "second raw token still present"
        );
    }

    #[test]
    fn mask_bearer_case_insensitive() {
        let variations = [
            "BEARER ABCDEFGHIJKLMNOPQRST",
            "Bearer ABCDEFGHIJKLMNOPQRST",
            "bearer ABCDEFGHIJKLMNOPQRST",
            "BeArEr ABCDEFGHIJKLMNOPQRST",
        ];
        for input in &variations {
            let result = sanitize_title_with_level(input, PiiFilterLevel::Strict);
            assert!(
                result.contains("[API_KEY]"),
                "case variant not masked: {input}"
            );
        }
    }

    #[test]
    fn mask_bearer_short_token_not_masked() {
        // Tokens shorter than 8 chars must not be replaced.
        let input = "Authorization: Bearer short";
        let result = sanitize_title_with_level(input, PiiFilterLevel::Strict);
        assert!(
            !result.contains("[API_KEY]"),
            "short bearer should not be masked"
        );
    }

    #[test]
    fn mask_private_key_block_single() {
        let input = format!(
            "key: -----BEGIN RSA {}-----\nMIIEowIBAAKCAQEA\n-----END RSA {}-----",
            "PRIVATE KEY", "PRIVATE KEY"
        );
        let result = sanitize_title_with_level(&input, PiiFilterLevel::Strict);
        assert!(result.contains("[PRIVATE_KEY]"), "PEM block not masked");
        assert!(
            !result.contains("MIIEowIBAAKCAQEA"),
            "raw key material still present"
        );
    }

    #[test]
    fn mask_private_key_block_non_private_unchanged() {
        // Public key blocks must not be masked.
        let input = "-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkq\n-----END PUBLIC KEY-----";
        let result = sanitize_title_with_level(input, PiiFilterLevel::Strict);
        assert!(
            !result.contains("[PRIVATE_KEY]"),
            "public key should not be masked"
        );
    }

    #[test]
    fn mask_ghs_token() {
        // ghs_ GitHub Actions token prefix added in this PR.
        let result = sanitize_title_with_level(
            "not-a-real token: ghs_16C7e42F292c6912E7710c838347Ae178B4a",
            PiiFilterLevel::Strict,
        );
        assert!(result.contains("[API_KEY]"), "ghs_ token not masked");
        assert!(!result.contains("ghs_"), "raw ghs_ token still present");
    }

    #[test]
    fn mask_ssn_standard_level() {
        let result =
            sanitize_title_with_level("SSN is 123-45-6789 on file", PiiFilterLevel::Standard);
        assert!(result.contains("[SSN]"), "SSN not masked");
        assert!(!result.contains("123-45-6789"), "raw SSN still present");
    }

    #[test]
    fn mask_ssn_not_at_basic_level() {
        let result = sanitize_title_with_level("SSN is 123-45-6789 on file", PiiFilterLevel::Basic);
        assert!(
            !result.contains("[SSN]"),
            "SSN should not be masked at Basic level"
        );
    }

    #[test]
    fn mask_ssn_does_not_match_longer_numbers() {
        // Preceded or followed by digits should not match
        let result = sanitize_title_with_level("code 1123-45-67890 test", PiiFilterLevel::Standard);
        assert!(
            !result.contains("[SSN]"),
            "should not match embedded number"
        );
    }

    #[test]
    fn detect_pii_markers_ssn() {
        let markers = detect_pii_markers_with_level("SSN: 987-65-4321", PiiFilterLevel::Standard);
        assert!(markers.contains(&PiiMarker::Ssn));
    }

    // ── IBAN masking tests ──────────────────────────────────────────────

    #[test]
    fn mask_iban_standard_level() {
        let result = sanitize_title_with_level(
            "IBAN: DE89370400440532013000 for payment",
            PiiFilterLevel::Standard,
        );
        assert!(result.contains("[IBAN]"), "IBAN not masked");
        assert!(
            !result.contains("0532013000"),
            "raw IBAN body still present"
        );
        // Country code + check digits preserved
        assert!(result.contains("DE89"), "IBAN prefix should be preserved");
    }

    #[test]
    fn mask_iban_formatted_with_spaces() {
        let result = sanitize_title_with_level(
            "transfer to GB29 NWBK 6016 1331 9268 19",
            PiiFilterLevel::Standard,
        );
        assert!(result.contains("[IBAN]"), "spaced IBAN not masked");
    }

    #[test]
    fn mask_iban_not_at_basic_level() {
        let result =
            sanitize_title_with_level("IBAN: DE89370400440532013000", PiiFilterLevel::Basic);
        assert!(
            !result.contains("[IBAN]"),
            "IBAN should not be masked at Basic level"
        );
    }

    #[test]
    fn mask_iban_too_short_not_matched() {
        // Only 10 alnum chars after country+check — below minimum 11
        let result = sanitize_title_with_level("code: AB12CDEF123456", PiiFilterLevel::Standard);
        assert!(
            !result.contains("[IBAN]"),
            "short code should not be matched as IBAN"
        );
    }

    #[test]
    fn detect_pii_markers_iban() {
        let markers = detect_pii_markers_with_level(
            "IBAN: FR7630006000011234567890189",
            PiiFilterLevel::Standard,
        );
        assert!(markers.contains(&PiiMarker::Iban));
    }

    // ── Passport masking tests ────────────────────────────────────────

    #[test]
    fn mask_passport_strict_level() {
        let result =
            sanitize_title_with_level("passport: A12345678 issued 2024", PiiFilterLevel::Strict);
        assert!(result.contains("[PASSPORT]"), "passport not masked");
        assert!(!result.contains("A12345678"), "raw passport still present");
    }

    #[test]
    fn mask_passport_seven_digits() {
        let result = sanitize_title_with_level("doc M1234567 verified", PiiFilterLevel::Strict);
        assert!(result.contains("[PASSPORT]"), "7-digit passport not masked");
    }

    #[test]
    fn mask_passport_not_at_standard_level() {
        let result =
            sanitize_title_with_level("passport: A12345678 issued 2024", PiiFilterLevel::Standard);
        assert!(
            !result.contains("[PASSPORT]"),
            "passport should not be masked at Standard level"
        );
    }

    #[test]
    fn mask_passport_mid_word_not_matched() {
        // "ABC12345678" — preceded by letters, should not match
        let result = sanitize_title_with_level("codeABC12345678 end", PiiFilterLevel::Strict);
        assert!(
            !result.contains("[PASSPORT]"),
            "mid-word should not match passport"
        );
    }

    #[test]
    fn detect_pii_markers_passport() {
        let markers =
            detect_pii_markers_with_level("passport number: B87654321", PiiFilterLevel::Strict);
        assert!(markers.contains(&PiiMarker::Passport));
    }

    #[test]
    fn vision_pii_sanitizer_trait_delegates() {
        use maekon_core::ports::pii_sanitizer::PiiSanitizer;

        let sanitizer = VisionPiiSanitizer;
        let result =
            sanitizer.sanitize_text("email: user@example.com path", PiiFilterLevel::Standard);
        assert!(result.contains("[EMAIL]"));
        assert!(!result.contains("user@example.com"));
    }

    // ── Credit card masking unit tests ─────────────────────────────────

    #[test]
    fn mask_credit_cards_contiguous() {
        assert_eq!(redaction::mask_credit_cards("4111111111111111"), "[CARD]");
    }

    #[test]
    fn mask_credit_cards_spaced() {
        assert_eq!(
            redaction::mask_credit_cards("Card: 4111 1111 1111 1111"),
            "Card: [CARD]"
        );
    }

    #[test]
    fn mask_credit_cards_hyphenated() {
        assert_eq!(
            redaction::mask_credit_cards("4111-1111-1111-1111"),
            "[CARD]"
        );
    }

    #[test]
    fn mask_credit_cards_mixed_text() {
        assert_eq!(
            redaction::mask_credit_cards("Call 1234567890 or card 4111111111111111 today"),
            "Call 1234567890 or card [CARD] today"
        );
    }

    #[test]
    fn mask_credit_cards_phone_not_masked() {
        assert_eq!(
            redaction::mask_credit_cards("Call 1234567890"),
            "Call 1234567890"
        );
    }

    #[test]
    fn mask_credit_cards_multiple() {
        let input = "Cards: 4111111111111111 and 5500000000000004";
        let result = redaction::mask_credit_cards(input);
        assert!(!result.contains("4111111111111111"));
        assert!(!result.contains("5500000000000004"));
    }

    #[test]
    fn mask_credit_cards_short_sequence() {
        assert_eq!(redaction::mask_credit_cards("123456789012"), "123456789012");
    }

    // ── #6003: encrypted-messaging / password / 2FA app keyword coverage ──

    #[test]
    fn sensitive_app_messaging_apps() {
        // Encrypted messaging apps added in #6003
        assert!(is_sensitive_app("Telegram"), "Telegram not detected");
        assert!(is_sensitive_app("WhatsApp"), "WhatsApp not detected");
        assert!(is_sensitive_app("Threema"), "Threema not detected");
        assert!(is_sensitive_app("Session"), "Session not detected");
        assert!(is_sensitive_app("Wire"), "Wire not detected");
    }

    #[test]
    fn sensitive_app_encrypted_email() {
        // Encrypted email clients added in #6003
        assert!(is_sensitive_app("Protonmail"), "Protonmail not detected");
        assert!(
            is_sensitive_app("ProtonMail"),
            "ProtonMail (mixed case) not detected"
        );
        assert!(is_sensitive_app("Tutanota"), "Tutanota not detected");
    }

    #[test]
    fn sensitive_app_2fa_authy() {
        // 2FA apps added in #6003
        assert!(is_sensitive_app("Authy"), "Authy not detected");
    }

    #[test]
    fn sensitive_app_category_hardening_covers_non_seeded_apps() {
        assert!(
            is_sensitive_app("Keeper"),
            "password manager outside the original seed list must be detected"
        );
        assert!(
            is_sensitive_app("Standard Notes"),
            "encrypted notes app must be detected"
        );
        assert!(
            is_sensitive_app("Evergreen Credit Union"),
            "regional financial apps without bank keyword must be detected"
        );
        assert!(
            is_sensitive_app("Acme EMR"),
            "health record apps must be detected"
        );
        assert!(
            is_sensitive_app("Robinhood"),
            "brokerage apps outside the original exchange list must be detected"
        );
        assert!(
            !is_sensitive_app("Bookkeeper Studio"),
            "whole-token matching must not flag unrelated app names"
        );
    }

    #[test]
    fn should_exclude_browser_title_protonmail() {
        // Browser (Chrome) showing Proton Mail tab — app is not sensitive,
        // but the title should trigger exclusion when auto_exclude_sensitive=true.
        assert!(
            should_exclude(
                "Google Chrome",
                "Proton Mail - Inbox — Google Chrome",
                &[],
                &[],
                &[],
                true,
            ),
            "browser title containing protonmail should be excluded"
        );
    }

    #[test]
    fn should_exclude_browser_title_tutanota() {
        assert!(
            should_exclude(
                "Firefox",
                "Tutanota — Encrypted Email — Mozilla Firefox",
                &[],
                &[],
                &[],
                true,
            ),
            "browser title containing tutanota should be excluded"
        );
    }

    #[test]
    fn should_exclude_browser_title_not_leaked_when_auto_disabled() {
        // With auto_exclude_sensitive=false the title alone must NOT exclude.
        assert!(
            !should_exclude(
                "Google Chrome",
                "Proton Mail - Inbox — Google Chrome",
                &[],
                &[],
                &[],
                false,
            ),
            "browser title exclusion must be opt-in (auto_exclude_sensitive=false)"
        );
    }

    #[test]
    fn should_exclude_normal_browser_title_not_excluded() {
        // A regular browser tab must not be excluded.
        assert!(
            !should_exclude(
                "Google Chrome",
                "GitHub — Build software better, together",
                &[],
                &[],
                &[],
                true,
            ),
            "non-sensitive browser title should not be excluded"
        );
    }

    #[test]
    fn should_exclude_browser_title_finance_health_legal() {
        // #6826: finance / health / brokerage / payment web tabs must be
        // suppressed via title even though the host app ("Chrome") is generic.
        for title in [
            "Coinbase – Buy & Sell Crypto — Google Chrome",
            "Robinhood - Portfolio — Google Chrome",
            "MyChart - Test Results — Google Chrome",
            "Fidelity - Account Summary — Google Chrome",
            "Open a brokerage account — Google Chrome",
            "DocuSign - Sign Document — Google Chrome",
        ] {
            assert!(
                should_exclude("Google Chrome", title, &[], &[], &[], true),
                "sensitive finance/health browser title should be excluded: {title:?}"
            );
        }
    }

    #[test]
    fn should_exclude_browser_title_precision_no_token_false_positives() {
        // #6826: the title path uses high-precision product names + category
        // phrases ONLY — bare category tokens (tax/legal/health) and generic
        // finance keywords (bank/session) must NOT over-exclude ordinary tabs.
        for title in [
            "Quarterly tax notes - Google Docs",
            "Legal Studies 101 Syllabus — Google Chrome",
            "Health and fitness blog — Google Chrome",
            "Online banking textbook.pdf — Google Chrome",
            "Work session recap — Notion",
            // Compact-substring artifacts that must NOT match (product "e trade"
            // compacts to "etrade"; whole-word matching avoids these).
            "live trade room — Google Chrome",
            "The Trade Desk - Campaign Dashboard — Google Chrome",
        ] {
            assert!(
                !should_exclude("Google Chrome", title, &[], &[], &[], true),
                "ordinary title must not be excluded by a bare token: {title:?}"
            );
        }
    }

    // ── review4 privacy masker regression tests ─────────────────────────

    #[test]
    fn phone_us_and_intl_format_masked() {
        // V2: numbers not starting with '+'/'0' must mask.
        for input in [
            "call 415-555-1234 now",
            "415 555 1234",
            "1-800-555-1234",
            "ring 20 7946 0958 ok",
        ] {
            let r = redaction::mask_phone_numbers(input);
            assert!(r.contains("[PHONE]"), "not masked: {input}");
        }
    }

    #[test]
    fn phone_separatorless_masked() {
        // V14: separator-less numbers must mask.
        let r = redaction::mask_phone_numbers("call 01012345678 now");
        assert!(r.contains("[PHONE]"));
        assert!(!r.contains("01012345678"));
        assert!(redaction::mask_phone_numbers("5551234567").contains("[PHONE]"));
    }

    #[test]
    fn phone_preserves_trailing_separator() {
        // V7: the trailing separator/punctuation must not be absorbed into [PHONE].
        assert_eq!(
            redaction::mask_phone_numbers("call 010-1234-5678 now"),
            "call [PHONE] now"
        );
        assert_eq!(
            redaction::mask_phone_numbers("Call 010-1234-5678. Done"),
            "Call [PHONE]. Done"
        );
    }

    #[test]
    fn phone_adjacent_numbers_both_masked_no_residual() {
        // V7: two adjacent phones each mask, with no leaked residual digits.
        assert_eq!(
            redaction::mask_phone_numbers("010-111-2222 010-333-4444"),
            "[PHONE] [PHONE]"
        );
    }

    #[test]
    fn phone_does_not_eat_ipv4_at_standard() {
        // Regression: the broadened phone scanner must not mask an IPv4 ('.' is not a
        // phone separator); at Standard the IP is left intact (IP masking is Strict).
        let r = sanitize_title_with_level("host 192.168.1.100 up", PiiFilterLevel::Standard);
        assert!(!r.contains("[PHONE]"), "IPv4 wrongly masked as phone: {r}");
        assert!(
            r.contains("192.168.1.100"),
            "IPv4 should survive at Standard"
        );
    }

    #[test]
    fn card_takes_precedence_over_phone_at_standard() {
        // Regression: a 16-digit card masks as [CARD], not [PHONE], at Standard
        // (numeric PII runs before the broadened phone scanner).
        let r =
            sanitize_title_with_level("card 4111 1111 1111 1111 paid", PiiFilterLevel::Standard);
        assert!(r.contains("[CARD]"), "card not masked: {r}");
        assert!(!r.contains("[PHONE]"), "card wrongly masked as phone: {r}");
    }

    #[test]
    fn iban_preserves_trailing_words() {
        // V6: trailing plain words after an IBAN must not be absorbed/deleted.
        let r = redaction::mask_iban("IBAN DE89370400440532013000 ok");
        assert!(r.contains("[IBAN]"));
        assert!(r.contains("ok"), "trailing word deleted: {r}");
        let r2 = redaction::mask_iban("transfer GB29 NWBK 6016 1331 9268 19 paid today");
        assert!(r2.contains("[IBAN]"));
        assert!(r2.contains("paid today"), "trailing words deleted: {r2}");
    }

    #[test]
    fn api_key_bearer_single_pass_correct_and_bounded() {
        // V1/V5: single-pass O(N) sanitize stays correct + bounded on large inputs
        // dense in bearer/key tokens (the previous O(N^2) impl took seconds here).
        let big_bearer = "Authorization: Bearer tokenABCDEFGHIJ\n".repeat(20_000);
        let big_keys = "export AKIAabcdefghij1234 \n".repeat(20_000);
        let started = Instant::now();
        let b = redaction::mask_api_keys(&big_bearer);
        let k = redaction::mask_api_keys(&big_keys);
        let elapsed_ms = started.elapsed().as_millis();
        assert!(b.contains("Bearer [API_KEY]") && !b.contains("tokenABCDEFGHIJ"));
        assert!(k.contains("[API_KEY]") && !k.contains("AKIAabcdefghij1234"));
        if perf_gates_enabled() {
            let budget = perf_budget_ms("MAEKON_PERF_BUDGET_APIKEY_MS", 500);
            assert!(
                elapsed_ms <= budget,
                "api-key/bearer sanitize O(N^2) regression: {elapsed_ms}ms > {budget}ms"
            );
        }
    }

    #[test]
    fn korean_id_and_private_key_single_pass_correct_and_bounded() {
        // review4 V1/V5 sibling: mask_korean_id + mask_private_key_blocks are now
        // single-pass O(N) (were O(N^2) rebuild-per-match). private-key masking runs
        // inside mask_api_keys (the Strict hot path V5 only half-closed).
        let kr = redaction::mask_korean_id("RRN 901010-1234567 end");
        assert!(
            kr.contains("[KR_ID]") && !kr.contains("901010-1234567"),
            "KR-ID not masked: {kr}"
        );
        assert!(
            kr.contains("RRN") && kr.contains("end"),
            "adjacent text dropped: {kr}"
        );
        let pem = sanitize_title_with_level(
            "not-a-real k: -----BEGIN RSA PRIVATE KEY-----\nABCDEFGH\n-----END RSA PRIVATE KEY----- tail",
            PiiFilterLevel::Strict,
        );
        assert!(
            pem.contains("[PRIVATE_KEY]") && !pem.contains("ABCDEFGH"),
            "PEM not masked: {pem}"
        );
        assert!(pem.contains("tail"), "trailing text dropped: {pem}");

        let big_kr = "901010-1234567 ".repeat(20_000);
        let big_pem =
            "not-a-real -----BEGIN RSA PRIVATE KEY-----\nX\n-----END RSA PRIVATE KEY-----\n"
                .repeat(20_000);
        let started = Instant::now();
        let k = redaction::mask_korean_id(&big_kr);
        let p = redaction::mask_api_keys(&big_pem);
        let elapsed_ms = started.elapsed().as_millis();
        assert!(k.contains("[KR_ID]") && !k.contains("901010-1234567"));
        assert!(p.contains("[PRIVATE_KEY]") && !p.contains("PRIVATE KEY-----"));
        if perf_gates_enabled() {
            let budget = perf_budget_ms("MAEKON_PERF_BUDGET_MASKER_MS", 1_000);
            assert!(
                elapsed_ms <= budget,
                "korean-id/private-key sanitize O(N^2) regression: {elapsed_ms}ms > {budget}ms"
            );
        }
    }

    #[test]
    fn korean_id_masks_entire_digit_run_not_fixed_window() {
        // #6830: the ENTIRE contiguous digit run around the '-' must be masked,
        // not a fixed last-6 / first-7 window — otherwise a longer leading run
        // leaks the RRN's first digit(s) and a longer trailing run leaks the tail.
        assert_eq!(redaction::mask_korean_id("1901010-1234567"), "[KR_ID]");
        assert_eq!(redaction::mask_korean_id("901010-1234567890123"), "[KR_ID]");
        assert_eq!(
            redaction::mask_korean_id("acct 1901010-1234567 end"),
            "acct [KR_ID] end"
        );
        // Standard 6-7 form: unchanged behavior, surrounding text preserved.
        assert_eq!(
            redaction::mask_korean_id("id 901010-1234567 ok"),
            "id [KR_ID] ok"
        );
        // Below threshold (5 digits before the '-') must NOT be masked.
        assert_eq!(redaction::mask_korean_id("12345-1234567"), "12345-1234567");
    }
}
