//! Shared types for autostart capability reporting.
//!
//! [`AutostartCapabilities`] is returned to the frontend via IPC to gate the
//! Settings UI toggle.  [`UnsupportedReason`] and [`EnvironmentKind`] are
//! serialised as tagged enums so the frontend can branch on `kind`.

/// Autostart capabilities — used by frontend to gate UI.
/// Returns environment-specific autostart support — the frontend uses this to
/// gate the Settings UI toggle.
#[derive(serde::Serialize, Debug, Clone)]
pub struct AutostartCapabilities {
    pub supported: bool,
    pub unsupported_reason: Option<UnsupportedReason>,
    pub environment: EnvironmentKind,
}

// #7719: `autostart/mod.rs` constructs each variant behind its own
// `#[cfg(target_os = "...")]` arm — on any single build target only that
// platform's variants are actually constructed, so the others read as "dead"
// there. Allowed enum-wide (mirrors `active_window_parse.rs`'s cross-platform
// pure-logic seam) rather than per-variant `cfg_attr`, since every variant
// here maps to exactly one `#[cfg(target_os = ...)]` arm in `mod.rs`.
#[allow(dead_code)]
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum UnsupportedReason {
    SnapSandbox,
    FlatpakSandbox,
    HeadlessSession,
    SystemctlUnavailable,
    UnsupportedPlatform,
}

#[allow(dead_code)]
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentKind {
    MacOs,
    Windows,
    LinuxSystemd,
    LinuxXdg,
    LinuxSnapSandbox,
    LinuxFlatpakSandbox,
    LinuxHeadless,
    Unknown,
}
