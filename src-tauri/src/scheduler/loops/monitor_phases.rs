use chrono::Utc;
use maekon_core::config::{AppConfig, PrivacyConfig};
use maekon_core::models::context::{UserContext, WindowBounds};
use maekon_core::models::focused_element::{AccessibilityElement, FocusedElementInfo};
use maekon_core::models::frame::OcrRegion;
use maekon_core::models::gui_activity::GuiActivitySummary;
use maekon_core::ports::monitor::ActivityMonitor;
use maekon_core::ports::vision::{CapturePauseState, FrameProcessor};
use maekon_vision::ring_buffer::{CaptureRingBuffer, RingFrame};
use std::time::{Duration, Instant};
// Normal ZIP/HMAC dependencies enable digest's public zeroize re-export.
use sha2::digest::zeroize::Zeroize;

use crate::scheduler::gui_pipeline::GuiPipelineState;

pub(super) fn clear_capture_pixels(
    frame: &mut Option<(Vec<u8>, u32, u32)>,
    regions: &mut Vec<OcrRegion>,
) {
    if let Some((mut pixels, _, _)) = frame.take() {
        pixels.zeroize();
    }
    for region in regions.iter_mut() {
        region.text.zeroize();
    }
    regions.clear();
}

/// Bind retained monitor content to the shared pause generation, including ABA.
/// This is a cache veto, not capture permission or cancellation of in-flight work.
#[derive(Default)]
pub(super) struct CaptureCacheEpoch(Option<u64>);

#[derive(Clone, Copy)]
pub(super) struct CaptureCacheDecision {
    preserve: bool,
    proceed: bool,
    pub(super) pause_epoch: Option<u64>,
}

impl CaptureCacheEpoch {
    pub(super) fn observe(
        &mut self,
        allowed: bool,
        pause: &CapturePauseState,
    ) -> CaptureCacheDecision {
        let current = pause.snapshot().ok().filter(|_| allowed);
        let preserve = current.is_some() && self.0 == current;
        self.0 = current;
        CaptureCacheDecision {
            preserve,
            proceed: current.is_some(),
            pause_epoch: current,
        }
    }
}

/// Retire closed or superseded cache before any next-tick consumer runs.
/// A fresh open epoch may proceed after cleanup; an unavailable epoch may not.
pub(super) fn discard_closed_capture_cache(
    decision: CaptureCacheDecision,
    ring: &CaptureRingBuffer,
    frame: &mut Option<(Vec<u8>, u32, u32)>,
    regions: &mut Vec<OcrRegion>,
    focused: &mut Option<FocusedElementInfo>,
    summary: &mut Option<GuiActivitySummary>,
    gui: Option<&mut GuiPipelineState>,
) -> bool {
    if decision.preserve {
        return false;
    }
    ring.clear();
    clear_capture_pixels(frame, regions);
    if let Some(mut element) = focused.take() {
        if let Some(label) = element.label.as_mut() {
            label.zeroize();
        }
        if let Some(text) = element.extracted_text.as_mut() {
            text.zeroize();
        }
    }
    *summary = None;
    if let Some(state) = gui {
        // Discard the accumulator result instead of publishing it after resume.
        let _ = state.aggregator.flush();
        state.uncertain_queue.clear();
        state.pending_summaries.clear();
        state.app_type_cache.clear();
        state.feedback_tick_counter = 0;
    }
    !decision.proceed
}

/// Capture-time exclusion decision for one monitor tick (#7909, T1.1).
///
/// The UI promises excluded/sensitive apps are excluded *from capture*, not
/// merely from egress, so the monitor loop gates every content-capture surface
/// of a tick (AX text extraction, ring thumbnail, trigger capture, post-event
/// forced capture, detection re-analysis) on this flag. Metadata context/window
/// events still flow (regime tracking depends on them) — they are PII-sanitized
/// at source and the egress boundary applies the same policy before upload.
///
/// A missing config snapshot falls back to `PrivacyConfig::default()`, whose
/// `auto_exclude_sensitive = true` keeps the sensitive-app auto-detection
/// fail-safe rather than fail-open.
pub(super) fn tick_capture_excluded(
    config: Option<&AppConfig>,
    app_name: &str,
    window_title: &str,
) -> bool {
    match config {
        Some(cfg) => {
            maekon_vision::privacy::should_exclude_by_policy(&cfg.privacy, app_name, window_title)
        }
        None => maekon_vision::privacy::should_exclude_by_policy(
            &PrivacyConfig::default(),
            app_name,
            window_title,
        ),
    }
}

/// Re-read the frontmost app immediately before a content grab and report
/// whether it changed since `snapshot_app` (the app resolved at tick start).
///
/// Closes the capture window-switch TOCTOU (#8054 P3-1): judgment + metadata
/// are taken from the tick-start snapshot, but the actual grab happens later in
/// the tick. If the user switched windows in between, the grab would pair stale
/// metadata with fresh pixels — and could capture an app that became frontmost
/// after the tick-start exclusion check. Missing or failed observations veto
/// the grab; unavailable foreground information is not evidence of stability.
pub(super) async fn frontmost_app_switched_since(
    activity_monitor: &dyn ActivityMonitor,
    snapshot_app: &str,
) -> bool {
    match activity_monitor.collect_active_context().await {
        Ok(ctx) => ctx
            .active_window
            .map(|window| window.app_name != snapshot_app)
            .unwrap_or(true),
        Err(_) => true,
    }
}

