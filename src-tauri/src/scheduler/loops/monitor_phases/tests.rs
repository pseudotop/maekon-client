use super::*;
use async_trait::async_trait;
use maekon_core::error::CoreError;
use maekon_core::models::context::WindowInfo;

use maekon_analysis::gui_aggregator::GuiActivityAggregator;
use maekon_core::config::{GuiIntelligenceConfig, PiiFilterLevel};
use maekon_core::models::frame::BoundingBox;
use maekon_core::models::gui_interaction::{
    GuiElement, GuiElementType, GuiInteractionEvent, GuiInteractionType,
};
use maekon_vision::contour_classifier::feedback::{FeatureSummary, UncertainElement};
use maekon_vision::gui_detector::GuiElementDetector;
use std::collections::{HashMap, VecDeque};

fn marker_event(marker: &str) -> GuiInteractionEvent {
    GuiInteractionEvent {
        timestamp: Utc::now(),
        element: GuiElement {
            text: marker.into(),
            bbox: BoundingBox {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
            element_type: GuiElementType::Button,
            confidence: 1.0,
            type_confidence: 1.0,
        },
        interaction_type: GuiInteractionType::Click,
        app_name: "Synthetic".into(),
        window_title: Some(marker.into()),
        screen_position: None,
        interaction: None,
    }
}

fn marker_frame(marker: &str, pixel: u8) -> RingFrame {
    RingFrame {
        timestamp: Utc::now(),
        thumbnail_data: vec![pixel, 17, 73, 255],
        app_name: "Synthetic".into(),
        window_title: marker.into(),
        accessibility_elements: Vec::new(),
    }
}

struct CaptureCacheFixture {
    ring: CaptureRingBuffer,
    frame: Option<(Vec<u8>, u32, u32)>,
    regions: Vec<OcrRegion>,
    focused: Option<FocusedElementInfo>,
    summary: Option<GuiActivitySummary>,
    gui: GuiPipelineState,
}

impl CaptureCacheFixture {
    fn new(marker: &str, pixel: u8) -> Self {
        let mut aggregator = GuiActivityAggregator::new(&GuiIntelligenceConfig::default());
        assert!(aggregator.push(marker_event(marker), marker).is_none());
        let summary = aggregator.flush().unwrap();
        assert!(aggregator.push(marker_event(marker), marker).is_none());
        let ring = CaptureRingBuffer::new(3, 2, 0.5);
        ring.push(marker_frame(marker, pixel));
        Self {
            ring,
            frame: Some((vec![pixel, 17, 73, 255], 1, 1)),
            regions: vec![OcrRegion {
                text: marker.into(),
                bbox: BoundingBox {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                },
                confidence: 1.0,
            }],
            focused: Some(FocusedElementInfo {
                role: "TextField".into(),
                label: Some(format!("{marker}-label")),
                extracted_text: Some(marker.into()),
                ..Default::default()
            }),
            summary: Some(summary.clone()),
            gui: GuiPipelineState {
                detector: GuiElementDetector::new((1, 1), PiiFilterLevel::Off),
                aggregator,
                uncertain_queue: VecDeque::from([UncertainElement {
                    app_name: "Synthetic".into(),
                    text: marker.into(),
                    current_type: "Unknown".into(),
                    confidence: 0.2,
                    features: FeatureSummary::from_aspect_ratio(1.0),
                }]),
                feedback_tick_counter: 29,
                app_type_cache: HashMap::from([(
                    marker.into(),
                    vec![(GuiElementType::Unknown, GuiElementType::Button)],
                )]),
                pending_summaries: VecDeque::from([summary]),
            },
        }
    }

    fn transition(&mut self, allowed: bool) -> bool {
        discard_closed_capture_cache(
            CaptureCacheDecision {
                preserve: allowed,
                proceed: allowed,
                pause_epoch: allowed.then_some(0),
            },
            &self.ring,
            &mut self.frame,
            &mut self.regions,
            &mut self.focused,
            &mut self.summary,
            Some(&mut self.gui),
        )
    }
}

fn epoch_tick(
    cache: &mut CaptureCacheFixture,
    epoch: &mut CaptureCacheEpoch,
    pause: &CapturePauseState,
    allowed: bool,
) -> bool {
    discard_closed_capture_cache(
        epoch.observe(allowed, pause),
        &cache.ring,
        &mut cache.frame,
        &mut cache.regions,
        &mut cache.focused,
        &mut cache.summary,
        Some(&mut cache.gui),
    )
}

#[test]
fn pause_aba_retires_old_marker_before_next_tick_flush() {
    use crate::commands::capture_status::{
        apply_capture_pause_request, tests::PauseFixture, CapturePauseRequest,
    };
    use std::sync::Arc;
    use tauri_plugin_global_shortcut::ShortcutState;

    for route in [
        CapturePauseRequest::Ipc,
        CapturePauseRequest::Shortcut(ShortcutState::Pressed),
        CapturePauseRequest::Tray,
    ] {
        let fixture = PauseFixture::new(true);
        let scheduler = fixture.scheduler();
        assert!(Arc::ptr_eq(
            &fixture.state.capture_pause_state,
            &scheduler.capture_pause_state
        ));
        assert!(Arc::ptr_eq(
            &fixture.state.capture_paused,
            &scheduler.capture_paused
        ));
        let pause = &scheduler.capture_pause_state;
        let mut epoch = CaptureCacheEpoch::default();
        assert!(epoch.observe(true, pause).proceed);
        let mut cache = CaptureCacheFixture::new("before-pause-marker", 231);
        assert!(!epoch_tick(&mut cache, &mut epoch, pause, true));
        assert_eq!(cache.frame.as_ref().unwrap().0, [231, 17, 73, 255]);
        assert_eq!(
            apply_capture_pause_request(&fixture.state, route).unwrap(),
            Some(true)
        );
        assert_eq!(
            apply_capture_pause_request(&fixture.state, route).unwrap(),
            Some(false)
        );
        assert!(!epoch_tick(&mut cache, &mut epoch, pause, true));
        assert!(cache.frame.is_none(), "pause ABA retained old pixels");
        assert!(cache.regions.is_empty());
        assert!(cache.focused.is_none());
        assert!(cache.summary.is_none());
        assert!(cache.gui.aggregator.flush().is_none());
        assert!(cache.gui.pending_summaries.is_empty());
        assert!(cache.gui.uncertain_queue.is_empty());
        assert!(cache.gui.app_type_cache.is_empty());
        assert_eq!(cache.gui.feedback_tick_counter, 0);
        let flushed = cache
            .ring
            .check_and_flush(1.0, marker_frame("after-pause", 77))
            .unwrap();
        assert!(
            flushed.pre_event_frames.is_empty(),
            "pause ABA flushed old pixels"
        );

        // Recovery in the same open epoch must not erase fresh content.
        let mut fresh = CaptureCacheFixture::new("fresh-after-resume", 77);
        assert!(!epoch_tick(&mut fresh, &mut epoch, pause, true));
        let flushed = fresh
            .ring
            .check_and_flush(1.0, marker_frame("trigger", 12))
            .unwrap();
        assert_eq!(flushed.pre_event_frames.len(), 1);
        assert_eq!(
            flushed.pre_event_frames[0].window_title,
            "fresh-after-resume"
        );
        assert_eq!(
            flushed.pre_event_frames[0].thumbnail_data,
            [77, 17, 73, 255]
        );
        assert_eq!(fresh.frame.as_ref().unwrap().0, [77, 17, 73, 255]);
    }
}

#[test]
fn unavailable_pause_epoch_clears_cache_and_vetoes_tick_even_with_open_snapshot() {
    use crate::commands::capture_status::{
        apply_capture_pause_request, tests::PauseFixture, CapturePauseRequest,
    };
    let fixture = PauseFixture::new(true);
    let pause = &fixture.state.capture_pause_state;
    let mut epoch = CaptureCacheEpoch::default();
    assert!(epoch.observe(true, pause).proceed);
    let mut cache = CaptureCacheFixture::new("revoked-marker", 91);
    assert_eq!(
        apply_capture_pause_request(&fixture.state, CapturePauseRequest::Ipc).unwrap(),
        Some(true)
    );
    assert!(matches!(
        pause.snapshot().unwrap_err(),
        CoreError::PermissionDenied {
            code: maekon_core::error_codes::PermissionCode::PrivacyDenied,
            message,
        } if message == "capture denied: capture paused"
    ));
    for _ in 0..2 {
        assert!(epoch_tick(&mut cache, &mut epoch, pause, true));
        assert!(cache.ring.is_empty());
        assert!(cache.frame.is_none());
        assert!(cache.focused.is_none());
    }
    assert_eq!(
        apply_capture_pause_request(&fixture.state, CapturePauseRequest::Ipc).unwrap(),
        Some(false)
    );
    assert!(!epoch_tick(&mut cache, &mut epoch, pause, true));
    let mut closed = CaptureCacheFixture::new("policy-closed-marker", 92);
    assert!(epoch_tick(&mut closed, &mut epoch, pause, false));
    assert!(closed.ring.is_empty());
    assert!(closed.regions.is_empty());
    assert!(!epoch_tick(&mut closed, &mut epoch, pause, true));
}

#[test]
fn ignored_pause_routes_and_independent_flags_preserve_current_cache_epoch() {
    use crate::commands::capture_status::{
        apply_capture_pause_request, tests::PauseFixture, CapturePauseRequest,
    };
    use std::sync::atomic::Ordering;
    use tauri_plugin_global_shortcut::ShortcutState;
    let fixture = PauseFixture::new(false);
    let scheduler = fixture.scheduler();
    let pause = &scheduler.capture_pause_state;
    let mut epoch = CaptureCacheEpoch::default();
    assert!(epoch.observe(true, pause).proceed);
    let mut cache = CaptureCacheFixture::new("untouched-marker", 117);
    assert_eq!(
        apply_capture_pause_request(&fixture.state, CapturePauseRequest::Tray).unwrap(),
        None
    );
    assert_eq!(
        apply_capture_pause_request(
            &fixture.state,
            CapturePauseRequest::Shortcut(ShortcutState::Released)
        )
        .unwrap(),
        None
    );
    fixture
        .state
        .indicator_visible
        .fetch_xor(true, Ordering::Relaxed);
    scheduler.detection_active.store(true, Ordering::Relaxed);
    assert!(!epoch_tick(&mut cache, &mut epoch, pause, true));
    let flushed = cache
        .ring
        .check_and_flush(1.0, marker_frame("trigger", 12))
        .unwrap();
    assert_eq!(flushed.pre_event_frames[0].window_title, "untouched-marker");
    assert_eq!(cache.frame.as_ref().unwrap().0, [117, 17, 73, 255]);
}

#[test]
fn closed_capture_gate_retires_pixels_text_and_pending_gui_content() {
    let mut cache = CaptureCacheFixture::new("retired-private-marker", 231);
    let closed = cache.transition(false);
    assert!(
        cache.frame.is_none(),
        "closed gate retained captured pixels"
    );
    assert!(cache.regions.is_empty(), "closed gate retained OCR text");
    assert!(
        cache.focused.is_none(),
        "closed gate retained focused label/text"
    );
    assert!(cache.summary.is_none());
    assert_eq!(cache.ring.len(), 0);
    assert!(cache.gui.aggregator.flush().is_none());
    assert!(cache.gui.uncertain_queue.is_empty());
    assert!(cache.gui.pending_summaries.is_empty());
    assert!(cache.gui.app_type_cache.is_empty());
    assert_eq!(cache.gui.feedback_tick_counter, 0);
    assert!(closed);
}

#[test]
fn open_capture_gate_preserves_actual_content_and_pending_feedback() {
    let mut cache = CaptureCacheFixture::new("allowed-marker", 203);
    let expected_summary = cache.summary.clone();
    assert!(!cache.transition(true));
    assert_eq!(
        cache.frame.as_ref().unwrap(),
        &(vec![203, 17, 73, 255], 1, 1)
    );
    assert_eq!(cache.regions.len(), 1);
    assert_eq!(cache.regions[0].text, "allowed-marker");
    let focused = cache.focused.as_ref().unwrap();
    assert_eq!(focused.label.as_deref(), Some("allowed-marker-label"));
    assert_eq!(focused.extracted_text.as_deref(), Some("allowed-marker"));
    assert_eq!(cache.summary, expected_summary);
    assert_eq!(cache.ring.len(), 1);
    assert_eq!(
        cache.gui.aggregator.flush().unwrap().content_label,
        "allowed-marker"
    );
    assert_eq!(cache.gui.uncertain_queue.len(), 1);
    assert_eq!(cache.gui.uncertain_queue[0].text, "allowed-marker");
    assert_eq!(cache.gui.pending_summaries.len(), 1);
    assert_eq!(cache.gui.pending_summaries[0], expected_summary.unwrap());
    assert_eq!(cache.gui.app_type_cache.len(), 1);
    assert_eq!(
        cache.gui.app_type_cache["allowed-marker"],
        [(GuiElementType::Unknown, GuiElementType::Button)]
    );
    assert_eq!(cache.gui.feedback_tick_counter, 29);
}

#[test]
fn closed_capture_gate_is_idempotent_without_optional_gui() {
    let mut cache = CaptureCacheFixture::new("optional-marker", 199);
    for _ in 0..2 {
        assert!(discard_closed_capture_cache(
            CaptureCacheDecision {
                preserve: false,
                proceed: false,
                pause_epoch: None,
            },
            &cache.ring,
            &mut cache.frame,
            &mut cache.regions,
            &mut cache.focused,
            &mut cache.summary,
            None,
        ));
        assert!(cache.frame.is_none());
        assert!(cache.regions.is_empty());
        assert!(cache.focused.is_none());
        assert!(cache.summary.is_none());
        assert_eq!(cache.ring.len(), 0);
    }
}

#[test]
fn reopened_capture_gate_can_only_flush_new_marker_content() {
    let mut cache = CaptureCacheFixture::new("retired-marker", 231);
    assert!(cache.transition(false));
    cache.ring.push(marker_frame("fresh-marker", 77));
    cache.frame = Some((vec![77, 17, 73, 255], 1, 1));
    cache.focused = Some(FocusedElementInfo {
        label: Some("fresh-label".into()),
        extracted_text: Some("fresh-text".into()),
        ..Default::default()
    });
    assert!(cache
        .gui
        .aggregator
        .push(marker_event("fresh-marker"), "fresh-marker")
        .is_none());
    assert!(!cache.transition(true));
    let flushed = cache
        .ring
        .check_and_flush(1.0, marker_frame("trigger", 12))
        .unwrap();
    assert_eq!(flushed.pre_event_frames.len(), 1);
    assert_eq!(
        flushed.pre_event_frames[0].thumbnail_data,
        [77, 17, 73, 255]
    );
    assert_eq!(flushed.pre_event_frames[0].window_title, "fresh-marker");
    let summary = cache.gui.aggregator.flush().unwrap();
    assert_eq!(summary.content_label, "fresh-marker");
    assert_eq!(summary.window_title, "fresh-marker");
    assert_eq!(summary.button_clicks, 1);
    assert_eq!(cache.frame.as_ref().unwrap().0, [77, 17, 73, 255]);
    assert_eq!(
        cache.focused.as_ref().unwrap().label.as_deref(),
        Some("fresh-label")
    );
    assert_eq!(
        cache.focused.as_ref().unwrap().extracted_text.as_deref(),
        Some("fresh-text")
    );
    assert!(cache.regions.is_empty());
    assert!(cache.summary.is_none());
    assert!(cache.gui.uncertain_queue.is_empty());
    assert!(cache.gui.pending_summaries.is_empty());
}

#[test]
fn idle_pixel_cleanup_also_clears_ocr_when_no_frame_remains() {
    let mut cache = CaptureCacheFixture::new("idle-marker", 91);
    clear_capture_pixels(&mut cache.frame, &mut cache.regions);
    assert!(cache.frame.is_none());
    assert!(cache.regions.is_empty());
    cache.regions = CaptureCacheFixture::new("remaining-ocr", 92).regions;
    clear_capture_pixels(&mut cache.frame, &mut cache.regions);
    assert!(cache.frame.is_none());
    assert!(cache.regions.is_empty());
}

/// Minimal `ActivityMonitor` double returning a fixed active-window app
/// name (or an error) so the grab-time TOCTOU re-check is testable without
/// a live window server.
struct StubActivityMonitor {
    active_app: Option<String>,
    fail: bool,
}

#[async_trait]
impl ActivityMonitor for StubActivityMonitor {
    async fn collect_context(&self) -> Result<UserContext, CoreError> {
        if self.fail {
            return Err(CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: "stub failure".to_string(),
            });
        }
        let active_window = self.active_app.as_ref().map(|app| WindowInfo {
            title: "Doc".to_string(),
            app_name: app.clone(),
            app_bundle_id: None,
            pid: 1,
            bounds: None,
        });
        Ok(UserContext {
            timestamp: Utc::now(),
            active_window,
            processes: vec![],
        })
    }
}

