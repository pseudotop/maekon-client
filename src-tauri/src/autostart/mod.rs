//! OS-level "start at login" registration.
//!
//! - macOS: `SMAppService.mainAppService` (primary) + LaunchAgent plist fallback
//!   (`~/Library/LaunchAgents/com.maekon.app.plist`)
//! - Windows: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`
//! - Linux: `~/.config/systemd/user/maekon.service` (systemd) or
//!   `~/.config/autostart/maekon.desktop` (XDG fallback)

pub mod types;
#[cfg(any(
    target_os = "linux",
    not(any(target_os = "macos", target_os = "windows", target_os = "linux"))
))]
use types::UnsupportedReason;
pub use types::{AutostartCapabilities, EnvironmentKind};

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(target_os = "linux")]
pub mod linux;

pub fn enable_autostart() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        macos::enable()
    }

    #[cfg(target_os = "windows")]
    {
        windows::enable()
    }

    #[cfg(target_os = "linux")]
    {
        linux::enable()
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        tracing::warn!("auto-start: unsupported platform");
        Ok(())
    }
}

pub fn disable_autostart() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        macos::disable()
    }

    #[cfg(target_os = "windows")]
    {
        windows::disable()
    }

    #[cfg(target_os = "linux")]
    {
        linux::disable()
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        tracing::warn!("auto-start disabled: unsupported platform");
        Ok(())
    }
}

pub fn is_autostart_enabled() -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        macos::is_enabled()
    }

    #[cfg(target_os = "windows")]
    {
        windows::is_enabled()
    }

    #[cfg(target_os = "linux")]
    {
        linux::is_enabled()
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        tracing::warn!("auto-start check: unsupported platform");
        Ok(false)
    }
}

/// Probe runtime environment to determine autostart capability.
pub fn detect_capabilities() -> AutostartCapabilities {
    #[cfg(target_os = "macos")]
    {
        AutostartCapabilities {
            supported: true,
            unsupported_reason: None,
            environment: EnvironmentKind::MacOs,
        }
    }
    #[cfg(target_os = "windows")]
    {
        AutostartCapabilities {
            supported: true,
            unsupported_reason: None,
            environment: EnvironmentKind::Windows,
        }
    }
    #[cfg(target_os = "linux")]
    {
        // Sandbox detection (highest priority — sandboxed envs can't write
        // service files outside the sandbox boundary)
        if std::env::var("SNAP").is_ok() {
            return AutostartCapabilities {
                supported: false,
                unsupported_reason: Some(UnsupportedReason::SnapSandbox),
                environment: EnvironmentKind::LinuxSnapSandbox,
            };
        }
        if std::env::var("FLATPAK_ID").is_ok() {
            return AutostartCapabilities {
                supported: false,
                unsupported_reason: Some(UnsupportedReason::FlatpakSandbox),
                environment: EnvironmentKind::LinuxFlatpakSandbox,
            };
        }

        // Headless detection (no display server)
        let has_display =
            std::env::var("DISPLAY").is_ok() || std::env::var("WAYLAND_DISPLAY").is_ok();
        if !has_display {
            return AutostartCapabilities {
                supported: false,
                unsupported_reason: Some(UnsupportedReason::HeadlessSession),
                environment: EnvironmentKind::LinuxHeadless,
            };
        }

        // Display present — choose systemd vs XDG fallback
        if linux::has_systemctl() {
            AutostartCapabilities {
                supported: true,
                unsupported_reason: None,
                environment: EnvironmentKind::LinuxSystemd,
            }
        } else {
            AutostartCapabilities {
                supported: true, // XDG .desktop fallback works without systemctl
                unsupported_reason: None,
                environment: EnvironmentKind::LinuxXdg,
            }
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        AutostartCapabilities {
            supported: false,
            unsupported_reason: Some(UnsupportedReason::UnsupportedPlatform),
            environment: EnvironmentKind::Unknown,
        }
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
