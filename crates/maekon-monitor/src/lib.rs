// Cast safety: system metrics, CPU percentages, process counters — precision loss acceptable.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
// P2 PR-C: `missing_const_for_fn` accepted crate-wide.
// Rationale: const-viral cascade + nursery false-positive rate outweigh the value.
#![allow(clippy::missing_const_for_fn)]
// P2 remaining-nursery-lints: stylistic/cosmetic nursery lints accepted crate-wide.
#![allow(
    clippy::use_self,
    clippy::option_if_let_else,
    clippy::redundant_pub_crate
)]
// P2 PR-A nursery-hardening. (Enforced workspace-wide via
// `[workspace.lints.clippy]`, #7719.)
#![cfg_attr(test, allow(clippy::significant_drop_tightening))]

//! # maekon-monitor

pub mod error;
pub use error::MonitorError;

pub mod activity;
pub mod capture_observer;
pub mod clipboard;
pub mod file_access;
// Foreground external-window fullscreen detection (#8849). The pure decision
// function compiles + unit-tests on every host; only platform-specific
// coordinate collection is delegated to each OS module.
pub mod foreground_fullscreen;
// Shared running-flag + thread-handle + platform-wake skeleton for
// `key_hook`/`mouse_hook` (#7727) -- private, reached crate-wide via
// `crate::hook_lifecycle`, mirroring the `trusted_binary` mod pattern below.
mod hook_lifecycle;
pub mod idle;
pub mod input_activity;
pub mod input_detail;
pub mod key_hook;
pub mod keyboard_pattern;
pub mod mouse_hook;
pub mod process;
pub mod system;
pub mod system_info;
pub mod window_layout;

// Pure, cfg-free parsers for the Windows active-window FFI shim (#5120). Kept
// un-gated so the logic is unit-testable on any OS, not only a Windows runner.
mod active_window_parse;
// Cfg-free macOS `pmset -g batt` → PowerStatus parser (#5138), same rationale.
mod power_parse;
// Compatibility re-export for the subprocess-spawn circuit breaker (#6828).
// The state machine itself now lives in `maekon_core::circuit_breaker`
// (#7720 E6 consolidation, shared with `maekon-vision`'s accessibility
// guards); this module keeps the `crate::circuit_breaker::CircuitBreaker`
// path stable for every existing call site in this crate.
mod circuit_breaker;
// Native EWMH active-window via pure-Rust x11rb (#6828). Un-gated so it compiles +
// the title-decode helper is unit-tested on every host; only `linux` CALLS the
// connection path (it returns None without a reachable X server elsewhere).
mod x11_active_window;
// SEC-MON-01: trusted-path resolver for bare-name external-command spawns
// (#7574, generalizing the macOS-only fix in #7483). Un-gated so the resolver
// and its per-OS directory table are unit-tested on every host; `macos`,
// `linux`, `clipboard`, and `key_hook::linux` all route their subprocess
// spawns through it. `pub` + re-exported below so `src-tauri` — which spawns
// the SAME class of bare-name system tools (osascript, powershell, xdotool,
// etc.) from the composition-root binary — shares this ONE allowlist instead
// of duplicating it (intra-binary hardening divergence closed).
mod trusted_binary;
pub use trusted_binary::resolve_trusted_binary;
// Content-free log digest for window titles + suggestion content (#5591, #6006)
// + workspace-wide textual guard against raw-title/content tracing.
// Un-gated so the guard runs on every host OS.
pub mod log_privacy;

/// Whether two nonempty rectangles overlap in one shared desktop coordinate space.
/// Touching edges do not overlap. Callers must provide native coordinates in the
/// same units; this predicate does not infer a display or convert DPI scales.
#[must_use]
pub fn capture_bounds_intersect(
    a: &maekon_core::models::context::WindowBounds,
    b: &maekon_core::models::context::WindowBounds,
) -> bool {
    capture_axis_intersects(a.x, a.width, b.x, b.width)
        && capture_axis_intersects(a.y, a.height, b.y, b.height)
}

