//! SEC-MON-01: trusted-path resolver for external-command helper spawns.
//!
//! `Command::new("pbpaste")` (a BARE name) makes the OS search `$PATH` at run
//! time to find the binary to execute. This crate spawns several OS helpers
//! (`pbpaste`, `xclip`/`xsel`, `powershell`, `gdbus`, `swaymsg`, `dbus-send`,
//! `xprintidle`, `xdotool`, `xinput`, …) from a monitoring agent that is
//! commonly granted elevated/TCC privilege (macOS Accessibility/Automation,
//! Screen Recording, etc.). An attacker who can influence `$PATH` resolution
//! for the agent's process — a writable directory placed ahead of the system
//! dirs, a poisoned shell profile / `launchd` environment, a compromised dev
//! tool earlier on `PATH` — can plant a same-named binary and have it run
//! with the agent's privilege instead of the real system tool. #7483 closed
//! this for the two macOS spawns it covered (`osascript`, `ioreg`) by hard-
//! coding their absolute `/usr/...` paths; every OTHER bare-name spawn in the
//! crate was still PATH-resolved and equally hijackable.
//!
//! [`resolve_trusted_binary`] generalizes the #7483 approach into one shared,
//! per-OS resolver: it searches ONLY a fixed set of absolute system
//! directories (never `$PATH`) and returns the first path where a regular
//! file named `name` exists. Every bare-name spawn in this crate MUST route
//! through it. On a resolution miss (the tool is not present under any
//! trusted directory) callers MUST fail closed — return the crate's existing
//! "helper unavailable" `None`/`Err` path — rather than falling back to a
//! PATH-based `Command::new(name)` spawn, which would silently reopen the
//! hijack this module exists to close.
//!
//! Cfg-free (mirrors `circuit_breaker`): the resolver and its per-OS
//! directory table compile and unit-test on every host OS, not only the
//! platform a given caller is gated to.

use std::path::{Path, PathBuf};

/// Resolve `name` to an absolute path under the current OS's trusted
/// system-directory allowlist.
///
/// Returns `None` (fail closed) when `name` is not a regular file under any
/// trusted directory — callers must NOT fall back to a bare-name (PATH-
/// resolved) spawn in that case.
///
/// `pub` (not `pub(crate)`): the SAME bare-name external-command spawns this
/// resolver was built to close also exist in `src-tauri`, which already
/// depends on this crate as its composition-root adapter. Re-exported at the
/// crate root as `maekon_monitor::resolve_trusted_binary` so both crates
/// share ONE allowlist instead of maintaining a second, divergent copy.
pub fn resolve_trusted_binary(name: &str) -> Option<PathBuf> {
    trusted_candidates(name).into_iter().find(|p| p.is_file())
}

/// Ordered list of absolute candidate paths for `name` under the current
/// OS's trusted directories, first match wins in [`resolve_trusted_binary`].
/// Kept separate (not just inlined) so tests can assert resolution lands in
/// the fixed candidate SET without depending on which entry happens to exist
/// on the test host.
fn trusted_candidates(name: &str) -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        macos_candidates(name)
    }
    #[cfg(target_os = "linux")]
    {
        linux_candidates(name)
    }
    #[cfg(target_os = "windows")]
    {
        windows_candidates(name)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = name;
        Vec::new()
    }
}

/// macOS trusted directories. System-owned, not user-writable without root:
/// `/usr/bin`, `/usr/sbin`, `/bin`, `/sbin`. Deliberately excludes
/// `/opt/homebrew/bin` (user/brew-writable) — none of this crate's macOS
/// helpers (`osascript`, `ioreg`, `pmset`, `pbpaste`) are Homebrew-installed;
/// they all ship with the base OS under `/usr` or `/bin`.
#[cfg(any(test, target_os = "macos"))]
fn macos_candidates(name: &str) -> Vec<PathBuf> {
    ["/usr/bin", "/usr/sbin", "/bin", "/sbin"]
        .iter()
        .map(|dir| Path::new(dir).join(name))
        .collect()
}

/// Linux trusted directories: `/usr/bin`, `/bin`, `/usr/local/bin`. The first
/// two cover every distro-packaged helper this crate spawns (`gdbus`,
/// `swaymsg`, `dbus-send`, `xprintidle`, `xdotool`, `xclip`, `xsel`);
/// `/usr/local/bin` is included as a fallback for locally-built installs of
/// the same tools (still root-owned by default on a standard install).
#[cfg(any(test, target_os = "linux"))]
fn linux_candidates(name: &str) -> Vec<PathBuf> {
    ["/usr/bin", "/bin", "/usr/local/bin"]
        .iter()
        .map(|dir| Path::new(dir).join(name))
        .collect()
}