/// Whether any currently-visible window belongs to an excluded/sensitive app
/// (#8054 P2-4 partial-occlusion guard).
///
/// `visible_apps` is the owner-app names of on-screen windows (from
/// [`maekon_monitor::visible_window_app_names`]). The active app is already
/// gated by [`tick_capture_excluded`]; this catches a *background* excluded app
/// sharing the same display, which a full-monitor grab would otherwise record.
/// Title-based matching is unavailable for background windows (titles need
/// screen-recording permission), so this is app-name / sensitive-app based. A
/// missing config falls back to the fail-safe [`PrivacyConfig::default`].
pub(super) fn any_excluded_app_visible(
    config: Option<&AppConfig>,
    visible_apps: &[String],
) -> bool {
    let default_privacy;
    let privacy = match config {
        Some(cfg) => &cfg.privacy,
        None => {
            default_privacy = PrivacyConfig::default();
            &default_privacy
        }
    };
    visible_apps
        .iter()
        .any(|app| maekon_vision::privacy::should_exclude_by_policy(privacy, app, ""))
}

#[derive(Debug, Clone, Default)]
pub(super) struct ActiveWindowSnapshot {
    pub(super) app_name: String,
    pub(super) window_title: String,
    pub(super) window_bounds: Option<WindowBounds>,
    pub(super) app_bundle_id: Option<String>,
}

impl ActiveWindowSnapshot {
    pub(super) fn from_context(ctx: &UserContext) -> Self {
        let Some(window) = ctx.active_window.as_ref() else {
            return Self::default();
        };

        Self {
            app_name: window.app_name.clone(),
            window_title: window.title.clone(),
            window_bounds: window.bounds,
            app_bundle_id: window.app_bundle_id.clone(),
        }
    }
}

#[derive(Default)]
pub(super) struct RingThumbnailCadence {
    last_capture_at: Option<Instant>,
}

impl RingThumbnailCadence {
    pub(super) fn should_capture(&mut self, now: Instant, throttle: Duration) -> bool {
        let throttle = throttle.max(Duration::from_millis(100));
        let should_capture = self
            .last_capture_at
            .map(|last| now.duration_since(last) >= throttle)
            .unwrap_or(true);

        if should_capture {
            self.last_capture_at = Some(now);
        }

        should_capture
    }
}

/// Late observation shared by ring and main/post acquisition routes.
/// App names and the existing visible-owner adapter are not native scene permits.
pub(super) struct CaptureGrabGuard<'a> {
    pub(super) activity_monitor: &'a dyn ActivityMonitor,
    pub(super) snapshot_app: &'a str,
    pub(super) config: Option<&'a AppConfig>,
    pub(super) visible_apps: &'a (dyn Fn() -> Vec<String> + Sync),
}

impl CaptureGrabGuard<'_> {
    pub(super) async fn denied(&self) -> bool {
        frontmost_app_switched_since(self.activity_monitor, self.snapshot_app).await
            || any_excluded_app_visible(self.config, &(self.visible_apps)())
    }
}

pub(super) struct RingThumbnailContext<'a> {
    pub(super) guard: CaptureGrabGuard<'a>,
    pub(super) pause: &'a CapturePauseState,
    pub(super) accepted_epoch: Option<u64>,
    pub(super) window_title: &'a str,
    pub(super) focused_element: Option<&'a FocusedElementInfo>,
    pub(super) window_bounds: Option<&'a WindowBounds>,
}

impl RingThumbnailContext<'_> {
    fn pause_denied(&self) -> bool {
        self.accepted_epoch
            .is_none_or(|epoch| self.pause.check(epoch).is_err())
    }

    /// Recheck the tick's epoch after the asynchronous app observation.
    pub(super) async fn denied(&self, ring: &CaptureRingBuffer) -> bool {
        let denied = self.pause_denied() || self.guard.denied().await || self.pause_denied();
        if denied {
            ring.clear();
        }
        denied
    }
}

pub(super) async fn capture_ring_thumbnail_if_due(
    cadence: &mut RingThumbnailCadence,
    processor: &dyn FrameProcessor,
    ring_buffer: &CaptureRingBuffer,
    context: &RingThumbnailContext<'_>,
    throttle: Duration,
) {
    if context.pause_denied() {
        ring_buffer.clear();
        return;
    }
    if !cadence.should_capture(Instant::now(), throttle) {
        return;
    }
    if context.denied(ring_buffer).await {
        return;
    }

    // The processor may yield while acquiring pixels. Observe again before
    // retaining them, and retire prior ring content on a late policy denial.
    let thumbnail = processor.capture_thumbnail(context.window_bounds).await;
    if context.pause_denied() {
        if let Ok(mut pixels) = thumbnail {
            pixels.zeroize();
        }
        ring_buffer.clear();
        return;
    }
    if let Ok(mut thumb_data) = thumbnail {
        if context.denied(ring_buffer).await {
            thumb_data.zeroize();
            return;
        }
        ring_buffer.push(RingFrame {
            timestamp: Utc::now(),
            thumbnail_data: thumb_data,
            app_name: context.guard.snapshot_app.to_string(),
            window_title: context.window_title.to_string(),
            accessibility_elements: context
                .focused_element
                .map(|focused| {
                    vec![AccessibilityElement {
                        role: focused.role.clone(),
                        label: focused.label.clone().unwrap_or_default(),
                        bounds: focused.position,
                    }]
                })
                .unwrap_or_default(),
        });
    }
}

#[cfg(test)]
mod tests;
