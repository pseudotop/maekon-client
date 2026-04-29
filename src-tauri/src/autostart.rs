//! - macOS: `~/Library/LaunchAgents/com.oneshim.agent.plist`
//! - Linux: `~/.config/systemd/user/oneshim.service` (systemd) or `~/.config/autostart/oneshim.desktop` (XDG fallback)

#[cfg(target_os = "macos")]
const APP_LABEL: &str = "com.oneshim.agent";

pub fn enable_autostart() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        macos::enable()
    }

    #[cfg(target_os = "windows")]
    {
        return windows::enable();
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
        return windows::disable();
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
        return windows::is_enabled();
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

#[cfg(target_os = "macos")]
mod macos {
    use super::APP_LABEL;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    pub fn plist_path() -> Result<PathBuf, String> {
        let home = std::env::var("HOME").map_err(|_| "HOME 환경변수 none".to_string())?;
        Ok(PathBuf::from(home)
            .join("Library")
            .join("LaunchAgents")
            .join(format!("{APP_LABEL}.plist")))
    }

    fn binary_path() -> Result<String, String> {
        std::env::current_exe()
            .map(|p| p.to_string_lossy().to_string())
            .map_err(|e| format!("Failed to verify binary path: {e}"))
    }

    pub fn generate_plist(program_path: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{APP_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{program_path}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <false/>
    <key>StandardOutPath</key>
    <string>/tmp/oneshim-client.out.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/oneshim-client.err.log</string>
</dict>
</plist>
"#
        )
    }

    pub fn enable() -> Result<(), String> {
        let path = plist_path()?;
        let bin = binary_path()?;
        let plist_content = generate_plist(&bin);

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create LaunchAgents directory: {e}"))?;
        }

        fs::write(&path, plist_content).map_err(|e| format!("Failed to write plist file: {e}"))?;

        // launchctl load
        Command::new("launchctl")
            .args(["load", &path.to_string_lossy()])
            .output()
            .map_err(|e| format!("launchctl load failure: {e}"))?;

        Ok(())
    }

    pub fn disable() -> Result<(), String> {
        let path = plist_path()?;

        if path.exists() {
            // launchctl unload
            let _ = Command::new("launchctl")
                .args(["unload", &path.to_string_lossy()])
                .output();

            fs::remove_file(&path).map_err(|e| format!("plist delete failure: {e}"))?;
        }

        Ok(())
    }

    pub fn is_enabled() -> Result<bool, String> {
        let path = plist_path()?;
        Ok(path.exists())
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use tracing::debug;

    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
        HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_SZ,
    };

    const SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE_NAME: &str = "ONESHIM";

    fn to_wide(s: &str) -> Vec<u16> {
        OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    pub fn enable() -> Result<(), String> {
        let exe =
            std::env::current_exe().map_err(|e| format!("Failed to verify binary path: {e}"))?;
        let exe_str = exe.to_string_lossy();
        let exe_wide = to_wide(&exe_str);

        let subkey_wide = to_wide(SUBKEY);
        let value_wide = to_wide(VALUE_NAME);

        // SAFETY: RegOpenKeyExW opens HKCU\...\Run with KEY_WRITE.
        // subkey_wide and value_wide are null-terminated UTF-16 vecs alive for the block.
        // hkey is written by RegOpenKeyExW and closed via RegCloseKey before return.
        // exe_wide cast to *const u8 is valid for REG_SZ byte data. No aliasing issues.
        unsafe {
            let mut hkey: HKEY = std::ptr::null_mut();
            let result = RegOpenKeyExW(
                HKEY_CURRENT_USER,
                subkey_wide.as_ptr(),
                0,
                KEY_WRITE,
                &mut hkey,
            );
            if result != 0 {
                return Err(format!("Failed to open registry: code {result}"));
            }

            let byte_len = (exe_wide.len() * 2) as u32;
            let result = RegSetValueExW(
                hkey,
                value_wide.as_ptr(),
                0,
                REG_SZ,
                exe_wide.as_ptr() as *const u8,
                byte_len,
            );
            RegCloseKey(hkey);

            if result != 0 {
                return Err(format!("Failed to set registry value: code {result}"));
            }
        }

        Ok(())
    }

    pub fn disable() -> Result<(), String> {
        let subkey_wide = to_wide(SUBKEY);
        let value_wide = to_wide(VALUE_NAME);

        // SAFETY: RegOpenKeyExW opens HKCU\...\Run with KEY_WRITE.
        // subkey_wide and value_wide are null-terminated UTF-16 vecs alive for the block.
        // hkey is closed via RegCloseKey before return. RegDeleteValueW failure is ignored.
        unsafe {
            let mut hkey: HKEY = std::ptr::null_mut();
            let result = RegOpenKeyExW(
                HKEY_CURRENT_USER,
                subkey_wide.as_ptr(),
                0,
                KEY_WRITE,
                &mut hkey,
            );
            if result != 0 {
                return Ok(());
            }

            let delete_result = RegDeleteValueW(hkey, value_wide.as_ptr());
            if delete_result != 0 {
                debug!("RegDeleteValueW failed with code: {delete_result}");
            }
            RegCloseKey(hkey);
        }

        Ok(())
    }

    pub fn is_enabled() -> Result<bool, String> {
        let subkey_wide = to_wide(SUBKEY);
        let value_wide = to_wide(VALUE_NAME);

        // SAFETY: RegOpenKeyExW opens HKCU\...\Run with KEY_READ.
        // RegQueryValueExW is called with null output pointers (existence check only).
        // hkey is closed via RegCloseKey before return. No data written.
        unsafe {
            let mut hkey: HKEY = std::ptr::null_mut();
            let result = RegOpenKeyExW(
                HKEY_CURRENT_USER,
                subkey_wide.as_ptr(),
                0,
                KEY_READ,
                &mut hkey,
            );
            if result != 0 {
                return Ok(false);
            }

            let result = RegQueryValueExW(
                hkey,
                value_wide.as_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            RegCloseKey(hkey);

            Ok(result == 0)
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) mod linux {
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
            .join("oneshim.service"))
    }

    /// Path for the XDG autostart desktop file (fallback).
    pub fn desktop_path() -> Result<PathBuf, String> {
        let home =
            std::env::var("HOME").map_err(|_| "HOME environment variable not set".to_string())?;
        Ok(PathBuf::from(home)
            .join(".config")
            .join("autostart")
            .join("oneshim.desktop"))
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

    /// Check whether `systemctl` is available on the system.
    pub fn has_systemctl() -> bool {
        Command::new("systemctl")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    pub fn enable() -> Result<(), String> {
        let bin = binary_path()?;

        if has_systemctl() {
            // Primary: systemd user service
            let path = service_path()?;
            let content = generate_service_file(&bin);

            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create systemd user directory: {e}"))?;
            }

            fs::write(&path, content).map_err(|e| format!("Failed to write service file: {e}"))?;

            // Reload systemd daemon to pick up the new unit
            let _ = Command::new("systemctl")
                .args(["--user", "daemon-reload"])
                .output();

            let output = Command::new("systemctl")
                .args(["--user", "enable", "oneshim.service"])
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
            if has_systemctl() {
                let _ = Command::new("systemctl")
                    .args(["--user", "disable", "oneshim.service"])
                    .output();
                let _ = Command::new("systemctl")
                    .args(["--user", "daemon-reload"])
                    .output();
            }
            fs::remove_file(&svc_path)
                .map_err(|e| format!("Failed to remove service file: {e}"))?;
        }

        // Remove XDG desktop file if it exists
        let desk_path = desktop_path()?;
        if desk_path.exists() {
            fs::remove_file(&desk_path)
                .map_err(|e| format!("Failed to remove desktop file: {e}"))?;
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
}

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
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    mod macos_tests {
        use super::*;

        #[test]
        fn plist_xml_contains_required_keys() {
            let plist = macos::generate_plist("/usr/local/bin/oneshim");
            assert!(plist.contains("<key>Label</key>"));
            assert!(plist.contains(APP_LABEL));
            assert!(plist.contains("<key>RunAtLoad</key>"));
            assert!(plist.contains("<true/>"));
            assert!(plist.contains("<key>KeepAlive</key>"));
            assert!(plist.contains("<false/>"));
            assert!(plist.contains("/usr/local/bin/oneshim"));
        }

        #[test]
        fn plist_path_under_launch_agents() {
            let path = macos::plist_path().unwrap();
            assert!(path.to_string_lossy().contains("LaunchAgents"));
            assert!(path.to_string_lossy().ends_with("com.oneshim.agent.plist"));
        }

        #[test]
        fn plist_is_valid_xml() {
            let plist = macos::generate_plist("/usr/local/bin/oneshim");
            assert!(plist.starts_with("<?xml version=\"1.0\""));
            assert!(plist.contains("<!DOCTYPE plist"));
            assert!(plist.contains("<plist version=\"1.0\">"));
            assert!(plist.trim().ends_with("</plist>"));
        }
    }

    #[cfg(target_os = "linux")]
    mod linux_tests {
        use super::*;

        #[test]
        fn service_file_contains_required_keys() {
            let service = linux::generate_service_file("/usr/bin/oneshim");
            assert!(service.contains("[Unit]"));
            assert!(service.contains("[Service]"));
            assert!(service.contains("[Install]"));
            assert!(service.contains("ExecStart=/usr/bin/oneshim"));
            assert!(
                service.contains("Type=notify"),
                "service file must use Type=notify"
            );
            assert!(
                service.contains("NotifyAccess=main"),
                "service file must include NotifyAccess=main"
            );
            assert!(
                service.contains("TimeoutStartSec=30"),
                "service file must include TimeoutStartSec=30"
            );
            assert!(service.contains("Restart=on-failure"));
            assert!(service.contains("WantedBy=default.target"));
        }

        #[test]
        fn service_path_under_systemd_user() {
            let path = linux::service_path().unwrap();
            assert!(path.to_string_lossy().contains("systemd/user"));
            assert!(path.to_string_lossy().ends_with("oneshim.service"));
        }

        #[test]
        fn desktop_file_contains_required_keys() {
            let desktop = linux::generate_desktop_file("/usr/bin/oneshim");
            assert!(desktop.contains("[Desktop Entry]"));
            assert!(desktop.contains("Type=Application"));
            assert!(desktop.contains("Exec=/usr/bin/oneshim"));
            assert!(desktop.contains("X-GNOME-Autostart-enabled=true"));
        }

        #[test]
        fn desktop_path_under_autostart() {
            let path = linux::desktop_path().unwrap();
            assert!(path.to_string_lossy().contains(".config/autostart"));
            assert!(path.to_string_lossy().ends_with("oneshim.desktop"));
        }

        #[test]
        fn service_file_has_restart_policy() {
            let service = linux::generate_service_file("/usr/bin/oneshim");
            assert!(service.contains("Restart=on-failure"));
            assert!(service.contains("RestartSec=5"));
        }
    }

    #[test]
    fn enable_disable_roundtrip_unsupported_platform() {
        let _ = enable_autostart();
        let _ = disable_autostart();
        let _ = is_autostart_enabled();
    }

    #[cfg(target_os = "linux")]
    mod linux_capability_tests {
        use super::*;
        use serial_test::serial;

        fn clear_env() {
            std::env::remove_var("SNAP");
            std::env::remove_var("FLATPAK_ID");
        }

        fn restore_display(prev_display: Option<String>, prev_wayland: Option<String>) {
            if let Some(v) = prev_display {
                std::env::set_var("DISPLAY", v);
            }
            if let Some(v) = prev_wayland {
                std::env::set_var("WAYLAND_DISPLAY", v);
            }
        }

        #[test]
        #[serial]
        fn detect_capabilities_returns_snap_sandbox_when_snap_set() {
            clear_env();
            std::env::set_var("SNAP", "/snap/oneshim/current");
            let caps = detect_capabilities();
            clear_env();
            assert!(!caps.supported);
            assert_eq!(caps.environment, EnvironmentKind::LinuxSnapSandbox);
            assert_eq!(
                caps.unsupported_reason,
                Some(UnsupportedReason::SnapSandbox)
            );
        }

        #[test]
        #[serial]
        fn detect_capabilities_returns_flatpak_sandbox_when_flatpak_id_set() {
            clear_env();
            std::env::set_var("FLATPAK_ID", "com.maekon.client");
            let caps = detect_capabilities();
            clear_env();
            assert!(!caps.supported);
            assert_eq!(caps.environment, EnvironmentKind::LinuxFlatpakSandbox);
            assert_eq!(
                caps.unsupported_reason,
                Some(UnsupportedReason::FlatpakSandbox)
            );
        }

        #[test]
        #[serial]
        fn detect_capabilities_returns_headless_when_no_display() {
            clear_env();
            let prev_display = std::env::var("DISPLAY").ok();
            let prev_wayland = std::env::var("WAYLAND_DISPLAY").ok();
            std::env::remove_var("DISPLAY");
            std::env::remove_var("WAYLAND_DISPLAY");
            let caps = detect_capabilities();
            restore_display(prev_display, prev_wayland);
            assert!(!caps.supported);
            assert_eq!(caps.environment, EnvironmentKind::LinuxHeadless);
            assert_eq!(
                caps.unsupported_reason,
                Some(UnsupportedReason::HeadlessSession)
            );
        }
    }
}