fn capture_axis_intersects(a: i32, a_length: u32, b: i32, b_length: u32) -> bool {
    a_length > 0
        && b_length > 0
        && i64::from(a) < i64::from(b) + i64::from(b_length)
        && i64::from(b) < i64::from(a) + i64::from(a_length)
}

/// Select one real target, never a nearest/primary fallback. Public for the
/// runtime's portable synthetic multi-display acceptance tests.
///
/// # Errors
/// Returns `GeometryUnavailable` for invalid or duplicate displays, an unknown
/// requested ID, an ambiguous center, or a selected display that does not overlap.
pub fn capture_display_for_window(
    displays: &[maekon_core::ports::vision::CaptureDisplay],
    bounds: &maekon_core::models::context::WindowBounds,
    requested: Option<u32>,
) -> Result<
    maekon_core::ports::vision::CaptureDisplay,
    maekon_core::ports::vision::CaptureObservationError,
> {
    use maekon_core::ports::vision::CaptureObservationError::GeometryUnavailable;
    let cx = i64::from(bounds.x) * 2 + i64::from(bounds.width);
    let cy = i64::from(bounds.y) * 2 + i64::from(bounds.height);
    let mut ids = std::collections::HashSet::new();
    if displays
        .iter()
        .any(|d| !ids.insert(d.id) || d.bounds.width == 0 || d.bounds.height == 0)
    {
        return Err(GeometryUnavailable);
    }
    let mut candidates = displays.iter().filter(|d| match requested {
        Some(id) => d.id == id,
        None => {
            cx >= i64::from(d.bounds.x) * 2
                && cx < (i64::from(d.bounds.x) + i64::from(d.bounds.width)) * 2
                && cy >= i64::from(d.bounds.y) * 2
                && cy < (i64::from(d.bounds.y) + i64::from(d.bounds.height)) * 2
        }
    });
    let display = candidates.next().ok_or(GeometryUnavailable)?;
    if candidates.next().is_some() || !capture_bounds_intersect(bounds, &display.bounds) {
        return Err(GeometryUnavailable);
    }
    Ok(display.clone())
}

#[cfg(test)]
mod capture_display_selection_tests {
    use super::capture_display_for_window;
    use maekon_core::models::context::WindowBounds;
    use maekon_core::ports::vision::{CaptureDisplay, CaptureObservationError};

    fn bounds(x: i32, y: i32, width: u32, height: u32) -> WindowBounds {
        WindowBounds {
            x,
            y,
            width,
            height,
        }
    }

    fn display(id: u32, bounds: WindowBounds) -> CaptureDisplay {
        CaptureDisplay { id, bounds }
    }

    fn assert_display(actual: &CaptureDisplay, expected: &CaptureDisplay) {
        assert_eq!(
            (
                actual.id,
                actual.bounds.x,
                actual.bounds.y,
                actual.bounds.width,
                actual.bounds.height
            ),
            (
                expected.id,
                expected.bounds.x,
                expected.bounds.y,
                expected.bounds.width,
                expected.bounds.height
            )
        );
    }

    fn assert_unavailable(
        displays: &[CaptureDisplay],
        window: &WindowBounds,
        requested: Option<u32>,
    ) {
        assert_eq!(
            capture_display_for_window(displays, window, requested).unwrap_err(),
            CaptureObservationError::GeometryUnavailable
        );
    }

    #[test]
    fn explicit_intersecting_display_preserves_its_full_identity_and_bounds() {
        let displays = [
            display(41, bounds(-1920, 0, 1920, 1080)),
            display(9, bounds(0, 0, 1440, 900)),
        ];
        // The window spans both displays and its center lies on display 9.
        // An explicit acquisition target 41 must still be honored exactly.
        let window = bounds(-100, 50, 500, 600);
        let explicit = capture_display_for_window(&displays, &window, Some(41)).unwrap();
        assert_display(&explicit, &displays[0]);
        let automatic = capture_display_for_window(&displays, &window, None).unwrap();
        assert_display(&automatic, &displays[1]);
    }

