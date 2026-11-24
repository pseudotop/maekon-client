use crate::error::VisionError;
use image::DynamicImage;
use maekon_core::error::CoreError;
use maekon_core::models::context::WindowBounds;
use maekon_core::ports::vision::{
    CaptureAccess, CaptureDisplay, CaptureObservationError, CapturePermit,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
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

/// Enumerate acquisition identities without xcap's lossy window filtering.
/// These bounds stay in the platform desktop coordinate space, not the
/// dimensions of a captured backing image (Retina images can be larger).
pub fn capture_displays() -> Result<Vec<CaptureDisplay>, CaptureObservationError> {
    let monitors = Monitor::all().map_err(|_| CaptureObservationError::EnumerationFailed)?;
    let mut displays = Vec::with_capacity(monitors.len());
    for monitor in monitors {
        let id = monitor
            .id()
            .map_err(|_| CaptureObservationError::GeometryUnavailable)?;
        let bounds = WindowBounds {
            x: monitor
                .x()
                .map_err(|_| CaptureObservationError::GeometryUnavailable)?,
            y: monitor
                .y()
                .map_err(|_| CaptureObservationError::GeometryUnavailable)?,
            width: monitor
                .width()
                .map_err(|_| CaptureObservationError::GeometryUnavailable)?,
            height: monitor
                .height()
                .map_err(|_| CaptureObservationError::GeometryUnavailable)?,
        };
        displays.push(CaptureDisplay { id, bounds });
    }
    validate_capture_displays(displays)
}

fn validate_capture_displays(
    displays: Vec<CaptureDisplay>,
) -> Result<Vec<CaptureDisplay>, CaptureObservationError> {
    let mut identities = std::collections::HashSet::with_capacity(displays.len());
    for display in &displays {
        if display.bounds.width == 0 || display.bounds.height == 0 || !identities.insert(display.id)
        {
            return Err(CaptureObservationError::GeometryUnavailable);
        }
    }
    if displays.is_empty() {
        return Err(CaptureObservationError::GeometryUnavailable);
    }
    Ok(displays)
}

/// Check the original permit before acquiring pixels and before returning them.
/// Call this boundary after selecting the display, inside the acquisition job.
/// A newer grant must never replace the permit bound to this acquisition.
pub fn capture_pixels_authorized(
    display_id: u32,
    permit: &CapturePermit,
    producer: &mut dyn FnMut() -> Result<DynamicImage, CoreError>,
) -> Result<DynamicImage, CoreError> {
    permit.check(CaptureAccess::Acquire { display_id })?;
    let pixels = producer()?;
    permit.check(CaptureAccess::Acquire { display_id })?;
    Ok(pixels)
}

/// Pixel source contract for guarded frame and thumbnail processor wiring.
pub trait CaptureSource: Send + Sync {
    fn capture(
        &self,
        bounds: Option<&WindowBounds>,
        permit: &CapturePermit,
    ) -> Result<(DynamicImage, Option<usize>), CoreError>;
}

pub(crate) fn check_processing_permit(
    permit: &Option<CapturePermit>,
    access: CaptureAccess,
) -> Result<(), CoreError> {
    if let Some(permit) = permit {
        permit.check(access)?;
    }
    Ok(())
}

/// Preserve the originating permit across both queue delay and result delivery.
pub(crate) async fn acquire_frame(
    source: Arc<dyn CaptureSource>,
    bounds: Option<WindowBounds>,
    permit: CapturePermit,
) -> Result<(DynamicImage, Option<usize>), CoreError> {
    permit.check(CaptureAccess::CurrentScene)?;
    let queued_permit = permit.clone();
    let frame = tokio::task::spawn_blocking(move || {
        queued_permit.check(CaptureAccess::Retain)?;
        let frame = source.capture(bounds.as_ref(), &queued_permit)?;
        queued_permit.check(CaptureAccess::Retain)?;
        Ok::<_, CoreError>(frame)
    })
    .await
    .map_err(|error| CoreError::Internal {
        code: maekon_core::error_codes::InternalCode::Generic,
        message: format!("capture task panicked: {error}"),
    })??;
    permit.check(CaptureAccess::Retain)?;
    Ok(frame)
}

/// Keep the original authority through queued acquisition, processing, and delivery.
pub(crate) async fn acquire_thumbnail(
    source: Arc<dyn CaptureSource>,
    bounds: Option<WindowBounds>,
    width: u32,
    height: u32,
    permit: CapturePermit,
) -> Result<Vec<u8>, CoreError> {
    permit.check(CaptureAccess::CurrentScene)?;
    let processing_permit = permit.clone();
    let bytes = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, CoreError> {
        let (frame, _) = source.capture(bounds.as_ref(), &processing_permit)?;
        processing_permit.check(CaptureAccess::Retain)?;
        let thumbnail = resize_uncached(&frame, width, height)?;
        let bytes = crate::encoder::encode_webp(&thumbnail, crate::encoder::WebPQuality::Low)?;
        processing_permit.check(CaptureAccess::Retain)?;
        Ok(bytes)
    })
    .await
    .map_err(|error| CoreError::Internal {
        code: maekon_core::error_codes::InternalCode::Generic,
        message: format!("thumbnail task panicked: {error}"),
    })??;
    // A worker's last check cannot cover revocation while its result is awaiting delivery.
    permit.check(CaptureAccess::Retain)?;
    Ok(bytes)
}

