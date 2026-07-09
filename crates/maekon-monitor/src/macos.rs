use crate::active_window_parse::parse_osascript_active_window;
use crate::circuit_breaker::CircuitBreaker;
use crate::error::MonitorError;
use crate::log_privacy::title_digest;
use core_foundation::array::CFArray;
use core_foundation::base::{CFType, TCFType};
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_graphics::window::{
    copy_window_info, kCGNullWindowID, kCGWindowBounds, kCGWindowIsOnscreen, kCGWindowLayer,
    kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly, kCGWindowOwnerName,
    kCGWindowOwnerPID,
};
use maekon_core::models::context::{WindowBounds, WindowInfo};
use maekon_core::models::system::PowerStatus;
use std::path::Path;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{debug, warn};

const SUBPROCESS_TIMEOUT_SECS: u64 = 5;

/// Bare tool names resolved via [`crate::trusted_binary::resolve_trusted_binary`]
/// (SEC-MON-01) at each spawn site, rather than baked in as hard-coded literal
/// paths — this generalizes the original #7483 fix (which hard-coded these two
/// exact `/usr/...` paths) through the shared trusted-directory resolver so
/// every helper in this crate goes through one mechanism instead of diverging
/// per call site.
const OSASCRIPT_TOOL: &str = "osascript";
const IOREG_TOOL: &str = "ioreg";
const PMSET_TOOL: &str = "pmset";

/// After this many consecutive timeouts, skip osascript entirely and return
/// `Ok(None)` until the counter is reset (e.g. after a successful call).
const CIRCUIT_BREAKER_THRESHOLD: u32 = 3;

/// After the circuit breaker trips, only retry once every N calls to check
/// if the permission was granted in the meantime.
const CIRCUIT_BREAKER_RETRY_INTERVAL: u32 = 60;

/// Circuit breaker to avoid spawning osascript every cycle when Accessibility
/// permission is missing. Shared `CircuitBreaker` struct (#7720 E6
/// consolidation) — previously a hand-rolled `AtomicU32` counter duplicating
/// the same state machine as [`IOREG_BREAKER`] below.
static OSASCRIPT_BREAKER: CircuitBreaker =
    CircuitBreaker::new(CIRCUIT_BREAKER_THRESHOLD, CIRCUIT_BREAKER_RETRY_INTERVAL);

/// #6830: independent breaker for the `ioreg` idle-time fork (a different binary
/// than osascript — distinct availability), so a host where `ioreg` is missing or
/// hangs cannot make the monitor fork it on every tick. Mirrors the linux idle breakers.
static IOREG_BREAKER: CircuitBreaker = CircuitBreaker::new(3, 60);

/// Public entry point for active-window detection on macOS.
///
/// Strategy (E20-2): try the NATIVE CoreGraphics + Accessibility path first —
/// it requires no per-cycle subprocess fork — and fall back to the legacy
/// osascript path only when the native path yields nothing (e.g. no window,
/// or its result is filtered as our own window). The osascript fallback keeps
/// the LIVE circuit breaker so a missing Accessibility permission cannot make
/// us fork `osascript` every second.
pub async fn get_active_window_macos() -> Result<Option<WindowInfo>, MonitorError> {
    // Native FFI is synchronous and touches the window server / AX; run it off
    // the async runtime so a slow CoreGraphics call can never stall the reactor.
    let native = tokio::task::spawn_blocking(get_active_window_native)
        .await
        .unwrap_or_else(|join_err| {
            warn!("native active-window task panicked/cancelled: {join_err}");
            None
        });

    if let Some(window) = native {
        // Title is PII — log a content-free digest only (#5638).
        debug!(
            "active window (native): {} ({})",
            window.app_name,
            title_digest(&window.title)
        );
        return Ok(Some(window));
    }

    // Native path returned nothing (no on-screen window, or it was our own /
    // filtered). Fall back to the osascript path, which carries the breaker.
    get_active_window_via_osascript().await
}