    #[test]
    fn automatic_selection_handles_negative_origins_on_both_axes() {
        let displays = [
            display(41, bounds(-1920, 0, 1920, 1080)),
            display(9, bounds(0, 0, 1440, 900)),
            display(73, bounds(0, -1200, 1200, 1200)),
        ];
        for (window, expected) in [
            (bounds(-1800, 20, 800, 600), &displays[0]),
            (bounds(200, 100, 600, 400), &displays[1]),
            (bounds(100, -1100, 600, 800), &displays[2]),
        ] {
            let actual = capture_display_for_window(&displays, &window, None).unwrap();
            assert_display(&actual, expected);
        }
    }

    #[test]
    fn fractional_centers_and_shared_edges_choose_one_exact_quadrant() {
        let displays = [
            display(11, bounds(-100, -100, 100, 100)),
            display(12, bounds(0, -100, 100, 100)),
            display(21, bounds(-100, 0, 100, 100)),
            display(22, bounds(0, 0, 100, 100)),
        ];
        for (window, expected) in [
            (bounds(-1, -1, 1, 1), &displays[0]),
            (bounds(-1, -1, 2, 1), &displays[1]),
            (bounds(-1, -1, 1, 2), &displays[2]),
            (bounds(-1, -1, 2, 2), &displays[3]),
            (bounds(-2, -2, 3, 3), &displays[0]),
            (bounds(0, 0, 1, 1), &displays[3]),
        ] {
            let actual = capture_display_for_window(&displays, &window, None).unwrap();
            assert_display(&actual, expected);
        }
        // A center exactly on the outer edge has no containing display.
        assert_unavailable(&displays, &bounds(99, 10, 2, 10), None);
        assert_unavailable(&displays, &bounds(10, 99, 10, 2), None);
    }

    #[test]
    fn empty_unknown_and_missing_targets_never_use_a_fallback() {
        let displays = [display(37, bounds(0, 0, 100, 100))];
        let window = bounds(10, 10, 20, 20);
        assert_unavailable(&[], &window, None);
        assert_unavailable(&[], &window, Some(37));
        assert_unavailable(&displays, &window, Some(999));
        assert_unavailable(&displays, &bounds(500, 500, 20, 20), None);
    }

    #[test]
    fn duplicate_ids_in_any_display_invalidate_the_observation() {
        let displays = [
            display(37, bounds(0, 0, 100, 100)),
            display(8, bounds(100, 0, 100, 100)),
            display(8, bounds(200, 0, 100, 100)),
        ];
        let window = bounds(10, 10, 20, 20);
        assert_unavailable(&displays, &window, Some(37));
        assert_unavailable(&displays, &window, None);
    }

    #[test]
    fn zero_display_dimensions_are_rejected_even_for_an_unselected_display() {
        let selected = display(37, bounds(0, 0, 100, 100));
        let window = bounds(10, 10, 20, 20);
        for invalid in [bounds(100, 0, 0, 100), bounds(100, 0, 100, 0)] {
            let displays = [selected.clone(), display(8, invalid)];
            assert_unavailable(&displays, &window, Some(37));
            assert_unavailable(&displays, &window, None);
        }
    }

    #[test]
    fn empty_window_bounds_cannot_authorize_an_intersection() {
        let displays = [display(37, bounds(0, 0, 100, 100))];
        for window in [bounds(10, 10, 0, 20), bounds(10, 10, 20, 0)] {
            assert_unavailable(&displays, &window, Some(37));
            assert_unavailable(&displays, &window, None);
        }
    }

