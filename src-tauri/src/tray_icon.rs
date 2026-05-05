//! Shape-based template tray icon generation.
//!
//! Takes the base template icon (black on transparent, 44x44 @2x)
//! and overlays a shape indicator to convey capture state:
//!
//! - **Active**: unmodified base logo (template icon, OS handles tinting)
//! - **Paused**: base logo + pause bars (two vertical bars at bottom-right)
//! - **Disabled**: base logo + diagonal slash (top-right to bottom-left)
//!
//! All variants remain black-on-transparent so they work correctly with
//! `icon_as_template(true)` on macOS (the OS inverts for dark/light mode).

// Use `::image` to disambiguate from `tauri::image`.
use ::image::{Rgba, RgbaImage};

/// Base icon (44x44 @2x) embedded at compile time.
const BASE_2X: &[u8] = include_bytes!("../icons/tray_icon@2x.png");

/// Tray icon state — determines which shape overlay is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Disabled variant used once tray.rs migrates to TrayIconState
pub enum TrayIconState {
    /// Actively capturing — unmodified base logo.
    Active,
    /// Capture paused — base logo with pause bars at bottom-right.
    Paused,
    /// Capture disabled — base logo with diagonal slash overlay.
    Disabled,
}

/// Backward-compatible conversion: `false` (capturing) → Active, `true` (paused) → Paused.
/// Will be removed once all callers migrate to `TrayIconState`.
impl From<bool> for TrayIconState {
    fn from(paused: bool) -> Self {
        if paused {
            TrayIconState::Paused
        } else {
            TrayIconState::Active
        }
    }
}

/// Generate a tray icon with the appropriate shape overlay for the given state.
///
/// Accepts `TrayIconState` directly, or `bool` via `Into<TrayIconState>`
/// for backward compatibility (`false` = Active, `true` = Paused).
///
/// Returns `(rgba_bytes, width, height)` suitable for
/// `tauri::image::Image::from_rgba()`.
pub fn status_icon(state: impl Into<TrayIconState>) -> (Vec<u8>, u32, u32) {
    let state = state.into();
    // Safe: BASE_2X is a compile-time embedded PNG via include_bytes!().
    let base = ::image::load_from_memory(BASE_2X).expect("embedded tray icon must be valid PNG");
    let mut img = base.to_rgba8();
    let w = img.width();
    let h = img.height();

    match state {
        TrayIconState::Active => { /* unmodified base */ }
        TrayIconState::Paused => draw_pause_bars(&mut img),
        TrayIconState::Disabled => draw_diagonal_slash(&mut img),
    }

    (img.into_raw(), w, h)
}

/// Draw two vertical pause bars at the bottom-right corner.
///
/// Each bar is 3px wide, 10px tall, with 2px gap between them,
/// positioned so the pause symbol center sits near (35, 35)
/// in the bottom-right quadrant of the 44x44 icon.
fn draw_pause_bars(img: &mut RgbaImage) {
    let color = Rgba([0, 0, 0, 255]); // black for template icon

    let bar_w = 3_u32;
    let bar_h = 10_u32;
    let gap = 2_u32;

    // Total width: bar + gap + bar = 3 + 2 + 3 = 8px
    // Center the symbol at x=35 in the bottom-right quadrant.
    // x_start = 35 - 8/2 = 31
    let x_start = 31_u32;
    let bar_y = 30_u32; // top of bars — centers vertically around y=35

    let left_bar_x = x_start;
    let right_bar_x = x_start + bar_w + gap; // 31 + 3 + 2 = 36

    // Left bar:  x=[31,33], y=[30,39]
    draw_filled_rect(img, left_bar_x, bar_y, bar_w, bar_h, color);
    // Right bar: x=[36,38], y=[30,39]
    draw_filled_rect(img, right_bar_x, bar_y, bar_w, bar_h, color);
}

/// Draw a diagonal slash from the top-right area to the bottom-left area.
///
/// The line is ~2.5px thick with anti-aliasing, drawn in black for template rendering.
fn draw_diagonal_slash(img: &mut RgbaImage) {
    let w = img.width() as f64;
    let h = img.height() as f64;

    // Slash endpoints: top-right area to bottom-left area, inset slightly
    let x0 = w - 8.0;
    let y0 = 8.0;
    let x1 = 8.0;
    let y1 = h - 8.0;

    draw_aa_line(img, x0, y0, x1, y1, 2.5, Rgba([0, 0, 0, 255]));
}