#[tokio::test]
async fn frontmost_switch_detected_when_app_changed() {
    let monitor = StubActivityMonitor {
        active_app: Some("Terminal".to_string()),
        fail: false,
    };
    // Snapshot said "Notes" but the frontmost is now "Terminal" → switched.
    assert!(frontmost_app_switched_since(&monitor, "Notes").await);
    // Same app → no switch.
    assert!(!frontmost_app_switched_since(&monitor, "Terminal").await);
}

#[tokio::test]
async fn frontmost_switch_vetoes_capture_on_monitor_error() {
    let monitor = StubActivityMonitor {
        active_app: None,
        fail: true,
    };
    // An unavailable foreground cannot justify pairing pixels with old metadata.
    assert!(frontmost_app_switched_since(&monitor, "Notes").await);
}

#[test]
fn excluded_app_visible_flags_background_sensitive_window() {
    let mut cfg = AppConfig::default_config();
    cfg.privacy = PrivacyConfig {
        excluded_apps: vec!["Slack".to_string()],
        ..Default::default()
    };
    // A background Slack window sharing the display → occlusion guard trips.
    assert!(any_excluded_app_visible(
        Some(&cfg),
        &["Chrome".to_string(), "Slack".to_string()],
    ));
    // Only non-excluded apps visible → no skip.
    assert!(!any_excluded_app_visible(
        Some(&cfg),
        &["Chrome".to_string(), "Notes".to_string()],
    ));
}

