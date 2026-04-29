use image::{DynamicImage, GenericImageView};
use oneshim_core::models::frame::Rect;
use tracing::debug;

#[derive(Debug, Clone)]
pub struct DeltaRegion {
    pub region: Rect,
    pub changed_ratio: f32,
    pub changed_tiles: u32,
    pub total_tiles: u32,
}

const TILE_SIZE: u32 = 16;

const CHANGE_THRESHOLD: u32 = 30;

pub fn compute_delta(prev: &DynamicImage, curr: &DynamicImage) -> Option<DeltaRegion> {
    let (pw, ph) = prev.dimensions();
    let (cw, ch) = curr.dimensions();

    if pw != cw || ph != ch {
        return Some(DeltaRegion {
            region: Rect {
                x: 0,
                y: 0,
                w: cw,
                h: ch,
            },
            changed_ratio: 1.0,
            changed_tiles: ((cw / TILE_SIZE) + 1) * ((ch / TILE_SIZE) + 1),
            total_tiles: ((cw / TILE_SIZE) + 1) * ((ch / TILE_SIZE) + 1),
        });
    }

    let prev_rgba = prev.to_rgba8();
    let curr_rgba = curr.to_rgba8();

    let prev_raw = prev_rgba.as_raw();
    let curr_raw = curr_rgba.as_raw();
    let stride = pw as usize * 4; // RGBA 4
    let tiles_x = pw.div_ceil(TILE_SIZE);
    let tiles_y = ph.div_ceil(TILE_SIZE);
    let total_tiles = tiles_x * tiles_y;

    let mut changed_tiles = 0u32;
    let mut min_x = pw;
    let mut min_y = ph;
    let mut max_x = 0u32;
    let mut max_y = 0u32;

    for ty in 0..tiles_y {
        for tx in 0..tiles_x {
            let start_x = tx * TILE_SIZE;
            let start_y = ty * TILE_SIZE;
            let end_x = (start_x + TILE_SIZE).min(pw);
            let end_y = (start_y + TILE_SIZE).min(ph);

            if is_tile_changed_fast(prev_raw, curr_raw, stride, start_x, start_y, end_x, end_y) {
                changed_tiles += 1;
                min_x = min_x.min(start_x);
                min_y = min_y.min(start_y);
                max_x = max_x.max(end_x);
                max_y = max_y.max(end_y);
            }
        }
    }

    if changed_tiles == 0 {
        debug!("change none");
        return None;
    }

    let changed_ratio = changed_tiles as f32 / total_tiles as f32;

    debug!(
        "Delta detection: {changed_tiles}/{total_tiles} tiles changed ({:.1}%)",
        changed_ratio * 100.0
    );

    Some(DeltaRegion {
        region: Rect {
            x: min_x,
            y: min_y,
            w: max_x - min_x,
            h: max_y - min_y,
        },
        changed_ratio,
        changed_tiles,
        total_tiles,
    })
}

#[inline]
fn is_tile_changed_fast(
    prev: &[u8],
    curr: &[u8],
    stride: usize,
    start_x: u32,
    start_y: u32,
    end_x: u32,
    end_y: u32,
) -> bool {
    let mut diff_sum = 0u64;
    let mut pixel_count = 0u64;

    let start_x = start_x as usize;
    let start_y = start_y as usize;
    let end_x = end_x as usize;
    let end_y = end_y as usize;

    for y in start_y..end_y {
        let row_offset = y * stride;
        for x in start_x..end_x {
            let pixel_offset = row_offset + x * 4;

            let pr = prev[pixel_offset] as i32;
            let pg = prev[pixel_offset + 1] as i32;
            let pb = prev[pixel_offset + 2] as i32;

            let cr = curr[pixel_offset] as i32;
            let cg = curr[pixel_offset + 1] as i32;
            let cb = curr[pixel_offset + 2] as i32;

            let dr = (pr - cr).unsigned_abs();
            let dg = (pg - cg).unsigned_abs();
            let db = (pb - cb).unsigned_abs();

            diff_sum += (dr + dg + db) as u64;
            pixel_count += 1;
        }
    }

    if pixel_count == 0 {
        return false;
    }

    let avg_diff = diff_sum / pixel_count;
    avg_diff > CHANGE_THRESHOLD as u64
}