/// Draw an anti-aliased line with the given thickness.
///
/// Uses perpendicular distance from each pixel center to the line segment
/// to compute coverage, producing a smooth anti-aliased result.
fn draw_aa_line(
    img: &mut RgbaImage,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    thickness: f64,
    color: Rgba<u8>,
) {
    let iw = img.width();
    let ih = img.height();

    let dx = x1 - x0;
    let dy = y1 - y0;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 0.001 {
        return;
    }

    // Unit direction and perpendicular
    let ux = dx / len;
    let uy = dy / len;
    // perpendicular (rotated 90 degrees)
    let px = -uy;
    let py = ux;

    let half_t = thickness / 2.0;
    let aa_margin = 1.0; // extra pixel for anti-aliasing fringe

    // Bounding box of the line (with thickness + AA margin)
    let extent = half_t + aa_margin;
    let min_x = x0.min(x1) - extent;
    let max_x = x0.max(x1) + extent;
    let min_y = y0.min(y1) - extent;
    let max_y = y0.max(y1) + extent;

    let ix_min = (min_x.floor().max(0.0)) as u32;
    let ix_max = (max_x.ceil().min(iw as f64 - 1.0)) as u32;
    let iy_min = (min_y.floor().max(0.0)) as u32;
    let iy_max = (max_y.ceil().min(ih as f64 - 1.0)) as u32;

    for y in iy_min..=iy_max {
        for x in ix_min..=ix_max {
            let fx = x as f64;
            let fy = y as f64;

            // Vector from line start to pixel center
            let vx = fx - x0;
            let vy = fy - y0;

            // Project onto line direction to get parameter t
            let t = vx * ux + vy * uy;

            // Perpendicular distance from the line
            let perp_dist = (vx * px + vy * py).abs();

            // Longitudinal distance: clamp t to [0, len] to handle endpoints
            let clamped_t = t.clamp(0.0, len);
            let long_excess = (t - clamped_t).abs();

            // Total distance from the line segment (including endpoint rounding)
            let dist = if long_excess > 0.001 {
                (perp_dist * perp_dist + long_excess * long_excess).sqrt()
            } else {
                perp_dist
            };

            // Coverage: 1.0 inside half_t, smooth falloff in the AA fringe
            if dist < half_t + 0.5 {
                let coverage = (half_t + 0.5 - dist).clamp(0.0, 1.0);
                let alpha = (color[3] as f64 * coverage) as u8;
                if alpha > 0 {
                    let fg = Rgba([color[0], color[1], color[2], alpha]);
                    let bg = *img.get_pixel(x, y);
                    img.put_pixel(x, y, alpha_blend(bg, fg));
                }
            }
        }
    }
}

/// Draw a filled rectangle onto an RGBA image.
fn draw_filled_rect(img: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, color: Rgba<u8>) {
    let iw = img.width();
    let ih = img.height();

    let x_end = (x + w).min(iw);
    let y_end = (y + h).min(ih);

    for py in y..y_end {
        for px in x..x_end {
            let bg = *img.get_pixel(px, py);
            img.put_pixel(px, py, alpha_blend(bg, color));
        }
    }
}

/// Alpha-composite foreground over background (standard Porter-Duff over).
fn alpha_blend(bg: Rgba<u8>, fg: Rgba<u8>) -> Rgba<u8> {
    let fa = fg[3] as f64 / 255.0;
    let ba = bg[3] as f64 / 255.0;
    let out_a = fa + ba * (1.0 - fa);

    if out_a < 0.001 {
        return Rgba([0, 0, 0, 0]);
    }

    Rgba([
        ((fg[0] as f64 * fa + bg[0] as f64 * ba * (1.0 - fa)) / out_a) as u8,
        ((fg[1] as f64 * fa + bg[1] as f64 * ba * (1.0 - fa)) / out_a) as u8,
        ((fg[2] as f64 * fa + bg[2] as f64 * ba * (1.0 - fa)) / out_a) as u8,
        (out_a * 255.0) as u8,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_icon_has_correct_dimensions() {
        let (rgba, w, h) = status_icon(TrayIconState::Active);
        assert_eq!(w, 44);
        assert_eq!(h, 44);
        assert_eq!(rgba.len(), (44 * 44 * 4) as usize);
    }

    #[test]
    fn paused_icon_has_correct_dimensions() {
        let (rgba, w, h) = status_icon(TrayIconState::Paused);
        assert_eq!(w, 44);
        assert_eq!(h, 44);
        assert_eq!(rgba.len(), (44 * 44 * 4) as usize);
    }

    #[test]
    fn disabled_icon_has_correct_dimensions() {
        let (rgba, w, h) = status_icon(TrayIconState::Disabled);
        assert_eq!(w, 44);
        assert_eq!(h, 44);
        assert_eq!(rgba.len(), (44 * 44 * 4) as usize);
    }

    #[test]
    fn active_icon_is_unmodified_base() {
        let (active, _, _) = status_icon(TrayIconState::Active);
        let base = ::image::load_from_memory(BASE_2X).unwrap().to_rgba8();
        assert_eq!(active, base.into_raw());
    }

    #[test]
    fn paused_icon_differs_from_active() {
        let (active, _, _) = status_icon(TrayIconState::Active);
        let (paused, _, _) = status_icon(TrayIconState::Paused);
        assert_ne!(active, paused);
    }

    #[test]
    fn disabled_icon_differs_from_active() {
        let (active, _, _) = status_icon(TrayIconState::Active);
        let (disabled, _, _) = status_icon(TrayIconState::Disabled);
        assert_ne!(active, disabled);
    }

    #[test]
    fn paused_icon_has_opaque_pixels_in_pause_bar_region() {
        let (rgba, w, _) = status_icon(TrayIconState::Paused);
        let px = 36_u32;
        let py = 36_u32;
        let idx = ((py * w + px) * 4 + 3) as usize;
        assert!(rgba[idx] > 0, "pause bar region should have opaque pixels");
    }

    #[test]
    fn disabled_icon_has_opaque_pixels_in_slash_region() {
        let (rgba, w, _) = status_icon(TrayIconState::Disabled);
        let px = 22_u32;
        let py = 22_u32;
        let idx = ((py * w + px) * 4 + 3) as usize;
        assert!(rgba[idx] > 0, "slash region should have opaque pixels");
    }

    #[test]
    fn alpha_blend_fully_opaque_fg_overwrites() {
        let bg = Rgba([100, 100, 100, 255]);
        let fg = Rgba([200, 50, 50, 255]);
        let result = alpha_blend(bg, fg);
        assert_eq!(result[0], 200);
    }

    #[test]
    fn alpha_blend_transparent_fg_keeps_bg() {
        let bg = Rgba([100, 100, 100, 255]);
        let fg = Rgba([200, 50, 50, 0]);
        let result = alpha_blend(bg, fg);
        assert_eq!(result[0], 100);
    }
}
