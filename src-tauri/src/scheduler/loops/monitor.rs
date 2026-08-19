use chrono::Utc;
use maekon_core::capture_gate;
use maekon_core::models::event::{Event, InputActivityEvent, KeyboardActivity, MouseActivity};
use maekon_core::models::frame::OcrRegion;
use maekon_core::ports::consent_manager::ConsentGate;
use maekon_monitor::idle::IdleTracker;
use maekon_monitor::input_activity::InputActivityCollector;
use maekon_monitor::window_layout::WindowLayoutTracker;
use maekon_vision::ring_buffer::{CaptureRingBuffer, RingFrame};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::Manager;
use tracing::{debug, info, warn};

use super::super::egress_policy::PlatformEgressPolicy;
use super::super::gui_pipeline::gui_feedback_pii_level;
use super::super::shared_regime_state::SharedRegimeState;
use super::super::Scheduler;
use super::coaching_helper::{CoachingEvalContext, CoachingTickState};
use super::helpers::{
    audit_consent_and_pii_changes, build_segment_stats_snapshot, emit_heatmap_and_goals,
    emit_pointer_context_highlight, handle_event_analysis, handle_frame_capture, handle_idle_tick,
    redact_window_title, IdleTickServices, PointerContextEmitterState,
};
use super::monitor_phases::{
    capture_ring_thumbnail_if_due, ActiveWindowSnapshot, RingThumbnailCadence,
};
use crate::focus_mode::FocusModeState;

