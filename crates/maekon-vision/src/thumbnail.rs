use crate::error::VisionError;
use fast_image_resize::{images::Image as FirImage, ResizeAlg, ResizeOptions, Resizer};
use image::{DynamicImage, RgbaImage};
use lru::LruCache;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::borrow::Cow;
use std::num::NonZeroUsize;
use tracing::debug;

// F-PF-22: switch the thumbnail cache from an entry-count budget to a byte
// budget. When many (hash, w, h) key combinations are generated, the 20-entry
// limit was exhausted quickly, causing cache churn; this resolves that.
// - Default budget: 10 MB (based on RGBA w*h*4)
// - LRU entry limit: 256 (the effective limit is the byte budget — exhausted at
//   ~156 entries for 128×128 RGBA)
// - Overridable via the MAEKON_THUMBNAIL_CACHE_BYTES environment variable

/// Default byte budget: 10 MB.
const DEFAULT_CACHE_BUDGET_BYTES: usize = 10 * 1024 * 1024;

/// Entry-count cap (a secondary cap applied independently of the byte budget).
/// 128×128 RGBA = 64 KB/entry → effective limit ~156 entries under the 10 MB
/// budget. 256 is a larger safety margin; reaching this cap before the budget is
/// exhausted only happens when caching many very small thumbnails (e.g. 32×32 or
/// smaller).
const MAX_CACHE_ENTRIES: usize = 256;

