//! Cfg-free generation of the macOS Seatbelt (SBPL) sandbox profile.
//!
//! `/usr/bin/sandbox-exec` only exists on macOS, but the *profile text* — the
//! security policy that decides what the sandboxed worker may read/write and
//! whether it may reach the network — is pure string construction. Extracting it
//! here (no `#[cfg]`, no FFI) makes that policy unit-testable on any OS (#5120),
//! so a mistake that would *widen* the sandbox (a missing `(deny default)`, a
//! path that escapes SBPL quoting, or `Strict` accidentally allowing the network)
//! is caught by the ordinary ubuntu/macOS test run, not only on a macOS runner.

// Consumed by the macOS sandbox adapter (`super::macos`) plus the tests below;
// it has no non-test caller on other targets.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use maekon_core::config::{SandboxConfig, SandboxProfile};

/// Build the SBPL (Seatbelt Profile Language) text for `config`.
///
/// `Permissive` allows-by-default (only hardening `/System` and `/usr` against
/// writes); `Standard` and `Strict` are deny-by-default and only re-allow the
/// minimum plus the configured paths. `Strict` **always** denies the network,
/// regardless of `config.allow_network` — that is the point of the strict tier.
pub(crate) fn generate_sbpl_profile(config: &SandboxConfig) -> String {
    let mut rules = String::new();
    rules.push_str("(version 1)\n");

    match config.profile {
        SandboxProfile::Permissive => {
            rules.push_str("(allow default)\n");
            rules.push_str("(deny file-write* (subpath \"/System\"))\n");
            rules.push_str("(deny file-write* (subpath \"/usr\"))\n");
        }
        SandboxProfile::Standard => {
            rules.push_str("(deny default)\n");
            rules.push_str("(allow process-exec)\n");
            rules.push_str("(allow process-fork)\n");
            rules.push_str("(allow sysctl-read)\n");
            rules.push_str("(allow mach-lookup)\n");

            rules.push_str("(allow file-read* (subpath \"/usr/lib\"))\n");
            rules.push_str("(allow file-read* (subpath \"/System/Library\"))\n");
            rules.push_str("(allow file-read* (subpath \"/Library/Frameworks\"))\n");
            rules.push_str("(allow file-read* (subpath \"/dev\"))\n");

            for path in &config.allowed_read_paths {
                let escaped = escape_sbpl_path(path);
                rules.push_str(&format!("(allow file-read* (subpath \"{}\"))\n", escaped));
            }

            for path in &config.allowed_write_paths {
                let escaped = escape_sbpl_path(path);
                rules.push_str(&format!("(allow file-write* (subpath \"{}\"))\n", escaped));
            }

            if !config.allow_network {
                rules.push_str("(deny network*)\n");
            } else {
                rules.push_str("(allow network*)\n");
            }
        }
        SandboxProfile::Strict => {
            rules.push_str("(deny default)\n");
            rules.push_str("(allow process-exec)\n");
            rules.push_str("(allow sysctl-read)\n");

            rules.push_str("(allow file-read* (subpath \"/usr/lib\"))\n");
            rules.push_str("(allow file-read* (subpath \"/dev/null\"))\n");
            rules.push_str("(allow file-read* (subpath \"/dev/urandom\"))\n");

            for path in &config.allowed_read_paths {
                let escaped = escape_sbpl_path(path);
                rules.push_str(&format!("(allow file-read* (subpath \"{}\"))\n", escaped));
            }

            rules.push_str("(deny network*)\n");
        }
    }

    rules
}

