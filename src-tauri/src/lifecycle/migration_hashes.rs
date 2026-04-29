//! Known SHA-256 hashes of prior-version systemd service file templates.
//!
//! Used by autostart_migration to determine whether an existing
//! `~/.config/systemd/user/oneshim.service` file matches a known template
//! (safe to overwrite) vs has been customized by the user (skip).

// Functions and constants are consumed by autostart_migration on Linux only.
// Non-Linux builds see them as unused (callers are cfg-gated).
#![allow(dead_code)]

use sha2::{Digest, Sha256};

/// (hash, label) pairs for every released template content prior to PR-B2.
///
/// Hashes are computed by `compute_hash(canonicalize(template, binary_path))`
/// where binary_path is the resolved current_exe() path.
///
/// # Adding a new entry
///
/// When `linux::generate_service_file()` semantics change in a way that affects
/// existing users (e.g., a future PR-Bn template), add the previous template's
/// hash here BEFORE shipping the change. Steps:
///
/// 1. Capture the canonical form of the prior template (with `{BINARY_PATH}`
///    placeholder, `\n` line endings, no trailing whitespace).
/// 2. Compute SHA-256 — either via Rust:
///    ```
///    let hash = compute_hash(canonicalize(prior_template, "/dummy/path"));
///    ```
///    or via shell:
///    ```bash
///    printf '<canonical content>' | shasum -a 256 | awk '{print $1}'
///    ```
/// 3. Append `(hash, "<descriptive-label>")` here. Keep older entries indefinitely
///    so users on legacy releases still get migrated when they finally upgrade.
/// 4. Add a unit test in `tests::pr_<version>_template_hash_matches_registry`
///    that pins the canonical form against the registered hash.
pub const KNOWN_PRIOR_HASHES: &[(&str, &str)] = &[
    // PR-B1 Type=simple (Maekon brand, v0.4.40-rc.3+ and v0.4.40 stable)
    (
        "9b1f5d384dc9246228a5601ac67de4127c7f5fdf9a24b2f447c78d1bb671047f",
        "PR-B1 Type=simple",
    ),
    // PR-B1 Type=simple (pre-rebrand ONESHIM brand, v0.4.40-rc.1 and v0.4.40-rc.2)
    (
        "beebd724df50096241ffe040d3fc044dc93493ba3ae265302bc2c3a3215c59ca",
        "PR-B1 Type=simple (pre-rebrand)",
    ),
];

/// Canonicalize the service file before hashing.
///
/// Handles both line-ending normalization AND word-boundary-aware
/// ExecStart line replacement to avoid:
/// - Binary path substring collision: `/home/user/oneshim` would match
///   `/home/user/oneshim-old` with a naive string replace.
/// - Line-ending variation: `\r\n` (Windows-edited files) producing a
///   different hash than `\n`.
///
/// Symlink edge case: if the user wrote the service file using a symlink path
/// but `current_exe()` returns the canonical path (or vice versa), the ExecStart
/// line won't match and the file will be treated as customized — log warn + skip.
/// This is acceptable behavior.
pub fn canonicalize(content: &str, binary_path: &str) -> String {
    // Step 1: normalize line endings
    let normalized = content.replace("\r\n", "\n");
    // Step 2: replace ExecStart line specifically (not arbitrary substring)
    let exec_line = format!("ExecStart={}\n", binary_path);
    normalized.replace(&exec_line, "ExecStart={BINARY_PATH}\n")
}

