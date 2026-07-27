//! Foreground external-window fullscreen detection (#8849) — for the overlay
//! fullscreen-suppression policy (CRT-PRV-OVL-005).
//!
//! The pure decision function [`window_covers_monitor`] is unit-testable on
//! every host (an honest oracle for synthetic scenarios that mimic an external
//! fullscreen window). Platform-specific coordinate collection (Win32 /
//! CGWindowList / X11) requires a real desktop session and demands live-runtime
//! verification on the supported targets.

use maekon_core::models::context::WindowBounds;
use maekon_core::ports::foreground_window::ForegroundFullscreenProbe;

/// Pixel tolerance allowed when comparing a window rect against a monitor rect.
/// A borderless "fake fullscreen" window matches the monitor exactly, but
/// exclusive fullscreen and DPI rounding can shift the edges off by a pixel or
/// two.
pub const COVER_TOLERANCE_PX: i32 = 2;

/// Pure decision: does `window` cover `monitor` within `tolerance` pixels on
/// each edge (fullscreen / monitor-covering)? A zero-area rect never covers.
///
/// Unit-testable on every platform — an honest oracle that validates the
/// "external fullscreen window" decision independent of real OS state (#8849).
#[must_use]
pub fn window_covers_monitor(window: WindowBounds, monitor: WindowBounds, tolerance: i32) -> bool {
    if window.width == 0 || window.height == 0 || monitor.width == 0 || monitor.height == 0 {
        return false;
    }
    let within = |a: i32, b: i32| (a - b).abs() <= tolerance;
    within(window.x, monitor.x)
        && within(window.y, monitor.y)
        && within(window.width as i32, monitor.width as i32)
        && within(window.height as i32, monitor.height as i32)
}

/// Platform adapter that implements [`ForegroundFullscreenProbe`].
#[derive(Debug, Default, Clone, Copy)]
pub struct PlatformForegroundFullscreenProbe;

impl ForegroundFullscreenProbe for PlatformForegroundFullscreenProbe {
    fn foreground_is_fullscreen(&self) -> Option<bool> {
        foreground_window_is_fullscreen()
    }
}

/// Detects whether the foreground **external** window is fullscreen /
/// monitor-covering. `None` when it cannot be determined (see
/// [`ForegroundFullscreenProbe`] for the detailed contract).
#[must_use]
pub fn foreground_window_is_fullscreen() -> Option<bool> {
    #[cfg(target_os = "windows")]
    {
        crate::windows::foreground_window_is_fullscreen_windows()
    }
    #[cfg(target_os = "macos")]
    {
        macos_foreground_is_fullscreen()
    }
    #[cfg(target_os = "linux")]
    {
        crate::linux::foreground_window_is_fullscreen_linux()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

/// macOS: determines fullscreen by comparing the bounds of the frontmost
/// **external** window (CGWindowList frontmost, excluding windows owned by our
/// PID) against the bounds of the display that contains it. CGDisplay access is
/// macOS-only, so this lives here (feature module, macOS cfg).
#[cfg(target_os = "macos")]
fn macos_foreground_is_fullscreen() -> Option<bool> {
    let front = crate::macos::frontmost_via_cgwindowlist()?;
    if front.owner_pid == std::process::id() {
        // Our own window — excluded (handled by the overlay policy's owned-window path).
        return None;
    }
    let bounds = front.bounds?;
    let monitor = macos_display_bounds_containing(&bounds)?;
    Some(window_covers_monitor(bounds, monitor, COVER_TOLERANCE_PX))
}

/// macOS: the global-coordinate bounds of the display containing the window's
/// center point. Falls back to the main display when no display matches.
#[cfg(target_os = "macos")]
fn macos_display_bounds_containing(win: &WindowBounds) -> Option<WindowBounds> {
    use core_graphics::display::CGDisplay;
    let cx = win.x as f64 + win.width as f64 / 2.0;
    let cy = win.y as f64 + win.height as f64 / 2.0;
    let to_bounds = |rect: core_graphics::geometry::CGRect| WindowBounds {
        x: rect.origin.x as i32,
        y: rect.origin.y as i32,
        width: rect.size.width.max(0.0) as u32,
        height: rect.size.height.max(0.0) as u32,
    };
    if let Ok(ids) = CGDisplay::active_displays() {
        for id in ids {
            let b = CGDisplay::new(id).bounds();
            if cx >= b.origin.x
                && cx < b.origin.x + b.size.width
                && cy >= b.origin.y
                && cy < b.origin.y + b.size.height
            {
                return Some(to_bounds(b));
            }
        }
    }
    Some(to_bounds(CGDisplay::main().bounds()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(x: i32, y: i32, w: u32, h: u32) -> WindowBounds {
        WindowBounds {
            x,
            y,
            width: w,
            height: h,
        }
    }

    #[test]
    fn exact_cover_is_fullscreen() {
        let monitor = bounds(0, 0, 1920, 1080);
        // Borderless fullscreen: the client rect matches the monitor exactly (#8858).
        assert!(window_covers_monitor(
            bounds(0, 0, 1920, 1080),
            monitor,
            COVER_TOLERANCE_PX
        ));
    }

    #[test]
    fn within_tolerance_is_fullscreen() {
        let monitor = bounds(0, 0, 1920, 1080);
        // A 1px offset from DPI rounding still counts as covering.
        assert!(window_covers_monitor(
            bounds(1, 0, 1919, 1080),
            monitor,
            COVER_TOLERANCE_PX
        ));
    }

    #[test]
    fn beyond_tolerance_is_not_fullscreen() {
        let monitor = bounds(0, 0, 1920, 1080);
        // Windowed mode (a small window) does not cover.
        assert!(!window_covers_monitor(
            bounds(100, 100, 1280, 720),
            monitor,
            COVER_TOLERANCE_PX
        ));
        // An edge off by more than the tolerance does not cover.
        assert!(!window_covers_monitor(
            bounds(0, 0, 1910, 1080),
            monitor,
            COVER_TOLERANCE_PX
        ));
    }

    #[test]
    fn secondary_monitor_offset_origin_covers() {
        // Secondary monitor with an offset origin: a window that matches that monitor's origin/size covers it.
        let monitor = bounds(1920, 0, 2560, 1440);
        assert!(window_covers_monitor(
            bounds(1920, 0, 2560, 1440),
            monitor,
            COVER_TOLERANCE_PX
        ));
        // Same size but a mismatched origin (a different location) does not cover.
        assert!(!window_covers_monitor(
            bounds(0, 0, 2560, 1440),
            monitor,
            COVER_TOLERANCE_PX
        ));
    }

    #[test]
    fn zero_area_never_covers() {
        let monitor = bounds(0, 0, 1920, 1080);
        assert!(!window_covers_monitor(
            bounds(0, 0, 0, 0),
            monitor,
            COVER_TOLERANCE_PX
        ));
        assert!(!window_covers_monitor(
            bounds(0, 0, 1920, 1080),
            bounds(0, 0, 0, 0),
            COVER_TOLERANCE_PX
        ));
    }
}