/// Escape a path for safe inclusion inside an SBPL `(subpath "...")` string
/// literal. Backslash and double-quote are the only metacharacters that can
/// terminate or break out of the quoted string — escaping them prevents a
/// crafted `allowed_*_paths` entry from injecting additional SBPL rules (e.g.
/// closing the subpath and appending `(allow network*)`).
///
/// The replace **order matters**: backslash MUST be doubled before the quote is
/// escaped, otherwise the `\` introduced by quote-escaping would itself be
/// re-doubled and leave the quote bare (re-opening the breakout). This is pinned
/// by `escape_order_neutralises_adjacent_backslash_quote`.
///
/// An interior-NUL path is not handled here and does not need to be: the profile
/// is passed as a `Command` argument, and the `CString` conversion of an arg
/// containing NUL fails at spawn time → the run is rejected fail-closed, so
/// `\` and `"` are the complete in-string-breakout set this escaper must cover.
fn escape_sbpl_path(path: &str) -> String {
    path.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(profile: SandboxProfile) -> SandboxConfig {
        SandboxConfig {
            profile,
            ..Default::default()
        }
    }

    #[test]
    fn permissive_allows_default_but_hardens_system_writes() {
        let p = generate_sbpl_profile(&config(SandboxProfile::Permissive));
        assert!(p.contains("(version 1)"));
        assert!(p.contains("(allow default)"));
        assert!(p.contains("(deny file-write* (subpath \"/System\"))"));
        assert!(p.contains("(deny file-write* (subpath \"/usr\"))"));
        // SBPL is last-match-wins: `(allow default)` MUST come before the
        // write-denies, otherwise the denies would be overridden and /System
        // would be writable.
        let allow_idx = p
            .find("(allow default)")
            .expect("permissive allows default");
        let deny_idx = p
            .find("(deny file-write* (subpath \"/System\"))")
            .expect("permissive denies /System writes");
        assert!(
            allow_idx < deny_idx,
            "(allow default) must precede the write-denies so they take effect"
        );
    }

    #[test]
    fn standard_is_deny_by_default_before_allows() {
        let p = generate_sbpl_profile(&config(SandboxProfile::Standard));
        let deny_idx = p
            .find("(deny default)")
            .expect("standard must deny by default");
        let allow_idx = p
            .find("(allow process-exec)")
            .expect("standard re-allows exec");
        assert!(
            deny_idx < allow_idx,
            "(deny default) must precede the re-allows (SBPL is order-sensitive)"
        );
    }

    #[test]
    fn standard_honors_network_toggle() {
        let mut cfg = config(SandboxProfile::Standard);
        cfg.allow_network = false;
        let denied = generate_sbpl_profile(&cfg);
        assert!(denied.contains("(deny network*)"));
        assert!(!denied.contains("(allow network*)"));

        cfg.allow_network = true;
        let allowed = generate_sbpl_profile(&cfg);
        assert!(allowed.contains("(allow network*)"));
        assert!(!allowed.contains("(deny network*)"));
    }

    #[test]
    fn strict_always_denies_network_even_when_allow_network_true() {
        // Security contract: the strict tier ignores `allow_network`.
        let mut cfg = config(SandboxProfile::Strict);
        cfg.allow_network = true;
        let p = generate_sbpl_profile(&cfg);
        assert!(
            p.contains("(deny network*)"),
            "Strict must deny the network regardless of allow_network"
        );
        assert!(
            !p.contains("(allow network*)"),
            "Strict must never emit (allow network*)"
        );
    }

    #[test]
    fn allowed_paths_are_emitted_for_standard() {
        let mut cfg = config(SandboxProfile::Standard);
        cfg.allowed_read_paths = vec!["/tmp/r".to_string()];
        cfg.allowed_write_paths = vec!["/tmp/w".to_string()];
        let p = generate_sbpl_profile(&cfg);
        assert!(p.contains("(allow file-read* (subpath \"/tmp/r\"))"));
        assert!(p.contains("(allow file-write* (subpath \"/tmp/w\"))"));
    }

    #[test]
    fn escape_quotes_and_backslashes() {
        assert_eq!(escape_sbpl_path("/normal/path"), "/normal/path");
        assert_eq!(escape_sbpl_path("/a\"b"), "/a\\\"b");
        assert_eq!(escape_sbpl_path("/a\\b"), "/a\\\\b");
    }

    #[test]
    fn escape_order_neutralises_adjacent_backslash_quote() {
        // Pins the load-bearing security property: backslash is doubled BEFORE
        // the quote is escaped. Input `\"` (backslash then quote) must become
        // `\\\"` — two backslashes (the escaped backslash) followed by `\"` (the
        // escaped quote). A quote-first reorder would instead re-double the
        // escape backslash and leave the quote bare, re-opening the breakout.
        // This `assert_eq!` is the airtight pin (a substring check on a profile
        // can't distinguish escaped from unescaped — see the note in
        // crafted_path_cannot_inject_an_sbpl_rule).
        assert_eq!(escape_sbpl_path("/a\\\"b"), "/a\\\\\\\"b");
    }

    #[test]
    fn crafted_path_cannot_inject_an_sbpl_rule() {
        // A malicious allowed-path that tries to close the `(subpath "...")` and
        // append its own rule must be neutralized: every `"` is escaped to `\"`,
        // so the attacker's quote never terminates the subpath string — the
        // whole payload stays inside ONE quoted argument. (The literal bytes
        // `(allow network*)` still appear *inside* that string, but as inert
        // text the Seatbelt parser reads as part of the path, not a rule.)
        let mut cfg = config(SandboxProfile::Strict);
        cfg.allowed_read_paths = vec!["/x\") (allow network*) (subpath \"/y".to_string()];
        let p = generate_sbpl_profile(&cfg);

        // Escaped form (safe): the quote after `/x` is `\"`, still inside the string.
        assert!(
            p.contains(r#"(subpath "/x\")"#),
            "the injected quote must be escaped (stay inside the subpath): {p}"
        );
        // The dangerous *unescaped* breakout — a bare `") ` that would close the
        // subpath and start a sibling rule — must NOT appear.
        assert!(
            !p.contains(r#"(subpath "/x") (allow"#),
            "the escaped path must not break out of the subpath string: {p}"
        );
        // Strict's own network denial is still present and well-formed.
        assert!(p.contains("(deny network*)"));
    }
}