/// Native macOS active-window detection: CGWindowList for app/pid/bounds (no
/// permission) + Accessibility `AXTitle` for the window title (gated on the
/// Accessibility permission). Returns `None` when no usable foreground window
/// is found, or when it resolves to our own window (same filters as osascript).
///
/// Synchronous by design; callers run it under `spawn_blocking`.
fn get_active_window_native() -> Option<WindowInfo> {
    // #4794 review: without Accessibility permission we cannot read the window
    // TITLE (AX), and CGWindowList alone can't either (kCGWindowName would need
    // Screen-Recording — deliberately avoided). Returning `None` here makes the
    // orchestrator fall through to the osascript path, which DOES get the title
    // (System Events TCC) and carries the circuit breaker — preserving parity with
    // the pre-#4794 behavior on AX-ungranted hosts. The per-1s osascript fork is
    // eliminated only on the happy path where AX IS granted (the perf goal). The
    // trust check is a cheap syscall (no fork, no prompt), not an osascript fork.
    if !crate::macos_ax_ffi::is_process_trusted() {
        return None;
    }

    let front = frontmost_via_cgwindowlist()?;

    // Apply the same self-window filters the osascript path uses.
    if is_own_app_name(&front.owner_name) {
        debug!("skipping own app window (native): {}", front.owner_name);
        return None;
    }
    if front.owner_pid > 0 && front.owner_pid == std::process::id() {
        debug!(
            "skipping own window by PID (native): {} (pid={})",
            front.owner_name, front.owner_pid
        );
        return None;
    }

    // AX is trusted here (checked above). A genuinely title-less window yields an
    // empty string — correct (the window really has no title; osascript would too),
    // so we keep the native result rather than re-forking osascript.
    let title = title_via_ax(front.owner_pid).unwrap_or_default();

    Some(WindowInfo {
        title,
        app_name: front.owner_name,
        // CGWindowList does not expose the bundle id; the osascript path is the
        // one that fills this in. Native leaves it `None`.
        app_bundle_id: None,
        pid: front.owner_pid,
        bounds: front.bounds,
    })
}

/// Read the focused window's title for `pid` via the Accessibility API.
///
/// Returns `None` when Accessibility permission is not granted (so the caller
/// can fall back to osascript), or when the app has no titled focused window.
fn title_via_ax(pid: u32) -> Option<String> {
    if pid == 0 || pid > i32::MAX as u32 {
        return None;
    }
    crate::macos_ax_ffi::focused_window_title(pid as i32)
}

/// A single CGWindowList entry, reduced to the fields we need. This plain-data
/// struct is what the pure-logic helpers operate on, so they stay headless-
/// testable (no live window server required).
#[derive(Debug, Clone)]
struct RawCgWindow {
    owner_name: String,
    owner_pid: u32,
    layer: i64,
    on_screen: bool,
    bounds: Option<WindowBounds>,
}

/// Query CGWindowList for the frontmost on-screen, layer-0 (normal) window and
/// reduce it to owner name / pid / bounds. No permission required.
fn frontmost_via_cgwindowlist() -> Option<RawCgWindow> {
    // On-screen windows, front-to-back order, excluding desktop chrome.
    let option = kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements;
    // Untyped `CFArray<*const c_void>`; each element is a CFDictionary.
    let info: CFArray = copy_window_info(option, kCGNullWindowID)?;

    let mut parsed: Vec<RawCgWindow> = Vec::with_capacity(info.len() as usize);
    for item in info.iter() {
        // `item` is a raw `*const c_void` from the array (get-rule borrow).
        // SAFETY: the array owns each element; we wrap it under the Get Rule so
        // the temporary `CFType` does not over-release the array's element.
        let cf = unsafe { CFType::wrap_under_get_rule(*item) };
        if let Some(dict) = cf.downcast::<CFDictionary>() {
            if let Some(window) = parse_cg_window_entry(&dict) {
                parsed.push(window);
            }
        }
    }

    pick_frontmost_layer_zero(&parsed).cloned()
}

