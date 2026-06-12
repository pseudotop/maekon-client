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

#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "kind")]
#[allow(dead_code)] // Linux-only variants are cfg-gated; dead_code fires on macOS/Windows builds
pub enum UnsupportedReason {
    SnapSandbox,
    FlatpakSandbox,
    HeadlessSession,
    SystemctlUnavailable,
    UnsupportedPlatform,
}

#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)] // Linux-only variants are cfg-gated; dead_code fires on macOS/Windows builds
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
