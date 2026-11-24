//! Authorized screen-change probe: samples the screen as per-tile signatures.
//!
//! Every sample asks the capture authority for a fresh permit and acquires one
//! frame through the guarded capture source, the same boundary that authorized
//! frame capture uses. The frame is reduced to one signature per
//! [`SCREEN_TILE_PX`] tile of a downscaled copy and every pixel buffer is
//! zeroed before returning, so only signatures leave this module.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use async_trait::async_trait;
use image::{DynamicImage, RgbaImage};
use maekon_core::error::CoreError;
use maekon_core::error_codes::InternalCode;
use maekon_core::models::screen_activity::{ScreenSample, SCREEN_TILE_PX};
use maekon_core::ports::screen_probe::ScreenChangeProbe;
use maekon_core::ports::vision::{CaptureAccess, CaptureAuthority};
use zeroize::Zeroize;

use crate::capture::{acquire_frame, CaptureSource};

/// Width the frame is reduced to before hashing. On a 1920 px wide display
/// each 16 px sample tile covers a 32 px square of the screen.
pub const DEFAULT_SAMPLE_WIDTH: u32 = 960;

/// [`ScreenChangeProbe`] over the guarded capture source.
pub struct AuthorizedScreenProbe {
    source: Arc<dyn CaptureSource>,
    authority: Arc<dyn CaptureAuthority>,
    sample_width: u32,
}

impl AuthorizedScreenProbe {
    pub fn new(source: Arc<dyn CaptureSource>, authority: Arc<dyn CaptureAuthority>) -> Self {
        Self {
            source,
            authority,
            sample_width: DEFAULT_SAMPLE_WIDTH,
        }
    }

    /// Override the reduced width; it never goes below one tile.
    pub fn with_sample_width(mut self, width: u32) -> Self {
        self.sample_width = width.max(SCREEN_TILE_PX);
        self
    }
}

#[async_trait]
impl ScreenChangeProbe for AuthorizedScreenProbe {
    async fn sample(&self) -> Result<ScreenSample, CoreError> {
        // A fresh permit per sample: withdrawn consent, a pause or an excluded
        // app in front stops sampling at the next call.
        let permit = self.authority.authorize(None)?;
        let (frame, _monitor) = acquire_frame(self.source.clone(), None, permit.clone()).await?;
        let sample_width = self.sample_width;
        let sample = tokio::task::spawn_blocking(move || tile_signatures(frame, sample_width))
            .await
            .map_err(|error| internal(format!("screen sample task failed: {error}")))??;
        // A permit revoked while hashing invalidates the observation as well.
        permit.check(CaptureAccess::Retain)?;
        Ok(sample)
    }

    fn name(&self) -> &str {
        "authorized-tile-signature"
    }
}

/// Reduce `frame` to per-tile signatures of a copy at most `sample_width`
/// pixels wide, zeroing every pixel buffer before returning.
pub fn tile_signatures(frame: DynamicImage, sample_width: u32) -> Result<ScreenSample, CoreError> {
    if frame.width() == 0 || frame.height() == 0 {
        frame.into_bytes().zeroize();
        return Err(internal("screen sample has no pixels".to_string()));
    }
    // `thumbnail` averages every source pixel into one target pixel, so a
    // change of a few pixels still alters its tile. Bounding the height by the
    // source keeps the copy no larger than the frame for any size: with an
    // unbounded height a 0-wide frame would ask for a 1 x u32::MAX buffer.
    let reduced = frame.thumbnail(sample_width.min(frame.width()), frame.height());
    frame.into_bytes().zeroize();
    let mut pixels = reduced.to_rgba8();
    reduced.into_bytes().zeroize();
    let signatures = signatures_of(&pixels);
    let (width, height) = pixels.dimensions();
    let buffer: &mut [u8] = &mut pixels;
    buffer.zeroize();
    ScreenSample::new(width, height, signatures)
        .map_err(|error| internal(format!("screen sample grid invalid: {error}")))
}

