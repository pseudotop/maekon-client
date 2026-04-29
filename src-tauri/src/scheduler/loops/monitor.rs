use chrono::Utc;
use oneshim_core::models::event::Event;
use oneshim_core::models::focused_element::AccessibilityElement;
use oneshim_core::models::frame::OcrRegion;
use oneshim_monitor::idle::IdleTracker;
use oneshim_monitor::input_activity::InputActivityCollector;
use oneshim_monitor::window_layout::WindowLayoutTracker;
use oneshim_vision::ring_buffer::{CaptureRingBuffer, RingFrame};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

use super::super::config::PlatformEgressPolicy;
use super::super::gui_pipeline::gui_feedback_pii_level;
use super::super::shared_regime_state::SharedRegimeState;
use super::super::Scheduler;
use super::coaching_helper::{CoachingEvalContext, CoachingTickState};
use super::helpers::{
    audit_consent_and_pii_changes, build_segment_stats_snapshot, emit_heatmap_and_goals,
    handle_event_analysis, handle_frame_capture, handle_idle_tick,
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
        let context_analyzer1 = self.context_analyzer.clone();
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
        let scene_finder_ref = self.scene_finder.clone();
        let event_tx_mon = self.event_tx.clone();

        tokio::spawn(async move {
            let mut prev_app: Option<String> = None;
            let mut prev_window_title: Option<String> = None;
            let mut prev_idle_secs: u64 = 0;
            let mut interval = tokio::time::interval(poll);
            let mut focus_block = super::autostart_helper::FocusBlockState::default();
            let mut idle_tracker = IdleTracker::new(Some(idle_threshold));
            let mut adaptive_trigger_state = adaptive_trigger_state;
            let window_tracker = WindowLayoutTracker::new();
            let input_collector = input_collector1;
            let ring_buffer = CaptureRingBuffer::new(6, 2, 0.5); // dashcam: 6 slots, 2 post-event, 0.5 threshold

            // GUI Activity Intelligence state (carried across ticks)
            use oneshim_core::models::focused_element::FocusedElementInfo;
            use oneshim_core::models::gui_activity::GuiActivitySummary;
            let mut last_gui_summary: Option<GuiActivitySummary> = None;
            let mut last_focused_element: Option<FocusedElementInfo> = None;
            let mut last_ocr_regions: Vec<OcrRegion> = Vec::new();
            let mut last_frame_rgba: Option<(Vec<u8>, u32, u32)> = None;
            let mut focus_hl = super::detection_helper::FocusHighlightState::new();
            let mut coaching_tick_state = CoachingTickState::new();
            let mut last_retention_check = Instant::now();
            let mut prev_full_text_consent = false;
            let mut prev_pii_level = config_manager1
                .as_ref()
                .map(|cm| cm.get().analysis.text_intelligence.pii_extraction_level)
                .unwrap_or_default();
            let mut ts_notify_state = (false, None::<Instant>); // A.18: (prev_active, last_notified)

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        // A4: Focus mode auto-expiry check
                        if focus_mode.check_expiry() {
                            if let Some(ref overlay) = overlay_ref {
                                overlay.emit_focus_mode(false, false);
                            }
                            info!("Focus mode expired — auto-deactivated");
                        }
                        let new_idle_secs = handle_idle_tick(
                            &mut idle_tracker,
                            &sqlite1,
                            &notif1,
                            &input_collector,
                            prev_idle_secs,
                            focus_mode.is_active(),
                            &event_tx_mon,  // reuse clone added by B3-1
                        ).await;

                        // A.18: TS window enter/exit → desktop notify (60s debounce)
                        super::tracking_schedule_helper::tick_ts_notifications(&config_manager1, notif1.as_deref(), &mut ts_notify_state.0, &mut ts_notify_state.1).await;

                        // PR-B1 §5.5: productive-session detection (Idle↔Active transitions, idempotent counter)
                        focus_block.tick(&mut prev_idle_secs, new_idle_secs, idle_threshold, app_handle.as_ref(), config_manager1.as_ref());
                        match act_mon.collect_context().await {
                            Ok(ctx) => {
                                let app_name = ctx.active_window.as_ref()
                                    .map(|w| w.app_name.clone())
                                    .unwrap_or_default();
                                let window_title = ctx.active_window.as_ref()
                                    .map(|w| w.title.clone())
                                    .unwrap_or_default();
                                let focus_window_title = window_title.clone();
                                let window_bounds = ctx.active_window.as_ref()
                                    .and_then(|w| w.bounds);
                                let mut focus_ocr_hint: Option<String> = None;

                                input_collector.set_current_app(&app_name);
                                if let Some(ref cm) = config_manager1 { super::focus_auto_helper::evaluate_focus_auto(&cm.get().focus_auto, &focus_mode, &app_name, overlay_ref.as_ref()); }

                                // ── Accessibility API extraction (Phase 2) ──
                                // Extract focused element info per tick when enabled.
                                // Result is stored for the GUI pipeline to consume.
                                if let Some(ref ax) = accessibility_extractor1 {
                                    let text_config = config_manager1
                                        .as_ref()
                                        .map(|cm| cm.get().analysis.text_intelligence.clone())
                                        .unwrap_or_default();
                                    let full_text_consent = consent_manager1
                                        .as_ref()
                                        .map(|cm| cm.is_permitted(|p| p.full_text_extraction))
                                        .unwrap_or(false);

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
                                        if let Some(upload_event) = egress1.prepare_event_for_upload(win_event) {
                                            sink.enqueue(upload_event);
                                        }
                                    }
                                }

                                let event = oneshim_core::models::event::ContextEvent {
                                    app_name: app_name.clone(),
                                    window_title,
                                    prev_app_name: prev_app.clone(),
                                    timestamp: Utc::now(),
                                    input_activity_level: input_collector.peek_activity_level(),
                                };

                                // Gate: consent · active_hours · tracking-schedule · tray-pause (A.7)
                                let consent = consent_manager1.as_ref()
                                    .and_then(|cm| cm.current_consent().map(|r| r.permissions.clone()))
                                    .unwrap_or_default();
                                let paused = capture_paused.load(std::sync::atomic::Ordering::Relaxed);
                                let permitted = config_manager1.as_ref()
                                    .map(|cm| crate::scheduler::capture_permitted_now(&cm.snapshot(), &consent, paused))
                                    .unwrap_or(!paused);
                                if permitted {
                                // --- Ring buffer: capture thumbnail every cycle ---
                                if let Ok(thumb_data) = processor.capture_thumbnail().await {
                                    ring_buffer.push(RingFrame {
                                        timestamp: Utc::now(),
                                        thumbnail_data: thumb_data,
                                        app_name: app_name.clone(),
                                        window_title: event.window_title.clone(),
                                        accessibility_elements: last_focused_element
                                            .as_ref()
                                            .map(|f| {
                                                vec![AccessibilityElement {
                                                    role: f.role.clone(),
                                                    label: f.label.clone().unwrap_or_default(),
                                                    bounds: f.position,
                                                }]
                                            })
                                            .unwrap_or_default(),
                                    });
                                }

                                {
                                    let capture_req = trigger.should_capture(&event);

                                    // Force capture during post-event window (dashcam "after" frames)
                                    let force_post = ring_buffer.should_force_post_capture();

                                    // A4: Elevate capture threshold in focus mode —
                                    // only process captures with importance >= 0.7
                                    let focus_threshold: f32 = if focus_mode.is_active() { 0.7 } else { 0.0 };

                                    if let Some(mut capture_req) = capture_req.filter(|r| r.importance >= focus_threshold) {
                                        // Inject active window bounds so the frame processor
                                        // captures the correct monitor in multi-monitor setups.
                                        capture_req.window_bounds = window_bounds;

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
                                        let capture_pii = config_manager1.as_ref().map(|cm| cm.get().privacy.pii_filter_level).unwrap_or_default();
                                        let (ocr_hint, regions, frame_rgba) = handle_frame_capture(&capture_req, &processor, &frame_storage1, &sqlite1, &session1, capture_pii, &event_tx_mon).await;
                                        focus_ocr_hint = ocr_hint;
                                        if !regions.is_empty() {
                                            last_ocr_regions = regions;
                                            last_frame_rgba = frame_rgba;
                                        } else {
                                            last_frame_rgba = None;
                                        }
                                    } else if force_post {
                                        // Post-event forced capture (dashcam "after" frames)
                                        if let Some(ref fs) = frame_storage1 {
                                            if let Ok(thumb_data) = processor.capture_thumbnail().await {
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
                                    if let Some(upload_event) = egress1.prepare_event_for_upload(ctx_event) {
                                        sink.enqueue(upload_event);
                                    }
                                }
                                } // end capture gate

                                let app_changed = prev_app.as_ref() != Some(&app_name);
                                if app_changed {
                                    if let Some(ref focus) = focus1 {
                                        focus
                                            .on_app_switch_with_context(
                                                &app_name,
                                                &focus_window_title,
                                                focus_ocr_hint.as_deref(),
                                            )
                                            .await;
                                    }

                                    // Event-driven LLM analysis on significant app switches
                                    handle_event_analysis(
                                        &context_analyzer1,
                                        &storage1,
                                        &app_name,
                                        &focus_window_title,
                                        focus_ocr_hint.as_deref(),
                                    ).await;
                                }

                                // ── Take input snapshot once for both pipelines ──
                                let input_snap = input_collector.take_snapshot();

                                // ── Adaptive tiered-memory pipeline ──
                                // Feed GUI summary from the previous cycle (N-1) into
                                // the current analysis tick (N).
                                if let Some(ref mut ts) = adaptive_trigger_state {
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
                                    ).await;
                                }
                                // Update ContextAnalyzer with current segment stats
                                // so that analyze() includes segment context in LLM prompts.
                                if let (Some(ref ts), Some(ref analyzer)) = (&adaptive_trigger_state, &context_analyzer1) {
                                    let stats = build_segment_stats_snapshot(ts);
                                    analyzer.set_segment_stats(stats).await;
                                }
                                // Update accessibility text for LLM context enrichment
                                if let Some(ref analyzer) = context_analyzer1 {
                                    let a11y_text = last_focused_element.as_ref()
                                        .and_then(|fe| fe.extracted_text.clone());
                                    analyzer.set_accessibility_text(a11y_text).await;
                                }

                                // Write current regime state for cross-loop sharing (C1)
                                shared_regime.update(
                                    adaptive_trigger_state.as_ref()
                                        .and_then(|ts| ts.current_regime_id.as_deref()),
                                    None, // regime_label populated by regime_manager if available
                                    &app_name,
                                );

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

                                        let recent_shortcuts = input_collector.take_recent_shortcuts();

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
                                            let event_id = uuid::Uuid::new_v4().to_string();
                                            let timestamp_str = chrono::Utc::now().to_rfc3339();

                                            let input = oneshim_core::models::storage_records::NewGuiInteraction {
                                                event_id: &event_id,
                                                segment_id: None,
                                                timestamp: &timestamp_str,
                                                element_text: None,
                                                element_type: Some("Click"),
                                                interaction_type: "Click",
                                                bbox_json: None,
                                                app_name: &app_name,
                                                type_confidence: 1.0,
                                            };

                                            if let Err(e) = sqlite1.save_gui_interaction(&input) {
                                                warn!("GUI interaction save failure: {e}");
                                            }
                                        }

                                        // LLM feedback: process uncertain GUI elements periodically
                                        gui_state.feedback_tick_counter += 1;
                                        if gui_state.feedback_tick_counter >= 30 && !gui_state.uncertain_queue.is_empty() {
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

                                // ── Coaching evaluation (Phase 1) ──
                                // A4: Skip coaching when focus mode active
                                if !focus_mode.is_active() {
                                if let Some(ref coaching) = coaching_engine_ref {
                                    let regime_id_for_coaching: Option<&str> =
                                        adaptive_trigger_state.as_ref().and_then(|ts| {
                                            ts.current_regime_id.as_deref()
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
                                        analysis_provider: &coaching_analysis_provider,
                                        regime_id: regime_id_for_coaching,
                                        prev_app: prev_app.as_deref(),
                                        drift_detected,
                                        poll_secs: poll.as_secs(),
                                        pii_sanitizer: &coaching_pii_sanitizer,
                                        pii_level: super::coaching_helper::resolve_pii_level(&config_manager1),
                                    };
                                    super::coaching_helper::evaluate_and_deliver(&ctx, &mut coaching_tick_state).await;
                                }
                                } // end A4: focus_mode coaching guard

                                // ── Detection overlay: re-analyze on window change ──
                                let title_changed = prev_window_title.as_ref() != Some(&focus_window_title);
                                super::detection_helper::maybe_reanalyze_detection(
                                    &detection_active, app_changed, title_changed,
                                    &scene_finder_ref, &overlay_ref,
                                );

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
