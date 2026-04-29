use crate::error::VisionError;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use image::{DynamicImage, GenericImageView};
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::debug;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebPQuality {
    Low = 60,
    Medium = 75,
    High = 85,
}

struct CompressionStats {
    cumulative_ratio_x1000: AtomicU64,
    sample_count: AtomicU64,
}

impl CompressionStats {
    const fn new() -> Self {
        Self {
            cumulative_ratio_x1000: AtomicU64::new(0),
            sample_count: AtomicU64::new(0),
        }
    }

    fn record(&self, raw_size: usize, encoded_size: usize) {
        if raw_size == 0 {
            return;
        }
        let ratio_x1000 = (encoded_size * 1000 / raw_size) as u64;
        self.cumulative_ratio_x1000
            .fetch_add(ratio_x1000, Ordering::Relaxed);
        self.sample_count.fetch_add(1, Ordering::Relaxed);
    }

    fn average_ratio(&self) -> f32 {
        let count = self.sample_count.load(Ordering::Relaxed);
        if count == 0 {
            return 0.3; // default estimate (30%)
        }
        let cumulative = self.cumulative_ratio_x1000.load(Ordering::Relaxed);
        (cumulative as f32 / count as f32) / 1000.0
    }
}

static STATS_HIGH: CompressionStats = CompressionStats::new();
static STATS_MEDIUM: CompressionStats = CompressionStats::new();
static STATS_LOW: CompressionStats = CompressionStats::new();

pub fn encode_webp(image: &DynamicImage, quality: WebPQuality) -> Result<Vec<u8>, VisionError> {
    let rgba = image.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    let raw_size = (w * h * 4) as usize;

    let encoder = webp::Encoder::from_rgba(&rgba, w, h);
    let encoded = encoder.encode(quality as u8 as f32);
    let encoded_vec = encoded.to_vec();

    match quality {
        WebPQuality::High => STATS_HIGH.record(raw_size, encoded_vec.len()),
        WebPQuality::Medium => STATS_MEDIUM.record(raw_size, encoded_vec.len()),
        WebPQuality::Low => STATS_LOW.record(raw_size, encoded_vec.len()),
    }

    debug!(
        "WebP encoding: {}x{} → {} bytes (quality {}, compression ratio {:.1}%)",
        w,
        h,
        encoded_vec.len(),
        quality as u8,
        (encoded_vec.len() as f32 / raw_size as f32) * 100.0
    );

    Ok(encoded_vec)
}

pub fn encode_webp_base64(
    image: &DynamicImage,
    quality: WebPQuality,
) -> Result<String, VisionError> {
    let bytes = encode_webp(image, quality)?;
    Ok(B64.encode(&bytes))
}

pub fn encode_adaptive(
    image: &DynamicImage,
    max_bytes: usize,
) -> Result<(Vec<u8>, WebPQuality), VisionError> {
    let (w, h) = image.dimensions();
    let raw_size = (w * h * 4) as usize;

    let target_ratio = max_bytes as f32 / raw_size as f32;

    let estimated_quality = estimate_quality_from_stats(target_ratio);

    let encoded = encode_webp(image, estimated_quality)?;

    if encoded.len() <= max_bytes {
        return Ok((encoded, estimated_quality));
    }

    let fallback_quality = match estimated_quality {
        WebPQuality::High => WebPQuality::Medium,
        WebPQuality::Medium => WebPQuality::Low,
        WebPQuality::Low => {
            return Ok((encoded, WebPQuality::Low));
        }
    };

    let fallback_encoded = encode_webp(image, fallback_quality)?;

    if fallback_encoded.len() <= max_bytes {
        return Ok((fallback_encoded, fallback_quality));
    }

    if fallback_quality != WebPQuality::Low {
        let low_encoded = encode_webp(image, WebPQuality::Low)?;
        return Ok((low_encoded, WebPQuality::Low));
    }

    Ok((fallback_encoded, WebPQuality::Low))
}

fn estimate_quality_from_stats(target_ratio: f32) -> WebPQuality {
    let high_ratio = STATS_HIGH.average_ratio();
    let medium_ratio = STATS_MEDIUM.average_ratio();
    let _low_ratio = STATS_LOW.average_ratio();

    let margin = 1.1; // 10%
    if high_ratio * margin <= target_ratio {
        WebPQuality::High
    } else if medium_ratio * margin <= target_ratio {
        WebPQuality::Medium
    } else {
        WebPQuality::Low
    }
}