#[test]
fn excluded_app_visible_uses_fail_safe_default_without_config() {
    // No config → PrivacyConfig::default() (auto_exclude_sensitive = true)
    // must still flag a visible sensitive app.
    assert!(any_excluded_app_visible(None, &["1Password".to_string()]));
    assert!(!any_excluded_app_visible(None, &["Chrome".to_string()]));
}

#[test]
fn active_window_snapshot_defaults_without_window() {
    let ctx = UserContext {
        timestamp: Utc::now(),
        active_window: None,
        processes: vec![],
    };

    let snapshot = ActiveWindowSnapshot::from_context(&ctx);
    assert!(snapshot.app_name.is_empty());
    assert!(snapshot.window_title.is_empty());
    assert!(snapshot.window_bounds.is_none());
    assert!(snapshot.app_bundle_id.is_none());
}

#[test]
fn active_window_snapshot_keeps_window_target_fields() {
    let bounds = WindowBounds {
        x: 10,
        y: 20,
        width: 300,
        height: 200,
    };
    let ctx = UserContext {
        timestamp: Utc::now(),
        active_window: Some(WindowInfo {
            title: "Notes".to_string(),
            app_name: "Notes.app".to_string(),
            app_bundle_id: Some("com.apple.Notes".to_string()),
            pid: 44,
            bounds: Some(bounds),
        }),
        processes: vec![],
    };

    let snapshot = ActiveWindowSnapshot::from_context(&ctx);
    assert_eq!(snapshot.app_name, "Notes.app");
    assert_eq!(snapshot.window_title, "Notes");
    assert!(snapshot.window_bounds.is_some());
    let actual_bounds = snapshot.window_bounds.unwrap();
    assert_eq!(actual_bounds.x, bounds.x);
    assert_eq!(actual_bounds.y, bounds.y);
    assert_eq!(actual_bounds.width, bounds.width);
    assert_eq!(actual_bounds.height, bounds.height);
    assert_eq!(snapshot.app_bundle_id.as_deref(), Some("com.apple.Notes"));
}

