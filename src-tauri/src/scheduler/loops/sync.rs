use chrono::Utc;
use oneshim_monitor::input_activity::InputActivityCollector;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, info, warn};

use super::super::config::PlatformEgressPolicy;
use super::super::shared_regime_state::SharedRegimeState;
use super::super::Scheduler;

impl Scheduler {
    /// Periodically check and refresh OAuth tokens.
    #[tracing::instrument(skip_all)]
    #[cfg(feature = "server")]
    pub(in crate::scheduler) fn spawn_oauth_refresh_loop(
        &self,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
        app_handle: Option<tauri::AppHandle>,
    ) -> Option<tokio::task::JoinHandle<()>> {
        use super::super::config::OAUTH_REFRESH_INTERVAL_SECS;
        use oneshim_core::ports::oauth::TokenEvent;
        use std::time::Duration;
        use tauri::Emitter;

        let coordinator = self.oauth_coordinator.as_ref()?.clone();

        Some(tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(Duration::from_secs(OAUTH_REFRESH_INTERVAL_SECS));
            let mut event_rx = coordinator.subscribe();
            let mut last_reauth_notify: Option<tokio::time::Instant> = None;
            let provider_id = "openai".to_string();

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let outcome = coordinator.check_and_refresh(&provider_id).await;
                        debug!(provider_id = %provider_id, ?outcome, "OAuth refresh tick");
                    }
                    event = event_rx.recv() => {
                        if let Ok(TokenEvent::ReauthRequired { ref provider_id }) = event {
                            let should_notify = last_reauth_notify
                                .map_or(true, |t| t.elapsed() > Duration::from_secs(300));
                            if should_notify {
                                warn!(
                                    provider_id = %provider_id,
                                    "OAuth re-authentication required — user must reconnect"
                                );
                                last_reauth_notify = Some(tokio::time::Instant::now());

                                // Emit Tauri event for frontend toast
                                if let Some(ref handle) = app_handle {
                                    let payload = serde_json::json!({
                                        "provider_id": provider_id,
                                    });
                                    if let Err(e) = handle.emit("oauth-reauth-required", &payload) {
                                        warn!("Failed to emit oauth-reauth-required event: {e}");
                                    }

                                    // Native OS notification for background/minimized state.
                                    // Body is English-only: i18n is frontend-side; Rust has no locale context.
                                    if let Err(e) = tauri_plugin_notification::NotificationExt::notification(handle)
                                        .builder()
                                        .title("Maekon")
                                        .body("OAuth re-authentication required — please reconnect in Settings")
                                        .show()
                                    {
                                        warn!("Failed to show native notification: {e}");
                                    }
                                }
                            }
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        debug!("OAuth refresh loop shutting down");
                        break;
                    }
                }
            }
        }))
    }

    /// 12. Cross-device sync loop (P3 Phase 3a-2).
    ///
    /// Runs the SyncEngine's pull/merge/push cycle at the configured interval.
    #[tracing::instrument(skip_all)]
    pub(in crate::scheduler) fn spawn_cross_device_sync_loop(
        &self,
        sync_interval: Duration,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        let sync_engine = self.sync_engine.clone();
        // D13: 4-term privacy gate DI (row 11 — cross-device sync is gated).
        let config_mgr_s = self.config_manager.clone();
        let consent_mgr_s = self.consent_manager.clone();
        let capture_paused_s = self.capture_paused.clone();

        tokio::spawn(async move {
            let engine = match sync_engine {
                Some(e) => e,
                None => {
                    let _ = shutdown_rx.changed().await;
                    return;
                }
            };

            // Startup delay: wait 10 seconds before first sync
            tokio::time::sleep(Duration::from_secs(10)).await;

            let mut interval = tokio::time::interval(sync_interval);

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        // D13: 4-term composite gate (CONS-PC02 / §3.3 A.9).
                        let consent = consent_mgr_s.as_ref()
                            .and_then(|cm| cm.current_consent().map(|r| r.permissions.clone()))
                            .unwrap_or_default();
                        let paused = capture_paused_s.load(Ordering::Relaxed);
                        let permitted = config_mgr_s.as_ref()
                            .map(|cm| crate::scheduler::capture_permitted_now(&cm.snapshot(), &consent, paused))
                            .unwrap_or(!paused);
                        if !permitted {
                            debug!("cross-device sync: capture gate closed (TS/consent/paused) — skipping tick");
                            continue;
                        }
                        match engine.run_cycle().await {
                            Ok(Some(result)) => {
                                info!(
                                    applied = result.applied,
                                    skipped = result.skipped_lww + result.skipped_dup,
                                    "cross-device sync cycle completed"
                                );
                            }
                            Ok(None) => {
                                debug!("cross-device sync cycle: no changes or skipped");
                            }
                            Err(e) => {
                                warn!(err.code = %e.code(), "cross-device sync cycle failed: {e}");
                            }
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        // Push pending changes before shutdown
                        if let Err(e) = engine.run_cycle().await {
                            warn!(err.code = %e.code(), "shutdown sync push failed: {e}");
                        }
                        info!("cross-device sync loop ended");
                        break;
                    }
                }
            }
        })
    }

    #[tracing::instrument(skip_all)]
    #[allow(unused_variables)]
    pub(in crate::scheduler) async fn run_scheduler_loops(
        &self,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
        app_handle: Option<tauri::AppHandle>,
    ) {
        let poll = self.config.poll_interval;
        let metrics_interval = self.config.metrics_interval;
        let process_interval = self.config.process_interval;
        let detailed_process_interval = self.config.detailed_process_interval;
        let input_activity_interval = self.config.input_activity_interval;
        let sync = self.config.sync_interval;
        let heartbeat = self.config.heartbeat_interval;
        let aggregation = self.config.aggregation_interval;
        let session_id = self.config.session_id.clone();
        let idle_threshold = self.config.idle_threshold_secs;
        let egress_policy = Arc::new(PlatformEgressPolicy::new(&self.config));

        info!(
            platform_sync_enabled = egress_policy.is_enabled(),
            "플랫폼 egress policy 적용"
        );

        self.initialize_session(&session_id).await;

        let shared_input_collector = Arc::new(InputActivityCollector::new());

        // -- Phase 1.5: Platform key-category hook --
        // Spawns a passive OS keyboard observer that classifies key events
        // into KeyCategory and feeds them into InputActivityCollector.
        // Gated by text_intelligence.input_pattern_detail config flag.
        let _key_hook = {
            let text_intel_config = self
                .config_manager
                .as_ref()
                .map(|cm| cm.get().analysis.text_intelligence.clone())
                .unwrap_or_default();

            if text_intel_config.enabled && text_intel_config.input_pattern_detail {
                oneshim_monitor::key_hook::KeyHook::start(shared_input_collector.clone())
            } else {
                debug!(
                    "key-category hook disabled \
                     (text_intelligence.input_pattern_detail = false \
                     or text_intelligence.enabled = false)"
                );
                None
            }
        };

        // Take adaptive trigger state out of Mutex — it is consumed by the
        // monitor loop and cannot be shared.
        let mut adaptive_trigger_state = self
            .adaptive_trigger
            .lock()
            .unwrap_or_else(|poisoned| {
                warn!("adaptive trigger lock poisoned — recovering inner data");
                poisoned.into_inner()
            })
            .take();

        // Clone the LLM summarizer Arc (if present) before the adaptive trigger
        // state is moved into the monitor loop. The aggregation loop uses this to
        // generate LLM narratives for daily digests.
        let llm_summarizer_for_digest = adaptive_trigger_state
            .as_ref()
            .and_then(|ts| ts.llm_summarizer.clone());

        // Construct GUI pipeline state if enabled + consented
        if let Some(ref mut ts) = adaptive_trigger_state {
            let gui_config = self
                .config_manager
                .as_ref()
                .map(|cm| cm.get().analysis.gui_intelligence.clone())
                .unwrap_or_default();

            // Consent is implicitly satisfied: AdaptiveTriggerState is only
            // constructed when the activity_pattern_learning consent has been
            // granted (agent_runtime.rs gates on that permission). The only
            // remaining gate is the gui_intelligence.enabled config flag.
            if gui_config.enabled {
                use oneshim_analysis::gui_aggregator::GuiActivityAggregator;
                use oneshim_vision::gui_detector::GuiElementDetector;

                use super::super::gui_pipeline::GuiPipelineState;

                let detector = GuiElementDetector::new(
                    (1920, 1080), // sensible default; updated per tick from WindowLayoutEvent
                    oneshim_core::config::PiiFilterLevel::Standard,
                );

                // Default: CV-based contour classifier (always available, no model file needed)
                let detector = detector.with_ml_classifier(std::sync::Arc::new(
                    oneshim_vision::contour_classifier::ContourGuiClassifier::new(),
                ));

                // Override with ONNX ML classifier when feature is enabled and model exists
                #[cfg(feature = "ml-detect")]
                let detector = {
                    use oneshim_vision::ml_classifier::OnnxGuiClassifier;

                    let model_path = if gui_config.ml_model_path.is_empty() {
                        match oneshim_core::config_manager::ConfigManager::data_dir() {
                            Ok(dir) => dir.join("models").join("gui-classifier.onnx"),
                            Err(e) => {
                                warn!("Cannot resolve data_dir for ML model: {e}");
                                std::path::PathBuf::from("gui-classifier.onnx")
                            }
                        }
                    } else {
                        std::path::PathBuf::from(&gui_config.ml_model_path)
                    };

                    match OnnxGuiClassifier::load(&model_path) {
                        Ok(Some(classifier)) => {
                            info!("GUI ML classifier loaded: {}", model_path.display());
                            detector.with_ml_classifier(std::sync::Arc::new(classifier))
                        }
                        Ok(None) => detector,
                        Err(e) => {
                            warn!("GUI ML classifier load failed: {e}");
                            detector
                        }
                    }
                };

                let aggregator = GuiActivityAggregator::new(&gui_config);
                ts.gui_pipeline_state = Some(GuiPipelineState {
                    detector,
                    aggregator,
                    uncertain_queue: std::collections::VecDeque::new(),
                    feedback_tick_counter: 0,
                    app_type_cache: std::collections::HashMap::new(),
                });
                info!("GUI Activity Intelligence pipeline enabled");
            }
        }

        // Shared regime state for cross-loop communication (C1):
        // monitor loop writes, coaching loop reads.
        // Uses the injected instance (shared with SessionManager) or creates a local fallback.
        let shared_regime = self
            .shared_regime
            .clone()
            .unwrap_or_else(|| Arc::new(SharedRegimeState::new()));

        let monitor_task = self.spawn_monitor_loop(
            poll,
            idle_threshold,
            session_id.clone(),
            egress_policy.clone(),
            shared_input_collector.clone(),
            adaptive_trigger_state,
            shared_regime.clone(),
            self.focus_mode.clone(),
            shutdown_rx.clone(),
            app_handle.clone(),
        );

        let metrics_task = self.spawn_metrics_loop(metrics_interval, shutdown_rx.clone());

        let process_task = self.spawn_process_loop(process_interval, shutdown_rx.clone());

        let sync_task = self.spawn_sync_loop(sync, egress_policy.clone(), shutdown_rx.clone());

        let heartbeat_task = self.spawn_heartbeat_loop(
            heartbeat,
            session_id.clone(),
            egress_policy.clone(),
            shutdown_rx.clone(),
        );

        let aggregation_task = self.spawn_aggregation_loop(
            aggregation,
            llm_summarizer_for_digest,
            shutdown_rx.clone(),
        );

        let notification_task =
            self.spawn_notification_loop(self.focus_mode.clone(), shutdown_rx.clone());

        let focus_task = self.spawn_focus_loop(shutdown_rx.clone());

        let event_snapshot_task = self.spawn_event_snapshot_loop(
            detailed_process_interval,
            input_activity_interval,
            egress_policy.clone(),
            shared_input_collector.clone(),
            shutdown_rx.clone(),
        );

        // 10. OAuth token refresh (conditional — returns None if no coordinator)
        #[cfg(feature = "server")]
        let oauth_task = self.spawn_oauth_refresh_loop(shutdown_rx.clone(), app_handle);

        // 11. LLM analysis loop (periodic + change-detection)
        let analysis_config = self.config.analysis_config.clone();
        let analysis_task = self.spawn_analysis_loop(analysis_config, shutdown_rx.clone());

        // 12. Cross-device sync loop (P3 Phase 3a-2)
        let cross_device_sync_task = self.spawn_cross_device_sync_loop(
            self.config.cross_device_sync_interval,
            shutdown_rx.clone(),
        );

        // 13. Coaching feedback evaluation loop
        let coaching_task = self.spawn_coaching_loop(shared_regime.clone(), shutdown_rx.clone());

        // 14. Health check loop — reads adapter health flags and updates connection flags
        let health_task = if let (
            Some(s_flag),
            Some(l_flag),
            Some(c_flag),
            Some(s_conn),
            Some(l_conn),
            Some(c_conn),
            Some(handle),
        ) = (
            self.server_health_flag.clone(),
            self.llm_health_flag.clone(),
            self.cli_health_flag.clone(),
            self.server_connected.clone(),
            self.llm_connected.clone(),
            self.cli_connected.clone(),
            self.tray_app_handle.clone(),
        ) {
            Some(super::health::spawn_health_check_loop(
                std::time::Duration::from_secs(5),
                super::health::AdapterHealthFlags {
                    server_ok: s_flag,
                    llm_ok: l_flag,
                    cli_ok: c_flag,
                },
                super::health::ConnectionFlags {
                    server: s_conn,
                    llm: l_conn,
                    cli: c_conn,
                },
                handle,
                shutdown_rx.clone(),
            ))
        } else {
            None
        };

        // 15. Suggestion SSE + maintenance loops (server feature only)
        #[cfg(feature = "server")]
        let suggestion_sse_task = if self.suggestions_enabled {
            self.suggestion_receiver.as_ref().map(|receiver| {
                super::suggestions::spawn_suggestion_sse_loop(
                    receiver.clone(),
                    session_id.clone(),
                    shutdown_rx.clone(),
                )
            })
        } else {
            None
        };

        #[cfg(feature = "server")]
        let suggestion_maintenance_task = if self.suggestions_enabled {
            self.suggestion_manager.as_ref().map(|mgr| {
                super::suggestions::spawn_suggestion_maintenance_loop(
                    mgr.queue().clone(),
                    mgr.deferred().clone(),
                    mgr.retry_queue().clone(),
                    mgr.feedback().clone(),
                    mgr.storage().clone(),
                    None, // on_change wired via on_new callback on receiver
                    shutdown_rx.clone(),
                )
            })
        } else {
            None
        };

        let _ = shutdown_rx.changed().await;
        info!("ended received");

        let sqlite_end = self.sqlite_storage.clone();
        if let Err(e) = sqlite_end.end_session(&session_id, Utc::now()).await {
            warn!("session ended record failure: {e}");
        }

        // Abort all loops and check for panics
        let tasks: Vec<(&str, tokio::task::JoinHandle<()>)> = vec![
            ("monitor", monitor_task),
            ("metrics", metrics_task),
            ("process", process_task),
            ("sync", sync_task),
            ("heartbeat", heartbeat_task),
            ("aggregation", aggregation_task),
            ("notification", notification_task),
            ("focus", focus_task),
            ("event_snapshot", event_snapshot_task),
            ("analysis", analysis_task),
            ("cross_device_sync", cross_device_sync_task),
            ("coaching", coaching_task),
        ];

        for (name, task) in tasks {
            task.abort();
            match task.await {
                Ok(()) => {}
                Err(e) if e.is_cancelled() => {}
                Err(e) => {
                    error!(
                        loop_name = name,
                        "scheduler loop panicked during shutdown: {e}"
                    );
                }
            }
        }

        // Feature-gated optional tasks
        #[cfg(feature = "server")]
        if let Some(task) = oauth_task {
            task.abort();
            if let Err(e) = task.await {
                if !e.is_cancelled() {
                    error!("oauth_refresh loop panicked: {e}");
                }
            }
        }
        if let Some(task) = health_task {
            task.abort();
            if let Err(e) = task.await {
                if !e.is_cancelled() {
                    error!("health_check loop panicked: {e}");
                }
            }
        }
        #[cfg(feature = "server")]
        if let Some(task) = suggestion_sse_task {
            task.abort();
            if let Err(e) = task.await {
                if !e.is_cancelled() {
                    error!("suggestion SSE loop panicked: {e}");
                }
            }
        }
        #[cfg(feature = "server")]
        if let Some(task) = suggestion_maintenance_task {
            task.abort();
            if let Err(e) = task.await {
                if !e.is_cancelled() {
                    error!("suggestion maintenance loop panicked: {e}");
                }
            }
        }
    }
}