pub fn encode_smart_adaptive(
    image: &DynamicImage,
    max_bytes: usize,
) -> Result<(Vec<u8>, WebPQuality), VisionError> {
    let (w, h) = image.dimensions();
    let raw_size = (w * h * 4) as usize;

    if w * h < 500_000 {
        return encode_adaptive(image, max_bytes);
    }

    let test_w = (w / 10).max(1);
    let test_h = (h / 10).max(1);
    let test_image = image.resize_exact(test_w, test_h, image::imageops::FilterType::Nearest);

    let test_raw_size = (test_w * test_h * 4) as usize;
    let test_encoded = encode_webp(&test_image, WebPQuality::High)?;
    let test_ratio = test_encoded.len() as f32 / test_raw_size as f32;

    let expected_high = (raw_size as f32 * test_ratio) as usize;

    let quality = if expected_high <= max_bytes {
        WebPQuality::High
    } else {
        let test_medium = encode_webp(&test_image, WebPQuality::Medium)?;
        let medium_ratio = test_medium.len() as f32 / test_raw_size as f32;
        let expected_medium = (raw_size as f32 * medium_ratio) as usize;

        if expected_medium <= max_bytes {
            WebPQuality::Medium
        } else {
            WebPQuality::Low
        }
    };

    let encoded = encode_webp(image, quality)?;
    Ok((encoded, quality))
}

#[cfg(test)]
pub fn reset_stats() {
    STATS_HIGH
        .cumulative_ratio_x1000
        .store(0, Ordering::Relaxed);
    STATS_HIGH.sample_count.store(0, Ordering::Relaxed);
    STATS_MEDIUM
        .cumulative_ratio_x1000
        .store(0, Ordering::Relaxed);
    STATS_MEDIUM.sample_count.store(0, Ordering::Relaxed);
    STATS_LOW.cumulative_ratio_x1000.store(0, Ordering::Relaxed);
    STATS_LOW.sample_count.store(0, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, RgbaImage};

    fn make_test_image(w: u32, h: u32) -> DynamicImage {
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            w,
            h,
            image::Rgba([128, 64, 200, 255]),
        ))
    }

    #[test]
    fn encode_webp_basic() {
        let img = make_test_image(100, 100);
        let bytes = encode_webp(&img, WebPQuality::Medium).unwrap();
        assert!(!bytes.is_empty());
    }

    #[test]
    fn encode_base64() {
        let img = make_test_image(50, 50);
        let b64 = encode_webp_base64(&img, WebPQuality::Low).unwrap();
        assert!(!b64.is_empty());
        assert!(B64.decode(&b64).is_ok());
    }

    #[test]
    fn quality_levels_all_produce_output() {
        let img = make_test_image(200, 200);
        let low = encode_webp(&img, WebPQuality::Low).unwrap();
        let medium = encode_webp(&img, WebPQuality::Medium).unwrap();
        let high = encode_webp(&img, WebPQuality::High).unwrap();
        assert!(!low.is_empty());
        assert!(!medium.is_empty());
        assert!(!high.is_empty());
    }

    #[test]
    fn adaptive_encoding() {
        let img = make_test_image(100, 100);
        let (bytes, _quality) = encode_adaptive(&img, 100_000).unwrap();
        assert!(!bytes.is_empty());
    }

    #[test]
    fn compression_stats_tracking() {
        reset_stats();

        let img = make_test_image(100, 100);

        for _ in 0..5 {
            let _ = encode_webp(&img, WebPQuality::High);
        }

        let ratio = STATS_HIGH.average_ratio();
        assert!(ratio > 0.0 && ratio < 1.0, "compression ratio: {}", ratio);
    }

    #[test]
    fn smart_adaptive_small_image() {
        let img = make_test_image(100, 100);
        let (bytes, quality) = encode_smart_adaptive(&img, 100_000).unwrap();
        assert!(!bytes.is_empty());
        assert!(matches!(
            quality,
            WebPQuality::High | WebPQuality::Medium | WebPQuality::Low
        ));
    }

    #[test]
    fn quality_estimation() {
        reset_stats();

        let quality = estimate_quality_from_stats(0.5);
        assert_eq!(quality, WebPQuality::High);
    }

    #[test]
    fn encode_adaptive_respects_size_limit() {
        let img = make_test_image(200, 200);
        let raw_size = 200 * 200 * 4; // 160,000 bytes

        let (bytes, quality) = encode_adaptive(&img, 1000).unwrap();
        assert!(!bytes.is_empty());
        assert!(bytes.len() <= 1000);
        assert!(matches!(
            quality,
            WebPQuality::High | WebPQuality::Medium | WebPQuality::Low
        ));

        let (bytes2, quality2) = encode_adaptive(&img, raw_size).unwrap();
        assert!(!bytes2.is_empty());
        assert!(matches!(
            quality2,
            WebPQuality::High | WebPQuality::Medium | WebPQuality::Low
        ));
    }
}
