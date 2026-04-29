//! macOS native border implementation using NSWindow + CAShapeLayer.
//!
//! Creates a dedicated NSWindow (no WebView) with a CAShapeLayer border stroke
//! and 5-band gradient glow (100px depth). Completely independent of WebKit.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSBackingStoreType, NSColor, NSScreen, NSWindow, NSWindowStyleMask};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::CGPath;
use objc2_foundation::ns_string;
use objc2_quartz_core::{CABasicAnimation, CAMediaTiming, CAShapeLayer};

use super::colors::{teal_cgcolor_dim, teal_cgcolor_full};

/// 5-band gradient: each band is 20px wide, total 100px depth from edge.
/// Values are base opacity for each band (edge → inner).
pub(super) const GLOW_OPACITIES: [f32; 5] = [0.35, 0.20, 0.10, 0.05, 0.02];

/// Main-thread-only inner state wrapping the NSWindow and its layers.
pub(super) struct BorderInner {
    pub(super) window: Retained<NSWindow>,
    pub(super) border_layer: Retained<CAShapeLayer>,
    /// 5 concentric CAShapeLayers creating a gradient glow (edge → inner).
    pub(super) glow_layers: Vec<Retained<CAShapeLayer>>,
}

/// Create border windows for all connected screens.
/// Deduplicates mirrored displays by frame coordinates.
pub(super) fn create_all_border_windows(mtm: MainThreadMarker) -> Vec<BorderInner> {
    let screens = NSScreen::screens(mtm);
    let mut borders = Vec::new();
    let mut seen_frames = std::collections::HashSet::new();

    for screen in screens.iter() {
        let frame = screen.frame();
        let key = (
            frame.origin.x as i64,
            frame.origin.y as i64,
            frame.size.width as i64,
            frame.size.height as i64,
        );
        if !seen_frames.insert(key) {
            continue; // skip mirrored display
        }
        if let Some(border) = create_border_window(mtm, frame) {
            borders.push(border);
        }
    }

    tracing::info!("Native border: created {} border window(s)", borders.len());
    borders
}

/// Compute a hash of all screen frames for topology change detection.
pub(super) fn screen_fingerprint(mtm: MainThreadMarker) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let screens = NSScreen::screens(mtm);
    let mut hasher = DefaultHasher::new();
    for screen in screens.iter() {
        let f = screen.frame();
        (f.origin.x as i64).hash(&mut hasher);
        (f.origin.y as i64).hash(&mut hasher);
        (f.size.width as i64).hash(&mut hasher);
        (f.size.height as i64).hash(&mut hasher);
    }
    screens.count().hash(&mut hasher);
    hasher.finish()
}

/// Create a single border window for the given screen frame.
fn create_border_window(mtm: MainThreadMarker, frame: CGRect) -> Option<BorderInner> {
    tracing::info!(
        "Native border frame: origin=({}, {}), size={}x{}",
        frame.origin.x,
        frame.origin.y,
        frame.size.width,
        frame.size.height
    );

    let style = NSWindowStyleMask::Borderless;
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            frame,
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };

    window.setOpaque(false);
    window.setBackgroundColor(Some(&NSColor::clearColor()));
    window.setIgnoresMouseEvents(true);
    window.setHasShadow(false);
    window.setLevel(26); // above NSStatusWindowLevel (25)

    let content_view = window.contentView()?;
    content_view.setWantsLayer(true);
    let root_layer = content_view.layer()?;

    // --- Gradient glow: 5 bands × 20px = 100px depth ---
    let mut glow_layers = Vec::with_capacity(GLOW_OPACITIES.len());
    for (i, &opacity) in GLOW_OPACITIES.iter().enumerate() {
        let glow = CAShapeLayer::new();
        glow.setFrame(frame);

        // Each band is centered on a rect inset by (10 + i*20)px.
        // lineWidth=20 → 10px outward + 10px inward from path = contiguous 20px band.
        let inset = 10.0 + (i as f64) * 20.0;
        let glow_rect = CGRect::new(
            CGPoint::new(inset, inset),
            CGSize::new(
                frame.size.width - inset * 2.0,
                frame.size.height - inset * 2.0,
            ),
        );
        let glow_path = unsafe { CGPath::with_rect(glow_rect, std::ptr::null()) };
        glow.setPath(Some(&glow_path));
        glow.setFillColor(None);
        let teal = teal_cgcolor_full();
        glow.setStrokeColor(Some(&teal));
        glow.setLineWidth(20.0);
        glow.setOpacity(opacity);

        // Opacity pulse animation (each band pulses proportionally)
        let anim = create_opacity_pulse(opacity, opacity * 0.25);
        glow.addAnimation_forKey(&anim, Some(ns_string!("glowPulse")));

        root_layer.addSublayer(&glow);
        glow_layers.push(glow);
    }

    // --- Sharp border stroke (3px, on top of glow) ---
    let border_layer = CAShapeLayer::new();
    border_layer.setFrame(frame);
    let inset_rect = CGRect::new(
        CGPoint::new(1.5, 1.5),
        CGSize::new(frame.size.width - 3.0, frame.size.height - 3.0),
    );
    let path = unsafe { CGPath::with_rect(inset_rect, std::ptr::null()) };
    border_layer.setPath(Some(&path));
    border_layer.setFillColor(None);
    let teal = teal_cgcolor_full();
    border_layer.setStrokeColor(Some(&teal));
    border_layer.setLineWidth(3.0);

    let anim = create_stroke_pulse_animation();
    border_layer.addAnimation_forKey(&anim, Some(ns_string!("borderPulse")));

    root_layer.addSublayer(&border_layer);

    Some(BorderInner {
        window,
        border_layer,
        glow_layers,
    })
}

/// Create opacity pulse animation for a glow band.
pub(super) fn create_opacity_pulse(from: f32, to: f32) -> Retained<CABasicAnimation> {
    use objc2_foundation::NSNumber;

    let anim = CABasicAnimation::animationWithKeyPath(Some(ns_string!("opacity")));
    let from_val = NSNumber::new_f32(from);
    let to_val = NSNumber::new_f32(to);
    unsafe {
        anim.setFromValue(Some(&*from_val));
        anim.setToValue(Some(&*to_val));
    }
    anim.setDuration(2.0);
    anim.setRepeatCount(f32::INFINITY);
    anim.setAutoreverses(true);
    anim
}

/// Create stroke color pulse animation (teal full ↔ teal dim).
pub(super) fn create_stroke_pulse_animation() -> Retained<CABasicAnimation> {
    let anim = CABasicAnimation::animationWithKeyPath(Some(ns_string!("strokeColor")));

    let from_color = teal_cgcolor_full();
    let to_color = teal_cgcolor_dim();
    unsafe {
        let from_ptr = (&*from_color as *const objc2_core_graphics::CGColor).cast::<AnyObject>();
        let to_ptr = (&*to_color as *const objc2_core_graphics::CGColor).cast::<AnyObject>();
        anim.setFromValue(Some(&*from_ptr));
        anim.setToValue(Some(&*to_ptr));
    }

    anim.setDuration(2.0);
    anim.setRepeatCount(f32::INFINITY);
    anim.setAutoreverses(true);
    anim
}
