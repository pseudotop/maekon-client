//! #8042 regression: the PRIMARY capture path must destructively mask the pixel
//! regions of OCR-detected PII in the STORED frame — not only the OCR text.
//!
//! These are end-to-end tests through the public `EdgeFrameProcessor` pipeline
//! (default features → `native-vision`, no leptess), driving a deterministic
//! `FakeOcrProvider` so the OCR regions are fixed. They decode the stored WebP
//! (`ImagePayload::Full.data`) and inspect its pixels, proving the fix at the
//! storage boundary rather than a unit helper in isolation.

#![cfg(all(feature = "native-vision", not(feature = "ocr")))]

use base64::Engine;
use image::{DynamicImage, Rgba, RgbaImage};
use maekon_core::config::PiiFilterLevel;
use maekon_core::models::frame::ImagePayload;
use maekon_core::ports::ocr_provider::{FakeOcrProvider, OcrResult};
use maekon_core::ports::vision::CaptureRequest;
use maekon_vision::processor::EdgeFrameProcessor;
use std::sync::Arc;

/// A solid white frame (OCR is faked, so pixel content is irrelevant — only the
/// dimensions and the fixed OCR boxes matter).
fn white_frame(w: u32, h: u32) -> DynamicImage {
    DynamicImage::ImageRgba8(RgbaImage::from_pixel(w, h, Rgba([255, 255, 255, 255])))
}

fn high_importance_request(scale_factor: Option<f64>) -> CaptureRequest {
    CaptureRequest {
        trigger_type: "ErrorDetected".to_string(),
        importance: 0.9, // high → full-frame + OCR + redaction branch
        app_name: "Terminal".to_string(),
        window_title: "session".to_string(),
        monitor_id: None,
        app_bundle_id: None,
        window_bounds: None,
        screen_scale_factor: scale_factor,
        ocr_processing_permitted: true,
    }
}

/// Decode the stored full-frame WebP payload into an RGBA image.
fn decode_stored_frame(payload: &ImagePayload) -> RgbaImage {
    match payload {
        ImagePayload::Full { data, .. } => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(data)
                .expect("stored frame payload must be valid base64");
            image::load_from_memory(&bytes)
                .expect("stored frame must be a decodable image")
                .to_rgba8()
        }
        other => panic!("expected ImagePayload::Full, got {other:?}"),
    }
}

fn is_near_black(p: &Rgba<u8>) -> bool {
    p.0[0] < 40 && p.0[1] < 40 && p.0[2] < 40
}

/// PII region pixels must be masked in the stored frame; a non-PII region on the
/// same frame must be preserved.
#[tokio::test]
async fn stored_frame_masks_pii_region_and_preserves_non_pii() {
    // Two OCR elements: one PII (email), one benign — far apart so their
    // margin-expanded boxes cannot overlap.
    let provider = Arc::new(FakeOcrProvider::new(vec![
        OcrResult {
            text: "admin@company.com".to_string(),
            x: 20,
            y: 20,
            width: 200,
            height: 30,
            confidence: 0.95,
        },
        OcrResult {
            text: "just plain words".to_string(),
            x: 20,
            y: 300,
            width: 200,
            height: 30,
            confidence: 0.95,
        },
    ]));
    let processor = EdgeFrameProcessor::with_pii_level(480, 270, None, PiiFilterLevel::Standard)
        .with_ocr_provider(Some(provider));

    let frame = processor
        .process_captured_frame(white_frame(640, 480), &high_importance_request(None), None)
        .await
        .expect("processing a high-importance frame must succeed");

    let stored = decode_stored_frame(
        frame
            .image_payload
            .as_ref()
            .expect("importance >= 0.8 must yield a full-frame payload"),
    );

    // The email element's center pixel must be masked (near-black; WebP is lossy).
    let email_center = stored.get_pixel(120, 35);
    assert!(
        is_near_black(email_center),
        "PII (email) pixels must be destructively masked in the stored frame, got {email_center:?}"
    );

    // The benign element's center pixel must be preserved (near-white).
    let benign_center = stored.get_pixel(120, 315);
    assert!(
        benign_center.0[0] > 200 && benign_center.0[1] > 200 && benign_center.0[2] > 200,
        "non-PII pixels must be preserved, got {benign_center:?}"
    );

    // Corner far from any box is untouched.
    let corner = stored.get_pixel(600, 450);
    assert!(
        corner.0[0] > 200,
        "unrelated pixels must be preserved, got {corner:?}"
    );
}

/// PiiFilterLevel::Off is an explicit operator opt-out: no pixel is masked even
/// when OCR clearly detects PII.
#[tokio::test]
async fn stored_frame_is_not_masked_when_level_off() {
    let provider = Arc::new(FakeOcrProvider::new(vec![OcrResult {
        text: "admin@company.com".to_string(),
        x: 20,
        y: 20,
        width: 200,
        height: 30,
        confidence: 0.95,
    }]));
    let processor = EdgeFrameProcessor::with_pii_level(480, 270, None, PiiFilterLevel::Off)
        .with_ocr_provider(Some(provider));

    let frame = processor
        .process_captured_frame(white_frame(640, 480), &high_importance_request(None), None)
        .await
        .expect("processing must succeed");

    let stored = decode_stored_frame(frame.image_payload.as_ref().expect("full-frame payload"));
    let email_center = stored.get_pixel(120, 35);
    assert!(
        email_center.0[0] > 200,
        "PiiFilterLevel::Off must leave the frame unmasked, got {email_center:?}"
    );
}

/// HiDPI coordinate alignment: OCR elements arrive in PHYSICAL source pixels and
/// masking is applied BEFORE the logical downscale, so with `scale_factor = 2.0`
/// the mask must land on the PHYSICAL region (x≈40..280), NOT the logical region
/// (x≈20..140) that the exposed `ocr_regions` are scaled to.
#[tokio::test]
async fn stored_frame_masks_physical_region_under_hidpi_scale() {
    let provider = Arc::new(FakeOcrProvider::new(vec![OcrResult {
        text: "admin@company.com".to_string(),
        x: 40,
        y: 40,
        width: 240,
        height: 40,
        confidence: 0.95,
    }]));
    let processor = EdgeFrameProcessor::with_pii_level(480, 270, None, PiiFilterLevel::Standard)
        .with_ocr_provider(Some(provider));

    let frame = processor
        .process_captured_frame(
            white_frame(640, 480),
            &high_importance_request(Some(2.0)),
            None,
        )
        .await
        .expect("processing must succeed");

    let stored = decode_stored_frame(frame.image_payload.as_ref().expect("full-frame payload"));

    // Physical region center (inside x=40..280, y=40..80) must be masked.
    let physical_center = stored.get_pixel(160, 60);
    assert!(
        is_near_black(physical_center),
        "mask must land on the PHYSICAL OCR region under HiDPI, got {physical_center:?}"
    );

    // The exposed ocr_regions are logical-scaled (÷2), but the frame is physical:
    // a pixel at the LOGICAL x (~80) but well above the physical band would be a
    // mis-scaled mask. Assert a point outside the physical band stays unmasked.
    let below_physical = stored.get_pixel(160, 200);
    assert!(
        below_physical.0[0] > 200,
        "only the physical PII band may be masked, got {below_physical:?}"
    );

    // Sanity: the exposed regions themselves are logical (÷2) — confirms the
    // frame is physical while regions are logical (the mismatch this fix bridges).
    assert_eq!(frame.ocr_regions.len(), 1);
    assert_eq!(
        frame.ocr_regions[0].bbox.x, 20,
        "exposed region is logical (40/2)"
    );
}
