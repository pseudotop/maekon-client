use crate::error::VisionError;
use fast_image_resize::{images::Image as FirImage, ResizeAlg, ResizeOptions, Resizer};
use image::{DynamicImage, RgbaImage};
use lru::LruCache;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::num::NonZeroUsize;
use tracing::debug;

const CACHE_CAPACITY: usize = 100;

type CacheKey = (u64, u32, u32);

static THUMBNAIL_CACHE: Lazy<Mutex<LruCache<CacheKey, Vec<u8>>>> = Lazy::new(|| {
    Mutex::new(LruCache::new(
        NonZeroUsize::new(CACHE_CAPACITY).expect("CACHE_CAPACITY must be > 0"),
    ))
});

#[inline]
fn compute_image_hash(image: &DynamicImage) -> u64 {
    let rgba = image.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    let raw = rgba.as_raw();

    let mut hash: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    hash ^= w as u64;
    hash = hash.wrapping_mul(FNV_PRIME);
    hash ^= h as u64;
    hash = hash.wrapping_mul(FNV_PRIME);

    let step_x = (w as usize).max(1) / 16;
    let step_y = (h as usize).max(1) / 16;
    let stride = w as usize * 4;

    for sy in 0..16 {
        let y = (sy * step_y).min((h as usize).saturating_sub(1));
        let row_offset = y * stride;

        for sx in 0..16 {
            let x = (sx * step_x).min((w as usize).saturating_sub(1));
            let pixel_offset = row_offset + x * 4;

            if pixel_offset + 3 < raw.len() {
                let pixel = u32::from_le_bytes([
                    raw[pixel_offset],
                    raw[pixel_offset + 1],
                    raw[pixel_offset + 2],
                    raw[pixel_offset + 3],
                ]);
                hash ^= pixel as u64;
                hash = hash.wrapping_mul(FNV_PRIME);
            }
        }
    }

    hash
}

pub fn fast_resize(
    image: &DynamicImage,
    width: u32,
    height: u32,
) -> Result<DynamicImage, VisionError> {
    let (src_w, src_h) = (image.width(), image.height());

    if src_w == width && src_h == height {
        return Ok(image.clone());
    }

    if src_w == 0 || src_h == 0 {
        return Err(VisionError::Internal(
            "Source image size is zero".to_string(),
        ));
    }
    if width == 0 || height == 0 {
        return Err(VisionError::Internal(
            "Target image size is zero".to_string(),
        ));
    }
    // Guard against buffer overflow inside fast_image_resize.
    // 32768 accommodates triple 6K displays (18048px) and 8K monitors.
    // The actual crash was from usize::unchecked_add — log dimensions for diagnosis.
    const MAX_DIM: u32 = 32768;
    if src_w > MAX_DIM || src_h > MAX_DIM {
        tracing::warn!(
            src_w,
            src_h,
            target_w = width,
            target_h = height,
            "Source image exceeds max resize dimension"
        );
        return Err(VisionError::Internal(format!(
            "Source image too large for resize: {src_w}x{src_h} (max {MAX_DIM})"
        )));
    }
    if width > MAX_DIM || height > MAX_DIM {
        return Err(VisionError::Internal(format!(
            "Target size too large: {width}x{height} (max {MAX_DIM})"
        )));
    }

    let hash = compute_image_hash(image);
    let cache_key = (hash, width, height);

    {
        let mut cache = THUMBNAIL_CACHE.lock();
        if let Some(cached) = cache.get(&cache_key) {
            debug!(
                "Thumbnail cache hit: {}x{} → {}x{} (hash={})",
                src_w, src_h, width, height, hash
            );

            let result = RgbaImage::from_raw(width, height, cached.clone()).ok_or_else(|| {
                VisionError::Internal("Failed to restore cached image".to_string())
            })?;

            return Ok(DynamicImage::ImageRgba8(result));
        }
    }

    let src_rgba = image.to_rgba8();

    let src_image = FirImage::from_vec_u8(
        src_w,
        src_h,
        src_rgba.into_raw(),
        fast_image_resize::PixelType::U8x4,
    )
    .map_err(|e| VisionError::Internal(format!("Failed to create source image: {e}")))?;

    let mut dst_image = FirImage::new(width, height, fast_image_resize::PixelType::U8x4);

    let mut resizer = Resizer::new();
    let options = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(
        fast_image_resize::FilterType::Bilinear,
    ));

    resizer
        .resize(&src_image, &mut dst_image, &options)
        .map_err(|e| VisionError::Internal(format!("Resize failed: {e}")))?;

    let raw_bytes = dst_image.into_vec();

    {
        let mut cache = THUMBNAIL_CACHE.lock();
        cache.put(cache_key, raw_bytes.clone());
    }

    let result = RgbaImage::from_raw(width, height, raw_bytes)
        .ok_or_else(|| VisionError::Internal("Failed to create result image".to_string()))?;

    debug!(
        "Thumbnail created: {}x{} → {}x{} (hash={}, cache save)",
        src_w, src_h, width, height, hash
    );

    Ok(DynamicImage::ImageRgba8(result))
}