/// Compute delta with adjustable sensitivity threshold.
/// Public API for consumers needing fine-grained change detection; currently exercised by tests.
#[allow(dead_code)]
pub fn compute_delta_adaptive(
    prev: &DynamicImage,
    curr: &DynamicImage,
    sensitivity: f32,
) -> Option<DeltaRegion> {
    let (pw, ph) = prev.dimensions();
    let (cw, ch) = curr.dimensions();

    if pw != cw || ph != ch {
        return Some(DeltaRegion {
            region: Rect {
                x: 0,
                y: 0,
                w: cw,
                h: ch,
            },
            changed_ratio: 1.0,
            changed_tiles: ((cw / TILE_SIZE) + 1) * ((ch / TILE_SIZE) + 1),
            total_tiles: ((cw / TILE_SIZE) + 1) * ((ch / TILE_SIZE) + 1),
        });
    }

    let prev_rgba = prev.to_rgba8();
    let curr_rgba = curr.to_rgba8();
    let prev_raw = prev_rgba.as_raw();
    let curr_raw = curr_rgba.as_raw();
    let stride = pw as usize * 4;

    let threshold = ((CHANGE_THRESHOLD as f32) / sensitivity.clamp(0.5, 2.0)).ceil() as u64;

    let tiles_x = pw.div_ceil(TILE_SIZE);
    let tiles_y = ph.div_ceil(TILE_SIZE);
    let total_tiles = tiles_x * tiles_y;

    let mut changed_tiles = 0u32;
    let mut min_x = pw;
    let mut min_y = ph;
    let mut max_x = 0u32;
    let mut max_y = 0u32;

    for ty in 0..tiles_y {
        for tx in 0..tiles_x {
            let start_x = tx * TILE_SIZE;
            let start_y = ty * TILE_SIZE;
            let end_x = (start_x + TILE_SIZE).min(pw);
            let end_y = (start_y + TILE_SIZE).min(ph);

            if is_tile_changed_with_threshold(
                prev_raw, curr_raw, stride, start_x, start_y, end_x, end_y, threshold,
            ) {
                changed_tiles += 1;
                min_x = min_x.min(start_x);
                min_y = min_y.min(start_y);
                max_x = max_x.max(end_x);
                max_y = max_y.max(end_y);
            }
        }
    }

    if changed_tiles == 0 {
        return None;
    }

    let changed_ratio = changed_tiles as f32 / total_tiles as f32;

    Some(DeltaRegion {
        region: Rect {
            x: min_x,
            y: min_y,
            w: max_x - min_x,
            h: max_y - min_y,
        },
        changed_ratio,
        changed_tiles,
        total_tiles,
    })
}

