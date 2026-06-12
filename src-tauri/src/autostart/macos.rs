//! macOS autostart implementation.
//!
//! Primary path: `SMAppService.mainAppService` (macOS 13+).
//! Fallback: legacy LaunchAgent plist at `~/Library/LaunchAgents/com.maekon.app.plist`.

use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2_foundation::NSString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const APP_LABEL: &str = "com.maekon.app";

#[link(name = "ServiceManagement", kind = "framework")]
unsafe extern "C" {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmAppServiceStatus {
    NotRegistered,
    Enabled,
    RequiresApproval,
    NotFound,
    Unknown(isize),
}

impl SmAppServiceStatus {
    fn from_raw(raw: isize) -> Self {
        match raw {
            0 => Self::NotRegistered,
            1 => Self::Enabled,
            2 => Self::RequiresApproval,
            3 => Self::NotFound,
            value => Self::Unknown(value),
        }
    }

    fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }

    fn requires_approval(self) -> bool {
        matches!(self, Self::RequiresApproval)
    }
}

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

fn app_bundle_path_from_executable(program_path: &str) -> Option<String> {
    let marker = ".app/Contents/MacOS/";
    let marker_start = program_path.find(marker)?;
    let bundle_end = marker_start + ".app".len();
    Some(program_path[..bundle_end].to_string())
}