    #[test]
    fn requested_display_must_overlap_in_both_axes() {
        let displays = [
            display(37, bounds(0, 0, 100, 100)),
            display(8, bounds(100, 0, 100, 100)),
        ];
        for window in [
            bounds(100, 10, 20, 20),
            bounds(10, 100, 20, 20),
            bounds(-20, 10, 20, 20),
            bounds(10, -20, 20, 20),
        ] {
            assert_unavailable(&displays, &window, Some(37));
        }
    }

    #[test]
    fn ambiguous_centers_require_an_explicit_unambiguous_id() {
        let displays = [
            display(37, bounds(0, 0, 100, 100)),
            display(8, bounds(20, 20, 100, 100)),
        ];
        let window = bounds(30, 30, 20, 20);
        assert_unavailable(&displays, &window, None);
        for expected in &displays {
            let actual = capture_display_for_window(&displays, &window, Some(expected.id)).unwrap();
            assert_display(&actual, expected);
        }
    }

    #[test]
    fn desktop_edge_and_center_arithmetic_is_widened_before_calculation() {
        let displays = [display(91, bounds(i32::MIN, i32::MIN, u32::MAX, u32::MAX))];
        let window = bounds(i32::MAX - 4, i32::MAX - 4, 4, 4);
        let actual = capture_display_for_window(&displays, &window, None).unwrap();
        assert_display(&actual, &displays[0]);
    }
}

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "macos")]
mod macos_ax_ffi;

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(target_os = "linux")]
pub mod linux;

/// Whether active-window detection is expected to work reliably on this
/// platform + runtime session.
///
/// macOS (`osascript`) and Windows (`GetForegroundWindow`) each have a single,
/// dependable native active-window API and always report `true`. Linux is
/// runtime-dependent: an X11 session queries `_NET_ACTIVE_WINDOW` natively and
/// reliably, but a Wayland session has no single dependable path — modern
/// GNOME disables `org.gnome.Shell.Eval` by default since GNOME 41, and any
/// compositor without a dedicated adapter falls back to XWayland (X11/XWayland
/// apps only; native Wayland apps stay invisible). See
/// `linux::get_active_window_linux()` for the full GNOME → Sway → XWayland
/// fallback chain and its own one-time `warn!` on the GNOME-disabled path.
///
/// This is a conservative, cheap (env-var read only, no subprocess spawn)
/// signal — it reads the same `$XDG_SESSION_TYPE`/`$WAYLAND_DISPLAY`/`$DISPLAY`
/// state as [`linux::detect_display_server`], not a live probe of which
/// compositor adapter actually succeeds. A Wayland session running Sway (which
/// DOES have a working native path via `swaymsg`) is conservatively reported
/// as `false` here — the safe direction (never over-promise a working
/// capability).
///
/// `FeatureCapabilitySnapshot.active_window_available` (src-tauri) is the
/// single consumer of this predicate (#7678).
#[cfg(target_os = "linux")]
#[must_use]
pub fn active_window_reliable() -> bool {
    !matches!(
        crate::linux::detect_display_server(),
        crate::linux::DisplayServer::Wayland
    )
}

#[cfg(not(target_os = "linux"))]
#[must_use]
pub const fn active_window_reliable() -> bool {
    true
}

