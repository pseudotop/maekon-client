//! Linux autostart implementation.
//!
//! Primary path: systemd user service unit at
//! `~/.config/systemd/user/maekon.service` (requires `systemctl` resolvable
//! under the SEC-MON-01 trusted-directory allowlist, not a bare PATH lookup).
//! Fallback: XDG autostart desktop file at
//! `~/.config/autostart/maekon.desktop`.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tracing::{debug, warn};

/// Path for the systemd user service file.
pub fn service_path() -> Result<PathBuf, String> {
    let home =
        std::env::var("HOME").map_err(|_| "HOME environment variable not set".to_string())?;
    Ok(PathBuf::from(home)
        .join(".config")
        .join("systemd")
        .join("user")
        .join("maekon.service"))
}

/// Path for the XDG autostart desktop file (fallback).
pub fn desktop_path() -> Result<PathBuf, String> {
    let home =
        std::env::var("HOME").map_err(|_| "HOME environment variable not set".to_string())?;
    Ok(PathBuf::from(home)
        .join(".config")
        .join("autostart")
        .join("maekon.desktop"))
}

fn binary_path() -> Result<String, String> {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|e| format!("Failed to determine binary path: {e}"))
}

/// Generate a systemd user service unit file.
pub fn generate_service_file(program_path: &str) -> String {
    format!(
        "[Unit]\n\
         Description=Maekon Desktop Agent\n\
         After=graphical-session.target\n\
         \n\
         [Service]\n\
         Type=notify\n\
         NotifyAccess=main\n\
         ExecStart={program_path}\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         TimeoutStartSec=30\n\
         Environment=DISPLAY=:0\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n"
    )
}

/// Generate an XDG autostart desktop file.
pub fn generate_desktop_file(program_path: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Maekon\n\
         Comment=Maekon Desktop Agent\n\
         Exec={program_path}\n\
         Hidden=false\n\
         X-GNOME-Autostart-enabled=true\n\
         StartupNotify=false\n"
    )
}

/// SEC-MON-01: resolve `systemctl` against the shared trusted-directory
/// allowlist (maekon-monitor) instead of a bare-name PATH lookup — fail
/// closed (`None`) when the binary is not found under any trusted system
/// directory.
fn systemctl_path() -> Option<PathBuf> {
    maekon_monitor::resolve_trusted_binary("systemctl")
}

/// Check whether `systemctl` is available on the system (trusted-dir
/// resolved AND functional).
pub fn has_systemctl() -> bool {
    let Some(path) = systemctl_path() else {
        return false;
    };
    Command::new(path)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn enable() -> Result<(), String> {
    let bin = binary_path()?;
    // Resolve once; only used when the functional check below also passes.
    let systemctl = systemctl_path().filter(|_| has_systemctl());

    if let Some(systemctl) = systemctl {
        // Primary: systemd user service
        let path = service_path()?;
        let content = generate_service_file(&bin);

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create systemd user directory: {e}"))?;
        }

        fs::write(&path, content).map_err(|e| format!("Failed to write service file: {e}"))?;

        // Reload systemd daemon to pick up the new unit
        let _ = Command::new(&systemctl)
            .args(["--user", "daemon-reload"])
            .output();

        let output = Command::new(&systemctl)
            .args(["--user", "enable", "maekon.service"])
            .output()
            .map_err(|e| format!("systemctl enable failed: {e}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("systemctl enable returned non-zero: {stderr}");
        }

        debug!("auto-start enabled via systemd user service");
    } else {
        // Fallback: XDG autostart desktop file
        let path = desktop_path()?;
        let content = generate_desktop_file(&bin);

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create autostart directory: {e}"))?;
        }

        fs::write(&path, content).map_err(|e| format!("Failed to write desktop file: {e}"))?;

        debug!("auto-start enabled via XDG desktop file (systemd not available)");
    }

    Ok(())
}

pub fn disable() -> Result<(), String> {
    // Disable systemd service if it exists
    let svc_path = service_path()?;
    if svc_path.exists() {
        if let Some(systemctl) = systemctl_path().filter(|_| has_systemctl()) {
            let _ = Command::new(&systemctl)
                .args(["--user", "disable", "maekon.service"])
                .output();
            let _ = Command::new(&systemctl)
                .args(["--user", "daemon-reload"])
                .output();
        }
        fs::remove_file(&svc_path).map_err(|e| format!("Failed to remove service file: {e}"))?;
    }

    // Remove XDG desktop file if it exists
    let desk_path = desktop_path()?;
    if desk_path.exists() {
        fs::remove_file(&desk_path).map_err(|e| format!("Failed to remove desktop file: {e}"))?;
    }

    debug!("auto-start disabled");
    Ok(())
}

pub fn is_enabled() -> Result<bool, String> {
    let svc_path = service_path()?;
    if svc_path.exists() {
        return Ok(true);
    }

    let desk_path = desktop_path()?;
    Ok(desk_path.exists())
}
