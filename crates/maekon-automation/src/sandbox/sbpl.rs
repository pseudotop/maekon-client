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
///
/// `worker_path` is the absolute path of the `maekon-sandbox-worker` binary that
/// `sandbox-exec` will launch. `sandbox-exec` applies this profile to itself and
/// then `execve`s the worker, so that one exec MUST be authorised or the worker
/// can never start. The deny-by-default tiers therefore re-allow `process-exec`
/// **only for that single literal path** (#6443 F6): the worker is a single-shot
/// stdin→stdout process that never forks or execs anything post-launch (see
/// `crates/maekon-sandbox-worker`), so a compromised or buggy worker cannot exec
/// a shell or any other binary, and `process-fork` is not granted at all. Pass a
/// canonical path — Seatbelt matches the literal against the canonical vnode path
/// of the exec target, so the caller must hand the same canonical path to both
/// this function and `sandbox-exec`.
pub(crate) fn generate_sbpl_profile(config: &SandboxConfig, worker_path: &str) -> String {
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
            // #6443 F6: scope exec to the worker binary only (see fn doc). The
            // worker never forks, so `process-fork` is intentionally NOT granted.
            rules.push_str(&format!(
                "(allow process-exec (literal \"{}\"))\n",
                escape_sbpl_path(worker_path)
            ));
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
            // #6443 F6: same single-binary exec constraint as Standard; Strict
            // already withholds process-fork.
            rules.push_str(&format!(
                "(allow process-exec (literal \"{}\"))\n",
                escape_sbpl_path(worker_path)
            ));
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

/// Escape a path for safe inclusion inside an SBPL `(subpath "...")` or
/// `(literal "...")` string. Backslash and double-quote are the only
/// metacharacters that can terminate or break out of the quoted string —
/// escaping them prevents a crafted `allowed_*_paths` entry (or worker path)
/// from injecting additional SBPL rules (e.g. closing the string and appending
/// `(allow network*)`).
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

    /// Stand-in worker path used by the profile tests. Any absolute path works —
    /// the tests assert how it is woven into the `process-exec` rule, not the
    /// value itself.
    const TEST_WORKER: &str = "/opt/maekon/maekon-sandbox-worker";

    fn config(profile: SandboxProfile) -> SandboxConfig {
        SandboxConfig {
            profile,
            ..Default::default()
        }
    }

    /// Generate a profile with the stand-in worker path.
    fn gen(cfg: &SandboxConfig) -> String {
        generate_sbpl_profile(cfg, TEST_WORKER)
    }

    #[test]
    fn permissive_allows_default_but_hardens_system_writes() {
        let p = gen(&config(SandboxProfile::Permissive));
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
        let p = gen(&config(SandboxProfile::Standard));
        let deny_idx = p
            .find("(deny default)")
            .expect("standard must deny by default");
        let allow_idx = p
            .find("(allow process-exec ")
            .expect("standard re-allows exec");
        assert!(
            deny_idx < allow_idx,
            "(deny default) must precede the re-allows (SBPL is order-sensitive)"
        );
    }

    #[test]
    fn standard_constrains_process_exec_to_the_worker_binary() {
        // #6443 F6: deny-by-default tiers must scope exec to the single worker
        // literal, never grant a blanket `(allow process-exec)`. A blanket grant
        // would let a compromised worker exec an arbitrary shell.
        let p = gen(&config(SandboxProfile::Standard));
        assert!(
            p.contains("(allow process-exec (literal \"/opt/maekon/maekon-sandbox-worker\"))"),
            "Standard must scope process-exec to the worker literal: {p}"
        );
        assert!(
            !p.contains("(allow process-exec)\n"),
            "Standard must NOT grant blanket (allow process-exec): {p}"
        );
    }

    #[test]
    fn standard_no_longer_grants_process_fork() {
        // #6443 F6: the worker never forks, so process-fork is withheld.
        let p = gen(&config(SandboxProfile::Standard));
        assert!(
            !p.contains("process-fork"),
            "Standard must not grant process-fork (the worker never forks): {p}"
        );
    }

    #[test]
    fn strict_constrains_process_exec_to_the_worker_binary() {
        let p = gen(&config(SandboxProfile::Strict));
        assert!(
            p.contains("(allow process-exec (literal \"/opt/maekon/maekon-sandbox-worker\"))"),
            "Strict must scope process-exec to the worker literal: {p}"
        );
        assert!(
            !p.contains("(allow process-exec)\n"),
            "Strict must NOT grant blanket (allow process-exec): {p}"
        );
        // Strict has never granted process-fork; keep it that way.
        assert!(
            !p.contains("process-fork"),
            "Strict must not grant process-fork: {p}"
        );
    }

    #[test]
    fn process_exec_literal_escapes_a_crafted_worker_path() {
        // The worker path is app-controlled, but escape it anyway (defence in
        // depth + consistency with the allowed_*_paths handling): a `"` in the
        // path must not be able to close the literal and append a sibling rule.
        let cfg = config(SandboxProfile::Strict);
        let p = generate_sbpl_profile(&cfg, "/x\") (allow network*) (literal \"/y");
        // The injected quote is escaped, so the whole payload stays inside ONE
        // literal string — the parser never sees a standalone (allow network*).
        assert!(
            p.contains(r#"(literal "/x\")"#),
            "the injected quote must be escaped: {p}"
        );
        assert!(
            !p.contains(r#"(literal "/x") (allow network*)"#),
            "the escaped worker path must not break out of the literal: {p}"
        );
        // Strict still denies the network.
        assert!(p.contains("(deny network*)"));
    }

    #[test]
    fn standard_honors_network_toggle() {
        let mut cfg = config(SandboxProfile::Standard);
        cfg.allow_network = false;
        let denied = gen(&cfg);
        assert!(denied.contains("(deny network*)"));
        assert!(!denied.contains("(allow network*)"));

        cfg.allow_network = true;
        let allowed = gen(&cfg);
        assert!(allowed.contains("(allow network*)"));
        assert!(!allowed.contains("(deny network*)"));
    }

    #[test]
    fn strict_always_denies_network_even_when_allow_network_true() {
        // Security contract: the strict tier ignores `allow_network`.
        let mut cfg = config(SandboxProfile::Strict);
        cfg.allow_network = true;
        let p = gen(&cfg);
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
        let p = gen(&cfg);
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
        let p = gen(&cfg);

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