pub struct CacheStats {
    pub size: usize,
    pub capacity: usize,
}

pub fn get_cache_stats() -> CacheStats {
    let cache = THUMBNAIL_CACHE.lock();
    CacheStats {
        size: cache.len(),
        capacity: CACHE_CAPACITY,
    }
}

#[cfg(test)]
pub fn clear_cache() {
    let mut cache = THUMBNAIL_CACHE.lock();
    cache.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, GenericImageView, RgbaImage};
    use std::sync::Mutex;

    static CACHE_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn make_test_image(w: u32, h: u32, color: [u8; 4]) -> DynamicImage {
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(w, h, image::Rgba(color)))
    }

    #[test]
    fn resize_basic() {
        let _guard = CACHE_TEST_LOCK.lock().unwrap();
        clear_cache();
        let img = make_test_image(1920, 1080, [100, 100, 100, 255]);
        let thumb = fast_resize(&img, 480, 270).unwrap();
        assert_eq!(thumb.dimensions(), (480, 270));
    }

    #[test]
    fn same_size_noop() {
        let _guard = CACHE_TEST_LOCK.lock().unwrap();
        clear_cache();
        let img = make_test_image(480, 270, [100, 100, 100, 255]);
        let result = fast_resize(&img, 480, 270).unwrap();
        assert_eq!(result.dimensions(), (480, 270));
    }

    #[test]
    fn cache_hit() {
        let _guard = CACHE_TEST_LOCK.lock().unwrap();
        clear_cache();
        let img = make_test_image(800, 600, [50, 100, 150, 255]);

        let size_before = get_cache_stats().size;

        let _thumb1 = fast_resize(&img, 200, 150).unwrap();
        let size_after_first = get_cache_stats().size;
        assert!(
            size_after_first >= size_before,
            "Cache size did not increase"
        );

        let _thumb2 = fast_resize(&img, 200, 150).unwrap();
        let size_after_second = get_cache_stats().size;
        assert_eq!(
            size_after_first, size_after_second,
            "Cache size should not increase on cache hit"
        );
    }

    #[test]
    fn different_sizes_cached_separately() {
        let _guard = CACHE_TEST_LOCK.lock().unwrap();
        clear_cache();
        let img = make_test_image(1000, 1000, [200, 100, 50, 255]);

        let size_before = get_cache_stats().size;

        let _t1 = fast_resize(&img, 100, 100).unwrap();
        let _t2 = fast_resize(&img, 200, 200).unwrap();
        let _t3 = fast_resize(&img, 300, 300).unwrap();

        let size_after = get_cache_stats().size;
        assert!(
            size_after >= size_before + 3,
            "Different sizes should be cached separately"
        );
    }

    #[test]
    fn different_images_cached_separately() {
        let _guard = CACHE_TEST_LOCK.lock().unwrap();
        clear_cache();
        let img1 = make_test_image(500, 500, [255, 0, 0, 255]);
        let img2 = make_test_image(500, 500, [0, 255, 0, 255]);
        let size_before = get_cache_stats().size;

        let _t1 = fast_resize(&img1, 100, 100).unwrap();
        let _t2 = fast_resize(&img2, 100, 100).unwrap();

        let size_after = get_cache_stats().size;
        assert!(
            size_after >= size_before + 2,
            "Different images should be cached separately"
        );
    }

    #[test]
    fn image_hash_deterministic() {
        let img = make_test_image(640, 480, [128, 128, 128, 255]);
        let hash1 = compute_image_hash(&img);
        let hash2 = compute_image_hash(&img);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn image_hash_different_for_different_images() {
        let img1 = make_test_image(640, 480, [0, 0, 0, 255]);
        let img2 = make_test_image(640, 480, [255, 255, 255, 255]);

        let hash1 = compute_image_hash(&img1);
        let hash2 = compute_image_hash(&img2);

        assert_ne!(hash1, hash2);
    }

    #[test]
    fn zero_size_source_error() {
        let img = DynamicImage::ImageRgba8(RgbaImage::new(0, 0));
        let result = fast_resize(&img, 100, 100);
        assert!(result.is_err());
    }

    #[test]
    fn zero_size_target_error() {
        let img = make_test_image(100, 100, [100, 100, 100, 255]);
        let result = fast_resize(&img, 0, 100);
        assert!(result.is_err());
    }

    #[test]
    fn oversized_target_error() {
        let img = make_test_image(100, 100, [100, 100, 100, 255]);
        let result = fast_resize(&img, 33000, 100);
        assert!(result.is_err());
    }

    #[test]
    fn oversized_target_height_error() {
        let img = make_test_image(100, 100, [100, 100, 100, 255]);
        let result = fast_resize(&img, 100, 33000);
        assert!(result.is_err());
    }
}
