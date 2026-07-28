//! Linux autostart implementation.
//!
//! Primary path: systemd user service unit at
//! `~/.config/systemd/user/maekon.service` (requires `systemctl` resolvable
//! under the SEC-MON-01 trusted-directory allowlist, not a bare PATH lookup).
//! Fallback: XDG autostart desktop file at
//! `~/.config/autostart/maekon.desktop`.
//!
//! Honesty contract (#8058 P2-7): `enable` reports success only when autostart
//! will ACTUALLY take effect at login. If `systemctl --user enable` cannot create
//! the enable symlink (typically no user D-Bus session), the orphan `.service`
//! file is removed and the XDG desktop-file fallback is used instead — rather
//! than the old fail-open `Ok(())` that reported "enabled" while nothing would
//! start. `is_enabled` likewise reflects real enablement (`systemctl is-enabled`
//! or the on-disk WantedBy symlink), never the mere presence of the unit file.

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

/// Path of the systemd `WantedBy=default.target` enable symlink for the user
/// unit (#8058 P2-7).
///
/// `systemctl --user enable` creates THIS symlink; it — not the mere presence of
/// the `.service` file — is what actually starts the unit at login. Its existence
/// is therefore the bus-independent ground truth for "is enabled", usable even in
/// a login session with no user D-Bus (where `systemctl` cannot answer).
pub fn wants_symlink_path() -> Result<PathBuf, String> {
    let home =
        std::env::var("HOME").map_err(|_| "HOME environment variable not set".to_string())?;
    Ok(PathBuf::from(home)
        .join(".config")
        .join("systemd")
        .join("user")
        .join("default.target.wants")
        .join("maekon.service"))
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

        let enabled_ok = Command::new(&systemctl)
            .args(["--user", "enable", "maekon.service"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        if enabled_ok {
            debug!("auto-start enabled via systemd user service");
            return Ok(());
        }

        // #8058 P2-7: `systemctl --user enable` did NOT create the enable symlink
        // — typically because this login has no user D-Bus session. Previously we
        // only `warn!`ed and returned `Ok(())`, leaving an ORPHAN `.service` file:
        // the UI reported "autostart ON, success" while nothing would actually
        // start at login (and the old `is_enabled` compounded the lie by treating
        // the orphan file as "enabled"). Remove the orphan and fall through to the
        // XDG desktop file, which takes effect at login without needing a user bus.
        warn!("systemctl --user enable failed; falling back to XDG autostart desktop file");
        let _ = fs::remove_file(&path);
    }

    // Fallback (systemd unavailable, or `enable` could not create the symlink):
    // XDG autostart desktop file.
    let path = desktop_path()?;
    let content = generate_desktop_file(&bin);

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create autostart directory: {e}"))?;
    }

    fs::write(&path, content).map_err(|e| format!("Failed to write desktop file: {e}"))?;

    debug!("auto-start enabled via XDG desktop file");
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
    // #8058 P2-7: the systemd unit is "enabled" only when it is ACTUALLY enabled
    // (the WantedBy symlink exists / `systemctl is-enabled` says so) — NOT merely
    // because the `.service` file is on disk. The old file-existence check was a
    // false positive: after a failed `enable` (no user bus) the orphan `.service`
    // file made the UI report autostart ON even though login would start nothing.
    let svc_path = service_path()?;
    if svc_path.exists() && systemd_unit_enabled()? {
        return Ok(true);
    }

    // Fallback path: the XDG autostart desktop file. Its presence DOES mean
    // autostart is active (no per-unit enable step for XDG).
    let desk_path = desktop_path()?;
    Ok(desk_path.exists())
}

/// Whether the systemd user unit is actually ENABLED (#8058 P2-7).
///
/// Prefers `systemctl --user is-enabled maekon.service`. When `systemctl` cannot
/// answer (no user D-Bus session — its stderr mentions the bus/connection), the
/// query is inconclusive, so we fall back to the on-disk WantedBy symlink, which
/// is the bus-independent ground truth that a successful `enable` creates. A
/// definitive `disabled`/`static`/`masked` (command ran, exited non-zero without
/// a bus error) is honored as "not enabled".
fn systemd_unit_enabled() -> Result<bool, String> {
    if let Some(systemctl) = systemctl_path().filter(|_| has_systemctl()) {
        if let Ok(output) = Command::new(&systemctl)
            .args(["--user", "is-enabled", "maekon.service"])
            .output()
        {
            if output.status.success() {
                return Ok(true);
            }
            let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
            let bus_inconclusive = stderr.contains("bus") || stderr.contains("connect");
            if !bus_inconclusive {
                // Ran and gave a definitive negative — trust it.
                return Ok(false);
            }
        }
    }
    // No systemctl available, or an inconclusive bus error: use the enable symlink.
    Ok(wants_symlink_path()?.exists())
}
