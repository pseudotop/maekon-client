use crate::error::VisionError;
use image::DynamicImage;
use maekon_core::models::context::WindowBounds;
use serde::{Deserialize, Serialize};
use tracing::debug;
use xcap::Monitor;

/// Metadata describing a single physical display.
///
/// Returned by [`ScreenCapture::list_monitors`] for UI enumeration and
/// monitor-selection flows. The `index` field matches the argument that
/// [`ScreenCapture::capture_monitor`] accepts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorInfo {
    /// Zero-based index in the order reported by the OS.
    pub index: usize,
    /// Display X position in global desktop coordinates.
    pub x: i32,
    /// Display Y position in global desktop coordinates.
    pub y: i32,
    /// Display width in pixels.
    pub width: u32,
    /// Display height in pixels.
    pub height: u32,
    /// True if this is the primary display.
    pub is_primary: bool,
    /// OS-reported display name (best-effort; may be empty on some platforms).
    pub name: String,
}

pub fn monitor_index_for_window(
    bounds: Option<&WindowBounds>,
    monitors: &[MonitorInfo],
) -> Option<usize> {
    let primary = || monitors.iter().find(|m| m.is_primary).map(|m| m.index);
    let first = || monitors.first().map(|m| m.index);

    let Some(bounds) = bounds else {
        return primary().or_else(first);
    };

    let cx = bounds.x as i64 + (bounds.width as i64 / 2);
    let cy = bounds.y as i64 + (bounds.height as i64 / 2);

    monitors
        .iter()
        .find(|m| {
            let mx = m.x as i64;
            let my = m.y as i64;
            let mw = m.width as i64;
            let mh = m.height as i64;
            mw > 0 && mh > 0 && cx >= mx && cx < mx + mw && cy >= my && cy < my + mh
        })
        .map(|m| m.index)
        .or_else(primary)
        .or_else(first)
}

#[derive(Clone)]
pub struct ScreenCapture;

impl ScreenCapture {
    pub fn new() -> Self {
        Self
    }

    pub fn capture_primary(&self) -> Result<DynamicImage, VisionError> {
        let monitors = Monitor::all()
            .map_err(|e| VisionError::Internal(format!("Failed to query monitor list: {e}")))?;

        let monitor = monitors
            .into_iter()
            .find(|m| m.is_primary().unwrap_or(false))
            .or_else(|| Monitor::all().ok()?.into_iter().next())
            .ok_or_else(|| VisionError::Internal("Monitor not found".to_string()))?;

        let image = monitor
            .capture_image()
            .map_err(|e| VisionError::Internal(format!("Screen capture failed: {e}")))?;

        debug!(
            "screen capture completed: {}x{}",
            image.width(),
            image.height()
        );

        Ok(DynamicImage::ImageRgba8(image))
    }

    /// Capture the monitor containing the active window.
    ///
    /// Falls back to the primary monitor when:
    /// - only one monitor exists,
    /// - no window bounds are provided, or
    /// - the window center does not intersect any known monitor.
    pub fn capture_for_window(
        &self,
        bounds: Option<&WindowBounds>,
    ) -> Result<DynamicImage, VisionError> {
        self.capture_for_window_with_monitor(bounds)
            .map(|(image, _)| image)
    }

    /// Capture the monitor containing the active window and return the selected
    /// monitor index alongside the image.
    ///
    /// The returned monitor index matches [`Self::list_monitors`] ordering.
    pub fn capture_for_window_with_monitor(
        &self,
        bounds: Option<&WindowBounds>,
    ) -> Result<(DynamicImage, Option<usize>), VisionError> {
        let monitors = Monitor::all()
            .map_err(|e| VisionError::Internal(format!("Failed to query monitor list: {e}")))?;

        if monitors.is_empty() {
            return Err(VisionError::Internal("Monitor not found".to_string()));
        }

        // Single monitor — capture the only available display and report index 0.
        if monitors.len() == 1 {
            let monitor = monitors
                .first()
                .ok_or_else(|| VisionError::Internal("Monitor not found".to_string()))?;
            let image = monitor
                .capture_image()
                .map_err(|e| VisionError::Internal(format!("Screen capture failed: {e}")))?;

            debug!(
                "screen capture completed: {}x{}",
                image.width(),
                image.height()
            );

            return Ok((DynamicImage::ImageRgba8(image), Some(0)));
        }

        let infos: Vec<MonitorInfo> = monitors
            .iter()
            .enumerate()
            .map(|(index, monitor)| MonitorInfo {
                index,
                x: monitor.x().unwrap_or(0),
                y: monitor.y().unwrap_or(0),
                width: monitor.width().unwrap_or(0),
                height: monitor.height().unwrap_or(0),
                is_primary: monitor.is_primary().unwrap_or(false),
                name: monitor.name().unwrap_or_default(),
            })
            .collect();

        let target_index = monitor_index_for_window(bounds, &infos).unwrap_or(0);
        let monitor = monitors
            .get(target_index)
            .or_else(|| monitors.iter().find(|m| m.is_primary().unwrap_or(false)))
            .or_else(|| monitors.first())
            .ok_or_else(|| VisionError::Internal("No monitor found".to_string()))?;

        let image = monitor
            .capture_image()
            .map_err(|e| VisionError::Internal(format!("Screen capture failed: {e}")))?;

        debug!(
            "captured monitor at ({}, {}) — {}x{}",
            monitor.x().unwrap_or(0),
            monitor.y().unwrap_or(0),
            image.width(),
            image.height()
        );

        Ok((DynamicImage::ImageRgba8(image), Some(target_index)))
    }