/// Windows trusted directory: `%SystemRoot%\System32` (falls back to
/// `C:\Windows` if the env var is absent, matching the OS's own default).
/// `powershell.exe` does NOT live directly under `System32` — it ships under
/// the versioned `System32\WindowsPowerShell\v1.0\` subdirectory — so that
/// candidate is added explicitly ahead of the generic `System32\<name>.exe`
/// guess.
#[cfg(any(test, target_os = "windows"))]
fn windows_candidates(name: &str) -> Vec<PathBuf> {
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
    let system32 = Path::new(&system_root).join("System32");

    let mut candidates = Vec::new();
    if name.eq_ignore_ascii_case("powershell") {
        candidates.push(
            system32
                .join("WindowsPowerShell")
                .join("v1.0")
                .join("powershell.exe"),
        );
    }

    let exe_name = if name.to_ascii_lowercase().ends_with(".exe") {
        name.to_string()
    } else {
        format!("{name}.exe")
    };
    candidates.push(system32.join(exe_name));

    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    // A tool guaranteed present under the CURRENT host OS's trusted dirs, so
    // this test is meaningful (exercises real `is_file()` resolution) on
    // whichever platform the workspace gate runs on, not only macOS.
    #[cfg(unix)]
    const KNOWN_TOOL: &str = "ls";
    #[cfg(windows)]
    const KNOWN_TOOL: &str = "cmd";

    #[test]
    fn known_tool_resolves_to_absolute_path_under_trusted_dir() {
        let resolved = resolve_trusted_binary(KNOWN_TOOL)
            .expect("a base-OS tool must resolve under the trusted allowlist");

        assert!(resolved.is_absolute(), "resolved path must be absolute");
        assert!(
            resolved.is_file(),
            "resolved path must exist as a regular file"
        );

        let candidates = trusted_candidates(KNOWN_TOOL);
        assert!(
            candidates.contains(&resolved),
            "resolved path {resolved:?} must be one of the fixed trusted-dir \
             candidates {candidates:?}, not an ad-hoc PATH lookup"
        );
    }

    #[test]
    fn unknown_tool_name_is_rejected_fail_closed() {
        // A name that does not exist under ANY trusted directory must resolve
        // to `None` — this is the fail-closed contract: no fallback to a
        // PATH-based spawn, which is exactly what makes a bare `Command::new`
        // hijackable (a same-named binary placed earlier on `$PATH` runs
        // instead of the real system tool). Regression target for this crate's
        // SEC-MON-01 sweep.
        let name = "definitely-not-a-real-oneshim-trusted-binary-xyz123";
        assert_eq!(resolve_trusted_binary(name), None);
    }

    #[test]
    fn resolve_never_consults_path_only_fixed_dirs() {
        // Even if a malicious same-named binary sat on `$PATH` ahead of the
        // trusted dirs, `resolve_trusted_binary` must only ever return one of
        // the fixed candidate paths (or `None`) — never something resolved by
        // searching `$PATH`. Characterizes the fix: every returned path (if
        // any) is a member of the fixed candidate set for `name`.
        for name in ["osascript", "ioreg", "pmset", "pbpaste", "powershell"] {
            let candidates = trusted_candidates(name);
            if let Some(resolved) = resolve_trusted_binary(name) {
                assert!(
                    candidates.contains(&resolved),
                    "{name} resolved outside the fixed trusted-dir candidate set: {resolved:?}"
                );
            }
        }
    }

    // Pure path-string check (no filesystem dependency) — ungated so it runs
    // on every host, exercising the macOS candidate-building logic even from
    // a Linux/Windows test runner (mirrors the windows_* tests below).
    #[test]
    fn macos_excludes_homebrew_from_trusted_dirs() {
        for candidate in macos_candidates("anything") {
            assert!(
                !candidate.starts_with("/opt/homebrew"),
                "macOS trusted dirs must not include the brew-writable prefix: {candidate:?}"
            );
        }
    }

    #[test]
    fn linux_candidates_cover_documented_dirs() {
        let candidates = linux_candidates("tool");
        let dirs: Vec<_> = candidates
            .iter()
            .map(|p| p.parent().expect("candidate has a parent dir"))
            .collect();
        assert!(dirs.contains(&Path::new("/usr/bin")));
        assert!(dirs.contains(&Path::new("/bin")));
        assert!(dirs.contains(&Path::new("/usr/local/bin")));
    }

    #[test]
    fn windows_powershell_resolves_under_versioned_subdirectory() {
        let candidates = windows_candidates("powershell");
        assert!(
            candidates.iter().any(|p| p
                .to_string_lossy()
                .replace('\\', "/")
                .ends_with("System32/WindowsPowerShell/v1.0/powershell.exe")),
            "powershell must have a candidate under the versioned subdirectory: {candidates:?}"
        );
    }

    #[test]
    fn windows_generic_tool_gets_exe_suffix_under_system32() {
        let candidates = windows_candidates("cmd");
        assert!(
            candidates.iter().any(|p| p
                .to_string_lossy()
                .replace('\\', "/")
                .ends_with("System32/cmd.exe")),
            "cmd must resolve to System32\\cmd.exe: {candidates:?}"
        );
    }
}
