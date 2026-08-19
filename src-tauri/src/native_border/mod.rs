//! Native screen recording border indicator.
//!
//! Platform: macOS only (Phase 1). Other platforms gracefully degrade (no border).
//! Supports multiple monitors — one NSWindow per screen.

#[cfg(target_os = "macos")]
mod colors;
#[cfg(target_os = "macos")]
mod macos;

use std::sync::atomic::{AtomicBool, Ordering};

/// Native screen recording border indicator.
///
/// Creates a dedicated NSWindow with CAShapeLayer border per connected screen.
/// Supports multi-monitor, periodic screen topology change detection, and
/// graceful rebuild when displays are connected/disconnected.
///
/// Thread safety: `MainThreadBound<RefCell<Vec<BorderInner>>>` wraps the NSWindows.
/// AtomicBool tracks visible/paused state from any thread.
/// All native mutations dispatch to the main thread via `get_on_main`.
pub struct NativeBorderIndicator {
    #[cfg(target_os = "macos")]
    inner: dispatch2::MainThreadBound<std::cell::RefCell<Vec<macos::BorderInner>>>,
    #[cfg(target_os = "macos")]
    fingerprint: std::sync::atomic::AtomicU64,
    visible: AtomicBool,
    paused: AtomicBool,
}

impl NativeBorderIndicator {
    /// Create border indicators for all connected screens.
    /// Returns `None` if no screens are available.
    #[cfg(target_os = "macos")]
    pub fn new(mtm: objc2::MainThreadMarker) -> Option<Self> {
        let borders = macos::create_all_border_windows(mtm);
        if borders.is_empty() {
            return None;
        }
        let fp = macos::screen_fingerprint(mtm);
        Some(Self {
            inner: dispatch2::MainThreadBound::new(std::cell::RefCell::new(borders), mtm),
            fingerprint: std::sync::atomic::AtomicU64::new(fp),
            visible: AtomicBool::new(false),
            paused: AtomicBool::new(false),
        })
    }

    // #7719: no call site constructs the no-arg stub today (the real
    // constructor call in setup_platform.rs is the macOS `new(mtm)` overload,
    // itself `#[cfg(target_os = "macos")]`-gated) — kept as the symmetric
    // graceful-degradation constructor for non-macOS platforms (struct doc:
    // "Other platforms gracefully degrade (no border)"), for whenever a
    // caller needs a platform-agnostic construction path without per-site
    // `cfg` branching.
    #[cfg(not(target_os = "macos"))]
    pub fn new() -> Option<Self> {
        None
    }