/// Both authorized consumers avoid the legacy cache and never upscale pixels.
pub(crate) fn resize_uncached(
    frame: &DynamicImage,
    width: u32,
    height: u32,
) -> Result<DynamicImage, CoreError> {
    if frame.width() == 0 || frame.height() == 0 {
        return Err(VisionError::Internal("Source image size is zero".into()).into());
    }
    if width == 0 || height == 0 {
        return Err(VisionError::Internal("Target image size is zero".into()).into());
    }
    Ok(frame.thumbnail(width.min(frame.width()), height.min(frame.height())))
}

/// Resolve one exact physical ID from the same native monitor enumeration.
fn select_authorized_monitor(
    display_id: u32,
    identities: &[Result<u32, String>],
) -> Result<usize, CoreError> {
    let mut selected = None;
    for (index, identity) in identities.iter().enumerate() {
        let id = identity.as_ref().map_err(|error| {
            VisionError::Internal(format!("Capture display identity unavailable: {error}"))
        })?;
        if *id == display_id && selected.replace(index).is_some() {
            return Err(
                VisionError::Internal("Capture display identity is ambiguous".into()).into(),
            );
        }
    }
    selected.ok_or_else(|| VisionError::Internal("Authorized display disappeared".into()).into())
}

/// Selection and acquisition share this boundary with portable regression tests.
fn capture_display_authorized(
    display_id: u32,
    identities: &[Result<u32, String>],
    permit: &CapturePermit,
    producer: &mut dyn FnMut(usize) -> Result<DynamicImage, CoreError>,
) -> Result<(DynamicImage, Option<usize>), CoreError> {
    let index = select_authorized_monitor(display_id, identities)?;
    let pixels = capture_pixels_authorized(display_id, permit, &mut || producer(index))?;
    Ok((pixels, Some(index)))
}

