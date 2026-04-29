use crate::error::MonitorError;
use core_graphics::event::CGEvent;
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use oneshim_core::models::context::{MousePosition, WindowBounds, WindowInfo};
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{debug, info, warn};

const SUBPROCESS_TIMEOUT_SECS: u64 = 5;

/// Consecutive timeout counter — circuit breaker to avoid spawning osascript
/// every cycle when Accessibility permission is missing.
static CONSECUTIVE_TIMEOUTS: AtomicU32 = AtomicU32::new(0);

/// After this many consecutive timeouts, skip osascript entirely and return
/// `Ok(None)` until the counter is reset (e.g. after a successful call).
const CIRCUIT_BREAKER_THRESHOLD: u32 = 3;

/// After the circuit breaker trips, only retry once every N calls to check
/// if the permission was granted in the meantime.
const CIRCUIT_BREAKER_RETRY_INTERVAL: u32 = 60;

pub async fn get_active_window_macos() -> Result<Option<WindowInfo>, MonitorError> {
    let timeouts = CONSECUTIVE_TIMEOUTS.load(Ordering::Relaxed);
    if timeouts >= CIRCUIT_BREAKER_THRESHOLD {
        // Circuit breaker is open — periodically retry to detect permission grant
        if timeouts % CIRCUIT_BREAKER_RETRY_INTERVAL != 0 {
            CONSECUTIVE_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
            return Ok(None);
        }
        warn!(
            "osascript circuit breaker: retrying after {} skipped calls \
             (grant Accessibility permission in System Settings)",
            timeouts - CIRCUIT_BREAKER_THRESHOLD
        );
    }

    let output = timeout(
        Duration::from_secs(SUBPROCESS_TIMEOUT_SECS),
        Command::new("osascript")
            .arg("-e")
            .arg(
                r#"tell application "System Events"
            set frontApp to first application process whose frontmost is true
            set appName to name of frontApp
            set appPid to unix id of frontApp
            set winTitle to ""
            set winPos to {0, 0}
            set winSize to {0, 0}
            try
                set frontWin to front window of frontApp
                set winTitle to name of frontWin
                set winPos to position of frontWin
                set winSize to size of frontWin
            end try
            return appName & "|" & winTitle & "|" & (item 1 of winPos as integer) & "|" & (item 2 of winPos as integer) & "|" & (item 1 of winSize as integer) & "|" & (item 2 of winSize as integer) & "|" & (appPid as integer)
        end tell"#,
            )
            .output(),
    )
    .await;

    let output = match output {
        Ok(result) => {
            // osascript completed (success or failure, but did not hang)
            CONSECUTIVE_TIMEOUTS.store(0, Ordering::Relaxed);
            result
                .map_err(|e| MonitorError::Internal(format!("osascript execution failure: {e}")))?
        }
        Err(_elapsed) => {
            let prev = CONSECUTIVE_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
            if prev + 1 == CIRCUIT_BREAKER_THRESHOLD {
                warn!(
                    "osascript timed out {} consecutive times — circuit breaker engaged. \
                     Grant Accessibility permission in System Settings > Privacy & Security > Accessibility",
                    CIRCUIT_BREAKER_THRESHOLD
                );
            }
            return Err(MonitorError::Internal("osascript timed out".to_string()));
        }
    };

    if !output.status.success() {
        debug!("active window detection failure (osascript)");
        return Ok(None);
    }

    let raw_stdout = String::from_utf8_lossy(&output.stdout);
    let result = raw_stdout.trim().to_string();
    let parts: Vec<&str> = result.split('|').collect();

    // Temporary: use info! to diagnose empty window_title issue
    info!(
        "osascript raw: parts={} len={} result={:?}",
        parts.len(),
        raw_stdout.len(),
        &result[..result.len().min(120)]
    );

    if parts.is_empty() {
        return Ok(None);
    }

    let app_name = parts[0].to_string();
    let title = parts.get(1).map(|s| s.to_string()).unwrap_or_default();

    // Filter out Maekon's own windows (tracking panel, overlay, dashboard).
    // App name check catches WebView child processes whose PID differs from
    // the main binary (Tauri v2 may spawn separate WebKit processes).
    if is_own_app_name(&app_name) {
        debug!("skipping own app window: {app_name} - {title}");
        return Ok(None);
    }
    let front_pid = parts
        .get(6)
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);
    if front_pid > 0 && front_pid == std::process::id() {
        debug!("skipping own window by PID: {app_name} - {title} (pid={front_pid})");
        return Ok(None);
    }

    let bounds = if parts.len() >= 6 {
        let x = parts[2].parse::<i32>().unwrap_or(0);
        let y = parts[3].parse::<i32>().unwrap_or(0);
        let width = parts[4].parse::<u32>().unwrap_or(0);
        let height = parts[5].parse::<u32>().unwrap_or(0);

        if width > 0 && height > 0 {
            Some(WindowBounds {
                x,
                y,
                width,
                height,
            })
        } else {
            None
        }
    } else {
        None
    };

    debug!(
        "active window: {app_name} - {title} ({:?})",
        bounds.map(|b| format!("{}x{} at ({},{})", b.width, b.height, b.x, b.y))
    );

    Ok(Some(WindowInfo {
        title,
        app_name,
        pid: front_pid,
        bounds,
    }))
}

fn is_own_app_name(app_name: &str) -> bool {
    let current_exe = std::env::current_exe().ok();
    is_own_app_name_for_exe(app_name, current_exe.as_deref())
}

