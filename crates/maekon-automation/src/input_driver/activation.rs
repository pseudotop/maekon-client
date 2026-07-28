use tracing::warn;

use maekon_core::error::CoreError;

#[cfg(target_os = "linux")]
use super::trusted_paths::resolve_trusted_helper;
#[cfg(target_os = "macos")]
use super::trusted_paths::{is_trusted_program, TRUSTED_OPEN_PATH};

/// Platform window activation for `EnigoInputDriver::activate_app`.
///
/// Returns `Ok(true)` when the activation command reported success, `Ok(false)`
/// when activation is unsupported (no tool available in a trusted location /
/// unknown platform) or the target window was not found, and
/// `Err(ExecutionTimeout)` when the helper outlives `ACTIVATE_APP_TIMEOUT` (the
/// bound that stops a wedged shell-out from hanging the caller, #8055 P2-3).
/// `app_name` is validated then passed without shell interpolation (argv on
/// macOS/Linux; env var on Windows) → no injection surface. Each helper binary
/// is resolved to a trusted absolute path (never the inherited `PATH`) so a
/// shadowing `PATH` entry cannot run a planted binary with the agent's
/// privileges (CWE-426/427, #7075) — mirroring the `/usr/bin/sandbox-exec` and
/// worker-resolution discipline in the `sandbox` module.
pub(super) async fn activate_app_platform(app_name: &str) -> Result<bool, CoreError> {
    // Bound + sanitize untrusted name (preset/LLM/template-driven).
    let name = app_name.trim();
    if name.is_empty() || name.len() > 256 || name.contains(['\n', '\r', '\0']) {
        return Err(CoreError::InvalidArguments {
            code: maekon_core::error_codes::ValidationCode::InvalidArguments,
            message: "invalid app_name for activation (empty / too long / control chars)"
                .to_string(),
        });
    }

    #[cfg(target_os = "macos")]
    {
        // `open -a <name>` activates a running app or launches it; name is an argv
        // element (no shell), so there is no AppleScript/shell injection surface.
        // Resolve `open` to its trusted absolute path (/usr/bin/open) instead of a
        // bare-name PATH lookup so a shadowing PATH entry cannot run a planted
        // binary (#7075).
        use tokio::process::Command;
        let open_path = std::path::Path::new(TRUSTED_OPEN_PATH);
        if !is_trusted_program(open_path) {
            warn!("activate_app: trusted /usr/bin/open unavailable — unsupported");
            return Ok(false);
        }
        let mut cmd = Command::new(open_path);
        cmd.arg("-a").arg(name);
        match run_activation_command(cmd, ACTIVATE_APP_TIMEOUT).await? {
            ActivationRun::Finished(activated) => Ok(activated),
            ActivationRun::SpawnFailed => Err(CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: "open -a failed to spawn".to_string(),
            }),
        }
    }

    #[cfg(target_os = "linux")]
    {
        // wmctrl -a matches a window by name and activates it; fall back to xdotool.
        // Both receive `name` as argv (no shell). These are OPTIONAL tools resolved
        // to a trusted absolute path under a fixed system directory — never the
        // inherited PATH — so a planted ~/.local/bin binary shadowing wmctrl/xdotool
        // cannot be executed (#7075). If neither tool is installed in a trusted
        // location, report unsupported (false) rather than erroring.
        use tokio::process::Command;
        if let Some(wmctrl) = resolve_trusted_helper("wmctrl") {
            let mut cmd = Command::new(&wmctrl);
            cmd.arg("-a").arg(name);
            // A `?` here surfaces a wedged helper as ExecutionTimeout instead of a
            // hang; a spawn failure falls through to the xdotool fallback below.
            if let ActivationRun::Finished(activated) =
                run_activation_command(cmd, ACTIVATE_APP_TIMEOUT).await?
            {
                return Ok(activated);
            }
        }
        if let Some(xdotool) = resolve_trusted_helper("xdotool") {
            let mut cmd = Command::new(&xdotool);
            cmd.args(["search", "--name"])
                .arg(name)
                .arg("windowactivate");
            if let ActivationRun::Finished(activated) =
                run_activation_command(cmd, ACTIVATE_APP_TIMEOUT).await?
            {
                return Ok(activated);
            }
        }
        warn!(
            app_name = name,
            "activate_app: neither wmctrl nor xdotool available in a trusted path — unsupported"
        );
        Ok(false)
    }

    #[cfg(target_os = "windows")]
    {
        // The builtin host alias needs stricter targeting than AppActivate provides.
        // Maekon owns a main dashboard plus tracking/overlay windows under one PID;
        // AppActivate(PID) selects the most-recent overlay, which may immediately hide
        // after an action and leave the desktop foregrounded. Enumerate this process's
        // top-level windows and restore the exact stable dashboard title instead.
        if windows_is_maekon_host_alias(name) {
            return Ok(windows_activate_current_maekon_dashboard());
        }

        // WScript.Shell.AppActivate retains title matching for user-authored third-party
        // targets. The name is passed via an env var and read as `$env:...` so it is
        // never interpolated into the script string → no PowerShell injection. Resolve
        // powershell.exe to its absolute System32 location (ACL-protected: only
        // admins/TrustedInstaller can write there) instead of a bare-name PATH
        // lookup so a shadowing PATH entry cannot run a planted powershell.exe
        // (#7075). %SystemRoot% is an OS-managed variable; if it were tampered with
        // the attacker would already have the agent's privileges.
        use tokio::process::Command;
        let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
        let powershell = std::path::PathBuf::from(format!(
            "{system_root}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"
        ));
        if !powershell.is_file() {
            warn!(
                powershell = %powershell.display(),
                "activate_app: trusted powershell.exe not found — unsupported"
            );
            return Ok(false);
        }
        let script = "$ErrorActionPreference='Stop'; \
            $n = $env:MAEKON_ACTIVATE_APP_NAME; \
            $w = New-Object -ComObject WScript.Shell; \
            if ($w.AppActivate($n)) { exit 0 } else { exit 1 }";
        let mut cmd = Command::new(&powershell);
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("MAEKON_ACTIVATE_APP_NAME", name);
        match run_activation_command(cmd, ACTIVATE_APP_TIMEOUT).await? {
            ActivationRun::Finished(activated) => Ok(activated),
            ActivationRun::SpawnFailed => Err(CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: "powershell AppActivate failed to spawn".to_string(),
            }),
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = name;
        Ok(false)
    }
}