impl CaptureSource for ScreenCapture {
    fn capture(
        &self,
        _bounds: Option<&WindowBounds>,
        permit: &CapturePermit,
    ) -> Result<(DynamicImage, Option<usize>), CoreError> {
        // Deny missing or revoked authority before querying native displays.
        let display_id = permit.display_id().ok_or_else(|| {
            maekon_core::ports::vision::capture_denied("capture display not authorized")
        })?;
        permit.check(CaptureAccess::Acquire { display_id })?;
        let monitors = Monitor::all()
            .map_err(|e| VisionError::Internal(format!("Failed to query monitor list: {e}")))?;
        // The authority resolves the physical ID; bounds and primary fallbacks
        // must not select pixels from a different display or coordinate space.
        let identities: Vec<Result<u32, String>> = monitors
            .iter()
            .map(|monitor| monitor.id().map_err(|error| error.to_string()))
            .collect();
        capture_display_authorized(display_id, &identities, permit, &mut |index| {
            let monitor = monitors
                .get(index)
                .ok_or_else(|| VisionError::Internal("Selected monitor disappeared".into()))?;
            monitor
                .capture_image()
                .map(DynamicImage::ImageRgba8)
                .map_err(|e| VisionError::Internal(format!("Screen capture failed: {e}")).into())
        })
    }
}

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
    #[test]
    fn display_metadata_rejects_each_invalid_axis_and_duplicate_identity() {
        let valid = CaptureDisplay {
            id: 7,
            bounds: WindowBounds {
                x: -1920,
                y: -200,
                width: 1920,
                height: 1080,
            },
        };
        let mut other = valid.clone();
        other.id = 8;
        other.bounds.x = 0;
        let accepted = validate_capture_displays(vec![valid.clone(), other.clone()]).unwrap();
        let observed: Vec<_> = accepted
            .into_iter()
            .map(|d| {
                (
                    d.id,
                    d.bounds.x,
                    d.bounds.y,
                    d.bounds.width,
                    d.bounds.height,
                )
            })
            .collect();
        assert_eq!(
            observed,
            [(7, -1920, -200, 1920, 1080), (8, 0, -200, 1920, 1080)]
        );
        let mut zero_width = valid.clone();
        zero_width.bounds.width = 0;
        let mut zero_height = valid.clone();
        zero_height.bounds.height = 0;
        for invalid in [
            vec![],
            vec![zero_width],
            vec![zero_height],
            vec![valid.clone(), valid],
        ] {
            assert_eq!(
                validate_capture_displays(invalid).unwrap_err(),
                CaptureObservationError::GeometryUnavailable
            );
        }
    }

    #[test]
    fn native_display_enumeration_never_reports_empty_success() {
        // A headless host may reject observation; it must not invent an empty
        // successful scene. Actual device geometry acceptance is separate.
        match capture_displays() {
            Ok(displays) => assert!(!displays.is_empty()),
            Err(error) => assert!(matches!(
                error,
                CaptureObservationError::EnumerationFailed
                    | CaptureObservationError::GeometryUnavailable
            )),
        }
    }

    use super::*;
    use maekon_core::error_codes::{InternalCode, PermissionCode};
    use maekon_core::ports::vision::{capture_denied, CaptureGuard};
    use std::cell::Cell;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    struct SyntheticGuard {
        allowed: Arc<AtomicBool>,
        display_id: u32,
    }

    impl CaptureGuard for SyntheticGuard {
        fn check(&self, access: CaptureAccess) -> Result<(), CoreError> {
            if !self.allowed.load(Ordering::SeqCst) {
                return Err(capture_denied("synthetic revoke"));
            }
            match access {
                CaptureAccess::Acquire { display_id } if display_id == self.display_id => Ok(()),
                _ => Err(capture_denied("synthetic display mismatch")),
            }
        }

        fn display_id(&self) -> Option<u32> {
            Some(self.display_id)
        }
    }

    fn synthetic_permit(allowed: &Arc<AtomicBool>) -> CapturePermit {
        CapturePermit::new(Arc::new(SyntheticGuard {
            allowed: allowed.clone(),
            display_id: 7,
        }))
    }

    fn marker_image() -> DynamicImage {
        DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            2,
            2,
            image::Rgba([37, 93, 17, 255]),
        ))
    }

    fn assert_privacy_denied(result: Result<DynamicImage, CoreError>) {
        match result {
            Err(CoreError::PermissionDenied { code, .. }) => {
                assert_eq!(code, PermissionCode::PrivacyDenied);
            }
            Err(error) => panic!("expected privacy denial, got {error}"),
            Ok(_) => panic!("unauthorized pixels crossed the acquisition boundary"),
        }
    }

    #[test]
    fn capture_boundary_returns_allowed_pixels_unchanged() {
        let allowed = Arc::new(AtomicBool::new(true));
        let permit = synthetic_permit(&allowed);
        let calls = Cell::new(0);
        let pixels = capture_pixels_authorized(7, &permit, &mut || {
            calls.set(calls.get() + 1);
            Ok(marker_image())
        })
        .unwrap();
        assert_eq!(calls.get(), 1);
        assert_eq!(pixels, marker_image());
    }

    #[test]
    fn capture_boundary_refuses_pixels_before_producer() {
        let allowed = Arc::new(AtomicBool::new(false));
        let permit = synthetic_permit(&allowed);
        let calls = Cell::new(0);
        let result = capture_pixels_authorized(7, &permit, &mut || {
            calls.set(calls.get() + 1);
            Ok(marker_image())
        });
        assert_privacy_denied(result);
        assert_eq!(calls.get(), 0, "denial must stop acquisition itself");
    }

    #[test]
    fn capture_boundary_discards_pixels_revoked_during_acquisition() {
        let allowed = Arc::new(AtomicBool::new(true));
        let permit = synthetic_permit(&allowed);
        let calls = Cell::new(0);
        let result = capture_pixels_authorized(7, &permit, &mut || {
            calls.set(calls.get() + 1);
            allowed.store(false, Ordering::SeqCst);
            Ok(marker_image())
        });
        assert_eq!(
            calls.get(),
            1,
            "revocation occurs after pixels are produced"
        );
        assert_privacy_denied(result);
    }

    #[test]
    fn capture_boundary_rejects_a_different_display_before_acquisition() {
        let allowed = Arc::new(AtomicBool::new(true));
        let permit = synthetic_permit(&allowed);
        let calls = Cell::new(0);
        let result = capture_pixels_authorized(8, &permit, &mut || {
            calls.set(calls.get() + 1);
            Ok(marker_image())
        });
        assert_privacy_denied(result);
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn capture_boundary_keeps_revoked_original_separate_from_a_new_grant() {
        let original_allowed = Arc::new(AtomicBool::new(true));
        let original = synthetic_permit(&original_allowed);
        let replacement_allowed = Arc::new(AtomicBool::new(true));
        let replacement = synthetic_permit(&replacement_allowed);
        original_allowed.store(false, Ordering::SeqCst);
        let calls = Cell::new(0);
        let result = capture_pixels_authorized(7, &original, &mut || {
            calls.set(calls.get() + 1);
            Ok(marker_image())
        });
        assert_privacy_denied(result);
        assert_eq!(calls.get(), 0);
        let pixels = capture_pixels_authorized(7, &replacement, &mut || {
            calls.set(calls.get() + 1);
            Ok(marker_image())
        })
        .unwrap();
        assert_eq!(calls.get(), 1);
        assert_eq!(pixels, marker_image());
    }

    #[test]
    fn capture_boundary_preserves_producer_errors() {
        let allowed = Arc::new(AtomicBool::new(true));
        let permit = synthetic_permit(&allowed);
        let calls = Cell::new(0);
        let result = capture_pixels_authorized(7, &permit, &mut || {
            calls.set(calls.get() + 1);
            Err(CoreError::Internal {
                code: InternalCode::Generic,
                message: "synthetic producer failure".into(),
            })
        });
        assert_eq!(calls.get(), 1);
        match result {
            Err(CoreError::Internal { code, message }) => {
                assert_eq!(code, InternalCode::Generic);
                assert_eq!(message, "synthetic producer failure");
            }
            Err(error) => panic!("producer error changed: {error}"),
            Ok(_) => panic!("failed producer returned pixels"),
        }
    }

    fn assert_capture_internal(
        result: Result<(DynamicImage, Option<usize>), CoreError>,
        expected: &str,
    ) {
        match result {
            Err(CoreError::Internal { code, message }) => {
                assert_eq!(code, InternalCode::Generic);
                assert_eq!(message, expected);
            }
            Err(error) => panic!("expected capture identity error, got {error}"),
            Ok(_) => panic!("unresolved display returned pixels"),
        }
    }

    #[test]
    fn capture_boundary_selects_authorized_physical_display() {
        let permit = synthetic_permit(&Arc::new(AtomicBool::new(true)));
        let identities = [Ok(41), Ok(41), Ok(7), Ok(93)];
        let calls = Cell::new(0);
        let (pixels, selected) =
            capture_display_authorized(7, &identities, &permit, &mut |index| {
                assert_eq!(index, 2, "produce the authorized physical display");
                calls.set(calls.get() + 1);
                Ok(marker_image())
            })
            .unwrap();
        assert_eq!(selected, Some(2));
        assert_eq!(calls.get(), 1);
        assert_eq!(pixels, marker_image());
    }

    #[test]
    fn capture_boundary_rejects_missing_display_without_fallback() {
        let permit = synthetic_permit(&Arc::new(AtomicBool::new(true)));
        for identities in [vec![], vec![Ok(41)], vec![Ok(41), Ok(93)]] {
            let calls = Cell::new(0);
            let result = capture_display_authorized(7, &identities, &permit, &mut |_| {
                calls.set(calls.get() + 1);
                Ok(marker_image())
            });
            assert_capture_internal(result, "Authorized display disappeared");
            assert_eq!(calls.get(), 0, "missing identity must not acquire pixels");
        }
    }

    #[test]
    fn capture_boundary_rejects_ambiguous_authorized_display() {
        let permit = synthetic_permit(&Arc::new(AtomicBool::new(true)));
        let calls = Cell::new(0);
        let result =
            capture_display_authorized(7, &[Ok(41), Ok(7), Ok(93), Ok(7)], &permit, &mut |_| {
                calls.set(calls.get() + 1);
                Ok(marker_image())
            });
        assert_capture_internal(result, "Capture display identity is ambiguous");
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn capture_boundary_preserves_identity_query_errors_anywhere() {
        let permit = synthetic_permit(&Arc::new(AtomicBool::new(true)));
        for identities in [
            vec![Err("identity failure marker".into()), Ok(7)],
            vec![Ok(7), Err("identity failure marker".into())],
        ] {
            let calls = Cell::new(0);
            let result = capture_display_authorized(7, &identities, &permit, &mut |_| {
                calls.set(calls.get() + 1);
                Ok(marker_image())
            });
            assert_capture_internal(
                result,
                "Capture display identity unavailable: identity failure marker",
            );
            assert_eq!(calls.get(), 0);
        }
    }

    #[test]
    fn capture_boundary_native_source_denies_missing_metadata() {
        struct MissingDisplay;
        impl CaptureGuard for MissingDisplay {
            fn check(&self, _: CaptureAccess) -> Result<(), CoreError> {
                Ok(())
            }
        }
        let permit = CapturePermit::new(Arc::new(MissingDisplay));
        let source: &dyn CaptureSource = &ScreenCapture;
        let result = source.capture(None, &permit);
        assert_privacy_denied(result.map(|(pixels, _)| pixels));
    }

    #[test]
    fn capture_boundary_native_source_denies_revoked_original() {
        let permit = synthetic_permit(&Arc::new(AtomicBool::new(false)));
        let source: &dyn CaptureSource = &ScreenCapture;
        let bounds = WindowBounds {
            x: 0,
            y: 0,
            width: 800,
            height: 600,
        };
        let result = source.capture(Some(&bounds), &permit);
        assert_privacy_denied(result.map(|(pixels, _)| pixels));
    }

    #[test]
    fn capture_boundary_selection_cannot_refresh_revoked_original() {
        let original = synthetic_permit(&Arc::new(AtomicBool::new(false)));
        let replacement = synthetic_permit(&Arc::new(AtomicBool::new(true)));
        let calls = Cell::new(0);
        let result = capture_display_authorized(7, &[Ok(7)], &original, &mut |_| {
            calls.set(calls.get() + 1);
            Ok(marker_image())
        });
        assert_privacy_denied(result.map(|(pixels, _)| pixels));
        assert_eq!(calls.get(), 0);
        let (pixels, selected) =
            capture_display_authorized(7, &[Ok(7)], &replacement, &mut |index| {
                assert_eq!(index, 0);
                calls.set(calls.get() + 1);
                Ok(marker_image())
            })
            .unwrap();
        assert_eq!(selected, Some(0));
        assert_eq!(pixels, marker_image());
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn capture_boundary_selection_rejects_revocation_during_pixels() {
        let allowed = Arc::new(AtomicBool::new(true));
        let permit = synthetic_permit(&allowed);
        let calls = Cell::new(0);
        let result = capture_display_authorized(7, &[Ok(41), Ok(7)], &permit, &mut |index| {
            assert_eq!(index, 1);
            calls.set(calls.get() + 1);
            allowed.store(false, Ordering::SeqCst);
            Ok(marker_image())
        });
        assert_privacy_denied(result.map(|(pixels, _)| pixels));
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn capture_boundary_selection_rejects_wrong_display_and_preserves_producer_error() {
        let permit = synthetic_permit(&Arc::new(AtomicBool::new(true)));
        let calls = Cell::new(0);
        let result = capture_display_authorized(8, &[Ok(8)], &permit, &mut |_| {
            calls.set(calls.get() + 1);
            Ok(marker_image())
        });
        assert_privacy_denied(result.map(|(pixels, _)| pixels));
        assert_eq!(calls.get(), 0);
        let result = capture_display_authorized(7, &[Ok(7)], &permit, &mut |_| {
            calls.set(calls.get() + 1);
            Err(VisionError::Internal("pixel failure marker".into()).into())
        });
        assert_capture_internal(result, "pixel failure marker");
        assert_eq!(calls.get(), 1);
    }

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

#[cfg(test)]
mod queue_tests;
