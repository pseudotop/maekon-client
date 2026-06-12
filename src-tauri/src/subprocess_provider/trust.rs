//! Binary-trust verification for the Codex `app-server` spawn path (E21 #4863
//! R7). Before spawning `codex app-server` as a long-lived child, the factory
//! cross-checks the resolved executable against an install-path allowlist.
//!
//! GRACEFUL CONTRACT (ADR-025 / #4871 tolerate-and-fallback): in the default
//! mode this is a WARN-AND-PROCEED telemetry signal, NOT a hard security
//! boundary. A binary outside the allowlist still spawns (with a warning) — the
//! allowlist provides audit/observability value so a PATH-hijacked `codex` is
//! visible in Loki/Grafana, but it does not block. Real enforcement (an opt-in
//! managed/hard-block mode) is a deferred follow-up. The companion `--version`
//! probe (in the factory) is the only branch that returns `Err`, and even that
//! degrades to `codex exec` rather than aborting the chat.
//!
//! Pure + injectable: [`install_path_trust`] takes the allowed roots as an
//! argument so it is unit-testable with `tempfile` (precedent:
//! `surface_selection.rs` tests). [`default_allowed_roots`] resolves the
//! environment-derived roots, including the additive `MAEKON_CODEX_ALLOWED_DIRS`
//! operator override.

use std::path::{Path, PathBuf};

/// Result of the install-path allowlist check (E21 #4863 R7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BinaryTrust {
    /// The canonicalized executable lives under one of the allowed install
    /// roots. Logged at `info` level; proceed.
    TrustedPath,
    /// The executable is NOT under any allowed root (or its path could not be
    /// canonicalized / there are no roots). In the default mode this is a `warn`
    /// + PROCEED — it is telemetry, not a gate.
    UnknownPath,
}

/// Classify `exe` against `allowed_roots`. Canonicalizes both sides so a
/// symlink or `..` traversal cannot dress an out-of-allowlist binary up as an
/// in-allowlist one. Returns [`BinaryTrust::UnknownPath`] when `exe` cannot be
/// canonicalized (e.g. it does not exist) or when no root contains it.
///
/// Roots that fail to canonicalize are skipped (a non-existent allowlist dir is
/// simply not a match), so a partially-resolvable allowlist still works.
pub(crate) fn install_path_trust(exe: &Path, allowed_roots: &[PathBuf]) -> BinaryTrust {
    let Ok(canonical_exe) = exe.canonicalize() else {
        return BinaryTrust::UnknownPath;
    };
    for root in allowed_roots {
        if let Ok(canonical_root) = root.canonicalize() {
            if canonical_exe.starts_with(&canonical_root) {
                return BinaryTrust::TrustedPath;
            }
        }
    }
    BinaryTrust::UnknownPath
}