#[test]
fn ring_thumbnail_cadence_respects_capture_throttle() {
    let mut cadence = RingThumbnailCadence::default();
    let throttle = Duration::from_secs(5);
    let start = Instant::now();

    assert!(cadence.should_capture(start, throttle));
    assert!(!cadence.should_capture(start + Duration::from_secs(1), throttle));
    assert!(!cadence.should_capture(start + Duration::from_secs(4), throttle));
    assert!(cadence.should_capture(start + Duration::from_secs(5), throttle));
}

#[test]
fn tick_capture_excluded_matches_configured_excluded_app() {
    let mut cfg = AppConfig::default_config();
    cfg.privacy = PrivacyConfig {
        excluded_apps: vec!["Slack".to_string()],
        ..Default::default()
    };

    assert!(tick_capture_excluded(Some(&cfg), "Slack", "General"));
    assert!(!tick_capture_excluded(Some(&cfg), "Chrome", "Google"));
}

#[test]
fn tick_capture_excluded_auto_excludes_sensitive_app_by_default() {
    let cfg = AppConfig::default_config();
    assert!(tick_capture_excluded(Some(&cfg), "1Password", "Unlock"));
}

#[test]
fn tick_capture_excluded_without_config_stays_fail_safe_for_sensitive_apps() {
    // No config snapshot → PrivacyConfig::default() (auto_exclude_sensitive
    // = true) must still auto-exclude sensitive apps.
    assert!(tick_capture_excluded(None, "1Password", "Unlock"));
    assert!(!tick_capture_excluded(None, "Chrome", "Google"));
}

