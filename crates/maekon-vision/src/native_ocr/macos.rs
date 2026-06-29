//! macOS Vision.framework native OCR via raw objc2 FFI.
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

use async_trait::async_trait;
use maekon_core::error::CoreError;
use maekon_core::ports::ocr_provider::{OcrProvider, OcrResult};
use tracing::debug;

/// macOS Vision.framework native OCR provider.
pub(crate) struct MacOsNativeOcr;

#[async_trait]
impl OcrProvider for MacOsNativeOcr {
    async fn extract_elements(
        &self,
        image: &[u8],
        _image_format: &str,
    ) -> Result<Vec<OcrResult>, CoreError> {
        let data = image.to_vec();
        tokio::task::spawn_blocking(move || recognize_text_blocking(&data))
            .await
            .map_err(|e| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!("Vision OCR task join error: {e}"),
            })?
    }

    fn provider_name(&self) -> &str {
        "macos-vision"
    }

    fn is_external(&self) -> bool {
        false
    }
}

/// Perform synchronous text recognition using Vision.framework.
fn recognize_text_blocking(data: &[u8]) -> Result<Vec<OcrResult>, CoreError> {
    use std::ffi::CStr;
    use std::ptr;

    use objc2::msg_send;
    use objc2::rc::{Allocated, Retained};
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2_core_foundation::CGRect;
    use objc2_foundation::{NSData, NSDictionary, NSString};

    // --- Get image dimensions for coordinate conversion ---
    let (img_width, img_height) = image::load_from_memory(data)
        .map(|img| {
            use image::GenericImageView;
            img.dimensions()
        })
        .map_err(|e| CoreError::OcrError {
            code: maekon_core::error_codes::ProviderCode::OcrFailed,
            message: format!("Failed to decode image for dimensions: {e}"),
        })?;

    debug!(
        width = img_width,
        height = img_height,
        "Vision OCR: image decoded for dimensions"
    );

    // --- Create NSData from raw bytes ---
    let ns_data = NSData::with_bytes(data);

    // --- Create VNImageRequestHandler initWithData:options: ---
    let handler_cls =
        AnyClass::get(c"VNImageRequestHandler").ok_or_else(|| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: "VNImageRequestHandler class not found".into(),
        })?;

    let empty_dict: Retained<NSDictionary<AnyObject, AnyObject>> = NSDictionary::new();

    // SAFETY: `handler_cls` is the VNImageRequestHandler class resolved via
    // AnyClass::get and checked non-null above. `alloc` yields a fresh
    // allocation; `initWithData:options:` is its designated initializer, which
    // consumes that allocation and borrows `ns_data` (live NSData) and
    // `empty_dict` (live NSDictionary). objc2 init semantics yield Retained.
    let handler: Retained<AnyObject> = unsafe {
        let alloc: Allocated<AnyObject> = msg_send![handler_cls, alloc];
        msg_send![alloc, initWithData: &*ns_data, options: &*empty_dict]
    };

    // --- Create VNRecognizeTextRequest ---
    let request_cls =
        AnyClass::get(c"VNRecognizeTextRequest").ok_or_else(|| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: "VNRecognizeTextRequest class not found".into(),
        })?;

    // SAFETY: `request_cls` is the VNRecognizeTextRequest class resolved via
    // AnyClass::get and checked non-null above. `alloc` yields a fresh
    // allocation and `init` is its designated initializer, consuming that
    // allocation and returning a Retained VNRecognizeTextRequest.
    let request: Retained<AnyObject> = unsafe {
        let alloc: Allocated<AnyObject> = msg_send![request_cls, alloc];
        msg_send![alloc, init]
    };

    // setRecognitionLevel: 1 = accurate (VNRequestTextRecognitionLevelAccurate)
    let level: i64 = 1;
    // SAFETY: `request` is the live VNRecognizeTextRequest created above and
    // retained for this scope; `setRecognitionLevel:` is a valid setter taking
    // a single NSInteger (`level`) and returning void.
    let _: () = unsafe { msg_send![&request, setRecognitionLevel: level] };

    // setUsesLanguageCorrection: YES
    let yes: bool = true;
    // SAFETY: `request` is the live VNRecognizeTextRequest from above;
    // `setUsesLanguageCorrection:` is a valid setter taking a single BOOL
    // (`yes`) and returning void.
    let _: () = unsafe { msg_send![&request, setUsesLanguageCorrection: yes] };

    // --- Create NSArray with single request ---
    let nsarray_cls = AnyClass::get(c"NSArray").ok_or_else(|| CoreError::Internal {
        code: maekon_core::error_codes::InternalCode::Generic,
        message: "NSArray class not found".into(),
    })?;

    // SAFETY: `nsarray_cls` is the NSArray class resolved via AnyClass::get and
    // checked non-null above; `arrayWithObject:` takes one object reference and
    // `request` is alive, producing a Retained NSArray that holds it.
    let request_array: Retained<AnyObject> =
        unsafe { msg_send![nsarray_cls, arrayWithObject: &*request] };

    // --- Perform requests ---
    let mut error_ptr: *mut AnyObject = ptr::null_mut();
    // SAFETY: `handler` is the live VNImageRequestHandler; `performRequests:error:`
    // takes an NSArray of requests (`request_array`, alive) and an `NSError **`
    // out-pointer — `&mut error_ptr` is a valid pointer to the null-initialized
    // object pointer declared above. The selector returns a BOOL.
    let success: bool =
        unsafe { msg_send![&handler, performRequests: &*request_array, error: &mut error_ptr] };

    if !success {
        let err_msg = if !error_ptr.is_null() {
            // SAFETY: this branch runs only when `!error_ptr.is_null()` (the
            // enclosing `if`), so `&*error_ptr` dereferences a valid NSError
            // object; `localizedDescription` returns a Retained NSString.
            let desc: Retained<NSString> = unsafe { msg_send![&*error_ptr, localizedDescription] };
            desc.to_string()
        } else {
            "unknown Vision error".to_string()
        };
        return Err(CoreError::OcrError {
            code: maekon_core::error_codes::ProviderCode::OcrFailed,
            message: format!("Vision performRequests failed: {err_msg}"),
        });
    }

    // --- Extract results ---
    // SAFETY: `request` is the live VNRecognizeTextRequest that was just run;
    // `performRequests` above returned success, so `results` is a non-nil
    // NSArray of observations (possibly empty), returned here as Retained.
    let observations: Retained<AnyObject> = unsafe { msg_send![&request, results] };
    // SAFETY: `observations` is the live NSArray returned above; `count` is a
    // valid accessor returning its element count as an NSUInteger.
    let obs_count: usize = unsafe { msg_send![&observations, count] };

    debug!(obs_count, "Vision OCR: observations received");

    let mut results = Vec::with_capacity(obs_count);

    for i in 0..obs_count {
        // SAFETY: `observations` is the live NSArray; `i` is bounded by the
        // `0..obs_count` loop range, so it is a valid in-range index for
        // `objectAtIndex:`, which returns a Retained VNRecognizedTextObservation.
        let observation: Retained<AnyObject> =
            unsafe { msg_send![&observations, objectAtIndex: i] };

        // topCandidates:1 returns NSArray of VNRecognizedText
        let max_candidates: usize = 1;
        // SAFETY: `observation` is the live VNRecognizedTextObservation from this
        // iteration; `topCandidates:` takes an NSUInteger maximum
        // (`max_candidates`) and returns a Retained NSArray of VNRecognizedText.
        let candidates: Retained<AnyObject> =
            unsafe { msg_send![&observation, topCandidates: max_candidates] };

        // SAFETY: `candidates` is the live NSArray returned above; `count` is a
        // valid accessor returning its element count as an NSUInteger.
        let cand_count: usize = unsafe { msg_send![&candidates, count] };
        if cand_count == 0 {
            continue;
        }

        // SAFETY: `cand_count == 0` was just rejected by the early `continue`,
        // so index 0 is in bounds for `objectAtIndex:` on the live `candidates`
        // array, returning a Retained VNRecognizedText.
        let candidate: Retained<AnyObject> =
            unsafe { msg_send![&candidates, objectAtIndex: 0usize] };

        // Extract text string
        // SAFETY: `candidate` is the live VNRecognizedText from above; `string`
        // is a valid accessor returning its recognized text as a Retained NSString.
        let ns_string: Retained<NSString> = unsafe { msg_send![&candidate, string] };
        // SAFETY: `ns_string` is the live NSString from above; `UTF8String`
        // returns a pointer to its NUL-terminated UTF-8 backing buffer, which is
        // owned by and valid for the lifetime of `ns_string`.
        let text_ptr: *const std::ffi::c_char = unsafe { msg_send![&ns_string, UTF8String] };
        let text = if text_ptr.is_null() {
            continue;
        } else {
            // SAFETY: this branch runs only when `!text_ptr.is_null()` (the
            // enclosing `if`), and `text_ptr` is the NUL-terminated C string from
            // `UTF8String`; `ns_string` is still alive in this iteration, so the
            // buffer stays valid for this read (the owned String is built before
            // `ns_string` is dropped at end of iteration).
            unsafe { CStr::from_ptr(text_ptr) }
                .to_string_lossy()
                .into_owned()
        };

        if text.trim().is_empty() {
            continue;
        }

        // Extract confidence
        // SAFETY: `candidate` is the live VNRecognizedText; `confidence` is a
        // valid accessor returning a VNConfidence (float) by value.
        let confidence: f32 = unsafe { msg_send![&candidate, confidence] };

        // Extract bounding box (CGRect, normalized 0..1, bottom-left origin)
        // Uses objc2_core_foundation::CGRect which implements Encode.
        // SAFETY: `observation` is the live observation; `boundingBox` returns a
        // CGRect by value, and objc2_core_foundation::CGRect implements Encode so
        // msg_send! decodes the struct return correctly.
        let bbox: CGRect = unsafe { msg_send![&observation, boundingBox] };

        // Convert from bottom-left normalized coords to top-left pixel coords
        let x = (bbox.origin.x * img_width as f64) as i32;
        let w = (bbox.size.width * img_width as f64) as u32;
        let h = (bbox.size.height * img_height as f64) as u32;
        // bottom-left origin -> top-left: y_pixel = (1 - origin_y - height) * img_height
        let y = ((1.0 - bbox.origin.y - bbox.size.height) * img_height as f64) as i32;

        results.push(OcrResult {
            text,
            x,
            y,
            width: w,
            height: h,
            confidence: confidence as f64,
        });
    }

    debug!(count = results.len(), "Vision OCR: text elements extracted");
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_metadata() {
        let provider = MacOsNativeOcr;
        assert_eq!(provider.provider_name(), "macos-vision");
        assert!(!provider.is_external());
    }
}