/// One hash per tile, row-major; partial edge tiles included.
fn signatures_of(pixels: &RgbaImage) -> Vec<u64> {
    let tile = SCREEN_TILE_PX as usize;
    let row_bytes = pixels.width() as usize * 4;
    let columns = pixels.width().div_ceil(SCREEN_TILE_PX) as usize;
    let mut signatures = Vec::new();
    for band in pixels.as_raw().chunks(row_bytes * tile) {
        let mut hashers = vec![DefaultHasher::new(); columns];
        for row in band.chunks(row_bytes) {
            for (hasher, segment) in hashers.iter_mut().zip(row.chunks(tile * 4)) {
                segment.hash(hasher);
            }
        }
        signatures.extend(hashers.iter().map(Hasher::finish));
    }
    signatures
}

fn internal(message: String) -> CoreError {
    CoreError::Internal {
        code: InternalCode::Generic,
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::models::context::{WindowBounds, WindowInfo};
    use maekon_core::ports::vision::{capture_denied, CaptureGuard, CapturePermit};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex;

    struct SwitchGuard {
        revoked: Arc<AtomicBool>,
    }

    impl CaptureGuard for SwitchGuard {
        fn check(&self, _access: CaptureAccess) -> Result<(), CoreError> {
            if self.revoked.load(Ordering::SeqCst) {
                Err(capture_denied("synthetic revocation"))
            } else {
                Ok(())
            }
        }
    }

    struct FakeAuthority {
        refuses: bool,
        revoked: Arc<AtomicBool>,
        grants: AtomicUsize,
    }

    impl FakeAuthority {
        fn new(refuses: bool) -> Arc<Self> {
            Arc::new(Self {
                refuses,
                revoked: Arc::new(AtomicBool::new(false)),
                grants: AtomicUsize::new(0),
            })
        }
    }

    impl CaptureAuthority for FakeAuthority {
        fn authorize(&self, _expected: Option<&WindowInfo>) -> Result<CapturePermit, CoreError> {
            if self.refuses {
                return Err(capture_denied("synthetic consent withdrawn"));
            }
            self.grants.fetch_add(1, Ordering::SeqCst);
            Ok(CapturePermit::new(Arc::new(SwitchGuard {
                revoked: self.revoked.clone(),
            })))
        }
    }

    /// Replays frames in order (repeating the last) and can revoke the grant
    /// while "capturing", as a consent withdrawal mid-acquisition would.
    struct FakeSource {
        frames: Mutex<Vec<DynamicImage>>,
        calls: AtomicUsize,
        revoke_during_capture: Option<Arc<AtomicBool>>,
    }

    impl FakeSource {
        fn new(frames: Vec<DynamicImage>) -> Arc<Self> {
            Arc::new(Self {
                frames: Mutex::new(frames),
                calls: AtomicUsize::new(0),
                revoke_during_capture: None,
            })
        }
    }

    impl CaptureSource for FakeSource {
        fn capture(
            &self,
            _bounds: Option<&WindowBounds>,
            _permit: &CapturePermit,
        ) -> Result<(DynamicImage, Option<usize>), CoreError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(flag) = &self.revoke_during_capture {
                flag.store(true, Ordering::SeqCst);
            }
            let frames = self.frames.lock().unwrap();
            let frame = frames.get(call).or_else(|| frames.last()).cloned().unwrap();
            Ok((frame, Some(0)))
        }
    }

    fn solid(width: u32, height: u32) -> RgbaImage {
        RgbaImage::from_pixel(width, height, image::Rgba([40, 80, 120, 255]))
    }

    fn with_block(mut image: RgbaImage, x: u32, y: u32, size: u32) -> DynamicImage {
        for dy in 0..size {
            for dx in 0..size {
                image.put_pixel(x + dx, y + dy, image::Rgba([250, 250, 250, 255]));
            }
        }
        DynamicImage::ImageRgba8(image)
    }

    /// A reference grid of the given size: `geometry_changed` is false only
    /// when a sample has exactly this width and height.
    fn grid(width: u32, height: u32) -> ScreenSample {
        let tiles = width.div_ceil(SCREEN_TILE_PX) * height.div_ceil(SCREEN_TILE_PX);
        ScreenSample::new(width, height, vec![0; tiles as usize]).unwrap()
    }

    #[tokio::test]
    async fn identical_frames_match_and_one_pixel_changes_one_tile() {
        // 64x48 is 4x3 tiles; the sample width keeps full resolution.
        let base = DynamicImage::ImageRgba8(solid(64, 48));
        let changed = with_block(solid(64, 48), 20, 20, 1);
        let source = FakeSource::new(vec![base.clone(), base, changed]);
        let probe =
            AuthorizedScreenProbe::new(source, FakeAuthority::new(false)).with_sample_width(64);

        let first = probe.sample().await.unwrap();
        let second = probe.sample().await.unwrap();
        let third = probe.sample().await.unwrap();
        assert!(!first.diff(&grid(64, 48)).geometry_changed);
        assert_eq!(first, second);
        assert_eq!(second.diff(&third).changed_tiles, 1);
    }

    #[test]
    fn each_pixel_lands_in_its_own_tile() {
        // 40x36 is 3x3 tiles with partial right and bottom edge tiles.
        let base = tile_signatures(DynamicImage::ImageRgba8(solid(40, 36)), 64).unwrap();
        assert!(!base.diff(&grid(40, 36)).geometry_changed);
        for (x, y) in [(0, 0), (15, 15), (16, 0), (39, 35), (17, 33)] {
            let moved = tile_signatures(with_block(solid(40, 36), x, y, 1), 64).unwrap();
            assert_eq!(base.diff(&moved).changed_tiles, 1, "pixel ({x}, {y})");
        }
        // Two pixels of one tile still change one tile; two tiles change two.
        let one_tile = {
            let mut image = solid(40, 36);
            image.put_pixel(17, 1, image::Rgba([0, 0, 0, 255]));
            image.put_pixel(30, 14, image::Rgba([0, 0, 0, 255]));
            tile_signatures(DynamicImage::ImageRgba8(image), 64).unwrap()
        };
        assert_eq!(base.diff(&one_tile).changed_tiles, 1);
        let two_tiles = {
            let mut image = solid(40, 36);
            image.put_pixel(17, 1, image::Rgba([0, 0, 0, 255]));
            image.put_pixel(17, 17, image::Rgba([0, 0, 0, 255]));
            tile_signatures(DynamicImage::ImageRgba8(image), 64).unwrap()
        };
        assert_eq!(base.diff(&two_tiles).changed_tiles, 2);
    }

    #[test]
    fn a_text_sized_change_survives_the_downscale() {
        let before = tile_signatures(DynamicImage::ImageRgba8(solid(1920, 1080)), 960).unwrap();
        let after = tile_signatures(with_block(solid(1920, 1080), 701, 433, 6), 960).unwrap();
        assert!(!before.diff(&grid(960, 540)).geometry_changed);
        let diff = before.diff(&after);
        assert!(diff.is_changed());
        assert!(
            diff.changed_tiles <= 2,
            "a 6 px block touches at most two tiles"
        );
    }

    #[test]
    fn an_empty_frame_is_rejected() {
        for (width, height) in [(0, 0), (0, 16), (16, 0)] {
            let error = tile_signatures(DynamicImage::new_rgba8(width, height), 960).unwrap_err();
            assert!(
                matches!(error, CoreError::Internal { .. }),
                "{width}x{height}"
            );
        }
    }

    #[tokio::test]
    async fn a_refused_authorization_never_touches_the_pixel_source() {
        let source = FakeSource::new(vec![DynamicImage::ImageRgba8(solid(32, 32))]);
        let probe = AuthorizedScreenProbe::new(source.clone(), FakeAuthority::new(true));
        let error = probe.sample().await.unwrap_err();
        assert!(matches!(error, CoreError::PermissionDenied { .. }));
        assert_eq!(source.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_permit_revoked_during_capture_discards_the_sample() {
        let authority = FakeAuthority::new(false);
        let source = Arc::new(FakeSource {
            frames: Mutex::new(vec![DynamicImage::ImageRgba8(solid(32, 32))]),
            calls: AtomicUsize::new(0),
            revoke_during_capture: Some(authority.revoked.clone()),
        });
        let probe = AuthorizedScreenProbe::new(source.clone(), authority);
        let error = probe.sample().await.unwrap_err();
        assert!(matches!(error, CoreError::PermissionDenied { .. }));
        assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn every_sample_asks_for_a_fresh_permit() {
        let authority = FakeAuthority::new(false);
        let source = FakeSource::new(vec![DynamicImage::ImageRgba8(solid(32, 32))]);
        let probe = AuthorizedScreenProbe::new(source, authority.clone());
        probe.sample().await.unwrap();
        probe.sample().await.unwrap();
        assert_eq!(authority.grants.load(Ordering::SeqCst), 2);
        assert_eq!(probe.name(), "authorized-tile-signature");
    }
}