#[test]
fn tick_capture_excluded_respects_auto_exclude_opt_out() {
    let mut cfg = AppConfig::default_config();
    cfg.privacy = PrivacyConfig {
        auto_exclude_sensitive: false,
        ..Default::default()
    };

    assert!(!tick_capture_excluded(Some(&cfg), "1Password", "Unlock"));
}

#[derive(Default)]
struct MarkerProcessor {
    captures: std::sync::atomic::AtomicUsize,
    bounds: std::sync::Mutex<Vec<Option<WindowBounds>>>,
    started: Option<std::sync::Arc<tokio::sync::Notify>>,
    resume: Option<std::sync::Arc<tokio::sync::Notify>>,
}

#[async_trait]
impl FrameProcessor for MarkerProcessor {
    async fn capture_and_process(
        &self,
        _: &maekon_core::ports::vision::CaptureRequest,
    ) -> Result<maekon_core::models::frame::ProcessedFrame, CoreError> {
        panic!("ring fixture must use the actual thumbnail route")
    }

    async fn capture_thumbnail(&self, bounds: Option<&WindowBounds>) -> Result<Vec<u8>, CoreError> {
        self.captures
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.bounds.lock().unwrap().push(bounds.copied());
        if let (Some(started), Some(resume)) = (&self.started, &self.resume) {
            started.notify_one();
            resume.notified().await;
        }
        Ok(vec![193, 47, 82, 255])
    }
}

