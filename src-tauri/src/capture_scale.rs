//! HiDPI capture scale-factor resolution (#8054 P2-1).
//!
//! `CaptureRequest.screen_scale_factor` drives
//! `maekon_vision::ocr_geometry::scale_ocr_regions_to_logical`, which converts
//! OCR boxes from captured *physical* pixels back into the *logical* pixel space
//! the overlay and GUI coordinates use. Before this module every capture site
//! injected `None`, so on Retina/HiDPI displays OCR regions stayed at 2x
//! physical resolution while overlay/element-finder coordinates were logical —
//! a silent 2x mismatch that broke element correlation and highlight placement.
//!
//! The scale factor is sourced from Tauri's per-monitor `scale_factor()` (the
//! same value the overlay/tracking-panel layout already uses), resolved for the
//! monitor containing the active window. The Tauri glue is kept thin; the
//! coordinate arithmetic and validation are pure helpers so they stay
//! unit-testable without a live window server.

use maekon_core::models::context::WindowBounds;
use tauri::{AppHandle, Monitor};

/// Center point of a window rect, used to resolve which monitor it sits on.
///
/// Pure helper (no Tauri dependency) so the arithmetic is unit-testable.
#[must_use]
pub fn window_center_point(bounds: &WindowBounds) -> (f64, f64) {
    let cx = f64::from(bounds.x) + f64::from(bounds.width) / 2.0;
    let cy = f64::from(bounds.y) + f64::from(bounds.height) / 2.0;
    (cx, cy)
}

/// Normalize a raw monitor scale factor into the `Option<f64>` the capture
/// pipeline expects: `Some(scale)` only for a finite value `> 1.0` (an actual
/// HiDPI magnification), else `None` (treated as 1:1 by the OCR scaler).
///
/// Mirrors `maekon_vision::ocr_geometry`'s own gate so a `1.0` (standard-DPI)
/// or bogus (`0.0`/NaN) value never triggers a needless divide.
#[must_use]
pub fn normalize_scale_factor(scale: f64) -> Option<f64> {
    if scale.is_finite() && scale > 1.0 {
        Some(scale)
    } else {
        None
    }
}

/// Resolve the HiDPI scale factor for the monitor that hosts the active window.
///
/// Resolution order:
/// 1. The monitor containing the window center (`monitor_from_point`) — the same
///    "monitor the window is on" intent the capture pipeline uses to pick which
///    display to grab, so scale and pixels stay consistent.
/// 2. The primary monitor, when bounds are absent or resolve to no monitor.
///
/// Returns `None` (1:1 passthrough) when no app handle is available, the lookups
/// fail, or the resolved scale is not a real magnification (`<= 1.0`).
#[must_use]
pub fn active_monitor_scale_factor(
    app_handle: Option<&AppHandle>,
    bounds: Option<&WindowBounds>,
) -> Option<f64> {
    let app = app_handle?;
    let monitor = monitor_for_window(app, bounds)?;
    normalize_scale_factor(monitor.scale_factor())
}

/// Pick the Tauri monitor for the active window: the one under the window
/// center, falling back to the primary monitor.
fn monitor_for_window(app: &AppHandle, bounds: Option<&WindowBounds>) -> Option<Monitor> {
    if let Some(bounds) = bounds {
        let (cx, cy) = window_center_point(bounds);
        if let Ok(Some(monitor)) = app.monitor_from_point(cx, cy) {
            return Some(monitor);
        }
    }
    app.primary_monitor().ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_center_is_midpoint_of_rect() {
        let bounds = WindowBounds {
            x: 100,
            y: 200,
            width: 800,
            height: 600,
        };
        let (cx, cy) = window_center_point(&bounds);
        assert_eq!(cx, 500.0);
        assert_eq!(cy, 500.0);
    }

    #[test]
    fn window_center_handles_negative_origin() {
        // A window on a monitor positioned left of / above the primary can have
        // a negative global origin; the center math must not underflow.
        let bounds = WindowBounds {
            x: -1920,
            y: -100,
            width: 400,
            height: 200,
        };
        let (cx, cy) = window_center_point(&bounds);
        assert_eq!(cx, -1720.0);
        assert_eq!(cy, 0.0);
    }

    #[test]
    fn normalize_keeps_only_real_magnification() {
        assert_eq!(normalize_scale_factor(2.0), Some(2.0));
        assert_eq!(normalize_scale_factor(1.5), Some(1.5));
        // Standard DPI and degenerate values collapse to passthrough (None).
        assert_eq!(normalize_scale_factor(1.0), None);
        assert_eq!(normalize_scale_factor(0.0), None);
        assert_eq!(normalize_scale_factor(-2.0), None);
        assert_eq!(normalize_scale_factor(f64::NAN), None);
        assert_eq!(normalize_scale_factor(f64::INFINITY), None);
    }

    #[test]
    fn no_app_handle_yields_passthrough() {
        let bounds = WindowBounds {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        };
        assert_eq!(active_monitor_scale_factor(None, Some(&bounds)), None);
        assert_eq!(active_monitor_scale_factor(None, None), None);
    }
}