/// Parse one CGWindowList CFDictionary entry into a [`RawCgWindow`].
///
/// Reads `kCGWindowOwnerName`, `kCGWindowOwnerPID`, `kCGWindowLayer`,
/// `kCGWindowIsOnscreen`, and the nested `kCGWindowBounds` dict.
fn parse_cg_window_entry(dict: &CFDictionary) -> Option<RawCgWindow> {
    // SAFETY: the kCGWindow* statics are CFStringRef constants exported by the
    // CoreGraphics framework (default `link` feature). We only borrow them to
    // build a lookup key; ownership is not transferred.
    let owner_name = cf_dict_string(dict, unsafe { kCGWindowOwnerName }).unwrap_or_default();
    let owner_pid = cf_dict_i64(dict, unsafe { kCGWindowOwnerPID })
        .filter(|pid| *pid >= 0)
        .map(|pid| pid as u32)
        .unwrap_or(0);
    let layer = cf_dict_i64(dict, unsafe { kCGWindowLayer }).unwrap_or(i64::MAX);
    let on_screen = cf_dict_i64(dict, unsafe { kCGWindowIsOnscreen })
        .map(|v| v != 0)
        .unwrap_or(true);
    let bounds = cf_dict_subdict(dict, unsafe { kCGWindowBounds })
        .and_then(|b| parse_cg_window_bounds_dict(&b));

    Some(RawCgWindow {
        owner_name,
        owner_pid,
        layer,
        on_screen,
        bounds,
    })
}

/// Resolve a value pointer for a CoreGraphics `CFStringRef` key constant.
///
/// SAFETY: `key` must be a valid `CFStringRef` (one of the `kCGWindow*`
/// statics). We wrap it under the Get Rule purely to obtain a borrowed
/// `*const c_void` lookup key; ownership of the static is not transferred.
unsafe fn cf_dict_value_for_cg_key(
    dict: &CFDictionary,
    key: core_foundation::string::CFStringRef,
) -> Option<CFType> {
    let key = CFString::wrap_under_get_rule(key);
    let value = dict.find(key.as_CFTypeRef())?;
    // SAFETY: the dictionary owns the value; Get Rule keeps the count balanced.
    Some(CFType::wrap_under_get_rule(*value))
}

/// Look up a `CFString` value by a CoreGraphics CFStringRef key constant.
fn cf_dict_string(
    dict: &CFDictionary,
    key: core_foundation::string::CFStringRef,
) -> Option<String> {
    // SAFETY: `key` is a `kCGWindow*` framework static (valid CFStringRef).
    let cf = unsafe { cf_dict_value_for_cg_key(dict, key) }?;
    cf.downcast::<CFString>().map(|s| s.to_string())
}

/// Look up a numeric value (as i64) by a CoreGraphics CFStringRef key constant.
fn cf_dict_i64(dict: &CFDictionary, key: core_foundation::string::CFStringRef) -> Option<i64> {
    // SAFETY: `key` is a `kCGWindow*` framework static (valid CFStringRef).
    let cf = unsafe { cf_dict_value_for_cg_key(dict, key) }?;
    cf.downcast::<CFNumber>().and_then(|n| n.to_i64())
}

/// Look up a nested CFDictionary value by a CoreGraphics CFStringRef key.
fn cf_dict_subdict(
    dict: &CFDictionary,
    key: core_foundation::string::CFStringRef,
) -> Option<CFDictionary> {
    // SAFETY: `key` is a `kCGWindow*` framework static (valid CFStringRef).
    let cf = unsafe { cf_dict_value_for_cg_key(dict, key) }?;
    cf.downcast::<CFDictionary>()
}

/// Parse a CGWindowBounds CFDictionary (`{X, Y, Width, Height}` numbers) into
/// a [`WindowBounds`], or `None` when the rect is degenerate (zero area).
///
/// Pure logic over a CoreFoundation dictionary — constructible headless, so
/// this is unit-tested directly (CoreFoundation needs no window server).
fn parse_cg_window_bounds_dict(bounds: &CFDictionary) -> Option<WindowBounds> {
    let x = cf_dict_f64_by_str(bounds, "X")?;
    let y = cf_dict_f64_by_str(bounds, "Y")?;
    let width = cf_dict_f64_by_str(bounds, "Width")?;
    let height = cf_dict_f64_by_str(bounds, "Height")?;

    if width <= 0.0 || height <= 0.0 {
        return None;
    }

    Some(WindowBounds {
        x: x as i32,
        y: y as i32,
        width: width as u32,
        height: height as u32,
    })
}

