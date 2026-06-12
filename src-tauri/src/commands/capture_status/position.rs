//! Panel position validation helpers (physical-pixel coordinate space).
//!
//! ADR-013 split from `capture_status/mod.rs`.

// PANEL_WIDTH is logical px (= physical at 1x). On HiDPI the physical panel
// is wider, but POSITION_MARGIN absorbs the difference.
pub(super) const PANEL_WIDTH: f64 = 260.0;
pub(super) const POSITION_MARGIN: f64 = 100.0;

#[derive(Debug, Clone)]
pub(super) struct MonitorBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Parse "x,y" position string into (f64, f64).
/// Returns None for missing, malformed, NaN, or Infinity values.
pub(super) fn parse_position(s: &str) -> Option<(f64, f64)> {
    let mut parts = s.splitn(2, ',');
    let x: f64 = parts.next()?.parse().ok()?;
    let y: f64 = parts.next()?.parse().ok()?;
    if x.is_finite() && y.is_finite() {
        Some((x, y))
    } else {
        None
    }
}

/// Check if position (x, y) falls within any monitor's physical bounds.
/// All values are in physical pixels. Returns false if monitors is empty.
pub(super) fn is_position_valid(x: f64, y: f64, monitors: &[MonitorBounds]) -> bool {
    monitors.iter().any(|m| monitor_contains_point(m, x, y))
}

fn monitor_contains_point(monitor: &MonitorBounds, x: f64, y: f64) -> bool {
    x >= monitor.x - PANEL_WIDTH + POSITION_MARGIN
        && x <= monitor.x + monitor.width - POSITION_MARGIN
        && y >= monitor.y
        && y < monitor.y + monitor.height
}

pub(super) fn resolve_window_monitor_index(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    monitors: &[MonitorBounds],
) -> Option<usize> {
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let center_x = x + (width / 2.0);
    let center_y = y + (height / 2.0);
    monitors.iter().position(|monitor| {
        center_x >= monitor.x
            && center_x < monitor.x + monitor.width
            && center_y >= monitor.y
            && center_y < monitor.y + monitor.height
    })
}

pub(super) fn resolve_point_monitor_index(
    x: f64,
    y: f64,
    monitors: &[MonitorBounds],
) -> Option<usize> {
    monitors.iter().position(|monitor| {
        x >= monitor.x
            && x < monitor.x + monitor.width
            && y >= monitor.y
            && y < monitor.y + monitor.height
    })
}