    /// Capture the monitor at the given zero-based `index`.
    ///
    /// The index corresponds to the order reported by the OS (same as
    /// [`Self::list_monitors`]). Returns [`VisionError::Internal`] when the
    /// index is out of range or the capture syscall fails.
    pub fn capture_monitor(&self, index: usize) -> Result<DynamicImage, VisionError> {
        let monitors = Monitor::all()
            .map_err(|e| VisionError::Internal(format!("Failed to query monitor list: {e}")))?;

        let monitor = monitors
            .into_iter()
            .nth(index)
            .ok_or_else(|| VisionError::Internal(format!("Monitor index {index} not found")))?;

        let image = monitor
            .capture_image()
            .map_err(|e| VisionError::Internal(format!("Screen capture failed: {e}")))?;

        Ok(DynamicImage::ImageRgba8(image))
    }

    /// Return the number of monitors currently attached to the system.
    pub fn monitor_count() -> Result<usize, VisionError> {
        Monitor::all()
            .map(|m| m.len())
            .map_err(|e| VisionError::Internal(format!("Failed to query monitor list: {e}")))
    }

    /// Enumerate attached monitors with metadata suitable for UI selection.
    ///
    /// Fields that the underlying driver cannot report fall back to safe
    /// defaults (`0` for numeric fields, empty string for `name`, `false` for
    /// `is_primary`) so enumeration never fails for a single bad monitor.
    pub fn list_monitors() -> Result<Vec<MonitorInfo>, VisionError> {
        let monitors = Monitor::all()
            .map_err(|e| VisionError::Internal(format!("Failed to query monitor list: {e}")))?;

        Ok(monitors
            .into_iter()
            .enumerate()
            .map(|(index, monitor)| MonitorInfo {
                index,
                x: monitor.x().unwrap_or(0),
                y: monitor.y().unwrap_or(0),
                width: monitor.width().unwrap_or(0),
                height: monitor.height().unwrap_or(0),
                is_primary: monitor.is_primary().unwrap_or(false),
                name: monitor.name().unwrap_or_default(),
            })
            .collect())
    }
}

impl Default for ScreenCapture {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_for_window_with_none_bounds_uses_primary() {
        let capture = ScreenCapture::new();
        // Should not panic — falls back to primary.
        // May fail in headless CI (no display), so we only assert no panic.
        let _ = capture.capture_for_window(None);
    }

    #[test]
    fn capture_for_window_with_bounds_does_not_panic() {
        let capture = ScreenCapture::new();
        let bounds = WindowBounds {
            x: 100,
            y: 200,
            width: 800,
            height: 600,
        };
        // Should not panic regardless of monitor configuration.
        let _ = capture.capture_for_window(Some(&bounds));
    }

    #[test]
    fn list_monitors_indices_match_count() {
        // On headless CI the list may be empty or query may fail — either is fine;
        // when it succeeds, indices must match iteration order and len.
        if let (Ok(infos), Ok(count)) = (
            ScreenCapture::list_monitors(),
            ScreenCapture::monitor_count(),
        ) {
            assert_eq!(infos.len(), count);
            for (i, info) in infos.iter().enumerate() {
                assert_eq!(info.index, i);
            }
            assert!(infos.iter().filter(|m| m.is_primary).count() <= 1);
        }
    }

    #[test]
    fn crt_prv_cap_008_window_center_selects_active_monitor() {
        let monitors = vec![
            MonitorInfo {
                index: 0,
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
                is_primary: true,
                name: "Primary".to_string(),
            },
            MonitorInfo {
                index: 1,
                x: 1920,
                y: 0,
                width: 1600,
                height: 900,
                is_primary: false,
                name: "Secondary".to_string(),
            },
        ];
        let bounds = WindowBounds {
            x: 2200,
            y: 100,
            width: 800,
            height: 600,
        };

        assert_eq!(monitor_index_for_window(Some(&bounds), &monitors), Some(1));
        assert_eq!(monitor_index_for_window(None, &monitors), Some(0));
    }
}