/// Helper for [`parse_cg_window_bounds_dict`]: read an `f64` keyed by a literal
/// Rust string (the bounds sub-dict uses plain "X"/"Y"/"Width"/"Height" keys).
fn cf_dict_f64_by_str(dict: &CFDictionary, key: &str) -> Option<f64> {
    let key = CFString::new(key);
    let value = dict.find(key.as_CFTypeRef())?;
    // SAFETY: the dictionary owns the value; Get Rule keeps the count balanced.
    let cf = unsafe { CFType::wrap_under_get_rule(*value) };
    cf.downcast::<CFNumber>().and_then(|n| n.to_f64())
}

/// Given parsed CGWindowList entries (in CGWindowList front-to-back order),
/// pick the frontmost normal window: the FIRST entry that is on-screen and on
/// layer 0 (the normal application-window layer). Returns `None` when none
/// qualifies (e.g. only menu-bar / overlay layers are present).
///
/// Pure logic — unit-tested headless.
fn pick_frontmost_layer_zero(windows: &[RawCgWindow]) -> Option<&RawCgWindow> {
    windows
        .iter()
        .find(|w| w.on_screen && w.layer == 0 && w.owner_pid > 0)
}

/// Circuit-breaker retry gate for the osascript fallback. Returns `true` if the
/// caller may proceed to spawn osascript, `false` if it must short-circuit.
///
/// Delegates to the shared [`CircuitBreaker`] (#7720 E6 consolidation) — this
/// used to be a hand-rolled `AtomicU32` state machine duplicating the same
/// atomic-claim logic (`compare_exchange` prevents two concurrent callers that
/// read the same retry-interval-boundary value from both spawning osascript,
/// #6007 finding 17). Kept as a thin named wrapper so the call site and the
/// existing test names below stay stable.
fn circuit_breaker_should_proceed() -> bool {
    OSASCRIPT_BREAKER.should_proceed()
}

