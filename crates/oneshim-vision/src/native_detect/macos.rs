//! macOS Vision.framework rectangle detection via raw objc2 FFI.
//!
//! `objc2-vision` does not exist, so all Vision.framework calls use
//! `AnyClass::get()` + `msg_send!`.
//!
//! In objc2 0.6, `msg_send!` handles `Retained<T>` return types directly
//! via retain-semantics inference (alloc → Allocated, init → Retained,
//! others → Retained when T: Message).
//!
//! Vision `boundingBox` returns a CGRect normalized 0..1 with bottom-left
//! origin; we convert to top-left pixel coordinates.

use oneshim_core::error::CoreError;
use oneshim_core::models::intent::ElementBounds;
use oneshim_core::models::ui_scene::NormalizedBounds;
use oneshim_core::ports::rectangle_detector::{DetectedRectangle, RectangleDetector};
use tracing::debug;

/// macOS Vision.framework rectangle detector.
pub(crate) struct MacOsRectangleDetector;

impl RectangleDetector for MacOsRectangleDetector {
    fn detect_rectangles(
        &self,
        image: &[u8],
        image_width: u32,
        image_height: u32,
        min_size: f32,
        max_results: usize,
    ) -> Result<Vec<DetectedRectangle>, CoreError> {
        detect_rectangles_blocking(image, image_width, image_height, min_size, max_results)
    }

    fn provider_name(&self) -> &str {
        "macos-vision-rect"
    }
}

/// Perform synchronous rectangle detection using Vision.framework.
fn detect_rectangles_blocking(
    data: &[u8],
    img_width: u32,
    img_height: u32,
    min_size: f32,
    max_results: usize,
) -> Result<Vec<DetectedRectangle>, CoreError> {
    use std::ptr;

    use objc2::msg_send;
    use objc2::rc::{Allocated, Retained};
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2_core_foundation::CGRect;
    use objc2_foundation::{NSData, NSDictionary, NSString};

    debug!(
        img_width,
        img_height, min_size, max_results, "Vision rect: starting detection"
    );

    // --- Create NSData from raw bytes ---
    let ns_data = NSData::with_bytes(data);

    // --- Create VNImageRequestHandler initWithData:options: ---
    let handler_cls =
        AnyClass::get(c"VNImageRequestHandler").ok_or_else(|| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: "VNImageRequestHandler class not found".into(),
        })?;

    let empty_dict: Retained<NSDictionary<AnyObject, AnyObject>> = NSDictionary::new();

    let handler: Retained<AnyObject> = unsafe {
        let alloc: Allocated<AnyObject> = msg_send![handler_cls, alloc];
        msg_send![alloc, initWithData: &*ns_data, options: &*empty_dict]
    };

    // --- Create VNDetectRectanglesRequest ---
    let request_cls =
        AnyClass::get(c"VNDetectRectanglesRequest").ok_or_else(|| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: "VNDetectRectanglesRequest class not found".into(),
        })?;

    let request: Retained<AnyObject> = unsafe {
        let alloc: Allocated<AnyObject> = msg_send![request_cls, alloc];
        msg_send![alloc, init]
    };

    // setMinimumSize: (f32, relative to image width/height, 0..1)
    let _: () = unsafe { msg_send![&request, setMinimumSize: min_size] };

    // setMaximumObservations: (usize, 0 = unlimited up to system max)
    let max_obs: usize = if max_results == 0 { 0 } else { max_results };
    let _: () = unsafe { msg_send![&request, setMaximumObservations: max_obs] };

    // setMinimumAspectRatio: 0.1 (wide range to capture both landscape + portrait)
    let min_aspect: f32 = 0.1;
    let _: () = unsafe { msg_send![&request, setMinimumAspectRatio: min_aspect] };

    // setMaximumAspectRatio: 10.0
    let max_aspect: f32 = 10.0;
    let _: () = unsafe { msg_send![&request, setMaximumAspectRatio: max_aspect] };

    // --- Create NSArray with single request ---
    let nsarray_cls = AnyClass::get(c"NSArray").ok_or_else(|| CoreError::Internal {
        code: oneshim_core::error_codes::InternalCode::Generic,
        message: "NSArray class not found".into(),
    })?;

    let request_array: Retained<AnyObject> =
        unsafe { msg_send![nsarray_cls, arrayWithObject: &*request] };

    // --- Perform requests ---
    let mut error_ptr: *mut AnyObject = ptr::null_mut();
    let success: bool =
        unsafe { msg_send![&handler, performRequests: &*request_array, error: &mut error_ptr] };

    if !success {
        let err_msg = if !error_ptr.is_null() {
            let desc: Retained<NSString> = unsafe { msg_send![&*error_ptr, localizedDescription] };
            desc.to_string()
        } else {
            "unknown Vision error".to_string()
        };
        return Err(CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("Vision performRequests failed: {err_msg}"),
        });
    }

    // --- Extract results ---
    let observations: Retained<AnyObject> = unsafe { msg_send![&request, results] };
    let obs_count: usize = unsafe { msg_send![&observations, count] };

    debug!(obs_count, "Vision rect: observations received");

    let mut results = Vec::with_capacity(obs_count);

    for i in 0..obs_count {
        let observation: Retained<AnyObject> =
            unsafe { msg_send![&observations, objectAtIndex: i] };

        // Extract confidence (f32 from VNRectangleObservation)
        let confidence: f32 = unsafe { msg_send![&observation, confidence] };

        // Extract bounding box (CGRect, normalized 0..1, bottom-left origin)
        let bbox: CGRect = unsafe { msg_send![&observation, boundingBox] };

        // Convert normalized bottom-left coords → top-left pixel coords
        let x = (bbox.origin.x * img_width as f64) as i32;
        let w = (bbox.size.width * img_width as f64) as u32;
        let h = (bbox.size.height * img_height as f64) as u32;
        // bottom-left origin → top-left: y_pixel = (1 - origin_y - height) * img_height
        let y = ((1.0 - bbox.origin.y - bbox.size.height) * img_height as f64) as i32;

        // Build normalized bounds (top-left convention, clamped 0..1)
        let norm_x = bbox.origin.x as f32;
        let norm_y = (1.0 - bbox.origin.y as f32 - bbox.size.height as f32).clamp(0.0, 1.0);
        let norm_w = bbox.size.width as f32;
        let norm_h = bbox.size.height as f32;

        results.push(DetectedRectangle {
            bounds: ElementBounds {
                x,
                y,
                width: w,
                height: h,
            },
            bounds_normalized: NormalizedBounds::new(norm_x, norm_y, norm_w, norm_h),
            confidence: confidence as f64,
            classification: None,
        });
    }

    debug!(count = results.len(), "Vision rect: rectangles extracted");
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_metadata() {
        let provider = MacOsRectangleDetector;
        assert_eq!(provider.provider_name(), "macos-vision-rect");
    }
}