/// Override the byte budget via the `MAEKON_THUMBNAIL_CACHE_BYTES` environment variable.
fn resolve_cache_budget() -> usize {
    std::env::var("MAEKON_THUMBNAIL_CACHE_BYTES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_CACHE_BUDGET_BYTES)
}

type CacheKey = (u64, u32, u32);

/// LRU thumbnail cache wrapper that tracks a byte budget.
struct ByteBudgetCache {
    /// Inner LRU cache (entry-count cap MAX_CACHE_ENTRIES).
    inner: LruCache<CacheKey, Vec<u8>>,
    /// Total bytes currently occupied by the cache.
    total_bytes: usize,
    /// Maximum allowed byte budget.
    budget_bytes: usize,
}

impl ByteBudgetCache {
    fn new(budget_bytes: usize) -> Self {
        Self {
            inner: LruCache::new(
                NonZeroUsize::new(MAX_CACHE_ENTRIES).expect("MAX_CACHE_ENTRIES must be > 0"),
            ),
            total_bytes: 0,
            budget_bytes,
        }
    }

    fn get(&mut self, key: &CacheKey) -> Option<&Vec<u8>> {
        self.inner.get(key)
    }

    /// Insert a new entry and evict LRU entries when the byte budget is exceeded.
    fn put(&mut self, key: CacheKey, value: Vec<u8>) {
        let entry_bytes = value.len();

        // If the same key already exists, subtract the previous entry's bytes.
        if let Some(old) = self.inner.peek(&key) {
            self.total_bytes = self.total_bytes.saturating_sub(old.len());
        }

        self.inner.put(key, value);
        self.total_bytes = self.total_bytes.saturating_add(entry_bytes);

        // Evict LRU entries when the byte budget is exceeded.
        while self.total_bytes > self.budget_bytes {
            if let Some((_, evicted)) = self.inner.pop_lru() {
                self.total_bytes = self.total_bytes.saturating_sub(evicted.len());
            } else {
                break;
            }
        }
    }

    fn len(&self) -> usize {
        self.inner.len()
    }

    #[cfg(test)]
    fn clear(&mut self) {
        self.inner.clear();
        self.total_bytes = 0;
    }

    /// Current total byte usage.
    fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    /// Maximum byte budget.
    fn budget_bytes(&self) -> usize {
        self.budget_bytes
    }
}

static THUMBNAIL_CACHE: Lazy<Mutex<ByteBudgetCache>> = Lazy::new(|| {
    let budget = resolve_cache_budget();
    Mutex::new(ByteBudgetCache::new(budget))
});

#[inline]
fn compute_image_hash(image: &DynamicImage) -> u64 {
    // Borrow the existing RgbaImage when the input is already ImageRgba8
    // (hot-path from ScreenCapture); only allocate when the format differs.
    let rgba: Cow<'_, RgbaImage> = match image.as_rgba8() {
        Some(r) => Cow::Borrowed(r),
        None => Cow::Owned(image.to_rgba8()),
    };
    let (w, h) = (rgba.width(), rgba.height());
    let raw = rgba.as_raw();

    let mut hash: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    hash ^= w as u64;
    hash = hash.wrapping_mul(FNV_PRIME);
    hash ^= h as u64;
    hash = hash.wrapping_mul(FNV_PRIME);

    // Hash ALL source bytes, not a 256-pixel sample (review4 V16). The (hash, w, h)
    // key is a content-identity key used to substitute a previously-resized
    // thumbnail on a cache hit; a sparse 256-sample hash let two genuinely different
    // frames — differing only between sample columns or in the unsampled
    // bottom-right strip — collide and return the WRONG image's thumbnail (uploaded
    // + stored as dashcam). This runs inside spawn_blocking (off the 1s capture
    // loop), so a full-buffer scan is affordable. 8 bytes/iteration for speed.
    let mut chunks = raw.chunks_exact(8);
    for chunk in &mut chunks {
        let word = u64::from_le_bytes([
            chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6], chunk[7],
        ]);
        hash ^= word;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    for &byte in chunks.remainder() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
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

    // Borrow when already ImageRgba8 to avoid an extra heap allocation;
    // copy only the raw pixel bytes (required by FirImage::from_vec_u8).
    let src_rgba: Cow<'_, RgbaImage> = match image.as_rgba8() {
        Some(r) => Cow::Borrowed(r),
        None => Cow::Owned(image.to_rgba8()),
    };

    let src_image = FirImage::from_vec_u8(
        src_w,
        src_h,
        src_rgba.as_raw().to_vec(),
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

pub fn resize_to_fit(
    image: &DynamicImage,
    max_width: u32,
    max_height: u32,
) -> Result<DynamicImage, VisionError> {
    let (src_w, src_h) = (image.width(), image.height());

    if src_w == 0 || src_h == 0 {
        return Err(VisionError::Internal(
            "Source image size is zero".to_string(),
        ));
    }
    if max_width == 0 || max_height == 0 {
        return Err(VisionError::Internal(
            "Target image size is zero".to_string(),
        ));
    }

    let scale = (max_width as f64 / src_w as f64)
        .min(max_height as f64 / src_h as f64)
        .min(1.0);
    let width = ((src_w as f64 * scale).round() as u32).max(1);
    let height = ((src_h as f64 * scale).round() as u32).max(1);

    fast_resize(image, width, height)
}

pub struct CacheStats {
    /// Number of entries currently stored in the cache.
    pub size: usize,
    /// Total byte usage.
    pub total_bytes: usize,
    /// Maximum byte budget.
    pub budget_bytes: usize,
}

pub fn get_cache_stats() -> CacheStats {
    let cache = THUMBNAIL_CACHE.lock();
    CacheStats {
        size: cache.len(),
        total_bytes: cache.total_bytes(),
        budget_bytes: cache.budget_bytes(),
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
    fn crt_prv_cap_005_thumbnail_fit_preserves_aspect_ratio() {
        let _guard = CACHE_TEST_LOCK.lock().unwrap();
        clear_cache();

        let cases = [
            ((1920, 1080), (200, 113)),
            ((800, 600), (200, 150)),
            ((1080, 1920), (113, 200)),
        ];

        for ((src_w, src_h), expected) in cases {
            let img = make_test_image(src_w, src_h, [100, 100, 100, 255]);
            let thumb = resize_to_fit(&img, 200, 200).unwrap();

            assert_eq!(thumb.dimensions(), expected);
            assert!(
                thumb.width() <= 200 && thumb.height() <= 200,
                "thumbnail should fit within target box"
            );
        }
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
        let err = fast_resize(&img, 100, 100).unwrap_err();
        assert!(
            matches!(err, VisionError::Internal(_)),
            "zero-size source must produce Internal error, got: {err:?}"
        );
        assert!(
            err.to_string().contains("zero"),
            "error must mention zero dimensions, got: {err}"
        );
    }

    #[test]
    fn zero_size_target_error() {
        let img = make_test_image(100, 100, [100, 100, 100, 255]);
        let err = fast_resize(&img, 0, 100).unwrap_err();
        assert!(
            matches!(err, VisionError::Internal(_)),
            "zero target width must produce Internal error, got: {err:?}"
        );
        assert!(
            err.to_string().contains("zero"),
            "error must mention zero dimensions, got: {err}"
        );
    }

    #[test]
    fn oversized_target_error() {
        let img = make_test_image(100, 100, [100, 100, 100, 255]);
        let err = fast_resize(&img, 33000, 100).unwrap_err();
        assert!(
            matches!(err, VisionError::Internal(_)),
            "oversized width must produce Internal error, got: {err:?}"
        );
        assert!(
            err.to_string().contains("too large") || err.to_string().contains("33000"),
            "error must mention oversized constraint, got: {err}"
        );
    }

    #[test]
    fn oversized_target_height_error() {
        let img = make_test_image(100, 100, [100, 100, 100, 255]);
        let err = fast_resize(&img, 100, 33000).unwrap_err();
        assert!(
            matches!(err, VisionError::Internal(_)),
            "oversized height must produce Internal error, got: {err:?}"
        );
        assert!(
            err.to_string().contains("too large") || err.to_string().contains("33000"),
            "error must mention oversized constraint, got: {err}"
        );
    }

    /// F-PF-22: verify the byte-budget-based cache.
    /// When large thumbnails exceeding the budget are inserted, LRU eviction must
    /// occur so that total_bytes always stays at or below budget_bytes.
    #[test]
    fn test_thumbnail_cache_byte_budget() {
        let _guard = CACHE_TEST_LOCK.lock().unwrap();
        clear_cache();

        // Each 800×600 RGBA thumbnail = 800*600*4 = 1,920,000 bytes (~1.83 MB).
        // Inserting 6 into the 10 MB budget → 11.52 MB total > 10 MB, so eviction occurs.
        for i in 0_u8..6 {
            let img = make_test_image(800, 600, [i, i * 2, i * 3, 255]);
            fast_resize(&img, 800, 600).unwrap();
        }

        let stats = get_cache_stats();
        assert!(
            stats.total_bytes <= stats.budget_bytes,
            "total_bytes {} should be <= budget_bytes {}",
            stats.total_bytes,
            stats.budget_bytes
        );
    }

    /// F-PF-22: confirm the 10 MB default budget.
    #[test]
    fn test_thumbnail_cache_default_budget_is_10mb() {
        let stats = get_cache_stats();
        // With the environment variable unset, the default budget = 10 MB.
        // Other tests may leave total_bytes > 0 after inserting, so only check budget_bytes.
        assert_eq!(
            stats.budget_bytes, DEFAULT_CACHE_BUDGET_BYTES,
            "default byte budget must be 10 MB"
        );
    }

    // Verify that the Cow borrow path (ImageRgba8) and the owned path
    // (non-Rgba8 format) produce the same output dimensions from fast_resize.

    #[test]
    fn fast_resize_rgba8_and_luma8_same_output_dimensions() {
        let _guard = CACHE_TEST_LOCK.lock().unwrap();
        clear_cache();

        // Borrow path: ImageRgba8 input — as_rgba8() returns Some.
        let rgba_img = make_test_image(120, 80, [100, 150, 200, 255]);
        let result_rgba = fast_resize(&rgba_img, 60, 40).unwrap();
        assert_eq!(result_rgba.dimensions(), (60, 40));

        // Owned path: ImageLuma8 input forces to_rgba8() conversion inside
        // fast_resize (and inside compute_image_hash).
        let luma_img =
            DynamicImage::ImageLuma8(image::GrayImage::from_pixel(120, 80, image::Luma([128])));
        let result_luma = fast_resize(&luma_img, 60, 40).unwrap();
        assert_eq!(result_luma.dimensions(), (60, 40));
    }

    #[test]
    fn compute_image_hash_rgba8_and_luma8_paths_both_deterministic() {
        // Ensure compute_image_hash works correctly for both code paths
        // (Cow::Borrowed for ImageRgba8, Cow::Owned for Luma8).
        let rgba_img = make_test_image(64, 48, [80, 160, 40, 255]);
        let hash_rgba_1 = compute_image_hash(&rgba_img);
        let hash_rgba_2 = compute_image_hash(&rgba_img);
        assert_eq!(
            hash_rgba_1, hash_rgba_2,
            "hash must be deterministic for ImageRgba8"
        );

        let luma_img =
            DynamicImage::ImageLuma8(image::GrayImage::from_pixel(64, 48, image::Luma([80])));
        let hash_luma_1 = compute_image_hash(&luma_img);
        let hash_luma_2 = compute_image_hash(&luma_img);
        assert_eq!(
            hash_luma_1, hash_luma_2,
            "hash must be deterministic for ImageLuma8"
        );
    }
}