/// Legacy osascript-based active-window detection. KEPT VERBATIM as the
/// fallback for when the native path returns `None` (e.g. Accessibility
/// permission missing → no AX title, or no usable native foreground window).
/// Carries the LIVE circuit breaker so a missing permission can't make us fork
/// `osascript` every cycle.
async fn get_active_window_via_osascript() -> Result<Option<WindowInfo>, MonitorError> {
    // Circuit-breaker retry gate (extracted so the atomic-claim logic is
    // unit-testable without forking osascript — see circuit_breaker_should_proceed).
    if !circuit_breaker_should_proceed() {
        return Ok(None);
    }

    // SEC-MON-01: resolve against the trusted-directory allowlist instead of
    // spawning a bare `osascript` — fail closed (no PATH fallback) when the
    // binary is not found under any trusted system directory.
    let osascript_path =
        crate::trusted_binary::resolve_trusted_binary(OSASCRIPT_TOOL).ok_or_else(|| {
            MonitorError::Internal(
                "osascript not found under the trusted directory allowlist".to_string(),
            )
        })?;

    let output = timeout(
        Duration::from_secs(SUBPROCESS_TIMEOUT_SECS),
        Command::new(osascript_path)
            .kill_on_drop(true)
            .arg("-e")
            .arg(
                r#"tell application "System Events"
            set fieldSeparator to ASCII character 31
            set frontApp to first application process whose frontmost is true
            set appName to name of frontApp
            set appPid to unix id of frontApp
            set appBundleId to ""
            try
                set appBundleId to bundle identifier of frontApp
            end try
            set winTitle to ""
            set winPos to {0, 0}
            set winSize to {0, 0}
            try
                set frontWin to front window of frontApp
                set winTitle to name of frontWin
                set winPos to position of frontWin
                set winSize to size of frontWin
            end try
            return appName & fieldSeparator & winTitle & fieldSeparator & (item 1 of winPos as integer) & fieldSeparator & (item 2 of winPos as integer) & fieldSeparator & (item 1 of winSize as integer) & fieldSeparator & (item 2 of winSize as integer) & fieldSeparator & (appPid as integer) & fieldSeparator & appBundleId
        end tell"#,
            )
            .output(),
    )
    .await;

    let output = match output {
        Ok(result) => {
            // osascript completed (success or failure, but did not hang)
            OSASCRIPT_BREAKER.record_success();
            result
                .map_err(|e| MonitorError::Internal(format!("osascript execution failure: {e}")))?
        }
        Err(_elapsed) => {
            OSASCRIPT_BREAKER.record_failure();
            if OSASCRIPT_BREAKER.failure_count() == CIRCUIT_BREAKER_THRESHOLD {
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
    let Some(parsed) = parse_osascript_active_window(&raw_stdout) else {
        return Ok(None);
    };

    // Filter out Maekon's own windows (tracking panel, overlay, dashboard).
    // App-name check catches WebView child processes whose PID differs from
    // the main binary (Tauri v2 may spawn separate WebKit processes).
    if is_own_app_name(&parsed.app_name) {
        // Title is PII — log a content-free digest only (#5638).
        debug!(
            "skipping own app window: {} ({})",
            parsed.app_name,
            title_digest(&parsed.title)
        );
        return Ok(None);
    }
    if parsed.pid > 0 && parsed.pid == std::process::id() {
        // Title is PII — log a content-free digest only (#5638).
        debug!(
            "skipping own window by PID: {} ({}) (pid={})",
            parsed.app_name,
            title_digest(&parsed.title),
            parsed.pid
        );
        return Ok(None);
    }

    // Title is PII — log a content-free digest only (#5638).
    debug!(
        "active window: {} ({}) ({:?})",
        parsed.app_name,
        title_digest(&parsed.title),
        parsed
            .bounds
            .map(|b| format!("{}x{} at ({},{})", b.width, b.height, b.x, b.y))
    );

    Ok(Some(WindowInfo {
        title: parsed.title,
        app_name: parsed.app_name,
        app_bundle_id: parsed.bundle_id,
        pid: parsed.pid,
        bounds: parsed.bounds,
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

    let Some(current_exe) = current_exe else {
        // Legacy display names from pre-Maekon bundles can still appear in
        // existing installs and in old macOS accessibility/TCC entries.
        return matches!(app_name, "MAEKON" | "Maekon");
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

pub async fn current_power_status_macos() -> Result<PowerStatus, MonitorError> {
    // SEC-MON-01: resolve against the trusted-directory allowlist instead of
    // a hard-coded literal path — fail closed when `pmset` is not found under
    // any trusted system directory.
    let pmset_path =
        crate::trusted_binary::resolve_trusted_binary(PMSET_TOOL).ok_or_else(|| {
            MonitorError::Internal(
                "pmset not found under the trusted directory allowlist".to_string(),
            )
        })?;

    let output = timeout(
        Duration::from_secs(2),
        Command::new(pmset_path)
            .kill_on_drop(true)
            .arg("-g")
            .arg("batt")
            .output(),
    )
    .await
    .map_err(|_| MonitorError::Internal("pmset power status timed out".to_string()))?
    .map_err(|e| MonitorError::Internal(format!("pmset execution failure: {e}")))?;

    if !output.status.success() {
        return Ok(PowerStatus::default());
    }

    Ok(crate::power_parse::parse_pmset_batt_output(
        &String::from_utf8_lossy(&output.stdout),
    ))
}

// pmset-parsing tests moved to the cfg-free `crate::power_parse` module (#5138)
// so the battery/AC detection + low-battery threshold are verified on every OS.

pub async fn get_idle_time_macos() -> Option<u64> {
    // #6830: short-circuit when the ioreg breaker is open (binary missing / hanging).
    if !IOREG_BREAKER.should_proceed() {
        return None;
    }

    // SEC-MON-01: resolve against the trusted-directory allowlist instead of
    // a hard-coded literal path. A resolution miss is treated the same as a
    // spawn failure (record_failure + None) — the same fail-closed outcome
    // the breaker already applies to an absent/hung `ioreg`.
    let Some(ioreg_path) = crate::trusted_binary::resolve_trusted_binary(IOREG_TOOL) else {
        IOREG_BREAKER.record_failure();
        return None;
    };

    let result = timeout(
        Duration::from_secs(SUBPROCESS_TIMEOUT_SECS),
        Command::new(ioreg_path)
            .kill_on_drop(true)
            .args(["-c", "IOHIDSystem", "-d", "4"])
            .output(),
    )
    .await;
    // Success = the process completed (regardless of exit status); a spawn error or
    // timeout advances the breaker. This follows the shared CircuitBreaker/xdotool
    // semantics (any completed run resets, absent/hung advances) — deliberately NOT
    // the legacy osascript breaker, which resets on spawn-error and advances only on timeout.
    let output = match result {
        Ok(Ok(output)) => {
            IOREG_BREAKER.record_success();
            output
        }
        Ok(Err(_)) | Err(_) => {
            IOREG_BREAKER.record_failure();
            return None;
        }
    };

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

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    // Tests below mutate the module-level OSASCRIPT_BREAKER static (shared
    // CircuitBreaker, #7720 E6 consolidation).
    // #[serial] forces them to run one at a time to prevent cross-test
    // races where one test's reset clobbers another's
    // set_failure_count(CIRCUIT_BREAKER_THRESHOLD) precondition.

    #[test]
    fn subprocess_paths_resolve_absolute_under_trusted_allowlist() {
        // SEC-MON-01: the paths are no longer literal constants — they are
        // resolved through the shared trusted-directory allowlist at each
        // spawn site. This locks that the base-OS tools this crate depends on
        // (osascript/ioreg/pmset all ship with every macOS install under
        // /usr/bin or /usr/sbin) still resolve to an absolute path there.
        for tool in [OSASCRIPT_TOOL, IOREG_TOOL, PMSET_TOOL] {
            let resolved = crate::trusted_binary::resolve_trusted_binary(tool)
                .unwrap_or_else(|| panic!("{tool} must resolve under the trusted allowlist"));
            assert!(resolved.is_absolute(), "{tool} path must be absolute");
        }
    }

    #[tokio::test]
    #[serial]
    async fn get_active_window_returns_result() {
        // Reset circuit breaker for test isolation
        OSASCRIPT_BREAKER.record_success();
        let result = get_active_window_macos().await;
        // Either Ok(Some(..)) if a foreground window is resolvable (native or
        // osascript), Ok(None) if not, or Err if osascript timed out.
        // This assertion is a tautology — it merely proves the call does not
        // panic or hang. Justified: macOS GUI session may or may not be
        // available on CI; both Ok and Err are valid outcomes (#5594).
        let _ = result; // call returns without panic — contract verified
    }

    #[tokio::test]
    async fn get_idle_time_returns_result() {
        let idle = get_idle_time_macos().await;
        if let Some(secs) = idle {
            assert!(secs < 86400 * 365); // less than 1 year
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
        // The circuit breaker is a property of the osascript fallback path, so
        // exercise it directly: the native CGWindowList path runs first in the
        // orchestrator and (on a host with a live window server) would resolve a
        // real window, bypassing the breaker entirely.
        OSASCRIPT_BREAKER.set_failure_count(CIRCUIT_BREAKER_THRESHOLD);

        // Should return Ok(None) immediately without spawning osascript.
        let result = get_active_window_via_osascript()
            .await
            .expect("tripped breaker must short-circuit to Ok");
        assert!(
            result.is_none(),
            "tripped breaker must skip osascript and yield None"
        );

        // Counter should have incremented
        let count = OSASCRIPT_BREAKER.failure_count();
        assert!(count > CIRCUIT_BREAKER_THRESHOLD);

        // Reset for other tests
        OSASCRIPT_BREAKER.record_success();
    }

    #[test]
    #[serial]
    fn circuit_breaker_reset_on_zero() {
        OSASCRIPT_BREAKER.record_success();
        assert_eq!(OSASCRIPT_BREAKER.failure_count(), 0);
    }

    #[test]
    fn own_app_name_detects_dev_bundle_name() {
        let exe = Path::new("/tmp/Maekon Dev.app/Contents/MacOS/maekon");
        assert!(is_own_app_name_for_exe("Maekon Dev", Some(exe)));
    }

    #[test]
    fn own_app_name_detects_executable_name_from_bundle() {
        let exe = Path::new("/tmp/Maekon Dev.app/Contents/MacOS/maekon");
        assert!(is_own_app_name_for_exe("maekon", Some(exe)));
    }

    #[test]
    fn own_app_name_does_not_skip_release_bundle_from_dev_bundle() {
        let exe = Path::new("/tmp/Maekon Dev.app/Contents/MacOS/maekon");
        assert!(!is_own_app_name_for_exe("Maekon", Some(exe)));
    }

    #[test]
    fn own_app_name_keeps_legacy_names() {
        assert!(is_own_app_name_for_exe("MAEKON", None));
        assert!(is_own_app_name_for_exe("Maekon", None));
    }

    // ── Native CGWindowList pure-logic tests ────────────────────────────────
    //
    // These exercise the headless-safe helpers only. CoreFoundation objects
    // (CFDictionary/CFNumber/CFString) are constructible without a window
    // server, so building a synthetic CGWindowBounds dict is reliable here.
    // We do NOT test `frontmost_via_cgwindowlist` / `title_via_ax` directly —
    // those need a live window server / AX permission and would be flaky.

    use core_foundation::dictionary::CFDictionary as CFDict;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString as CFStr;

    /// Build a synthetic CGWindowBounds-style dict (`{X, Y, Width, Height}`).
    fn bounds_dict(x: f64, y: f64, w: f64, h: f64) -> CFDictionary {
        CFDict::from_CFType_pairs(&[
            (CFStr::new("X"), CFNumber::from(x)),
            (CFStr::new("Y"), CFNumber::from(y)),
            (CFStr::new("Width"), CFNumber::from(w)),
            (CFStr::new("Height"), CFNumber::from(h)),
        ])
        .into_untyped()
    }

    fn raw(owner: &str, pid: u32, layer: i64, on_screen: bool) -> RawCgWindow {
        RawCgWindow {
            owner_name: owner.to_string(),
            owner_pid: pid,
            layer,
            on_screen,
            bounds: None,
        }
    }

    #[test]
    fn parse_bounds_dict_reads_all_four_fields() {
        let dict = bounds_dict(12.0, 34.0, 800.0, 600.0);
        let bounds = parse_cg_window_bounds_dict(&dict).expect("bounds present");
        assert_eq!(bounds.x, 12);
        assert_eq!(bounds.y, 34);
        assert_eq!(bounds.width, 800);
        assert_eq!(bounds.height, 600);
    }

    #[test]
    fn parse_bounds_dict_truncates_fractional_coords() {
        // CGWindowBounds values can be fractional on HiDPI; we truncate to ints.
        let dict = bounds_dict(-5.9, 10.4, 1280.6, 720.2);
        let bounds = parse_cg_window_bounds_dict(&dict).expect("bounds present");
        assert_eq!(bounds.x, -5);
        assert_eq!(bounds.y, 10);
        assert_eq!(bounds.width, 1280);
        assert_eq!(bounds.height, 720);
    }

    #[test]
    fn parse_bounds_dict_rejects_zero_area() {
        assert!(parse_cg_window_bounds_dict(&bounds_dict(0.0, 0.0, 0.0, 100.0)).is_none());
        assert!(parse_cg_window_bounds_dict(&bounds_dict(0.0, 0.0, 100.0, 0.0)).is_none());
    }

    #[test]
    fn parse_bounds_dict_missing_key_is_none() {
        let dict = CFDict::from_CFType_pairs(&[
            (CFStr::new("X"), CFNumber::from(1.0)),
            (CFStr::new("Y"), CFNumber::from(2.0)),
            // Width / Height intentionally absent.
        ])
        .into_untyped();
        assert!(parse_cg_window_bounds_dict(&dict).is_none());
    }

    #[test]
    fn pick_frontmost_skips_non_zero_layers() {
        // Menu-bar / overlay layers (non-zero) appear first; the first layer-0
        // on-screen window should win.
        let windows = vec![
            raw("Menubar", 100, 25, true), // status bar layer
            raw("Dock", 101, 20, true),    // dock layer
            raw("Safari", 200, 0, true),   // real app window
            raw("Finder", 300, 0, true),   // behind Safari
        ];
        let front = pick_frontmost_layer_zero(&windows).expect("a layer-0 window");
        assert_eq!(front.owner_name, "Safari");
        assert_eq!(front.owner_pid, 200);
    }

    #[test]
    fn pick_frontmost_requires_on_screen() {
        let windows = vec![
            raw("Hidden", 100, 0, false), // off-screen, must be skipped
            raw("Visible", 200, 0, true),
        ];
        let front = pick_frontmost_layer_zero(&windows).expect("an on-screen window");
        assert_eq!(front.owner_name, "Visible");
    }

    #[test]
    fn pick_frontmost_requires_real_pid() {
        // pid 0 entries (no owner) must not be selected.
        let windows = vec![raw("Phantom", 0, 0, true), raw("Real", 42, 0, true)];
        let front = pick_frontmost_layer_zero(&windows).expect("a real-pid window");
        assert_eq!(front.owner_pid, 42);
    }

    #[test]
    fn pick_frontmost_none_when_no_normal_window() {
        let windows = vec![raw("Menubar", 100, 25, true), raw("Cursor", 101, 99, true)];
        assert!(pick_frontmost_layer_zero(&windows).is_none());
    }

    #[test]
    fn pick_frontmost_empty_is_none() {
        assert!(pick_frontmost_layer_zero(&[]).is_none());
    }

    /// Verify that when the counter is exactly at a retry boundary, only ONE
    /// concurrent caller is allowed to proceed (the CAS winner); a caller that
    /// arrives while the counter still reads the same multiple-of-N value loses
    /// the CAS and short-circuits to Ok(None).
    ///
    /// This is a black-box regression check against this site's exact tuning
    /// constants (threshold=3, retry_interval=60), through the shared
    /// [`CircuitBreaker`]'s public API. The double-spawn race this guards
    /// against (finding #6007) — and the canonical white-box proof of the
    /// underlying `compare_exchange` semantics — now live once, centrally, in
    /// `maekon_core::circuit_breaker::tests::retry_slot_claimed_only_once`
    /// (#7720 E6 consolidation).
    #[test]
    #[serial]
    fn circuit_breaker_retry_slot_claimed_only_once() {
        // Exercise the pure gate decision directly (no osascript fork): this avoids
        // the environment dependency where a real osascript call SUCCEEDS on a GUI
        // host and resets the counter to 0, and it isolates the atomic-claim logic.
        let retry_boundary = CIRCUIT_BREAKER_RETRY_INTERVAL;
        assert!(
            retry_boundary >= CIRCUIT_BREAKER_THRESHOLD,
            "test precondition: retry interval must be >= threshold"
        );

        // First caller at the boundary claims the slot (proceeds) and advances the
        // counter exactly one past the boundary.
        OSASCRIPT_BREAKER.set_failure_count(retry_boundary);
        assert!(
            circuit_breaker_should_proceed(),
            "first caller at the retry boundary must claim the slot and proceed"
        );
        let after_first = OSASCRIPT_BREAKER.failure_count();
        assert_eq!(
            after_first,
            retry_boundary + 1,
            "counter must advance exactly one past the retry boundary"
        );
        assert!(
            !after_first.is_multiple_of(CIRCUIT_BREAKER_RETRY_INTERVAL),
            "counter ({after_first}) must not be a multiple of the retry interval \
             ({CIRCUIT_BREAKER_RETRY_INTERVAL}) after the slot is claimed"
        );

        // A caller arriving after the slot was claimed (counter past the boundary)
        // must skip rather than spawn a second osascript.
        assert!(
            !circuit_breaker_should_proceed(),
            "a caller arriving after the slot was claimed must short-circuit"
        );

        OSASCRIPT_BREAKER.record_success();
    }
}
