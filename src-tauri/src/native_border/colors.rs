//! CGColor helpers for the native border indicator.

use objc2_core_foundation::CFRetained;
use objc2_core_graphics::{CGColor, CGColorSpace};

/// Brand teal at full opacity — rgba(20, 184, 166, 1.0).
pub(super) fn teal_cgcolor_full() -> CFRetained<CGColor> {
    create_srgb_color(20.0 / 255.0, 184.0 / 255.0, 166.0 / 255.0, 1.0)
}

/// Brand teal at reduced opacity — rgba(20, 184, 166, 0.3).
pub(super) fn teal_cgcolor_dim() -> CFRetained<CGColor> {
    create_srgb_color(20.0 / 255.0, 184.0 / 255.0, 166.0 / 255.0, 0.3)
}

/// Gray indicator for paused state — rgba(156, 163, 175, 0.6).
pub(super) fn gray_cgcolor() -> CFRetained<CGColor> {
    create_srgb_color(156.0 / 255.0, 163.0 / 255.0, 175.0 / 255.0, 0.6)
}

fn create_srgb_color(r: f64, g: f64, b: f64, a: f64) -> CFRetained<CGColor> {
    let color_space = CGColorSpace::new_device_rgb();
    let components = [r, g, b, a];
    // Safe: sRGB color space + 4 valid float components [0.0, 1.0] cannot fail.
    unsafe { CGColor::new(color_space.as_deref(), components.as_ptr()) }.expect("CGColor creation")
}