#[cfg(target_os = "windows")]
const WINDOWS_MAEKON_MAIN_WINDOW_TITLE: &str = "Maekon";

#[cfg(target_os = "windows")]
pub(super) fn windows_is_maekon_host_alias(app_name: &str) -> bool {
    app_name.trim().eq_ignore_ascii_case("maekon")
}

#[cfg(target_os = "windows")]
pub(super) fn windows_is_maekon_main_window_title(title: &str) -> bool {
    title == WINDOWS_MAEKON_MAIN_WINDOW_TITLE
}

#[cfg(target_os = "windows")]
struct WindowsHostWindowSearch {
    process_id: u32,
    handle: windows_sys::Win32::Foundation::HWND,
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn windows_find_maekon_dashboard(
    handle: windows_sys::Win32::Foundation::HWND,
    search_ptr: windows_sys::Win32::Foundation::LPARAM,
) -> windows_sys::core::BOOL {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
    };

    // SAFETY: `windows_activate_current_maekon_dashboard` passes a valid mutable
    // `WindowsHostWindowSearch` pointer that remains alive for the synchronous
    // EnumWindows call. User32 supplies valid top-level HWND values.
    let search = unsafe { &mut *(search_ptr as *mut WindowsHostWindowSearch) };
    if unsafe { IsWindowVisible(handle) } == 0 {
        return 1;
    }