/// Compute a lowercase hex SHA-256 digest of the given string.
pub fn compute_hash(content: &str) -> String {
    let digest = Sha256::digest(content.as_bytes());
    let mut out = String::with_capacity(64);
    for byte in digest.as_slice() {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

/// Returns `Some(label)` if the canonicalized content matches a known prior hash.
/// Returns `None` if the user has customized the file (or it is the new template already).
pub fn matches_known_template(content: &str, binary_path: &str) -> Option<&'static str> {
    let canonical = canonicalize(content, binary_path);
    let hash = compute_hash(&canonical);
    KNOWN_PRIOR_HASHES
        .iter()
        .find(|(known_hash, _)| *known_hash == hash)
        .map(|(_, label)| *label)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Canonicalized PR-B1 Maekon-branded template (Type=simple) for hash matching.
    /// Must match the actual content `linux::generate_service_file()` produced
    /// in v0.4.40-rc.3 / v0.4.40 stable (post-Maekon-rebrand, pre-PR-B2).
    const PR_B1_MAEKON_CANONICAL: &str = "[Unit]\nDescription=Maekon Desktop Agent\nAfter=graphical-session.target\n\n[Service]\nType=simple\nExecStart={BINARY_PATH}\nRestart=on-failure\nRestartSec=5\nEnvironment=DISPLAY=:0\n\n[Install]\nWantedBy=default.target\n";

    /// Canonicalized PR-B1 ONESHIM-branded template (Type=simple, pre-rebrand)
    /// shipped in v0.4.40-rc.1 and v0.4.40-rc.2 before #520 renamed Description.
    const PR_B1_ONESHIM_CANONICAL: &str = "[Unit]\nDescription=ONESHIM Desktop Agent\nAfter=graphical-session.target\n\n[Service]\nType=simple\nExecStart={BINARY_PATH}\nRestart=on-failure\nRestartSec=5\nEnvironment=DISPLAY=:0\n\n[Install]\nWantedBy=default.target\n";

    #[test]
    fn pr_b1_maekon_template_hash_matches_registry() {
        let computed = compute_hash(PR_B1_MAEKON_CANONICAL);
        let known = KNOWN_PRIOR_HASHES
            .iter()
            .find(|(_, label)| *label == "PR-B1 Type=simple")
            .expect("PR-B1 Maekon entry must exist in KNOWN_PRIOR_HASHES");
        assert_eq!(
            computed, known.0,
            "computed hash {computed} should match registered {} for PR-B1 Maekon template",
            known.0
        );
    }

    #[test]
    fn pr_b1_oneshim_template_hash_matches_registry() {
        let computed = compute_hash(PR_B1_ONESHIM_CANONICAL);
        let known = KNOWN_PRIOR_HASHES
            .iter()
            .find(|(_, label)| *label == "PR-B1 Type=simple (pre-rebrand)")
            .expect("PR-B1 ONESHIM entry must exist in KNOWN_PRIOR_HASHES");
        assert_eq!(
            computed, known.0,
            "computed hash {computed} should match registered {} for PR-B1 ONESHIM template",
            known.0
        );
    }

    #[test]
    fn canonicalize_replaces_exec_line_only() {
        let binary = "/home/user/oneshim";
        let content = format!(
            "[Unit]\n[Service]\nExecStart={}\nRestart=on-failure\n",
            binary
        );
        let canonical = canonicalize(&content, binary);
        assert!(canonical.contains("ExecStart={BINARY_PATH}\n"));
        assert!(!canonical.contains("/home/user/oneshim"));
    }

    #[test]
    fn canonicalize_does_not_replace_substring_of_other_paths() {
        // Edge case: binary_path is substring of a longer path elsewhere
        let binary = "/home/user/oneshim";
        let content = format!(
            "[Service]\nExecStart={}\nReadOnlyPaths=/home/user/oneshim-data\n",
            binary
        );
        let canonical = canonicalize(&content, binary);
        // ExecStart line replaced
        assert!(canonical.contains("ExecStart={BINARY_PATH}\n"));
        // ReadOnlyPaths line NOT replaced (different context, not ExecStart line)
        assert!(canonical.contains("/home/user/oneshim-data"));
    }

    #[test]
    fn canonicalize_normalizes_crlf() {
        let binary = "/usr/bin/oneshim";
        let content = format!("[Service]\r\nExecStart={}\r\n", binary);
        let canonical = canonicalize(&content, binary);
        assert!(!canonical.contains("\r"));
        assert!(canonical.contains("ExecStart={BINARY_PATH}\n"));
    }

    #[test]
    fn matches_known_template_returns_some_for_maekon_pr_b1() {
        let binary = "/usr/bin/oneshim";
        let content = PR_B1_MAEKON_CANONICAL.replace("{BINARY_PATH}", binary);
        let result = matches_known_template(&content, binary);
        assert_eq!(result, Some("PR-B1 Type=simple"));
    }

    #[test]
    fn matches_known_template_returns_some_for_oneshim_pr_b1() {
        let binary = "/usr/bin/oneshim";
        let content = PR_B1_ONESHIM_CANONICAL.replace("{BINARY_PATH}", binary);
        let result = matches_known_template(&content, binary);
        assert_eq!(result, Some("PR-B1 Type=simple (pre-rebrand)"));
    }

    #[test]
    fn matches_known_template_returns_none_for_customized() {
        let binary = "/usr/bin/oneshim";
        let mut content = PR_B1_MAEKON_CANONICAL.replace("{BINARY_PATH}", binary);
        content.push_str("\n# Custom comment from user\n");
        let result = matches_known_template(&content, binary);
        assert_eq!(result, None);
    }

    #[test]
    fn matches_known_template_returns_none_for_empty_file() {
        let binary = "/usr/bin/oneshim";
        let result = matches_known_template("", binary);
        assert_eq!(result, None);
    }

    #[test]
    fn compute_hash_is_deterministic() {
        let h1 = compute_hash("hello world");
        let h2 = compute_hash("hello world");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 hex = 64 chars
    }
}
