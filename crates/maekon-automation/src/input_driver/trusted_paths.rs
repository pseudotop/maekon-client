/// Trusted absolute path for the macOS `open` helper. A SIP-protected system
/// binary; resolving it directly avoids a bare-name `PATH` lookup (#7075).
#[cfg(target_os = "macos")]
#[cfg_attr(not(feature = "enigo"), allow(dead_code))]
pub(super) const TRUSTED_OPEN_PATH: &str = "/usr/bin/open";

/// Trusted absolute directories searched for the optional Linux window-manager
/// helpers (`wmctrl`/`xdotool`). The inherited `PATH` is deliberately NOT
/// consulted: a user-writable `PATH` entry (e.g. `~/.local/bin`) shadowing these
/// tools would let a planted binary run with the agent's privileges
/// (CWE-426/427, #7075). User-writable locations are intentionally excluded.
#[cfg(target_os = "linux")]
#[cfg_attr(not(feature = "enigo"), allow(dead_code))]
pub(super) const TRUSTED_HELPER_DIRS: &[&str] = &["/usr/bin", "/usr/local/bin", "/bin"];

/// Returns `true` when `path` is safe to spawn without a `PATH` lookup: an
/// absolute, regular file that on unix is owned by root and is not group- or
/// world-writable. Mirrors the ownership/mode discipline of
/// `sandbox::macos::trusted_sandbox_exec_path`.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[cfg_attr(not(feature = "enigo"), allow(dead_code))]
pub(super) fn is_trusted_program(path: &std::path::Path) -> bool {
    if !path.is_absolute() || !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let Ok(metadata) = std::fs::metadata(path) else {
            return false;
        };
        // Reject files that are not root-owned or are group/world-writable: those
        // can be replaced by a non-privileged attacker (the PATH-hijack vector).
        if metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
            return false;
        }
    }
    true
}

/// Resolve a Linux window-manager helper to a trusted absolute path under a fixed
/// system directory, never consulting the inherited `PATH`. Returns `None` when
/// the helper is not installed in a trusted location, so the caller reports
/// activation as unsupported instead of running a PATH-resolved (possibly planted)
/// binary (#7075).
#[cfg(target_os = "linux")]
#[cfg_attr(not(feature = "enigo"), allow(dead_code))]
pub(super) fn resolve_trusted_helper(program: &str) -> Option<std::path::PathBuf> {
    TRUSTED_HELPER_DIRS.iter().find_map(|dir| {
        let candidate = std::path::Path::new(dir).join(program);
        is_trusted_program(&candidate).then_some(candidate)
    })
}