    let mut process_id = 0;
    unsafe { GetWindowThreadProcessId(handle, &mut process_id) };
    if process_id != search.process_id {
        return 1;
    }

    let title_len = unsafe { GetWindowTextLengthW(handle) };
    if title_len <= 0 {
        return 1;
    }
    let mut title = vec![0_u16; title_len as usize + 1];
    let copied = unsafe { GetWindowTextW(handle, title.as_mut_ptr(), title.len() as i32) };
    if copied <= 0 {
        return 1;
    }
    let title = String::from_utf16_lossy(&title[..copied as usize]);
    if windows_is_maekon_main_window_title(&title) {
        search.handle = handle;
        return 0;
    }
    1
}

/// Restore and foreground the stable main dashboard owned by this Maekon process.
#[cfg(target_os = "windows")]
fn windows_activate_current_maekon_dashboard() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, SetForegroundWindow, ShowWindow, SW_RESTORE,
    };

    let mut search = WindowsHostWindowSearch {
        process_id: std::process::id(),
        handle: std::ptr::null_mut(),
    };
    // SAFETY: EnumWindows invokes the callback synchronously while `search` is alive.
    // The callback validates process ownership and exact title before retaining HWND.
    unsafe {
        EnumWindows(
            Some(windows_find_maekon_dashboard),
            (&mut search as *mut WindowsHostWindowSearch) as isize,
        );
    }
    if search.handle.is_null() {
        return false;
    }

    // SAFETY: the retained HWND came from EnumWindows during this call. ShowWindow
    // restores a minimized dashboard; SetForegroundWindow reports whether Windows
    // accepted the foreground transition.
    unsafe {
        ShowWindow(search.handle, SW_RESTORE);
        SetForegroundWindow(search.handle) != 0
    }
}

/// Upper bound for a single window-activation shell-out (#8055 P2-3).
///
/// A normal activate/launch returns in well under a second; this cap converts a
/// wedged helper (a launch modal, a stalled LaunchServices / window-manager
/// call) into a prompt `ExecutionTimeout` instead of an unbounded hang. Without
/// it, a hung `activate_app` pins the per-suggestion action reservation RAII
/// guard (`src-tauri/src/commands/suggestions/action.rs`) for the whole process
/// lifetime, so the suggestion stays locked as "action already running" until
/// the agent restarts. Mirrors the sandbox worker's timeout + `kill_on_drop`
/// discipline (`sandbox/*.rs`) and the 10s per-action GUI cap.
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
const ACTIVATE_APP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Outcome of a window-activation shell-out that did NOT time out.
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
#[derive(Debug)]
pub(super) enum ActivationRun {
    /// The helper ran to completion; the bool is `ExitStatus::success()`.
    Finished(bool),
    /// The helper could not be spawned. macOS/Windows treat this as a hard
    /// error; the Linux path uses it to fall through to the next helper.
    SpawnFailed,
}

/// Spawn `cmd` with `kill_on_drop` and wait for it under a `timeout`.
///
/// On timeout the returned future (which owns the spawned child) is dropped;
/// `kill_on_drop(true)` then SIGKILLs the helper so no orphan is left behind
/// (same discipline as the sandbox workers, finding #5967). `cmd` is always a
/// trusted absolute path with `name` passed as argv / env var — no shell
/// interpolation — so this helper adds bounding only, no new execution surface.
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
pub(super) async fn run_activation_command(
    mut cmd: tokio::process::Command,
    timeout: std::time::Duration,
) -> Result<ActivationRun, CoreError> {
    cmd.kill_on_drop(true);
    match tokio::time::timeout(timeout, cmd.status()).await {
        Ok(Ok(status)) => Ok(ActivationRun::Finished(status.success())),
        Ok(Err(_spawn_err)) => Ok(ActivationRun::SpawnFailed),
        Err(_elapsed) => Err(CoreError::ExecutionTimeout {
            code: maekon_core::error_codes::SandboxCode::Timeout,
            timeout_ms: timeout.as_millis() as u64,
        }),
    }
}