fn ring_context<'a>(
    monitor: &'a dyn ActivityMonitor,
    config: Option<&'a AppConfig>,
    visible: &'a (dyn Fn() -> Vec<String> + Sync),
) -> RingThumbnailContext<'a> {
    static PAUSE: std::sync::OnceLock<CapturePauseState> = std::sync::OnceLock::new();
    let pause = PAUSE.get_or_init(|| {
        CapturePauseState::new(std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
            false,
        )))
    });
    RingThumbnailContext {
        pause,
        accepted_epoch: pause.snapshot().ok(),
        guard: CaptureGrabGuard {
            activity_monitor: monitor,
            snapshot_app: "Notes",
            config,
            visible_apps: visible,
        },
        window_title: "Doc",
        focused_element: None,
        window_bounds: None,
    }
}

#[tokio::test]
async fn late_denied_ring_marker_never_reaches_subsequent_flush() {
    let processor = MarkerProcessor::default();
    let ring = CaptureRingBuffer::new(3, 2, 0.5);
    let mut cadence = RingThumbnailCadence::default();
    let monitor = StubActivityMonitor {
        active_app: Some("Terminal".into()),
        fail: false,
    };
    let context = ring_context(&monitor, None, &Vec::new);
    // The tick snapshot was Notes. Exercise the actual due production route.
    capture_ring_thumbnail_if_due(
        &mut cadence,
        &processor,
        &ring,
        &context,
        Duration::from_secs(5),
    )
    .await;
    assert!(context.guard.denied().await);
    let flushed = ring
        .check_and_flush(1.0, marker_frame("later-allowed-trigger", 9))
        .unwrap();
    assert!(
        flushed
            .pre_event_frames
            .iter()
            .all(|frame| frame.thumbnail_data != [193, 47, 82, 255]),
        "late-denied ring marker reached subsequent pre-event flush"
    );
    assert_eq!(
        processor
            .captures
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
}

#[tokio::test]
async fn allowed_ring_marker_reaches_subsequent_flush() {
    let processor = MarkerProcessor::default();
    let ring = CaptureRingBuffer::new(3, 2, 0.5);
    let mut cadence = RingThumbnailCadence::default();
    let monitor = StubActivityMonitor {
        active_app: Some("Notes".into()),
        fail: false,
    };
    let reads = std::sync::atomic::AtomicUsize::new(0);
    let visible = || {
        reads.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        vec!["Notes".into()]
    };
    let bounds = WindowBounds {
        x: -1440,
        y: 0,
        width: 800,
        height: 600,
    };
    let mut context = ring_context(&monitor, None, &visible);
    context.window_bounds = Some(&bounds);
    for _ in 0..2 {
        capture_ring_thumbnail_if_due(
            &mut cadence,
            &processor,
            &ring,
            &context,
            Duration::from_secs(5),
        )
        .await;
    }
    // An idle/not-due call performs neither another grab nor another OS query.
    assert_eq!(reads.load(std::sync::atomic::Ordering::Relaxed), 2);
    let flushed = ring
        .check_and_flush(1.0, marker_frame("allowed-trigger", 9))
        .unwrap();
    assert_eq!(flushed.pre_event_frames.len(), 1);
    assert_eq!(
        flushed.pre_event_frames[0].thumbnail_data,
        [193, 47, 82, 255]
    );
    assert_eq!(
        processor
            .captures
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    let observed = processor.bounds.lock().unwrap();
    assert_eq!(observed.len(), 1);
    let observed = observed[0].unwrap();
    assert_eq!(
        (observed.x, observed.y, observed.width, observed.height),
        (-1440, 0, 800, 600)
    );
}

#[tokio::test]
async fn unavailable_foreground_or_excluded_background_retires_ring_before_grab() {
    let mut cfg = AppConfig::default_config();
    cfg.privacy.excluded_apps = vec!["Blocked".into()];
    for (active_app, fail, visible_apps) in [
        (None, false, vec!["Notes".to_string()]),
        (None, true, vec!["Notes".to_string()]),
        (
            Some("Notes".into()),
            false,
            vec!["Notes".into(), "Blocked".into()],
        ),
    ] {
        let processor = MarkerProcessor::default();
        let ring = CaptureRingBuffer::new(3, 2, 0.5);
        ring.push(marker_frame("previous-content", 231));
        let monitor = StubActivityMonitor { active_app, fail };
        let visible = || visible_apps.clone();
        let context = ring_context(&monitor, Some(&cfg), &visible);
        capture_ring_thumbnail_if_due(
            &mut RingThumbnailCadence::default(),
            &processor,
            &ring,
            &context,
            Duration::from_secs(5),
        )
        .await;
        assert!(context.guard.denied().await);
        let flushed = ring
            .check_and_flush(1.0, marker_frame("later-trigger", 9))
            .unwrap();
        assert!(flushed.pre_event_frames.is_empty());
        assert_eq!(
            processor
                .captures
                .load(std::sync::atomic::Ordering::Relaxed),
            0
        );
    }
}

#[tokio::test]
async fn exclusion_during_acquisition_discards_returned_and_old_pixels_then_recovers() {
    let started = std::sync::Arc::new(tokio::sync::Notify::new());
    let resume = std::sync::Arc::new(tokio::sync::Notify::new());
    let mut processor = MarkerProcessor {
        started: Some(started.clone()),
        resume: Some(resume.clone()),
        ..Default::default()
    };
    let ring = CaptureRingBuffer::new(3, 2, 0.5);
    ring.push(marker_frame("old-marker", 231));
    let mut cadence = RingThumbnailCadence::default();
    let monitor = StubActivityMonitor {
        active_app: Some("Notes".into()),
        fail: false,
    };
    let mut cfg = AppConfig::default_config();
    cfg.privacy.excluded_apps = vec!["Blocked".into()];
    let owners = std::sync::Mutex::new(vec!["Notes".to_string()]);
    let visible = || owners.lock().unwrap().clone();
    let context = ring_context(&monitor, Some(&cfg), &visible);
    let capture = capture_ring_thumbnail_if_due(
        &mut cadence,
        &processor,
        &ring,
        &context,
        Duration::from_secs(5),
    );
    let transition = async {
        started.notified().await;
        *owners.lock().unwrap() = vec!["Blocked".into()];
        resume.notify_one();
    };
    tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(capture, transition);
    })
    .await
    .expect("controlled thumbnail acquisition must finish");
    let flushed = ring
        .check_and_flush(1.0, marker_frame("later-trigger", 9))
        .unwrap();
    assert!(
        flushed.pre_event_frames.is_empty(),
        "post-await denied pixels reached the flush"
    );
    assert_eq!(
        processor
            .captures
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    processor.started = None;
    processor.resume = None;
    *owners.lock().unwrap() = vec!["Notes".into()];
    cadence.last_capture_at = None;
    capture_ring_thumbnail_if_due(
        &mut cadence,
        &processor,
        &ring,
        &context,
        Duration::from_secs(5),
    )
    .await;
    let recovered = ring
        .check_and_flush(1.0, marker_frame("fresh-trigger", 9))
        .unwrap();
    assert_eq!(recovered.pre_event_frames.len(), 1);
    assert_eq!(
        recovered.pre_event_frames[0].thumbnail_data,
        [193, 47, 82, 255]
    );
    assert_eq!(
        processor
            .captures
            .load(std::sync::atomic::Ordering::Relaxed),
        2
    );
}

mod ring_pause;