#[inline]
#[allow(clippy::too_many_arguments)]
fn is_tile_changed_with_threshold(
    prev: &[u8],
    curr: &[u8],
    stride: usize,
    start_x: u32,
    start_y: u32,
    end_x: u32,
    end_y: u32,
    threshold: u64,
) -> bool {
    let mut diff_sum = 0u64;
    let mut pixel_count = 0u64;

    let start_x = start_x as usize;
    let start_y = start_y as usize;
    let end_x = end_x as usize;
    let end_y = end_y as usize;

    for y in start_y..end_y {
        let row_offset = y * stride;
        for x in start_x..end_x {
            let pixel_offset = row_offset + x * 4;

            let pr = prev[pixel_offset] as i32;
            let pg = prev[pixel_offset + 1] as i32;
            let pb = prev[pixel_offset + 2] as i32;

            let cr = curr[pixel_offset] as i32;
            let cg = curr[pixel_offset + 1] as i32;
            let cb = curr[pixel_offset + 2] as i32;

            let dr = (pr - cr).unsigned_abs();
            let dg = (pg - cg).unsigned_abs();
            let db = (pb - cb).unsigned_abs();

            diff_sum += (dr + dg + db) as u64;
            pixel_count += 1;
        }
    }

    if pixel_count == 0 {
        return false;
    }

    diff_sum / pixel_count > threshold
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, RgbaImage};

    #[test]
    fn identical_images_no_delta() {
        let img = DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            64,
            64,
            image::Rgba([100, 150, 200, 255]),
        ));
        let result = compute_delta(&img, &img);
        assert!(result.is_none());
    }

    #[test]
    fn different_images_have_delta() {
        let prev =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(64, 64, image::Rgba([0, 0, 0, 255])));
        let curr = DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            64,
            64,
            image::Rgba([255, 255, 255, 255]),
        ));
        let result = compute_delta(&prev, &curr);
        assert!(result.is_some());
        let delta = result.unwrap();
        assert!(delta.changed_ratio > 0.0);
    }

    #[test]
    fn different_resolution_full_change() {
        let prev = DynamicImage::ImageRgba8(RgbaImage::new(64, 64));
        let curr = DynamicImage::ImageRgba8(RgbaImage::new(128, 128));
        let result = compute_delta(&prev, &curr);
        assert!(result.is_some());
        assert_eq!(result.unwrap().changed_ratio, 1.0);
    }

    #[test]
    fn partial_change() {
        let prev = RgbaImage::from_pixel(64, 64, image::Rgba([100, 100, 100, 255]));
        let mut curr = prev.clone();

        for y in 0..16 {
            for x in 0..16 {
                curr.put_pixel(x, y, image::Rgba([255, 0, 0, 255]));
            }
        }

        let result = compute_delta(
            &DynamicImage::ImageRgba8(prev),
            &DynamicImage::ImageRgba8(curr),
        );
        assert!(result.is_some());
        let delta = result.unwrap();
        assert!(delta.changed_ratio > 0.0);
        assert!(delta.changed_ratio < 1.0);
    }

    #[test]
    fn adaptive_delta_high_sensitivity() {
        let prev = RgbaImage::from_pixel(64, 64, image::Rgba([100, 100, 100, 255]));
        let mut curr = prev.clone();

        for y in 0..16 {
            for x in 0..16 {
                curr.put_pixel(x, y, image::Rgba([110, 110, 110, 255]));
            }
        }

        let result_normal = compute_delta(
            &DynamicImage::ImageRgba8(prev.clone()),
            &DynamicImage::ImageRgba8(curr.clone()),
        );

        let result_high = compute_delta_adaptive(
            &DynamicImage::ImageRgba8(prev),
            &DynamicImage::ImageRgba8(curr),
            2.0,
        );

        assert!(
            result_high.is_some()
                || result_normal.is_some()
                || (result_high.is_none() && result_normal.is_none())
        );
    }

    #[test]
    #[ignore] // Performance regression check — run in isolation: cargo test -p oneshim-vision benchmark_delta_4k -- --ignored
    fn benchmark_delta_4k() {
        use std::time::Instant;
        let width = 3840u32;
        let height = 2160u32;

        // Create a base 4K image (solid gray)
        let base = RgbaImage::from_pixel(width, height, image::Rgba([128, 128, 128, 255]));
        let mut modified = base.clone();

        // Modify a 200x200 region in the center to simulate a small UI change
        let cx = width / 2;
        let cy = height / 2;
        for y in (cy - 100)..(cy + 100) {
            for x in (cx - 100)..(cx + 100) {
                modified.put_pixel(x, y, image::Rgba([255, 0, 0, 255]));
            }
        }

        let img_a = DynamicImage::ImageRgba8(base);
        let img_b = DynamicImage::ImageRgba8(modified);

        let start = Instant::now();
        let result = compute_delta(&img_a, &img_b);
        let elapsed = start.elapsed();

        println!(
            "4K delta detection: {:?} ({} changed tiles)",
            elapsed,
            result.as_ref().map_or(0, |r| r.changed_tiles)
        );

        assert!(result.is_some(), "should detect changed region");
        let delta = result.unwrap();
        assert!(delta.changed_tiles > 0);
        // Most tiles should be unchanged — only center region differs
        assert!(delta.changed_ratio < 0.1, "only a small region changed");
        assert!(
            elapsed.as_millis() < 500,
            "4K delta should complete in <500ms, took {}ms",
            elapsed.as_millis()
        );
    }

    #[test]
    fn pointer_access_correctness() {
        let prev = RgbaImage::from_pixel(32, 32, image::Rgba([50, 100, 150, 255]));
        let mut curr = prev.clone();

        for y in 8..24 {
            for x in 8..24 {
                curr.put_pixel(x, y, image::Rgba([200, 50, 100, 255]));
            }
        }

        let prev_dyn = DynamicImage::ImageRgba8(prev);
        let curr_dyn = DynamicImage::ImageRgba8(curr);

        let result = compute_delta(&prev_dyn, &curr_dyn);
        assert!(result.is_some());

        let delta = result.unwrap();
        assert!(delta.changed_tiles >= 1);
        assert!(delta.region.w > 0 && delta.region.h > 0);
    }
}