    /// Show border on all screens. No-op if already visible.
    // #7719: called from tray.rs / commands/capture_status/mod.rs, both
    // `#[cfg(target_os = "macos")]`-gated call sites — dead on other
    // platforms by design (macOS-only border indicator, see module doc).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn show(&self) {
        if !self.visible.swap(true, Ordering::Relaxed) {
            #[cfg(target_os = "macos")]
            self.inner.get_on_main(|cell| {
                for border in cell.borrow().iter() {
                    border.window.orderFront(None);
                }
            });
        }
    }

    /// Hide border on all screens. No-op if already hidden.
    // #7719: called from tray.rs / commands/capture_status/mod.rs, both
    // `#[cfg(target_os = "macos")]`-gated call sites — dead on other
    // platforms by design (macOS-only border indicator, see module doc).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn hide(&self) {
        if self.visible.swap(false, Ordering::Relaxed) {
            #[cfg(target_os = "macos")]
            self.inner.get_on_main(|cell| {
                for border in cell.borrow().iter() {
                    border.window.orderOut(None);
                }
            });
        }
    }

    /// Update paused state on all screens.
    // #7719: called from tray.rs / setup_shortcuts.rs / commands/
    // capture_status/mod.rs, all `#[cfg(target_os = "macos")]`-gated call
    // sites — dead on other platforms by design (macOS-only border
    // indicator, see module doc). (Previously mis-cited a nonexistent
    // "scheduler border loop" — no such loop exists in scheduler/loops/.)
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
        #[cfg(target_os = "macos")]
        self.inner.get_on_main(|cell| {
            use objc2_foundation::ns_string;
            for border in cell.borrow().iter() {
                if paused {
                    border
                        .border_layer
                        .removeAnimationForKey(ns_string!("borderPulse"));
                    let gray = colors::gray_cgcolor();
                    border.border_layer.setStrokeColor(Some(&gray));
                    for glow in &border.glow_layers {
                        glow.removeAnimationForKey(ns_string!("glowPulse"));
                        let gray = colors::gray_cgcolor();
                        glow.setStrokeColor(Some(&gray));
                    }
                } else {
                    let anim = macos::create_stroke_pulse_animation();
                    border
                        .border_layer
                        .addAnimation_forKey(&anim, Some(ns_string!("borderPulse")));
                    let teal = colors::teal_cgcolor_full();
                    border.border_layer.setStrokeColor(Some(&teal));
                    for (i, glow) in border.glow_layers.iter().enumerate() {
                        let teal = colors::teal_cgcolor_full();
                        glow.setStrokeColor(Some(&teal));
                        let base = macos::GLOW_OPACITIES[i];
                        glow.setOpacity(base);
                        let anim = macos::create_opacity_pulse(base, base * 0.25);
                        glow.addAnimation_forKey(&anim, Some(ns_string!("glowPulse")));
                    }
                }
            }
        });
    }

    /// Check if screen topology changed and rebuild border windows if needed.
    /// Called periodically from a Tokio task (every 5 seconds).
    #[cfg(target_os = "macos")]
    pub fn check_and_rebuild(&self) {
        self.inner.get_on_main(|cell| {
            let Some(mtm) = objc2::MainThreadMarker::new() else {
                panic!("get_on_main guarantees main thread");
            };
            let new_fp = macos::screen_fingerprint(mtm);
            let old_fp = self.fingerprint.load(Ordering::Relaxed);
            if new_fp == old_fp {
                return;
            }

            tracing::info!("Screen topology changed, rebuilding border windows");
            self.fingerprint.store(new_fp, Ordering::Relaxed);

            let is_visible = self.visible.load(Ordering::Relaxed);
            let is_paused = self.paused.load(Ordering::Relaxed);

            // Tear down old windows
            {
                let borders = cell.borrow();
                for border in borders.iter() {
                    border.window.orderOut(None);
                }
            }

            // Create new windows for current screens
            let new_borders = macos::create_all_border_windows(mtm);

            // Restore state
            for border in &new_borders {
                if is_visible {
                    border.window.orderFront(None);
                }
                if is_paused {
                    use objc2_foundation::ns_string;
                    border
                        .border_layer
                        .removeAnimationForKey(ns_string!("borderPulse"));
                    let gray = colors::gray_cgcolor();
                    border.border_layer.setStrokeColor(Some(&gray));
                    for glow in &border.glow_layers {
                        glow.removeAnimationForKey(ns_string!("glowPulse"));
                        let gray = colors::gray_cgcolor();
                        glow.setStrokeColor(Some(&gray));
                    }
                }
            }

            // Replace vec contents
            *cell.borrow_mut() = new_borders;
        });
    }
}

#[cfg(target_os = "macos")]
impl Drop for NativeBorderIndicator {
    fn drop(&mut self) {
        self.inner.get_on_main(|cell| {
            for border in cell.borrow().iter() {
                border.window.orderOut(None);
            }
        });
    }
}

/// Tauri managed state wrapper. Uses `Arc` for sharing with the screen monitor task.
// #7719: `app.manage(NativeBorderState(..))` (setup_platform.rs) and every
// `try_state::<NativeBorderState>()` consumer (tray.rs, setup_shortcuts.rs,
// commands/capture_status/mod.rs) are all `#[cfg(target_os = "macos")]`-gated
// — dead on other platforms by design (macOS-only border indicator, see
// module doc). (Previously mis-cited "accessed via AppState in scheduler
// loops" — no scheduler loop touches this type; it is only reached through
// Tauri's `State<NativeBorderState>` extractor at the call sites above.)
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct NativeBorderState(pub std::sync::Arc<NativeBorderIndicator>);