fn program_arguments(program_path: &str) -> Vec<String> {
    if let Some(bundle_path) = app_bundle_path_from_executable(program_path) {
        return vec!["/usr/bin/open".to_string(), "-na".to_string(), bundle_path];
    }

    vec![program_path.to_string()]
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn render_program_arguments(program_path: &str) -> String {
    program_arguments(program_path)
        .into_iter()
        .map(|argument| format!("        <string>{}</string>\n", escape_xml(&argument)))
        .collect()
}

pub fn generate_plist(program_path: &str) -> String {
    let program_arguments = render_program_arguments(program_path);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{APP_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
{program_arguments}    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <false/>
    <key>StandardOutPath</key>
    <string>/tmp/maekon-client.out.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/maekon-client.err.log</string>
</dict>
</plist>
"#
    )
}

pub fn launchctl_bootstrap_args(domain: &str, path: &Path) -> Vec<String> {
    vec![
        "bootstrap".to_string(),
        domain.to_string(),
        path.to_string_lossy().to_string(),
    ]
}

pub fn launchctl_bootout_args(domain: &str, path: &Path) -> Vec<String> {
    vec![
        "bootout".to_string(),
        domain.to_string(),
        path.to_string_lossy().to_string(),
    ]
}

pub fn launchctl_kickstart_args(domain: &str) -> Vec<String> {
    vec![
        "kickstart".to_string(),
        "-k".to_string(),
        format!("{domain}/{APP_LABEL}"),
    ]
}

fn launchctl_gui_domain() -> Result<String, String> {
    let output = Command::new("id")
        .arg("-u")
        .output()
        .map_err(|e| format!("id -u failure: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "id -u failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let uid = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if uid.is_empty() {
        return Err("id -u returned empty uid".to_string());
    }
    Ok(format!("gui/{uid}"))
}

fn run_launchctl(args: &[String]) -> Result<(), String> {
    let output = Command::new("launchctl")
        .args(args)
        .output()
        .map_err(|e| format!("launchctl failure: {e}"))?;
    if output.status.success() {
        return Ok(());
    }

    Err(format!(
        "launchctl {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

fn smappservice_class() -> Result<&'static AnyClass, String> {
    AnyClass::get(c"SMAppService").ok_or_else(|| "SMAppService class unavailable".to_string())
}

fn smappservice_main_app() -> Result<*mut AnyObject, String> {
    let service_class = smappservice_class()?;
    let service: *mut AnyObject = unsafe { msg_send![service_class, mainAppService] };
    if service.is_null() {
        return Err("SMAppService.mainAppService returned null".to_string());
    }
    Ok(service)
}

fn smappservice_error_message(error: *mut AnyObject) -> Option<String> {
    if error.is_null() {
        return None;
    }

    unsafe {
        let desc: Retained<NSString> = msg_send![&*error, localizedDescription];
        let domain: Retained<NSString> = msg_send![&*error, domain];
        let code: isize = msg_send![&*error, code];
        Some(format!("{} (domain: {}, code: {})", desc, domain, code))
    }
}

pub fn smappservice_main_app_status() -> Result<SmAppServiceStatus, String> {
    let service = smappservice_main_app()?;
    let raw_status: isize = unsafe { msg_send![service, status] };
    Ok(SmAppServiceStatus::from_raw(raw_status))
}

fn register_main_app_service() -> Result<(), String> {
    let service = smappservice_main_app()?;
    let mut error_ptr: *mut AnyObject = std::ptr::null_mut();
    let registered: Bool = unsafe { msg_send![service, registerAndReturnError: &mut error_ptr] };
    if registered.as_bool() {
        return Ok(());
    }

    let status = smappservice_main_app_status()
        .map(|status| format!("{status:?}"))
        .unwrap_or_else(|status_error| format!("status unavailable: {status_error}"));
    let message = smappservice_error_message(error_ptr)
        .unwrap_or_else(|| "unknown SMAppService register error".to_string());
    Err(format!(
        "SMAppService main app register failed: {message}; status: {status}"
    ))
}

fn unregister_main_app_service() -> Result<(), String> {
    let service = smappservice_main_app()?;
    let mut error_ptr: *mut AnyObject = std::ptr::null_mut();
    let unregistered: Bool =
        unsafe { msg_send![service, unregisterAndReturnError: &mut error_ptr] };
    if unregistered.as_bool() {
        return Ok(());
    }

    let status = smappservice_main_app_status()?;
    if matches!(
        status,
        SmAppServiceStatus::NotRegistered | SmAppServiceStatus::NotFound
    ) {
        return Ok(());
    }

    let message = smappservice_error_message(error_ptr)
        .unwrap_or_else(|| "unknown SMAppService unregister error".to_string());
    Err(format!(
        "SMAppService main app unregister failed: {message}; status: {status:?}"
    ))
}

fn enable_legacy_launch_agent() -> Result<(), String> {
    let path = plist_path()?;
    let bin = binary_path()?;
    let plist_content = generate_plist(&bin);
    let domain = launchctl_gui_domain()?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create LaunchAgents directory: {e}"))?;
    }

    fs::write(&path, plist_content).map_err(|e| format!("Failed to write plist file: {e}"))?;

    let _ = run_launchctl(&launchctl_bootout_args(&domain, &path));
    run_launchctl(&launchctl_bootstrap_args(&domain, &path))?;
    run_launchctl(&launchctl_kickstart_args(&domain))?;

    Ok(())
}

fn disable_legacy_launch_agent() -> Result<(), String> {
    let path = plist_path()?;
    let domain = launchctl_gui_domain()?;

    if path.exists() {
        let _ = run_launchctl(&launchctl_bootout_args(&domain, &path));

        fs::remove_file(&path).map_err(|e| format!("plist delete failure: {e}"))?;
    }

    Ok(())
}

fn is_legacy_launch_agent_enabled() -> Result<bool, String> {
    let path = plist_path()?;
    Ok(path.exists())
}

pub fn enable() -> Result<(), String> {
    match register_main_app_service() {
        Ok(()) => Ok(()),
        Err(error) => {
            let status = smappservice_main_app_status().ok();
            if status.is_some_and(SmAppServiceStatus::requires_approval) {
                return Err(error);
            }
            tracing::warn!(
                error,
                "SMAppService main app autostart failed; falling back to legacy LaunchAgent"
            );
            enable_legacy_launch_agent()
        }
    }
}

pub fn disable() -> Result<(), String> {
    let smappservice_result = unregister_main_app_service();
    let legacy_result = disable_legacy_launch_agent();
    match (smappservice_result, legacy_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => {
            if smappservice_main_app_status()
                .map(SmAppServiceStatus::is_enabled)
                .unwrap_or(false)
            {
                Err(error)
            } else {
                Ok(())
            }
        }
        (Ok(()), Err(error)) | (Err(_), Err(error)) => Err(error),
    }
}

pub fn is_enabled() -> Result<bool, String> {
    let smappservice_enabled = smappservice_main_app_status()
        .map(SmAppServiceStatus::is_enabled)
        .unwrap_or(false);
    if smappservice_enabled {
        return Ok(true);
    }
    is_legacy_launch_agent_enabled()
}