impl Scheduler {
    #[tracing::instrument(skip_all)]
    #[allow(clippy::too_many_arguments)]
    pub(in crate::scheduler) fn spawn_monitor_loop(
        &self,
        poll: Duration,
        idle_threshold: u64,
        session_id: String,
        egress_policy: Arc<PlatformEgressPolicy>,
        input_collector: Arc<InputActivityCollector>,
        adaptive_trigger_state: Option<super::super::AdaptiveTriggerState>,
        shared_regime: Arc<SharedRegimeState>,
        focus_mode: Arc<FocusModeState>,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
        app_handle: Option<tauri::AppHandle>,
    ) -> tokio::task::JoinHandle<()> {
        let act_mon = self.activity_monitor.clone();
        let trigger = self.capture_trigger.clone();
        let processor = self.frame_processor.clone();
        let storage1 = self.storage.clone();
        let sqlite1 = self.sqlite_storage.clone();
        let frame_storage1 = self.frame_storage.clone();
        let uploader1 = self.batch_sink.clone();
        let egress1 = egress_policy;
        let session1 = session_id;
        let notif1 = self.notification_manager.clone();
        let focus1 = self.focus_analyzer.clone();
        // #7652: shared runtime slot (Arc<RwLock<Option<Arc<ContextAnalyzer>>>>).
        // This loop only READS it (single-writer: `spawn_analysis_loop` owns
        // install/teardown) — re-read once per tick below so a runtime
        // enable/disable transition is observed without a restart.
        let context_analyzer1 = self.context_analyzer.clone();
        // E20-24 (#4816): live suggestion queue for the event-driven analysis producer.
        #[cfg(feature = "local-suggestions")]
        let event_suggestion_queue1 = self.suggestion_manager.as_ref().map(|m| m.queue().clone());
        #[cfg(not(feature = "local-suggestions"))]
        let event_suggestion_queue1: Option<
            std::sync::Arc<tokio::sync::Mutex<maekon_suggestion::queue::SuggestionQueue>>,
        > = None;
        // #7914: shared FeedbackScorer handle for the uniform relevance gate on
        // every LOCAL producer this loop feeds. `None` => pass-through (cfg fork
        // lives on the accessor to keep this LOC-capped loop cfg-free).
        let scorer_for_gates = self.relevance_scorer();
        let input_collector1 = input_collector;
        let accessibility_extractor1 = self.accessibility_extractor.clone();
        let config_manager1 = self.config_manager.clone();
        let consent_manager1 = self.consent_manager.clone();
        let coaching_engine_ref = self.coaching_engine.clone();
        let overlay_ref = self.magic_overlay.clone();
        let coaching_storage_ref = self.coaching_storage.clone();
        let coaching_analysis_provider = self.analysis_provider.clone();
        let gui_feedback_pii_san = super::super::gui_pipeline::gui_feedback_pii_sanitizer();
        let coaching_pii_sanitizer = super::coaching_helper::build_pii_sanitizer();
        let capture_paused = self.capture_paused.clone();
        let overlay_driver_ref = self.overlay_driver.clone();
        let detection_active = self.detection_active.clone();
        let scene_finder_slot = self.scene_finder_slot.clone();
        let event_tx_mon = self.event_tx.clone();

        tokio::spawn(async move {
            let mut prev_app: Option<String> = None;
            let mut prev_window_title: Option<String> = None;
            // #7909: previous tick's capture-exclusion state — the ledger records
            // one "capture_blocked" entry per transition INTO an excluded app,
            // not one per 1s tick.
            let mut prev_capture_excluded = false;
            let mut prev_idle_secs: u64 = 0;
            let mut interval = super::intervals::coalescing_interval(poll);
            let mut focus_block = super::autostart_helper::FocusBlockState::default();
            let mut idle_tracker = IdleTracker::new(Some(idle_threshold));
            let mut adaptive_trigger_state = adaptive_trigger_state;
            let window_tracker = WindowLayoutTracker::new();
            let input_collector = input_collector1;
            let ring_buffer = CaptureRingBuffer::new(6, 2, 0.5); // dashcam: 6 slots, 2 post-event, 0.5 threshold
            let mut thumbnail_cadence = RingThumbnailCadence::default();

            // GUI Activity Intelligence state (carried across ticks)
            use maekon_core::models::focused_element::FocusedElementInfo;
            use maekon_core::models::gui_activity::GuiActivitySummary;
            let mut last_gui_summary: Option<GuiActivitySummary> = None;
            let mut last_focused_element: Option<FocusedElementInfo> = None;
            let mut last_ocr_regions: Vec<OcrRegion> = Vec::new();
            let mut last_frame_rgba: Option<(Vec<u8>, u32, u32)> = None;
            let mut focus_hl = super::detection_helper::FocusHighlightState::new();
            let mut coaching_tick_state = CoachingTickState::new();
            let mut pointer_context_state = PointerContextEmitterState::default();
            let mut last_retention_check = Instant::now();
            let mut prev_full_text_consent = false;
            let mut prev_pii_level = config_manager1
                .as_ref()
                .map(|cm| cm.get().analysis.text_intelligence.pii_extraction_level)
                .unwrap_or_default();
            let mut ts_notify_state = (false, None::<Instant>); // A.18: (prev_active, last_notified)
                                                                // #4795/#4798: power-aware cadence counter (increments by 1 each 1s tick).
                                                                // Used to decide the idle-backoff + battery-saver throttle phase — idle/TS notification
                                                                // ticks are unaffected; only the expensive collect_context/AX/analysis blocks are gated.
            let mut power_tick_counter: u64 = 0;
            // #8686 AC4: OS screen-capture permission edge watch (mid-session
            // TCC revocation → fail-closed stop + one-shot recovery surface).
            let mut os_permission_watch = capture_gate::OsPermissionWatch::default();

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        // E20-4 (#4796): take ONE cheap Arc<AppConfig> snapshot per tick
                        // and read all sub-trees off it, instead of calling
                        // `config_manager1.get()` (a FULL deep clone of AppConfig)
                        // multiple times per 1s tick. The snapshot is the same value
                        // every per-call `get()` would have observed at tick start
                        // (both read `sender.borrow()`), so semantics + the ≤1s
                        // propagation window are unchanged — only the per-second
                        // deep-clone heap churn on the idle hot path is removed.
                        let config_snapshot = config_manager1.as_ref().map(|cm| cm.snapshot());
                        // One live fail-closed consent snapshot per tick. Idle,
                        // switch, capture, and analysis paths consume the same
                        // point-in-time value so own-field gates cannot disagree.
                        let consent = ConsentGate::from_ref(consent_manager1.as_ref())
                            .permissions_snapshot();
                        if let Some(ref focus) = focus1 {
                            // Reconcile before any composite capture-gate `continue`.
                            // Expired/update-required/missing consent must clear
                            // pattern state even when capture work is skipped.
                            focus
                                .reconcile_activity_pattern_consent(&consent)
                                .await;
                        }
                        // #7652: re-read the shared analyzer slot once per tick — the
                        // guard is cloned and dropped immediately (no `.await` held),
                        // so a runtime install/teardown by `spawn_analysis_loop`
                        // (analysis.enabled flip) is picked up without a restart.
                        let context_analyzer_now = context_analyzer1.read().clone();
                        // A4: Focus mode auto-expiry check
                        if focus_mode.check_expiry() {
                            if let Some(ref overlay) = overlay_ref {
                                overlay.emit_focus_mode(false, false);
                            }
                            info!("Focus mode expired — auto-deactivated");
                        }
                        let idle_outcome = handle_idle_tick(
                            &mut idle_tracker,
                            IdleTickServices {
                                sqlite: &sqlite1,
                                notif: &notif1,
                                focus: &focus1,
                                consent: consent.clone(),
                                input_collector: &input_collector,
                                event_tx: &event_tx_mon,
                            },
                            prev_idle_secs,
                            focus_mode.is_active(),
                        ).await;
                        let new_idle_secs = idle_outcome.idle_secs;
                        let idle_resume_suggestions = idle_outcome.resume_suggestions;
                        // #7492: FocusAnalyzer idle-resume can produce rule
                        // suggestions (notably playbook-pattern flushes). It
                        // now runs on the real Idle→Active edge, so mirror the
                        // app-switch/focus-loop bridge: server coexistence,
                        // regime filter, then live queue + overlay refresh.
                        #[cfg(feature = "local-suggestions")]
                        if !idle_resume_suggestions.is_empty() {
                            let idle_resume_queue_for_push = {
                                let server_recent = match config_snapshot
                                    .as_ref()
                                    .map(|cfg| cfg.analysis.server_coexistence_lookback_secs)
                                {
                                    Some(lookback) => {
                                        let sqlite_coexist = sqlite1.clone();
                                        tokio::task::spawn_blocking(move || {
                                            sqlite_coexist
                                                .has_recent_server_suggestions(lookback)
                                        })
                                        .await
                                        .unwrap_or_else(|join_err| {
                                            warn!(
                                                "idle-resume coexistence check task panicked: {join_err}"
                                            );
                                            Ok(false)
                                        })
                                        .unwrap_or(false)
                                    }
                                    None => false,
                                };
                                if server_recent {
                                    None
                                } else {
                                    event_suggestion_queue1.as_ref()
                                }
                            };
                            if let Some(q) = idle_resume_queue_for_push {
                                // #7914: uniform gate seam — regime filter + learned
                                // per-regime acceptance + FeedbackScorer, applied inside
                                // enqueue_and_surface like every other LOCAL producer.
                                let idle_resume_regime = shared_regime.snapshot().regime;
                                let idle_resume_gates = super::helpers::relevance_gates(
                                    scorer_for_gates.as_ref(),
                                    idle_resume_regime.as_ref(),
                                    adaptive_trigger_state.as_ref(),
                                );
                                let idle_resume_on_changed = overlay_ref.as_ref().map(|o| {
                                    let o = o.clone();
                                    move |c: usize| o.emit_suggestions_changed(c)
                                });
                                let idle_resume_on_changed_ref: Option<
                                    &(dyn Fn(usize) + Send + Sync),
                                > = idle_resume_on_changed
                                    .as_ref()
                                    .map(|f| f as &(dyn Fn(usize) + Send + Sync));
                                super::helpers::enqueue_and_surface(
                                    q,
                                    idle_resume_suggestions,
                                    idle_resume_gates,
                                    idle_resume_on_changed_ref,
                                    None,
                                    false,
                                )
                                .await;
                            }
                        }
                        #[cfg(not(feature = "local-suggestions"))]
                        let _ = idle_resume_suggestions;

                        // A.18: TS window enter/exit → desktop notify (60s debounce)
                        // #7735 E-3: `tick_ts_notifications` now takes `Option<&dyn
                        // capture_gate::TsNotifier>` (core cannot see the concrete
                        // `NotificationManager` type) — `Option` does not auto-coerce
                        // an inner reference to a trait object, so the cast is explicit.
                        capture_gate::tick_ts_notifications(
                            config_snapshot.as_deref(),
                            notif1
                                .as_deref()
                                .map(|n| n as &dyn capture_gate::TsNotifier),
                            &mut ts_notify_state.0,
                            &mut ts_notify_state.1,
                        )
                        .await;

                        // #6830: release the carried full-res RGBA frame (~33MB at 4K) + its
                        // paired OCR regions once idle backoff engages. MUST run BEFORE the
                        // consent gate and the power-backoff gate — during idle backoff most
                        // ticks `continue` at the power-backoff gate, so a drop placed after
                        // either gate would never fire in the idle window this targets. Frame
                        // and regions are dropped together to preserve the frame<->regions
                        // lockstep invariant (mirrors the empty-region reset below).
                        if capture_gate::should_release_idle_frame(new_idle_secs)
                            && (last_frame_rgba.is_some() || !last_ocr_regions.is_empty())
                        {
                            last_frame_rgba = None;
                            last_ocr_regions.clear();
                        }

                        // PR-B1 §5.5: productive-session detection (Idle↔Active transitions, idempotent counter)
                        focus_block.tick(&mut prev_idle_secs, new_idle_secs, idle_threshold, app_handle.as_ref(), config_manager1.as_ref());
                        let paused = capture_paused.load(std::sync::atomic::Ordering::Relaxed);
                        let capture_permitted_for_tick = config_manager1.as_ref()
                            .map(|cm| crate::scheduler::capture_permitted_now(&cm.snapshot(), &consent, paused))
                            .unwrap_or(false);
                        // #8686 AC4: OS permission axis — probed only while the
                        // config/consent gate is open (a closed gate already
                        // stops capture; probing then would emit banner noise).
                        // Revocation stops capture fail-closed within one tick
                        // and surfaces a one-shot event + desktop notification.
                        let os_capture_ok = if capture_permitted_for_tick {
                            super::os_permission_helper::observe_os_capture_permission(
                                &mut os_permission_watch,
                                app_handle.as_ref(),
                                notif1
                                    .as_deref()
                                    .map(|n| n as &dyn capture_gate::TsNotifier),
                            )
                            .await
                        } else {
                            true
                        };
                        if !capture_permitted_for_tick || !os_capture_ok {
                            // #8045 B3: a closed capture gate (consent withdrawn/
                            // revoked, capture paused, or capture disabled by
                            // config) must not retain pre-consent dashcam frames
                            // in RAM. Drop + zeroize any buffered thumbnails/
                            // titles/accessibility trees now. On a consent
                            // withdrawal the fail-closed ConsentGate above closes
                            // this gate within one tick (<=1s), wiping the in-
                            // memory buffer that `withdraw_consent`'s on-disk
                            // erase cannot reach. Cheap no-op once the buffer is
                            // already empty.
                            ring_buffer.clear();
                            debug!("monitor loop: capture gate closed - skipping context, accessibility, capture, and analysis tick");
                            continue;
                        }

                        // ── #4795 idle adaptive backoff + #4798 battery-saver throttle ──
                        // When idle (no input ≥ MONITOR_IDLE_BACKOFF_SECS) or in battery-saver mode,
                        // stretch the cadence of the expensive osascript (collect_context) + AX + analysis.
                        // The idle/TS notification + focus_block ticks already ran every tick above, so
                        // edge detection is not delayed. On an input edge (return to active), idle_secs
                        // drops below the threshold and the 1s cadence is restored immediately.
                        let battery_saver = crate::scheduler::schedule::BATTERY_SAVER_ACTIVE
                            .load(std::sync::atomic::Ordering::Relaxed);
                        let tick_decision = capture_gate::decide_monitor_tick(
                            power_tick_counter,
                            new_idle_secs,
                            battery_saver,
                        );
                        power_tick_counter = power_tick_counter.wrapping_add(1);
                        if !tick_decision.process {
                            debug!(
                                idle_secs = new_idle_secs,
                                battery_saver,
                                "monitor loop: power backoff - skipping expensive context tick"
                            );
                            continue;
                        }
                        // In battery-saver mode, skip the most expensive optional blocks (AX extraction / GUI-LLM feedback).
                        let skip_expensive = tick_decision.skip_expensive;

                        // #6441 (F13): the monitor loop only uses the active window (+
                        // mouse), never ctx.processes — use the lightweight collector that
                        // skips the per-tick full process-table walk.
                        match act_mon.collect_active_context().await {
                            Ok(ctx) => {
                                let active_window = ActiveWindowSnapshot::from_context(&ctx);
                                let app_name = active_window.app_name;
                                // Own-field gate: collecting the window title requires window_title_collection consent.
                                // Even if only the composite gate (screen_capture) is granted, window_title_collection
                                // defaults to false, so the title is redacted to an empty string. CRITICAL: this is
                                // ConsentPermissions.window_title_collection (the Valid-only value from
                                // effective_permissions()), NOT a config toggle. Unifying on an empty string ensures
                                // every downstream consumer (ContextEvent / window_tracker / capture_req / focus /
                                // analysis) sees the redacted value.
                                // review4 monitor re-verify: PII-mask the (consented)
                                // title before any downstream use. redact_window_title
                                // is consent-only (raw title when granted, else empty)
                                // with NO PII masking, and the raw title was persisted
                                // at rest via save_event(Event::Window) + save_event(
                                // Event::Context) and embedded into the Context
                                // event_id PK — while every other at-rest title sink
                                // (frame metadata, analysis pipeline, egress) masks via
                                // sanitize_title_with_level. This was the lone unmasked
                                // sink (sibling of the #6298 file-path fix). Masking at
                                // the source covers window_tracker + context + the PK
                                // uniformly and matches analysis_pipeline, which masks
                                // the title before its own downstream use.
                                let window_title = {
                                    let redacted = redact_window_title(
                                        active_window.window_title,
                                        consent.window_title_collection,
                                    );
                                    let title_pii_level = config_snapshot
                                        .as_ref()
                                        .map(|cfg| cfg.privacy.pii_filter_level)
                                        .unwrap_or_default();
                                    maekon_vision::privacy::sanitize_title_with_level(
                                        &redacted,
                                        title_pii_level,
                                    )
                                };
                                let focus_window_title = window_title.clone();
                                let window_bounds = active_window.window_bounds;
                                let app_bundle_id = active_window.app_bundle_id;
                                let mut focus_ocr_hint: Option<String> = None;

                                // ── Capture-time exclusion (#7909, T1.1) ──
                                // Gate every content-capture surface of this tick
                                // (AX extraction, ring thumbnail, trigger capture,
                                // post-event forced capture, detection re-analysis)
                                // on the exclusion policy. Metadata events still
                                // flow; see tick_capture_excluded docs.
                                let capture_excluded = super::monitor_phases::tick_capture_excluded(
                                    config_snapshot.as_deref(),
                                    &app_name,
                                    &window_title,
                                );
                                if capture_excluded && !prev_capture_excluded {
                                    debug!(app = %app_name, "capture excluded by privacy policy (transition) — recording ledger entry");
                                    let consent_state = egress1.consent_state_snapshot();
                                    super::super::egress_policy::record_capture_block(
                                        &sqlite1,
                                        &consent_state,
                                    )
                                    .await;
                                }
                                prev_capture_excluded = capture_excluded;

                                input_collector.set_current_app(&app_name);
                                if let Some(ref cfg) = config_snapshot { super::focus_auto_helper::evaluate_focus_auto(&cfg.focus_auto, &focus_mode, &app_name, overlay_ref.as_ref()); }

                                // ── Accessibility API extraction (Phase 2) ──
                                // Extract focused element info per tick when enabled.
                                // Result is stored for the GUI pipeline to consume.
                                // #4798: in battery-saver mode, skip AX extraction (the most expensive optional block)
                                // and clear the highlight (same handling as the collect_context failure branch).
                                // #7909: same skip when the active app is capture-excluded — the focused
                                // element's extracted text IS captured content of the excluded app.
                                if skip_expensive || capture_excluded {
                                    super::detection_helper::clear_focus_highlight(
                                        &mut focus_hl, &overlay_driver_ref,
                                    ).await;
                                    last_focused_element = None;
                                } else if let Some(ref ax) = accessibility_extractor1 {
                                    let text_config = config_snapshot
                                        .as_ref()
                                        .map(|cfg| cfg.analysis.text_intelligence.clone())
                                        .unwrap_or_default();
                                    let full_text_consent =
                                        ConsentGate::from_ref(consent_manager1.as_ref())
                                            .may_extract_full_text();

                                    // Audit: log consent / PII level changes
                                    (prev_full_text_consent, prev_pii_level) =
                                        audit_consent_and_pii_changes(
                                            full_text_consent,
                                            prev_full_text_consent,
                                            text_config.pii_extraction_level,
                                            prev_pii_level,
                                        );

                                    match ax
                                        .extract_focused_element(
                                            text_config.pii_extraction_level,
                                            full_text_consent,
                                        )
                                        .await
                                    {
                                        Ok(info) => {
                                            last_focused_element = super::detection_helper::update_focus_highlight(
                                                info, &mut focus_hl, &overlay_driver_ref,
                                            ).await;
                                        }
                                        Err(e) => {
                                            debug!("accessibility extraction failed: {e}");
                                            super::detection_helper::clear_focus_highlight(
                                                &mut focus_hl, &overlay_driver_ref,
                                            ).await;
                                            last_focused_element = None;
                                        }
                                    }
                                }

                                if let Some(layout_event) = window_tracker.update(&app_name, &window_title, window_bounds) {
                                    // Update GUI detector + heatmap resolution from the latest layout event
                                    let (res_w, res_h) = layout_event.screen_resolution;
                                    if let Some(ref mut ts) = adaptive_trigger_state {
                                        if let Some(ref mut gui_state) = ts.gui_pipeline_state {
                                            gui_state.detector.update_resolution(res_w, res_h);
                                        }
                                        ts.heatmap_aggregator.update_resolution(res_w, res_h);
                                    }

                                    let win_event = Event::Window(layout_event);
                                    if let Err(e) = storage1.save_event(&win_event).await {
                                        warn!(err.code = %e.code(), "window event save failure: {e}");
                                    }
                                    if let Some(ref sink) = uploader1 {
                                        // #4803: egress audit — compute the type/size before consumption and
                                        // record the uploaded/blocked disposition in the ledger.
                                        let etype = super::super::egress_policy::egress_event_type(&win_event);
                                        let bytes = super::super::egress_policy::egress_byte_count(&win_event);
                                        let consent_state = egress1.consent_state_snapshot();
                                        // #7946: pair the upload payload with the PERSISTED id
                                        // (derived from the original event — egress filtering can
                                        // change id-relevant fields) so flush marks the right row.
                                        let upload_storage_id =
                                            maekon_storage::sqlite::storage_event_id(&win_event);
                                        if let Some(upload_event) = egress1.prepare_event_for_upload(win_event) {
                                            sink.enqueue(maekon_core::ports::batch_sink::QueuedUpload {
                                                storage_id: upload_storage_id,
                                                event: upload_event,
                                            });
                                            super::super::egress_policy::record_event_egress(
                                                &sqlite1, etype, bytes, "uploaded", &consent_state,
                                            ).await;
                                        } else {
                                            super::super::egress_policy::record_event_egress(
                                                &sqlite1, etype, bytes, "blocked", &consent_state,
                                            ).await;
                                        }
                                    }
                                }

                                let event = maekon_core::models::event::ContextEvent {
                                    app_name: app_name.clone(),
                                    window_title,
                                    prev_app_name: prev_app.clone(),
                                    timestamp: Utc::now(),
                                    input_activity_level: input_collector.peek_activity_level(),
                                };

                                let thumbnail_throttle = config_snapshot
                                    .as_ref()
                                    .map(|cfg| Duration::from_millis(cfg.vision.capture_throttle_ms))
                                    .unwrap_or(Duration::from_secs(5));

                                // #7909: no ring thumbnail while the active app is
                                // capture-excluded — excluded-app pixels must never
                                // enter the dashcam pre-event buffer.
                                if !capture_excluded {
                                    capture_ring_thumbnail_if_due(
                                        &mut thumbnail_cadence,
                                        processor.as_ref(),
                                        &ring_buffer,
                                        &app_name,
                                        &event.window_title,
                                        last_focused_element.as_ref(),
                                        window_bounds.as_ref(),
                                        thumbnail_throttle,
                                    ).await;
                                }
                                {
                                    // #7909: skip the trigger entirely for excluded apps —
                                    // the frame pipeline (capture → OCR → storage → replay)
                                    // must not see them. Not calling should_capture also
                                    // leaves the trigger's throttle state untouched.
                                    let capture_req = if capture_excluded {
                                        None
                                    } else {
                                        trigger.should_capture(&event)
                                    };

                                    // Force capture during post-event window (dashcam "after" frames).
                                    // #7909: while excluded, don't consume the post-event window —
                                    // freezing it means the "after" frames resume only once a
                                    // non-excluded app is frontmost again.
                                    let force_post =
                                        !capture_excluded && ring_buffer.should_force_post_capture();

                                    // A4: Elevate capture threshold in focus mode —
                                    // only process captures with importance >= 0.7
                                    let focus_threshold: f32 = if focus_mode.is_active() { 0.7 } else { 0.0 };

                                    // #8054 P3-1 + P2-4: grab-time safety gate. Evaluated only
                                    // when a content grab would actually run (throttled — kept
                                    // off the idle hot path). Skips the capture when the
                                    // frontmost app switched since the tick-start snapshot
                                    // (stale-metadata / just-excluded-app TOCTOU) or when a
                                    // background window of an excluded/sensitive app is visible
                                    // on the same display. `||` short-circuits so the window
                                    // enumeration only runs when the app did NOT switch.
                                    let would_capture = capture_req
                                        .as_ref()
                                        .map(|r| r.importance >= focus_threshold)
                                        .unwrap_or(false)
                                        || force_post;
                                    let grab_skip = would_capture
                                        && (super::monitor_phases::frontmost_app_switched_since(
                                            act_mon.as_ref(),
                                            &app_name,
                                        )
                                        .await
                                            || super::monitor_phases::any_excluded_app_visible(
                                                config_snapshot.as_deref(),
                                                &maekon_monitor::visible_window_app_names(),
                                            ));
                                    if grab_skip {
                                        debug!(app = %app_name, "capture skipped at grab time (window switch or occluded excluded app)");
                                    }

                                    if let Some(mut capture_req) = capture_req.filter(|r| !grab_skip && r.importance >= focus_threshold) {
                                        // Inject active window bounds so the frame processor
                                        // captures the correct monitor in multi-monitor setups.
                                        capture_req.window_bounds = window_bounds;
                                        capture_req.app_bundle_id = app_bundle_id.clone();
                                        // #8054 P2-1: inject the HiDPI scale factor of the
                                        // active window's monitor so OCR regions are scaled
                                        // back to logical pixels, matching overlay / element
                                        // finder coordinates (previously always None → 2x
                                        // mismatch on Retina).
                                        capture_req.screen_scale_factor =
                                            crate::capture_scale::active_monitor_scale_factor(
                                                app_handle.as_ref(),
                                                window_bounds.as_ref(),
                                            );

                                        // --- Ring buffer: flush pre-event frames on significant capture ---
                                        if let Some(ref fs) = frame_storage1 {
                                            let flush_frame = RingFrame {
                                                timestamp: Utc::now(),
                                                thumbnail_data: vec![],
                                                app_name: capture_req.app_name.clone(),
                                                window_title: capture_req.window_title.clone(),
                                                accessibility_elements: Vec::new(),
                                            };
                                            if let Some(flush) = ring_buffer.check_and_flush(capture_req.importance, flush_frame) {
                                                let batch: Vec<_> = flush.pre_event_frames
                                                    .into_iter()
                                                    .filter(|f| !f.thumbnail_data.is_empty())
                                                    .map(|f| (f.timestamp, f.thumbnail_data))
                                                    .collect();
                                                if !batch.is_empty() {
                                                    debug!("ring buffer: saving {} pre-event frames", batch.len());
                                                    let results = fs.save_frames_batch(batch).await;
                                                    for result in &results {
                                                        if let Err(e) = result {
                                                            warn!("frame batch write failed (possible disk full): {e}");
                                                        }
                                                    }
                                                }
                                            }
                                        }

                                        // D5 iter-3: pii_level for OCR sanitization at storage boundary.
                                        let capture_pii = config_snapshot.as_ref().map(|cfg| cfg.privacy.pii_filter_level).unwrap_or_default();
                                        // Own-field gate: extracting OCR text requires ocr_processing consent.
                                        // Even if only the composite gate (screen_capture) is granted, ocr_processing defaults
                                        // to false, so the frame is captured but the OCR text/regions are discarded
                                        // (ConsentPermissions.ocr_processing).
                                        let (ocr_hint, regions, frame_rgba) = handle_frame_capture(&capture_req, &processor, &frame_storage1, &sqlite1, &session1, capture_pii, consent.ocr_processing, &event_tx_mon).await;
                                        focus_ocr_hint = ocr_hint;
                                        if !regions.is_empty() {
                                            last_ocr_regions = regions;
                                            last_frame_rgba = frame_rgba;
                                        } else {
                                            // Reset OCR regions and the frame they describe in
                                            // lockstep: an empty-region frame must not leave stale
                                            // regions paired with a None frame (geometry/staleness
                                            // mismatch in the GUI pipeline).
                                            last_ocr_regions.clear();
                                            last_frame_rgba = None;
                                        }
                                    } else if force_post && !grab_skip {
                                        // Post-event forced capture (dashcam "after" frames).
                                        // #8054 P2-3: target the active window's monitor.
                                        if let Some(ref fs) = frame_storage1 {
                                            if let Ok(thumb_data) = processor.capture_thumbnail(window_bounds.as_ref()).await {
                                                debug!("ring buffer: post-event forced capture");
                                                if let Err(e) = fs.save_frame(Utc::now(), &thumb_data).await {
                                                    warn!("frame write failed (possible disk full): {e}");
                                                }
                                            }
                                        }
                                    }
                                }
                                let ctx_event = Event::Context(event);
                                if let Err(e) = storage1.save_event(&ctx_event).await {
                                    warn!(err.code = %e.code(), "event save failure: {e}");
                                }
                                if let Err(e) = sqlite1.increment_session_counters(&session1, 1, 0, 0).await {
                                    debug!("increment_session_counters failed: {e}");
                                }
                                if let Some(ref sink) = uploader1 {
                                    // #4803: egress audit (uploaded/blocked).
                                    let etype = super::super::egress_policy::egress_event_type(&ctx_event);
                                    let bytes = super::super::egress_policy::egress_byte_count(&ctx_event);
                                    let consent_state = egress1.consent_state_snapshot();
                                    // #7946: persisted id travels with the filtered payload.
                                    let upload_storage_id =
                                        maekon_storage::sqlite::storage_event_id(&ctx_event);
                                    if let Some(upload_event) = egress1.prepare_event_for_upload(ctx_event) {
                                        sink.enqueue(maekon_core::ports::batch_sink::QueuedUpload {
                                            storage_id: upload_storage_id,
                                            event: upload_event,
                                        });
                                        super::super::egress_policy::record_event_egress(
                                            &sqlite1, etype, bytes, "uploaded", &consent_state,
                                        ).await;
                                    } else {
                                        super::super::egress_policy::record_event_egress(
                                            &sqlite1, etype, bytes, "blocked", &consent_state,
                                        ).await;
                                    }
                                }
                                let app_changed = prev_app.as_ref() != Some(&app_name);
                                if app_changed {
                                    if let Some(ref focus) = focus1 {
                                        // Independent own-field gates: app-usage aggregation follows
                                        // app_usage_analytics, while workflow patterns follow
                                        // activity_pattern_learning. Focus-session tracking remains
                                        // protected by the composite capture gate.
                                        let rule_suggestions = focus
                                            .on_app_switch_with_context(
                                                &app_name,
                                                &focus_window_title,
                                                focus_ocr_hint.as_deref(),
                                                &consent,
                                            )
                                            .await;
                                        // #5696: bridge rule suggestions (restore-context /
                                        // playbook) into the live queue so the overlay shows
                                        // them this session. notifier=None — the rules send
                                        // their own one-shot OS notification already.
                                        if !rule_suggestions.is_empty() {
                                            if let Some(q) = event_suggestion_queue1.as_ref() {
                                                // #7914: uniform gate seam — the
                                                // restore-context / playbook rule producer
                                                // now runs the SAME decision function.
                                                let rs_regime =
                                                    shared_regime.snapshot().regime;
                                                let rs_gates = super::helpers::relevance_gates(
                                                    scorer_for_gates.as_ref(),
                                                    rs_regime.as_ref(),
                                                    adaptive_trigger_state.as_ref(),
                                                );
                                                let rs_on_changed =
                                                    overlay_ref.as_ref().map(|o| {
                                                        let o = o.clone();
                                                        move |c: usize| {
                                                            o.emit_suggestions_changed(c)
                                                        }
                                                    });
                                                let rs_on_changed_ref: Option<
                                                    &(dyn Fn(usize) + Send + Sync),
                                                > = rs_on_changed
                                                    .as_ref()
                                                    .map(|f| f as &(dyn Fn(usize) + Send + Sync));
                                                super::helpers::enqueue_and_surface(
                                                    q,
                                                    rule_suggestions,
                                                    rs_gates,
                                                    rs_on_changed_ref,
                                                    None,
                                                    false,
                                                )
                                                .await;
                                            }
                                        }
                                    }

                                    // Event-driven LLM analysis on significant app switches.
                                    // E20-24 (#4816): mirror the periodic analysis loop's
                                    // server-coexistence guard (intelligence.rs) — suppress the
                                    // local queue push when the server has recently delivered SSE
                                    // suggestions, so server builds never inject competing local
                                    // suggestions into the SSE-fed queue. In OSS this is always a
                                    // no-op (no server ⇒ has_recent_server_suggestions == false).
                                    #[cfg(feature = "local-suggestions")]
                                    let event_queue_for_push = {
                                        // #6083: offload the SYNCHRONOUS SqliteStorage
                                        // read off the monitor loop's tokio worker — it
                                        // takes the shared connection mutex and can park
                                        // under a concurrent VACUUM/optimize pass.
                                        let server_recent = match config_snapshot.as_ref().map(
                                            |cfg| cfg.analysis.server_coexistence_lookback_secs,
                                        ) {
                                            Some(lookback) => {
                                                let sqlite_coexist = sqlite1.clone();
                                                tokio::task::spawn_blocking(move || {
                                                    sqlite_coexist
                                                        .has_recent_server_suggestions(lookback)
                                                })
                                                .await
                                                .unwrap_or_else(|join_err| {
                                                    tracing::warn!(
                                                        "event coexistence check task panicked: {join_err}"
                                                    );
                                                    Ok(false) // fail-open
                                                })
                                                .unwrap_or(false)
                                            }
                                            None => false,
                                        };
                                        if server_recent {
                                            None
                                        } else {
                                            event_suggestion_queue1.as_ref()
                                        }
                                    };
                                    #[cfg(not(feature = "local-suggestions"))]
                                    let event_queue_for_push = event_suggestion_queue1.as_ref();
                                    // E20-26 (#4818): current regime for context-aware gating
                                    // of event-driven suggestions. Read from the shared state
                                    // (written at the end of the previous tick); `None` =>
                                    // pass-through. The owned clone keeps the borrow short.
                                    let event_regime = shared_regime.snapshot().regime;
                                    // #7914: uniform gate seam. The event path is where the
                                    // learned acceptance rate is reachable (RegimeClassifier
                                    // lives on adaptive_trigger_state, owned by this loop), so
                                    // it feeds the FULL gate set (regime + acceptance + scorer).
                                    let event_gates = super::helpers::relevance_gates(
                                        scorer_for_gates.as_ref(),
                                        event_regime.as_ref(),
                                        adaptive_trigger_state.as_ref(),
                                    );
                                    // #5694: surface accepted suggestions — overlay
                                    // auto-refresh + High+ toast (focus-gated inside).
                                    let event_on_changed = overlay_ref.as_ref().map(|o| {
                                        let o = o.clone();
                                        move |c: usize| o.emit_suggestions_changed(c)
                                    });
                                    let event_on_changed_ref: Option<&(dyn Fn(usize) + Send + Sync)> =
                                        event_on_changed
                                            .as_ref()
                                            .map(|f| f as &(dyn Fn(usize) + Send + Sync));
                                    handle_event_analysis(
                                        &context_analyzer_now,
                                        &storage1,
                                        &app_name,
                                        &focus_window_title,
                                        focus_ocr_hint.as_deref(),
                                        event_queue_for_push,
                                        event_gates,
                                        event_on_changed_ref,
                                        notif1.as_ref(),
                                        focus_mode.is_active(),
                                    ).await;
                                }

                                // ── Take input snapshot once for both pipelines ──
                                let input_snap = monitor_input_snapshot(
                                    &input_collector,
                                    consent.input_activity,
                                );

                                // ── Adaptive tiered-memory pipeline ──
                                // Feed GUI summary from the previous cycle (N-1) into
                                // the current analysis tick (N).
                                if let Some(ref mut ts) = adaptive_trigger_state {
                                    // Live PII level so the source-masked content label
                                    // (review4 F6) tracks runtime privacy-level changes.
                                    let content_pii_level = config_snapshot
                                        .as_ref()
                                        .map(|cfg| cfg.privacy.pii_filter_level)
                                        .unwrap_or_default();
                                    super::super::analysis_pipeline::run_analysis_tick(
                                        ts,
                                        &app_name,
                                        &focus_window_title,
                                        &prev_app,
                                        app_changed,
                                        &input_snap,
                                        last_gui_summary.as_ref(),
                                        last_focused_element.as_ref(),
                                        &storage1,
                                        content_pii_level,
                                    ).await;
                                }
                                // Update ContextAnalyzer with current segment stats
                                // so that analyze() includes segment context in LLM prompts.
                                if let (Some(ref ts), Some(ref analyzer)) = (&adaptive_trigger_state, &context_analyzer_now) {
                                    let stats = build_segment_stats_snapshot(ts);
                                    analyzer.set_segment_stats(stats).await;
                                }
                                // Update accessibility text for LLM context enrichment.
                                // Scrub argument-borne secrets (API keys, bearer tokens,
                                // passwords, connection-string userinfo) before this raw
                                // extracted text reaches the LLM payload: the PII floor
                                // masks email/phone but NOT secrets at the Basic level
                                // where accessibility text is exposed. (review4 F4 sibling)
                                if let Some(ref analyzer) = context_analyzer_now {
                                    let a11y_text = last_focused_element.as_ref()
                                        .and_then(|fe| fe.extracted_text.clone())
                                        .map(|t| maekon_analysis::terminal_detector::scrub_text_secrets(&t));
                                    analyzer.set_accessibility_text(a11y_text).await;
                                }

                                // Write current regime state for cross-loop sharing (C1).
                                // E20-26 (#4818): resolve the full owned `Regime` from the
                                // regime manager so regime-aware consumers (local-suggestion
                                // filter, coaching) can read name/auto_label. `None` when no
                                // regime is classified — consumers pass suggestions through.
                                let current_regime_owned = adaptive_trigger_state.as_ref().and_then(|ts| {
                                    ts.current_regime_id.as_deref().and_then(|id| {
                                        let mgr = ts.regime_manager.lock();
                                        let regimes = mgr.all_regimes();
                                        // Prefer the Active entry for this id (review4 F3
                                        // defense-in-depth): under a duplicate-id condition a
                                        // stale Inactive entry must not publish the wrong
                                        // regime label/centroid into SharedRegimeState.
                                        regimes
                                            .iter()
                                            .find(|r| {
                                                r.regime_id == id
                                                    && r.status
                                                        == maekon_core::models::tiered_memory::RegimeStatus::Active
                                            })
                                            .or_else(|| regimes.iter().find(|r| r.regime_id == id))
                                            .cloned()
                                    })
                                });
                                shared_regime.update(current_regime_owned.as_ref(), &app_name);

                                // Consume the GUI summary after feeding it to the analysis pipeline
                                last_gui_summary = None;

                                // ── GUI Activity Intelligence pipeline ──
                                if let Some(ref mut ts) = adaptive_trigger_state {
                                    if let Some(ref mut gui_state) = ts.gui_pipeline_state {
                                        let parsed_content_label = ts
                                            .title_bar_parser
                                            .parse(&app_name, &focus_window_title)
                                            .map(|c| c.content_label)
                                            .unwrap_or_default();

                                        let recent_shortcuts = monitor_recent_shortcuts(
                                            &input_collector,
                                            consent.input_activity,
                                        );

                                        let (fs, fw, fh) = last_frame_rgba.as_ref().map_or((None, 0, 0), |(r, w, h)| (Some(r.as_slice()), *w, *h));
                                        let gui_summary = super::super::gui_pipeline::run_gui_tick(
                                            gui_state, &last_ocr_regions, &input_snap, &recent_shortcuts,
                                            &app_name, &focus_window_title, &parsed_content_label,
                                            last_focused_element.as_ref(), fs, fw, fh,
                                        ).await;

                                        if gui_summary.is_some() {
                                            last_gui_summary = gui_summary;
                                        }

                                        // Persist GUI interaction to SQLite (V13 table)
                                        if input_snap.mouse.click_count > 0 {
                                            // F-PF-19: save_gui_interaction is a sync SQLite write, so offload it via
                                            // spawn_blocking to avoid blocking the tokio worker thread.
                                            // Capture NewGuiInteraction's lifetime fields as owned Strings.
                                            let event_id = uuid::Uuid::new_v4().to_string();
                                            let timestamp_str = chrono::Utc::now().to_rfc3339();
                                            let app_name_owned = app_name.clone();
                                            let sqlite_gui = sqlite1.clone();

                                            tokio::task::spawn_blocking(move || {
                                                let input = maekon_core::models::storage_records::NewGuiInteraction {
                                                    event_id: &event_id,
                                                    timestamp: &timestamp_str,
                                                    interaction_type: "Click",
                                                    app_name: &app_name_owned,
                                                    type_confidence: 1.0,
                                                };
                                                if let Err(e) = sqlite_gui.save_gui_interaction(&input) {
                                                    tracing::warn!("GUI interaction save failure: {e}");
                                                }
                                            });
                                        }

                                        // LLM feedback: process uncertain GUI elements periodically
                                        // #4798: in battery-saver mode, skip GUI-LLM feedback (a remote LLM call).
                                        gui_state.feedback_tick_counter += 1;
                                        if !skip_expensive && gui_state.feedback_tick_counter >= 30 && !gui_state.uncertain_queue.is_empty() {
                                            gui_state.feedback_tick_counter = 0;
                                            if let Some(ref p) = coaching_analysis_provider {
                                                super::super::gui_pipeline::process_gui_feedback(gui_state, p.as_ref(), gui_feedback_pii_san.as_ref(), gui_feedback_pii_level(&config_manager1)).await;
                                            }
                                        }
                                    }
                                }

                                // ── Heatmap aggregation + goal progress ──
                                emit_heatmap_and_goals(
                                    &mut adaptive_trigger_state,
                                    &input_snap,
                                    &overlay_ref,
                                    &coaching_engine_ref,
                                ).await;
                                let indicator_visible_for_tick = app_handle
                                    .as_ref()
                                    .and_then(|app| {
                                        app.try_state::<crate::runtime_state::AppState>()
                                    })
                                    .map(|state| {
                                        state.indicator_visible.load(
                                            std::sync::atomic::Ordering::Relaxed,
                                        )
                                    })
                                    .unwrap_or(true);
                                emit_pointer_context_highlight(
                                    &mut pointer_context_state,
                                    &input_snap,
                                    &overlay_ref,
                                    paused,
                                    indicator_visible_for_tick,
                                );

                                // ── Coaching evaluation (Phase 1) ──
                                // A4: Skip coaching when focus mode active
                                if !focus_mode.is_active() {
                                if let Some(ref coaching) = coaching_engine_ref {
                                    let regime_id_for_coaching: Option<&str> =
                                        adaptive_trigger_state.as_ref().and_then(|ts| {
                                            ts.current_regime_id.as_deref()
                                        });
                                    // #7480: coaching consumers key on the HUMAN
                                    // regime label (name > auto_label), not the
                                    // opaque positional id ("regime-N"). Derive it
                                    // from the regime already resolved above for
                                    // SharedRegimeState so profile matching, goal +
                                    // habit tracking and the LLM personalization
                                    // prompt see a semantic label. `None` when no
                                    // regime is classified this tick.
                                    let regime_label_for_coaching: Option<&str> =
                                        current_regime_owned.as_ref().map(|r| {
                                            r.name.as_deref().unwrap_or(r.auto_label.as_str())
                                        });
                                    let drift_detected = adaptive_trigger_state
                                        .as_ref()
                                        .map(|ts| ts.last_drift_detected.swap(false, std::sync::atomic::Ordering::Relaxed))
                                        .unwrap_or(false);

                                    let ctx = CoachingEvalContext {
                                        coaching_engine: coaching,
                                        overlay: &overlay_ref,
                                        notifier: &notif1,
                                        coaching_storage: &coaching_storage_ref,
                                        scheduler_storage: &sqlite1,
                                        analysis_provider: &coaching_analysis_provider,
                                        regime_id: regime_id_for_coaching,
                                        regime_label: regime_label_for_coaching,
                                        prev_app: prev_app.as_deref(),
                                        drift_detected,
                                        poll_secs: poll.as_secs(),
                                        pii_sanitizer: &coaching_pii_sanitizer,
                                        pii_level: super::coaching_helper::resolve_pii_level(config_snapshot.as_deref()),
                                    };
                                    super::coaching_helper::evaluate_and_deliver(&ctx, &mut coaching_tick_state).await;
                                }
                                } // end A4: focus_mode coaching guard

                                // ── Detection overlay: re-analyze on window change ──
                                // #7909: analyze_scene captures the current screen, so it is
                                // gated like every other capture surface of this tick.
                                let title_changed = prev_window_title.as_ref() != Some(&focus_window_title);
                                if !capture_excluded {
                                    let scene_finder_ref = scene_finder_slot
                                        .as_ref()
                                        .and_then(|slot| slot.get().cloned());
                                    super::detection_helper::maybe_reanalyze_detection(
                                        &detection_active, app_changed, title_changed,
                                        &scene_finder_ref, &overlay_ref,
                                    );
                                }

                                super::vision_helper::log_ring_buffer_evictions(&ring_buffer);

                                // ── Periodic frame retention enforcement ──
                                if last_retention_check.elapsed() >= super::helpers::FRAME_RETENTION_INTERVAL {
                                    last_retention_check = Instant::now();
                                    if let Some(ref fs) = frame_storage1 {
                                        super::helpers::enforce_frame_retention(fs.as_ref()).await;
                                    }
                                }

                                prev_window_title = Some(focus_window_title);
                                prev_app = Some(app_name);
                            }
                            Err(e) => {
                                warn!("context collect failure: {e}");
                            }
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        info!("monitoring ended");
                        break;
                    }
                }
            }
        })
    }
}

