//! Native capture observations use exact window identities and one coordinate
//! space. Missing observations must never become an empty, successful scene.

use std::collections::HashSet;

use maekon_core::ports::vision::{
    CaptureDisplay, CaptureObservationError, CaptureScene, CaptureWindow,
};

/// Bind a complete native window enumeration to one actual capture display.
/// Collectors must omit hidden/minimized windows and supply physical pixels on
/// Windows or desktop points on macOS, matching the supplied display geometry.
/// An untitled window remains visible to policy; a missing owner does not.
///
/// # Errors
/// Rejects a missing foreground, ambiguous native identities, invalid geometry,
/// or an unidentified owner on the selected display. The caller must separately
/// reject failed enumeration and foreground changes during observation.
pub fn capture_scene_from_windows(
    displays: &[CaptureDisplay],
    windows: Vec<CaptureWindow>,
    foreground_id: u64,
    requested_display: Option<u32>,
) -> Result<CaptureScene, CaptureObservationError> {
    use CaptureObservationError::{
        EnumerationFailed, ForegroundUnavailable, GeometryUnavailable, OwnerUnavailable,
    };

    if foreground_id == 0 {
        return Err(ForegroundUnavailable);
    }
    let mut identities = HashSet::new();
    for window in &windows {
        if window.native_id == 0 || !identities.insert(window.native_id) {
            return Err(EnumerationFailed);
        }
        if window.bounds.width == 0 || window.bounds.height == 0 {
            return Err(GeometryUnavailable);
        }
    }
    let foreground = windows
        .iter()
        .find(|window| window.native_id == foreground_id)
        .ok_or(ForegroundUnavailable)?
        .clone();
    let display =
        crate::capture_display_for_window(displays, &foreground.bounds, requested_display)?;
    let visible: Vec<_> = windows
        .into_iter()
        .filter(|window| crate::capture_bounds_intersect(&window.bounds, &display.bounds))
        .collect();
    if visible
        .iter()
        .any(|window| window.pid == 0 || window.app_name.trim().is_empty())
    {
        return Err(OwnerUnavailable);
    }
    Ok(CaptureScene {
        foreground,
        display_id: display.id,
        display_bounds: display.bounds,
        windows: visible,
    })
}

#[cfg(test)]
mod tests;