/// The environment-resolved install-path allowlist for the `codex` binary
/// (E21 #4863 R7). Data-driven from common install locations per platform, plus
/// the ADDITIVE `MAEKON_CODEX_ALLOWED_DIRS` operator override (a
/// platform-path-separator-split list that is ADDED to — never subtracted from
/// — the built-in roots; an operator can broaden, never narrow, the allowlist
/// via env). Unresolvable / empty entries are dropped here so the caller never
/// sees a bogus root.
pub(crate) fn default_allowed_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();

    #[cfg(unix)]
    {
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            roots.push(home.join(".codex"));
            roots.push(home.join(".local").join("bin"));
            roots.push(home.join(".npm-global").join("bin"));
            roots.push(home.join(".volta").join("bin"));
            roots.push(home.join(".asdf").join("shims"));
        }
        roots.push(PathBuf::from("/usr/local/bin"));
        roots.push(PathBuf::from("/opt/homebrew/bin"));
        roots.push(PathBuf::from("/usr/bin"));
    }

    #[cfg(windows)]
    {
        // Windows trust is intentionally weaker than unix (no install-path
        // provenance equivalent to a process group). We allowlist the common
        // per-user / npm / Program Files roots; hardening beyond this is a
        // deferred follow-up, and an unmatched path still WARNS-AND-PROCEEDS.
        if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
            roots.push(local.join("codex"));
        }
        if let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) {
            roots.push(appdata.join("npm"));
        }
        if let Some(pf) = std::env::var_os("ProgramFiles").map(PathBuf::from) {
            roots.push(pf);
        }
    }

    // Additive operator override: split on the platform path separator (`:` on
    // unix, `;` on windows) and append. Never subtracts from the built-ins.
    if let Some(extra) = std::env::var_os("MAEKON_CODEX_ALLOWED_DIRS") {
        for dir in std::env::split_paths(&extra) {
            if !dir.as_os_str().is_empty() {
                roots.push(dir);
            }
        }
    }

    roots
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Write a fake executable file under `dir` and return its path. The trust
    /// check only inspects the path (not the contents), so an empty file is fine.
    fn write_fake_exe(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, b"#!/bin/sh\n").expect("write fake exe");
        path
    }

    #[test]
    fn exe_under_allowed_root_is_trusted() {
        let root = tempfile::tempdir().expect("tempdir");
        let exe = write_fake_exe(root.path(), "codex");
        let trust = install_path_trust(&exe, &[root.path().to_path_buf()]);
        assert_eq!(trust, BinaryTrust::TrustedPath);
    }

    #[test]
    fn exe_outside_all_roots_is_unknown() {
        let allowed = tempfile::tempdir().expect("allowed dir");
        let elsewhere = tempfile::tempdir().expect("other dir");
        let exe = write_fake_exe(elsewhere.path(), "codex");
        let trust = install_path_trust(&exe, &[allowed.path().to_path_buf()]);
        assert_eq!(trust, BinaryTrust::UnknownPath);
    }

    #[test]
    fn empty_roots_yields_unknown_not_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let exe = write_fake_exe(dir.path(), "codex");
        // No roots configured at all → UnknownPath (warn-and-proceed), never a
        // panic or hard failure.
        assert_eq!(install_path_trust(&exe, &[]), BinaryTrust::UnknownPath);
    }

    #[test]
    fn nonexistent_exe_is_unknown() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("does-not-exist");
        assert_eq!(
            install_path_trust(&missing, &[dir.path().to_path_buf()]),
            BinaryTrust::UnknownPath
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_defeated_by_canonicalize() {
        // A binary lives OUTSIDE the allowlist; a symlink inside the allowlist
        // points at it. Canonicalization must resolve the symlink to its real
        // (out-of-allowlist) target → UnknownPath, not a spoofed TrustedPath.
        let allowed = tempfile::tempdir().expect("allowed dir");
        let real_home = tempfile::tempdir().expect("real dir");
        let real_exe = write_fake_exe(real_home.path(), "codex-real");
        let link = allowed.path().join("codex");
        std::os::unix::fs::symlink(&real_exe, &link).expect("symlink");
        let trust = install_path_trust(&link, &[allowed.path().to_path_buf()]);
        assert_eq!(
            trust,
            BinaryTrust::UnknownPath,
            "a symlink into the allowlist must not launder an out-of-allowlist binary"
        );
    }

    #[test]
    #[serial_test::serial(maekon_codex_allowed_dirs_env)]
    fn additive_override_broadens_allowlist() {
        // MAEKON_CODEX_ALLOWED_DIRS appends a root; the default roots remain.
        // (Read-only: we only assert the override path is INCLUDED, not the exact
        // built-in set, to avoid coupling to the host environment.) `serial_test`
        // guards the process-global env var against concurrent mutators.
        let extra = tempfile::tempdir().expect("extra dir");
        let extra_path = extra.path().to_path_buf();
        std::env::set_var("MAEKON_CODEX_ALLOWED_DIRS", extra.path());
        let roots = default_allowed_roots();
        std::env::remove_var("MAEKON_CODEX_ALLOWED_DIRS");
        assert!(
            roots.contains(&extra_path),
            "operator override dir must be appended to the allowlist; got {roots:?}"
        );
    }
}