fn empty_monitor_input_snapshot() -> InputActivityEvent {
    InputActivityEvent {
        timestamp: Utc::now(),
        period_secs: 1,
        mouse: MouseActivity::default(),
        keyboard: KeyboardActivity::default(),
        app_name: String::new(),
        keystroke_profile: None,
    }
}

fn monitor_input_snapshot(
    input_collector: &InputActivityCollector,
    input_activity_allowed: bool,
) -> InputActivityEvent {
    if input_activity_allowed {
        input_collector.take_snapshot()
    } else {
        empty_monitor_input_snapshot()
    }
}

fn monitor_recent_shortcuts(
    input_collector: &InputActivityCollector,
    input_activity_allowed: bool,
) -> Vec<String> {
    if input_activity_allowed {
        input_collector.take_recent_shortcuts()
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitor_input_helpers_do_not_drain_without_input_activity_consent() {
        let collector = InputActivityCollector::new();
        collector.record_click();
        collector.record_scroll();
        collector.record_shortcut_name("Cmd+S");

        let blocked_snapshot = monitor_input_snapshot(&collector, false);
        let blocked_shortcuts = monitor_recent_shortcuts(&collector, false);

        assert_eq!(blocked_snapshot.mouse.click_count, 0);
        assert_eq!(blocked_snapshot.mouse.scroll_count, 0);
        assert_eq!(blocked_snapshot.keyboard.total_keystrokes, 0);
        assert!(blocked_shortcuts.is_empty());

        let retained_snapshot = collector.take_snapshot();
        let retained_shortcuts = collector.take_recent_shortcuts();

        assert_eq!(retained_snapshot.mouse.click_count, 1);
        assert_eq!(retained_snapshot.mouse.scroll_count, 1);
        assert_eq!(retained_snapshot.keyboard.total_keystrokes, 1);
        assert_eq!(retained_shortcuts, vec!["Cmd+S".to_string()]);
    }

    #[test]
    fn monitor_input_helpers_drain_when_input_activity_consent_is_granted() {
        let collector = InputActivityCollector::new();
        collector.record_click();
        collector.record_shortcut_name("Cmd+S");

        let allowed_snapshot = monitor_input_snapshot(&collector, true);
        let allowed_shortcuts = monitor_recent_shortcuts(&collector, true);

        assert_eq!(allowed_snapshot.mouse.click_count, 1);
        assert_eq!(allowed_snapshot.keyboard.total_keystrokes, 1);
        assert_eq!(allowed_shortcuts, vec!["Cmd+S".to_string()]);

        let drained_snapshot = collector.take_snapshot();
        let drained_shortcuts = collector.take_recent_shortcuts();

        assert_eq!(drained_snapshot.mouse.click_count, 0);
        assert_eq!(drained_snapshot.keyboard.total_keystrokes, 0);
        assert!(drained_shortcuts.is_empty());
    }
}
