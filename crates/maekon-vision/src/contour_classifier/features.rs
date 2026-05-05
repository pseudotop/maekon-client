//! Visual feature extraction from GUI element crops.

/// Visual features extracted from an RGBA image crop.
#[derive(Debug, Clone)]
pub struct VisualFeatures {
    /// Average brightness difference between border and interior (0.0-1.0).
    pub border_contrast: f32,
    /// Inverse of interior pixel variance — higher = more uniform fill (0.0-1.0).
    pub fill_uniformity: f32,
    /// True if 3+ edges show significant contrast against interior.
    pub has_distinct_border: bool,
    /// True if interior brightness is neither pure white nor pure black.
    pub has_background_fill: bool,
    /// Width / height ratio.
    pub aspect_ratio: f32,
}

/// Extract visual features from an RGBA crop for element classification.
pub fn extract_visual_features(rgba: &[u8], w: u32, h: u32) -> VisualFeatures {
    let aspect_ratio = w as f32 / h.max(1) as f32;

    // Guard: buffer length must match dimensions; tiny crops can't split border from interior
    if rgba.len() != (w as usize * h as usize * 4) || w < 5 || h < 5 {
        return VisualFeatures {
            border_contrast: 0.0,
            fill_uniformity: 1.0,
            has_distinct_border: false,
            has_background_fill: false,
            aspect_ratio,
        };
    }

    // RGBA → Luma grayscale (BT.601 approximation)
    let gray: Vec<u8> = rgba
        .chunks_exact(4)
        .map(|px| ((px[0] as u32 * 77 + px[1] as u32 * 150 + px[2] as u32 * 29) >> 8) as u8)
        .collect();

    // Border analysis: sample 2px strips on each edge
    let top = avg_brightness(&gray, w, 0, 0, w, 2);
    let bottom = avg_brightness(&gray, w, 0, h - 2, w, 2);
    let left = avg_brightness(&gray, w, 0, 0, 2, h);
    let right = avg_brightness(&gray, w, w - 2, 0, 2, h);
    let interior = avg_brightness(&gray, w, 2, 2, w - 4, h - 4);

    let diffs = [
        (top - interior).abs(),
        (bottom - interior).abs(),
        (left - interior).abs(),
        (right - interior).abs(),
    ];
    let border_contrast = (diffs.iter().sum::<f32>() / 4.0) / 255.0;
    let edges_with_border = diffs.iter().filter(|&&d| d > 20.0).count();

    // Fill uniformity: std_dev of interior pixels (lower variance → higher uniformity)
    let interior_std = std_dev_brightness(&gray, w, 2, 2, w - 4, h - 4);
    let fill_uniformity = 1.0 - (interior_std / 128.0).min(1.0);

    // Background detection
    let has_background_fill = interior > 30.0 && interior < 225.0;

    VisualFeatures {
        border_contrast,
        fill_uniformity,
        has_distinct_border: edges_with_border >= 3,
        has_background_fill,
        aspect_ratio,
    }
}

/// Average brightness of a rectangular sub-region in a grayscale buffer.
fn avg_brightness(gray: &[u8], stride: u32, x: u32, y: u32, w: u32, h: u32) -> f32 {
    if w == 0 || h == 0 {
        return 0.0;
    }
    let mut sum: u64 = 0;
    let count = w as u64 * h as u64;
    for row in y..y + h {
        let start = (row * stride + x) as usize;
        let end = start + w as usize;
        if end <= gray.len() {
            for &px in &gray[start..end] {
                sum += px as u64;
            }
        }
    }
    sum as f32 / count as f32
}

/// Standard deviation of brightness in a rectangular sub-region.
fn std_dev_brightness(gray: &[u8], stride: u32, x: u32, y: u32, w: u32, h: u32) -> f32 {
    if w == 0 || h == 0 {
        return 0.0;
    }
    let mean = avg_brightness(gray, stride, x, y, w, h);
    let mut var_sum: f64 = 0.0;
    let mut count: u64 = 0;
    for row in y..y + h {
        let start = (row * stride + x) as usize;
        let end = start + w as usize;
        if end <= gray.len() {
            for &px in &gray[start..end] {
                let diff = px as f64 - mean as f64;
                var_sum += diff * diff;
                count += 1;
            }
        }
    }
    if count == 0 {
        return 0.0;
    }
    (var_sum / count as f64).sqrt() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a solid-color RGBA buffer.
    fn solid_rgba(w: u32, h: u32, r: u8, g: u8, b: u8) -> Vec<u8> {
        let mut buf = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..w * h {
            buf.extend_from_slice(&[r, g, b, 255]);
        }
        buf
    }

    /// Create an RGBA buffer with a border of one color and interior of another.
    fn bordered_rgba(w: u32, h: u32, border: u8, interior: u8) -> Vec<u8> {
        let mut buf = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let c = if x < 2 || x >= w - 2 || y < 2 || y >= h - 2 {
                    border
                } else {
                    interior
                };
                buf.extend_from_slice(&[c, c, c, 255]);
            }
        }
        buf
    }

    #[test]
    fn solid_crop_high_uniformity() {
        let rgba = solid_rgba(60, 30, 128, 128, 128);
        let f = extract_visual_features(&rgba, 60, 30);
        assert!(
            f.fill_uniformity > 0.95,
            "solid color should be ~1.0: {}",
            f.fill_uniformity
        );
        assert!(
            f.border_contrast < 0.05,
            "no border expected: {}",
            f.border_contrast
        );
        assert!(!f.has_distinct_border);
    }

    #[test]
    fn bordered_crop_high_contrast() {
        let rgba = bordered_rgba(60, 30, 0, 200);
        let f = extract_visual_features(&rgba, 60, 30);
        assert!(
            f.border_contrast > 0.3,
            "border should be detected: {}",
            f.border_contrast
        );
        assert!(f.has_distinct_border);
        assert!(f.has_background_fill);
    }

    #[test]
    fn tiny_crop_returns_defaults() {
        let rgba = solid_rgba(3, 3, 128, 128, 128);
        let f = extract_visual_features(&rgba, 3, 3);
        assert_eq!(f.border_contrast, 0.0);
        assert_eq!(f.fill_uniformity, 1.0);
        assert!(!f.has_distinct_border);
    }

    #[test]
    fn misaligned_buffer_returns_defaults() {
        // Buffer too short for claimed dimensions — should not panic
        let rgba = vec![0u8; 100]; // 100 bytes, but 60×30 needs 7200
        let f = extract_visual_features(&rgba, 60, 30);
        assert_eq!(f.border_contrast, 0.0);
        assert_eq!(f.fill_uniformity, 1.0);
    }

    #[test]
    fn aspect_ratio_calculation() {
        let rgba = solid_rgba(120, 30, 0, 0, 0);
        let f = extract_visual_features(&rgba, 120, 30);
        assert!((f.aspect_ratio - 4.0).abs() < 0.01);
    }

    #[test]
    fn noisy_interior_low_uniformity() {
        // Alternating bright/dark pixels
        let mut rgba = Vec::with_capacity(60 * 30 * 4);
        for i in 0..60 * 30 {
            let c = if i % 2 == 0 { 50 } else { 200 };
            rgba.extend_from_slice(&[c, c, c, 255]);
        }
        let f = extract_visual_features(&rgba, 60, 30);
        assert!(
            f.fill_uniformity < 0.6,
            "noisy should have low uniformity: {}",
            f.fill_uniformity
        );
    }
}