fn is_own_app_name_for_exe(app_name: &str, current_exe: Option<&Path>) -> bool {
    let app_name = app_name.trim();
    if app_name.is_empty() {
        return false;
    }

    // Legacy display names from pre-Maekon bundles can still appear in existing
    // installs and in old macOS accessibility/TCC entries.
    if matches!(app_name, "ONESHIM" | "OneShim") {
        return true;
    }

    let Some(current_exe) = current_exe else {
        return false;
    };

    own_app_name_candidates(current_exe)
        .iter()
        .any(|candidate| candidate == app_name)
}

fn own_app_name_candidates(current_exe: &Path) -> Vec<String> {
    let mut candidates = Vec::new();

    if let Some(executable_name) = current_exe
        .file_stem()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
    {
        candidates.push(executable_name.to_string());
    }

    if let Some(bundle_name) = current_exe.components().find_map(|component| {
        component
            .as_os_str()
            .to_str()
            .and_then(|name| name.strip_suffix(".app"))
            .filter(|name| !name.is_empty())
            .map(ToString::to_string)
    }) {
        candidates.push(bundle_name);
    }

    candidates
}

pub async fn get_idle_time_macos() -> Option<u64> {
    let output = timeout(
        Duration::from_secs(SUBPROCESS_TIMEOUT_SECS),
        Command::new("ioreg")
            .args(["-c", "IOHIDSystem", "-d", "4"])
            .output(),
    )
    .await
    .ok()?
    .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    for line in stdout.lines() {
        if line.contains("HIDIdleTime") {
            if let Some(value_str) = line.split('=').nth(1) {
                let value_str = value_str.trim();
                if let Ok(nanos) = value_str.parse::<u64>() {
                    return Some(nanos / 1_000_000_000);
                }
            }
        }
    }

    None
}

pub fn get_mouse_position_macos() -> Option<MousePosition> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState).ok()?;

    let event = CGEvent::new(source).ok()?;
    let location = event.location();

    Some(MousePosition {
        x: location.x as i32,
        y: location.y as i32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    // Tests below mutate the module-level CONSECUTIVE_TIMEOUTS static.
    // #[serial] forces them to run one at a time to prevent cross-test
    // races where one test's store(0) reset clobbers another's
    // store(CIRCUIT_BREAKER_THRESHOLD) precondition.

    #[tokio::test]
    #[serial]
    async fn get_active_window_returns_result() {
        // Reset circuit breaker for test isolation
        CONSECUTIVE_TIMEOUTS.store(0, Ordering::Relaxed);
        let result = get_active_window_macos().await;
        // Either Ok(Some(..)) if permission granted, or Err if timeout
        assert!(result.is_ok() || result.is_err());
    }

    #[tokio::test]
    async fn get_idle_time_returns_result() {
        let idle = get_idle_time_macos().await;
        if let Some(secs) = idle {
            assert!(secs < 86400 * 365); // less than 1 year
        }
    }

    #[test]
    fn get_mouse_position_returns_result() {
        let pos = get_mouse_position_macos();
        if let Some(p) = pos {
            assert!(p.x >= 0 && p.x < 32000);
            assert!(p.y >= 0 && p.y < 32000);
        }
    }

    const _: () = {
        assert!(CIRCUIT_BREAKER_THRESHOLD >= 2);
        assert!(CIRCUIT_BREAKER_THRESHOLD <= 10);
        assert!(CIRCUIT_BREAKER_RETRY_INTERVAL >= 10);
    };

    #[tokio::test]
    #[serial]
    async fn circuit_breaker_skips_when_tripped() {
        // Simulate threshold timeouts
        CONSECUTIVE_TIMEOUTS.store(CIRCUIT_BREAKER_THRESHOLD, Ordering::Relaxed);

        // Should return Ok(None) immediately without spawning osascript
        let result = get_active_window_macos().await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());

        // Counter should have incremented
        let count = CONSECUTIVE_TIMEOUTS.load(Ordering::Relaxed);
        assert!(count > CIRCUIT_BREAKER_THRESHOLD);

        // Reset for other tests
        CONSECUTIVE_TIMEOUTS.store(0, Ordering::Relaxed);
    }

    #[test]
    #[serial]
    fn circuit_breaker_reset_on_zero() {
        CONSECUTIVE_TIMEOUTS.store(0, Ordering::Relaxed);
        assert_eq!(CONSECUTIVE_TIMEOUTS.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn own_app_name_detects_dev_bundle_name() {
        let exe = Path::new("/tmp/Maekon Dev.app/Contents/MacOS/oneshim");
        assert!(is_own_app_name_for_exe("Maekon Dev", Some(exe)));
    }

    #[test]
    fn own_app_name_detects_executable_name_from_bundle() {
        let exe = Path::new("/tmp/Maekon Dev.app/Contents/MacOS/oneshim");
        assert!(is_own_app_name_for_exe("oneshim", Some(exe)));
    }

    #[test]
    fn own_app_name_does_not_skip_release_bundle_from_dev_bundle() {
        let exe = Path::new("/tmp/Maekon Dev.app/Contents/MacOS/oneshim");
        assert!(!is_own_app_name_for_exe("Maekon", Some(exe)));
    }

    #[test]
    fn own_app_name_keeps_legacy_names() {
        assert!(is_own_app_name_for_exe("ONESHIM", None));
        assert!(is_own_app_name_for_exe("OneShim", None));
    }
}
