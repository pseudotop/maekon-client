use super::tests::{assert_denied, permissions, Harness};
use super::*;
use crate::capture::{acquire_frame, capture_pixels_authorized, CaptureSource};
use crate::processor::EdgeFrameProcessor;
use crate::ring_buffer::{CaptureRingBuffer, RingFrame};
use image::{DynamicImage, Rgba, RgbaImage};
use maekon_core::ports::vision::FrameProcessor;

fn pixels(marker: u8) -> DynamicImage {
    DynamicImage::ImageRgba8(RgbaImage::from_pixel(16, 16, Rgba([marker, 23, 31, 255])))
}

fn ring_frame(marker: u8) -> RingFrame {
    RingFrame {
        timestamp: chrono::Utc::now(),
        thumbnail_data: vec![marker; 64],
        app_name: "CaptureFixture".into(),
        window_title: "Allowed fixture".into(),
        accessibility_elements: Vec::new(),
    }
}

#[test]
fn actual_pixel_boundary_rejects_revocation_during_acquisition() {
    let h = Harness::new();
    let permit = h.permit();
    let mut calls = 0;
    let result = capture_pixels_authorized(7, &permit, &mut || {
        calls += 1;
        h.consent.revoke_consent().unwrap();
        h.consent.grant_consent(permissions(), 30).unwrap();
        Ok(pixels(91))
    });
    assert_eq!(calls, 1);
    assert_denied(result, "capture consent changed");
    let fresh = capture_pixels_authorized(7, &h.permit(), &mut || Ok(pixels(92))).unwrap();
    assert_eq!(fresh.to_rgba8().get_pixel(0, 0).0, [92, 23, 31, 255]);
}

#[test]
fn privacy_mode_during_acquisition_discards_pixels_and_buffered_origin_after_restore() {
    let h = Harness::new();
    let old = h.permit();
    let ring = CaptureRingBuffer::new(4, 2, 0.5);
    ring.push_authorized(ring_frame(91), old.clone()).unwrap();
    let mut acquisitions = 0;
    let result = capture_pixels_authorized(7, &old, &mut || {
        acquisitions += 1;
        h.config.update_with(|config| {
            config.vision.privacy_mode = true;
            Ok(())
        })?;
        assert_denied(
            h.authority.authorize(None),
            "capture policy disallows access",
        );
        h.config.update_with(|config| {
            config.vision.privacy_mode = false;
            Ok(())
        })?;
        Ok(pixels(91))
    });
    assert_eq!(acquisitions, 1);
    assert_denied(result, "capture policy changed");
    let fresh = h.permit();
    let image = capture_pixels_authorized(7, &fresh, &mut || Ok(pixels(92))).unwrap();
    assert_eq!(image.to_rgba8().get_pixel(0, 0).0, [92, 23, 31, 255]);
    ring.push_authorized(ring_frame(92), fresh.clone()).unwrap();
    let frames = ring.flush_authorized(0.5, &fresh).unwrap();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].frame.thumbnail_data, vec![92; 64]);
    frames[0].permit.check(CaptureAccess::Retain).unwrap();
}

#[test]
fn blocked_scene_never_invokes_the_pixel_producer() {
    let h = Harness::new();
    let permit = h.permit();
    h.authority.toggle_pause().unwrap();
    let result = capture_pixels_authorized(7, &permit, &mut || {
        panic!("paused capture must not acquire pixels")
    });
    assert_denied(result, "capture paused");
}

#[test]
fn ring_flush_preserves_origin_and_drops_old_grant_pixels() {
    let h = Harness::new();
    let ring = CaptureRingBuffer::new(4, 2, 0.5);
    let old = h.permit();
    ring.push_authorized(ring_frame(91), old.clone()).unwrap();
    h.consent.revoke_consent().unwrap();
    h.consent.grant_consent(permissions(), 30).unwrap();
    assert_denied(
        ring.push_authorized(ring_frame(93), old),
        "capture consent changed",
    );
    let fresh = h.permit();
    ring.push_authorized(ring_frame(92), fresh.clone()).unwrap();
    assert!(ring.flush_authorized(0.49, &fresh).unwrap().is_empty());
    let frames = ring.flush_authorized(0.5, &fresh).unwrap();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].frame.thumbnail_data, vec![92; 64]);
    frames[0].permit.check(CaptureAccess::Retain).unwrap();
    assert!(ring.flush_authorized(1.0, &fresh).unwrap().is_empty());
}

struct PixelSource {
    consent: Arc<maekon_core::consent::ConsentManager>,
    revoke: bool,
}

impl CaptureSource for PixelSource {
    fn capture(
        &self,
        _: Option<&WindowBounds>,
        permit: &CapturePermit,
    ) -> Result<(DynamicImage, Option<usize>), CoreError> {
        let image = capture_pixels_authorized(7, permit, &mut || Ok(pixels(92)))?;
        // Queue/result boundary must check independently of the native source.
        if self.revoke {
            self.consent.revoke_consent()?;
            self.consent.grant_consent(permissions(), 30)?;
        }
        Ok((image, Some(7)))
    }
}

#[tokio::test]
async fn queued_acquisition_and_processor_reject_revoked_results_and_allow_fresh_images() {
    let h = Harness::new();
    let revoked = Arc::new(PixelSource {
        consent: h.consent.clone(),
        revoke: true,
    });
    assert_denied(
        acquire_frame(revoked.clone(), None, h.permit()).await,
        "capture consent changed",
    );
    let processor = EdgeFrameProcessor::new(8, 8, None)
        .with_capture_authority(Arc::new(h.authority.clone()))
        .with_capture_source(revoked);
    assert_denied(
        processor
            .thumbnail_authorized(None, &processor.begin_capture(None).unwrap())
            .await,
        "capture consent changed",
    );
    let positive = EdgeFrameProcessor::new(8, 8, None)
        .with_capture_authority(Arc::new(h.authority.clone()))
        .with_capture_source(Arc::new(PixelSource {
            consent: h.consent.clone(),
            revoke: false,
        }));
    let bytes = positive
        .thumbnail_authorized(None, &positive.begin_capture(None).unwrap())
        .await
        .unwrap();
    let decoded = image::load_from_memory(&bytes).unwrap().to_rgba8();
    assert_eq!(decoded.dimensions(), (8, 8));
    assert!(
        decoded.get_pixel(0, 0).0[0] > 80,
        "fresh marker must survive actual WebP encoding"
    );
}