/// Owner-app names of all normal, on-screen windows on the current display(s).
///
/// Used by the capture-time partial-occlusion guard (#8054 P2-4) to detect
/// background windows of excluded/sensitive apps that share the screen with the
/// active (non-excluded) window. macOS enumerates via CGWindowList (owner names
/// only — titles need screen-recording permission). Windows and Linux have no
/// cheap, permission-free all-window enumeration wired yet, so they return an
/// empty list (the active-app exclusion check still applies on every platform);
/// broader coverage is tracked as follow-up.
#[must_use]
pub fn visible_window_app_names() -> Vec<String> {
    #[cfg(target_os = "macos")]
    {
        crate::macos::visible_window_app_names()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

#[cfg(test)]
mod active_window_capability_tests {
    use super::active_window_reliable;

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn non_linux_platforms_are_always_reliable() {
        assert!(active_window_reliable());
    }

    /// On Linux, the flag must track `detect_display_server()` directly (not
    /// merely the same env-var reads duplicated) so a future edit to one
    /// without the other is caught here instead of silently drifting
    /// (mirrors maekon-vision's `native_ocr` cross-check, #7602/#7678).
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_flag_matches_detect_display_server() {
        let expected = !matches!(
            crate::linux::detect_display_server(),
            crate::linux::DisplayServer::Wayland
        );
        assert_eq!(active_window_reliable(), expected);
    }
}

#[cfg(test)]
mod capture_geometry_tests {
    use super::capture_bounds_intersect;
    use maekon_core::models::context::WindowBounds;

    fn bounds(x: i32, y: i32, width: u32, height: u32) -> WindowBounds {
        WindowBounds {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn negative_origins_overlap_and_containment_are_symmetric() {
        let display = bounds(-1920, -1080, 1920, 1080);
        for window in [
            bounds(-1800, -1000, 900, 700),
            bounds(-100, -100, 200, 200),
            display,
        ] {
            assert!(capture_bounds_intersect(&display, &window));
            assert!(capture_bounds_intersect(&window, &display));
        }
    }

    #[test]
    fn overlap_requires_both_axes() {
        let a = bounds(-10, -10, 10, 10);
        for b in [bounds(-5, 1, 10, 10), bounds(1, -5, 10, 10)] {
            assert!(!capture_bounds_intersect(&a, &b));
            assert!(!capture_bounds_intersect(&b, &a));
        }
        assert!(capture_bounds_intersect(&a, &bounds(-5, -5, 10, 10)));
    }

    #[test]
    fn edge_and_corner_contact_are_not_pixel_overlap() {
        let a = bounds(-10, -10, 10, 10);
        for b in [
            bounds(0, -10, 10, 10),
            bounds(-10, 0, 10, 10),
            bounds(0, 0, 10, 10),
            bounds(-20, -10, 10, 10),
            bounds(-10, -20, 10, 10),
        ] {
            assert!(!capture_bounds_intersect(&a, &b));
            assert!(!capture_bounds_intersect(&b, &a));
        }
    }

    #[test]
    fn one_coordinate_unit_of_overlap_is_retained() {
        let a = bounds(-10, -10, 10, 10);
        for b in [bounds(-1, -10, 10, 10), bounds(-10, -1, 10, 10)] {
            assert!(capture_bounds_intersect(&a, &b));
            assert!(capture_bounds_intersect(&b, &a));
        }
    }

    #[test]
    fn either_zero_dimension_rejects_in_both_argument_orders() {
        let full = bounds(-10, -10, 20, 20);
        for empty in [bounds(0, 0, 0, 1), bounds(0, 0, 1, 0), bounds(0, 0, 0, 0)] {
            assert!(!capture_bounds_intersect(&full, &empty));
            assert!(!capture_bounds_intersect(&empty, &full));
        }
    }

    #[test]
    fn signed_origins_and_unsigned_extents_do_not_overflow() {
        let wide = bounds(i32::MIN, i32::MIN, u32::MAX, u32::MAX);
        let inside = bounds(i32::MAX - 1, i32::MAX - 1, 1, 1);
        let touching = bounds(i32::MAX, i32::MAX, 1, 1);
        let high = bounds(i32::MAX, i32::MAX, u32::MAX, u32::MAX);
        assert!(capture_bounds_intersect(&wide, &inside));
        assert!(capture_bounds_intersect(&inside, &wide));
        assert!(!capture_bounds_intersect(&wide, &touching));
        assert!(!capture_bounds_intersect(&touching, &wide));
        assert!(capture_bounds_intersect(&high, &high));
        assert!(!capture_bounds_intersect(
            &high,
            &bounds(i32::MIN, i32::MIN, 1, 1)
        ));
    }
}
