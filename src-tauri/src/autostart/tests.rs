//! Unit tests for the autostart module.

use super::*;

#[cfg(target_os = "macos")]
mod macos_tests {
    use super::*;

    #[test]
    fn plist_xml_contains_required_keys() {
        let plist = macos::generate_plist("/usr/local/bin/maekon");
        assert!(plist.contains("<key>Label</key>"));
        assert!(plist.contains("com.maekon.app"));
        assert!(plist.contains("<key>RunAtLoad</key>"));
        assert!(plist.contains("<true/>"));
        assert!(plist.contains("<key>KeepAlive</key>"));
        assert!(plist.contains("<false/>"));
        assert!(plist.contains("/usr/local/bin/maekon"));
    }

    #[test]
    fn plist_uses_open_for_macos_app_bundle_binary() {
        let plist =
            macos::generate_plist("/Users/admin/Applications/Maekon Dev.app/Contents/MacOS/maekon");

        assert!(plist.contains("<string>/usr/bin/open</string>"));
        assert!(plist.contains("<string>-na</string>"));
        assert!(plist.contains("<string>/Users/admin/Applications/Maekon Dev.app</string>"));
        assert!(!plist.contains(
            "<string>/Users/admin/Applications/Maekon Dev.app/Contents/MacOS/maekon</string>"
        ));
    }

    #[test]
    fn plist_path_under_launch_agents() {
        let path = macos::plist_path().unwrap();
        assert!(path.to_string_lossy().contains("LaunchAgents"));
        assert!(path.to_string_lossy().ends_with("com.maekon.app.plist"));
    }

    #[test]
    fn plist_is_valid_xml() {
        let plist = macos::generate_plist("/usr/local/bin/maekon");
        assert!(plist.starts_with("<?xml version=\"1.0\""));
        assert!(plist.contains("<!DOCTYPE plist"));
        assert!(plist.contains("<plist version=\"1.0\">"));
        assert!(plist.trim().ends_with("</plist>"));
    }

    #[test]
    fn launchctl_commands_use_gui_bootstrap_domain() {
        let path =
            std::path::PathBuf::from("/Users/admin/Library/LaunchAgents/com.maekon.app.plist");

        assert_eq!(
            macos::launchctl_bootstrap_args("gui/501", &path),
            vec![
                "bootstrap".to_string(),
                "gui/501".to_string(),
                path.to_string_lossy().to_string()
            ]
        );
        assert_eq!(
            macos::launchctl_bootout_args("gui/501", &path),
            vec![
                "bootout".to_string(),
                "gui/501".to_string(),
                path.to_string_lossy().to_string()
            ]
        );
        assert_eq!(
            macos::launchctl_kickstart_args("gui/501"),
            vec![
                "kickstart".to_string(),
                "-k".to_string(),
                "gui/501/com.maekon.app".to_string()
            ]
        );
    }
}

#[cfg(target_os = "linux")]
mod linux_tests {
    use super::*;

    #[test]
    fn service_file_contains_required_keys() {
        let service = linux::generate_service_file("/usr/bin/maekon");
        assert!(service.contains("[Unit]"));
        assert!(service.contains("[Service]"));
        assert!(service.contains("[Install]"));
        assert!(service.contains("ExecStart=/usr/bin/maekon"));
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
        assert!(path.to_string_lossy().ends_with("maekon.service"));
    }

    #[test]
    fn desktop_file_contains_required_keys() {
        let desktop = linux::generate_desktop_file("/usr/bin/maekon");
        assert!(desktop.contains("[Desktop Entry]"));
        assert!(desktop.contains("Type=Application"));
        assert!(desktop.contains("Exec=/usr/bin/maekon"));
        assert!(desktop.contains("X-GNOME-Autostart-enabled=true"));
    }

    #[test]
    fn desktop_path_under_autostart() {
        let path = linux::desktop_path().unwrap();
        assert!(path.to_string_lossy().contains(".config/autostart"));
        assert!(path.to_string_lossy().ends_with("maekon.desktop"));
    }

    // #8058 P2-7: the enable-symlink lives under the WantedBy target's `.wants`
    // dir; its existence — not the `.service` file's — is what `is_enabled` and
    // the honesty contract rely on. `WantedBy=default.target` (asserted above)
    // ⇒ `default.target.wants/`.
    #[test]
    fn wants_symlink_under_default_target_wants() {
        let path = linux::wants_symlink_path().unwrap();
        let s = path.to_string_lossy();
        assert!(s.contains("systemd/user/default.target.wants"));
        assert!(s.ends_with("maekon.service"));
    }

    #[test]
    fn service_file_has_restart_policy() {
        let service = linux::generate_service_file("/usr/bin/maekon");
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
        std::env::set_var("SNAP", "/snap/maekon/current");
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
        std::env::set_var("FLATPAK_ID", "com.maekon.app");
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
