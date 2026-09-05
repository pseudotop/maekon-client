use chrono::Utc;
use maekon_core::ports::consent_manager::ConsentGate;
use maekon_monitor::input_activity::InputActivityCollector;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

use super::super::egress_policy::PlatformEgressPolicy;
use super::super::shared_regime_state::SharedRegimeState;
use super::super::Scheduler;

async fn wait_for_startup_delay_or_shutdown(
    delay: Duration,
    shutdown_rx: &mut tokio::sync::watch::Receiver<bool>,
) -> bool {
    if *shutdown_rx.borrow() {
        return true;
    }
    tokio::select! {
        _ = tokio::time::sleep(delay) => false,
        changed = shutdown_rx.changed() => changed.is_ok(),
    }
}

impl Scheduler {
    /// Periodically check and refresh OAuth tokens.
    #[tracing::instrument(skip_all)]
    #[cfg(feature = "analysis")]
    pub(in crate::scheduler) fn spawn_oauth_refresh_loop(
        &self,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
        app_handle: Option<tauri::AppHandle>,
    ) -> Option<tokio::task::JoinHandle<()>> {
        use super::super::config::OAUTH_REFRESH_INTERVAL_SECS;
        use maekon_core::ports::oauth::TokenEvent;
        use std::time::Duration;
        use tauri::Emitter;

        let coordinator = self.oauth_coordinator.as_ref()?.clone();

        // Collect the configured provider IDs at spawn time.  The registry reads
        // from a static catalog so no async work is needed here, and the list is
        // stable for the lifetime of the loop.  An empty list means no managed-
        // OAuth providers are configured — the loop still runs so it can process
        // ReauthRequired events that were emitted before startup.
        let provider_ids = crate::oauth_provider_registry::configured_oauth_provider_ids();

        Some(tokio::spawn(async move {
            let mut interval = super::intervals::coalescing_interval(Duration::from_secs(
                OAUTH_REFRESH_INTERVAL_SECS,
            ));
            let mut event_rx = coordinator.subscribe();
            let mut last_reauth_notify: Option<tokio::time::Instant> = None;

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        for provider_id in &provider_ids {
                            let outcome = coordinator.check_and_refresh(provider_id).await;
                            debug!(provider_id = %provider_id, ?outcome, "OAuth refresh tick");
                        }
                    }
                    event = event_rx.recv() => {
                        if let Ok(TokenEvent::ReauthRequired { ref provider_id }) = event {
                            let should_notify = last_reauth_notify
                                .is_none_or(|t| t.elapsed() > Duration::from_secs(300));
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
        // #5069: feature-perf recorder (None ⇒ pass-through). Times one sync push
        // cycle as the `sync` feature execution. Off-by-default sync ⇒ 0 samples
        // until cross-device sync is enabled — honest, not faked.
        #[cfg(feature = "analysis")]
        let perf_recorder_s: Option<
            Arc<dyn maekon_core::ports::feature_perf::FeaturePerfRecorder>,
        > = self
            .feature_perf
            .clone()
            .map(|u| u as Arc<dyn maekon_core::ports::feature_perf::FeaturePerfRecorder>);
        #[cfg(not(feature = "analysis"))]
        let perf_recorder_s: Option<
            Arc<dyn maekon_core::ports::feature_perf::FeaturePerfRecorder>,
        > = None;

        tokio::spawn(async move {
            let engine = match sync_engine {
                Some(e) => e,
                None => {
                    let _ = shutdown_rx.changed().await;
                    return;
                }
            };

            if wait_for_startup_delay_or_shutdown(Duration::from_secs(10), &mut shutdown_rx).await {
                return;
            }

            let mut interval = super::intervals::coalescing_interval(sync_interval);

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        // D13: 4-term composite gate (CONS-PC02 / §3.3 A.9).
                        // ConsentGate is fail-closed both on a stale (Expired/UpdateRequired)
                        // consent record AND on a missing ConsentManager (#7728).
                        let consent = ConsentGate::from_ref(consent_mgr_s.as_ref()).permissions_snapshot();
                        let paused = capture_paused_s.load(Ordering::Relaxed);
                        let permitted = config_mgr_s.as_ref()
                            .map(|cm| crate::scheduler::capture_permitted_now(&cm.snapshot(), &consent, paused))
                            .unwrap_or(false);
                        if !permitted {
                            // #5165: the capture gate (screen_capture / active-hours /
                            // paused) must NOT block GDPR Art. 17 erasure propagation — a
                            // tombstone is an erasure, not data collection. Propagate any
                            // pending erasure, then skip the (gated) normal data sync.
                            if let Err(e) = engine.propagate_pending_erasure().await {
                                warn!(err.code = %e.code(), "cross-device erasure propagation failed: {e}");
                            }
                            debug!("cross-device sync: capture gate closed — propagated any pending erasure, skipping normal sync");
                            continue;
                        }
                        let cycle = maekon_core::ports::feature_perf::time_feature(
                            perf_recorder_s.as_ref(),
                            maekon_core::models::feature_performance::feature_keys::SYNC,
                            engine.run_cycle(),
                        )
                        .await;
                        match cycle {
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
                        // #6243: reclaim consumed transport-side artifacts (e.g. old
                        // changeset files in a shared sync folder) each cycle so the
                        // folder does not grow unbounded. No-op for remote/in-memory
                        // transports (default trait impl).
                        if let Err(e) = engine.enforce_transport_retention().await {
                            warn!(err.code = %e.code(), "transport retention failed: {e}");
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

    /// #5069: periodic feature-performance flush loop. Drains the per-feature_key
    /// buffer and ships samples to the server (consent-gated + egress-audited
    /// inside `flush()`). Returns `None` when the emitter is not wired (non-analysis
    /// builds or missing consent/sink) so no idle task is spawned.
    #[cfg(feature = "analysis")]
    pub(in crate::scheduler) fn spawn_feature_perf_flush_loop(
        &self,
        flush_interval: Duration,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> Option<tokio::task::JoinHandle<()>> {
        let uploader = self.feature_perf.clone()?;
        Some(tokio::spawn(async move {
            let mut interval = super::intervals::coalescing_interval(flush_interval);
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let report = uploader.flush().await;
                        if report.uploaded + report.requeued + report.dropped + report.blocked > 0 {
                            debug!(
                                uploaded = report.uploaded,
                                requeued = report.requeued,
                                dropped = report.dropped,
                                blocked = report.blocked,
                                "feature-perf flush"
                            );
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        // Best-effort final drain (may be cut short by the
                        // run_scheduler_loops abort — operational samples only).
                        let _ = uploader.flush().await;
                        info!("feature-perf flush loop ended");
                        break;
                    }
                }
            }
        }))
    }

    #[tracing::instrument(skip_all)]
    #[allow(unused_variables)]
    pub(in crate::scheduler) async fn run_scheduler_loops(
        &self,
        shutdown_rx: tokio::sync::watch::Receiver<bool>,
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
        // #4805: bind egress to telemetry consent — inject the shared ConsentManager.
        let egress_policy = Arc::new(
            PlatformEgressPolicy::new(&self.config)
                .with_consent_manager(self.consent_manager.clone())
                .with_config_manager(self.config_manager.clone()),
        );

        info!(
            platform_sync_enabled = egress_policy.is_enabled(),
            "platform egress policy applied"
        );

        self.initialize_session(&session_id).await;

        let shared_input_collector = Arc::new(InputActivityCollector::new());

        // -- Platform key-category / mouse-activity hooks --
        // #7698 S1: hook lifecycle management (start on consent grant, stop +
        // collector-drain on consent revoke) now lives in
        // `spawn_event_snapshot_loop` (loops/events.rs) — see
        // `reconcile_key_hook`/`reconcile_mouse_hook` there — instead of being
        // spawned once here from a startup snapshot of consent that
        // `withdraw_consent()` could never reach. That loop already ticks on
        // `input_activity_interval` and reads live consent every tick, so it
        // is the natural single owner of both hook handles.

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
        let (
            llm_summarizer_for_digest,
            llm_summary_provider_class_for_digest,
            llm_summary_unavailable_reason_for_digest,
        ) = adaptive_trigger_state.as_ref().map_or(
            (
                None,
                None,
                Some(maekon_core::models::ai_summary::AiSummaryFailureReason::PipelineDisabled),
            ),
            |ts| {
                (
                    ts.llm_summarizer.clone(),
                    ts.llm_summary_provider_class,
                    ts.llm_summary_unavailable_reason,
                )
            },
        );

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
                use maekon_analysis::gui_aggregator::GuiActivityAggregator;
                use maekon_vision::gui_detector::GuiElementDetector;

                use super::super::gui_pipeline::GuiPipelineState;

                let detector = GuiElementDetector::new(
                    (1920, 1080), // sensible default; updated per tick from WindowLayoutEvent
                    maekon_core::config::PiiFilterLevel::Standard,
                );

                // Default: CV-based contour classifier (always available, no model file needed)
                let detector = detector.with_ml_classifier(std::sync::Arc::new(
                    maekon_vision::contour_classifier::ContourGuiClassifier::new(),
                ));

                // Override with ONNX ML classifier when feature is enabled and model exists
                #[cfg(feature = "ml-detect")]
                let detector = {
                    use maekon_vision::ml_classifier::OnnxGuiClassifier;

                    let model_path = if gui_config.ml_model_path.is_empty() {
                        match maekon_core::config_manager::ConfigManager::data_dir() {
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
                    pending_summaries: std::collections::VecDeque::new(),
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

        // ── Supervised loop set (crash-respawn with capped backoff, #8045) ────
        // Every long-lived loop is registered as a (name, factory) pair. The
        // supervisor (`loops/supervisor.rs`) owns each loop: it respawns a loop
        // that exits UNEXPECTEDLY during runtime with a capped exponential
        // backoff (logging the loop name + attempt count), and on shutdown gives
        // each loop a bounded drain window before aborting it. Each factory
        // re-invokes the same `self.spawn_*` method so a crashed loop — most
        // importantly the monitor loop, which OWNS screen capture — recovers
        // without a full scheduler restart. Before this, a silent monitor-loop
        // death stopped capture for the whole session with only an error! log.
        //
        // Optional loops (oauth_refresh / feature_perf_flush / health_check /
        // suggestion_sse / suggestion_maintenance) are registered only when their
        // backing resource is wired; an unlikely `None` on a respawn maps to a
        // parked task (`park_task`) so the supervisor never hot-loops a `None`.
        let mut factories: Vec<(&'static str, super::supervisor::LoopFactory<'_>)> = Vec::new();

        macro_rules! reg {
            ($name:literal, $factory:expr) => {
                factories.push((
                    $name,
                    Box::new($factory) as super::supervisor::LoopFactory<'_>,
                ));
            };
        }

        // The monitor loop consumes the non-Clone adaptive-trigger state, so the
        // factory `take()`s it on the first spawn; a respawn runs with `None`
        // (screen capture continues on its regular cadence — only adaptive
        // triggering is degraded until the next clean restart), which is far
        // better than the whole capture subsystem dying silently.
        let mut monitor_adaptive_state = adaptive_trigger_state;
        reg!("monitor", {
            let egress = egress_policy.clone();
            let input = shared_input_collector.clone();
            let regime = shared_regime.clone();
            let session = session_id.clone();
            let app = app_handle.clone();
            move |rx| {
                self.spawn_monitor_loop(
                    poll,
                    idle_threshold,
                    session.clone(),
                    egress.clone(),
                    input.clone(),
                    monitor_adaptive_state.take(),
                    regime.clone(),
                    self.focus_mode.clone(),
                    rx,
                    app.clone(),
                )
            }
        });

        reg!("metrics", move |rx| self
            .spawn_metrics_loop(metrics_interval, rx));

        reg!("process", move |rx| self
            .spawn_process_loop(process_interval, rx));

        reg!("sync", {
            let egress = egress_policy.clone();
            move |rx| self.spawn_sync_loop(sync, egress.clone(), rx)
        });

        reg!("heartbeat", {
            let egress = egress_policy.clone();
            let session = session_id.clone();
            move |rx| self.spawn_heartbeat_loop(heartbeat, session.clone(), egress.clone(), rx)
        });

        reg!("aggregation", move |rx| self.spawn_aggregation_loop(
            aggregation,
            llm_summarizer_for_digest.clone(),
            llm_summary_provider_class_for_digest,
            llm_summary_unavailable_reason_for_digest,
            rx
        ));

        reg!("notification", move |rx| self
            .spawn_notification_loop(self.focus_mode.clone(), rx));

        reg!("focus", {
            let regime = shared_regime.clone();
            move |rx| self.spawn_focus_loop(regime.clone(), rx)
        });

        reg!("event_snapshot", {
            let egress = egress_policy.clone();
            let input = shared_input_collector.clone();
            move |rx| {
                self.spawn_event_snapshot_loop(
                    detailed_process_interval,
                    input_activity_interval,
                    egress.clone(),
                    input.clone(),
                    rx,
                )
            }
        });

        // 10. OAuth token refresh (optional — only when a coordinator is wired).
        #[cfg(feature = "analysis")]
        if self.oauth_coordinator.is_some() {
            let app = app_handle.clone();
            reg!("oauth_refresh", move |rx| self
                .spawn_oauth_refresh_loop(rx, app.clone())
                .unwrap_or_else(super::supervisor::park_task));
        }

        // 11. LLM analysis loop (periodic + change-detection). E20-26 (#4818):
        //     shared regime state keeps the local-suggestion enqueue path aware.
        let analysis_config = self.config.analysis_config.clone();
        reg!("analysis", {
            let regime = shared_regime.clone();
            move |rx| self.spawn_analysis_loop(analysis_config.clone(), regime.clone(), rx)
        });

        // #5069: feature-performance flush loop (optional — only in analysis
        //     builds with a wired emitter). 5-min cadence matches cross-device sync.
        #[cfg(feature = "analysis")]
        if self.feature_perf.is_some() {
            reg!("feature_perf_flush", move |rx| self
                .spawn_feature_perf_flush_loop(Duration::from_secs(300), rx)
                .unwrap_or_else(super::supervisor::park_task));
        }

        // 12. Cross-device sync loop (P3 Phase 3a-2).
        reg!("cross_device_sync", move |rx| self
            .spawn_cross_device_sync_loop(
                self.config.cross_device_sync_interval,
                rx
            ));

        // 13. Coaching feedback evaluation loop.
        reg!("coaching", {
            let regime = shared_regime.clone();
            move |rx| self.spawn_coaching_loop(regime.clone(), rx)
        });

        // 14. Health check loop — optional; only when the adapter/connection
        //     flags and the tray handle are all wired.
        if let (
            Some(s_flag),
            Some(l_flag),
            Some(c_flag),
            Some(s_conn),
            Some(l_conn),
            Some(c_conn),
            Some(tray),
        ) = (
            self.server_health_flag.clone(),
            self.llm_health_flag.clone(),
            self.cli_health_flag.clone(),
            self.server_connected.clone(),
            self.llm_connected.clone(),
            self.cli_connected.clone(),
            self.tray_app_handle.clone(),
        ) {
            reg!("health_check", move |rx| {
                super::health::spawn_health_check_loop(
                    std::time::Duration::from_secs(
                        super::super::config::HEALTH_CHECK_INTERVAL_SECS,
                    ),
                    super::health::AdapterHealthFlags {
                        server_ok: s_flag.clone(),
                        llm_ok: l_flag.clone(),
                        cli_ok: c_flag.clone(),
                    },
                    super::health::ConnectionFlags {
                        server: s_conn.clone(),
                        llm: l_conn.clone(),
                        cli: c_conn.clone(),
                    },
                    tray.clone(),
                    rx,
                )
            });
        }

        // 14b. Self-resource-budget sampling (#7918/#7927) as an always-on loop
        //      (#7947). Runs in EVERY configuration (including minimal / OSS
        //      builds), so the periodic RSS/CPU budget + leak logging is never
        //      silently dropped in a config without the health-probe wiring.
        //      Local diagnostics only, never egressed (ADR-016).
        reg!("resource_health", move |rx| {
            super::resource_health::spawn_resource_health_loop(
                std::time::Duration::from_secs(super::super::config::HEALTH_CHECK_INTERVAL_SECS),
                rx,
            )
        });

        // 15. Suggestion SSE consumer (server feature only). #7099: it runs its
        //     own inner respawn supervisor for SSE reconnect / permanent outage;
        //     this outer supervisor additionally recovers the whole task if it
        //     ever crashes. Registered only when a receiver is wired.
        #[cfg(feature = "server")]
        if self.suggestions_enabled {
            if let Some(receiver) = self.suggestion_receiver.as_ref() {
                let receiver = receiver.clone();
                let server_connected = self.server_connected.clone();
                let session = session_id.clone();
                reg!("suggestion_sse", move |rx| {
                    super::suggestions::spawn_suggestion_sse_supervisor(
                        receiver.clone(),
                        session.clone(),
                        server_connected.clone(),
                        rx,
                    )
                });
            }
        }

        // Suggestion maintenance loop (local-suggestions builds). Only spawns
        // when config.suggestions.enabled is true (default false) — inert under
        // stock config; wired so the surface is correct the moment the flag flips.
        #[cfg(feature = "local-suggestions")]
        if self.suggestions_enabled {
            if let Some(mgr) = self.suggestion_manager.as_ref() {
                let mgr = mgr.clone();
                let overlay = self.magic_overlay.clone();
                reg!("suggestion_maintenance", move |rx| {
                    super::suggestions::spawn_suggestion_maintenance_loop(
                        mgr.queue().clone(),
                        mgr.deferred().clone(),
                        mgr.retry_queue().clone(),
                        mgr.feedback().clone(),
                        mgr.storage().clone(),
                        // #5694: resurfaced (deferred) suggestions refresh the overlay.
                        overlay.as_ref().map(|o| {
                            let o = o.clone();
                            std::sync::Arc::new(move |count: usize| {
                                o.emit_suggestions_changed(count)
                            })
                                as std::sync::Arc<dyn Fn(usize) + Send + Sync>
                        }),
                        rx,
                    )
                });
            }
        }

        // ── Drive the supervised set ──────────────────────────────────────────
        // Each supervisor future returns only when the global shutdown fires
        // (after its loop drains its own flush/push arm). They are driven INLINE
        // (borrowing `&self`, so the factories can re-invoke `self.spawn_*` on
        // respawn) via FuturesUnordered rather than a spawned JoinSet — a spawned
        // 'static task could not borrow `self` to respawn. The session-end record
        // is written concurrently the moment shutdown is observed.
        use futures::StreamExt;
        let mut supervisors: futures::stream::FuturesUnordered<_> = factories
            .into_iter()
            .map(|(name, factory)| {
                super::supervisor::supervise_loop(name, factory, shutdown_rx.clone())
            })
            .collect();

        let sqlite_end = self.sqlite_storage.clone();
        let end_session_id = session_id.clone();
        let mut end_shutdown_rx = shutdown_rx.clone();
        tokio::join!(
            async { while supervisors.next().await.is_some() {} },
            async move {
                let _ = end_shutdown_rx.changed().await;
                info!("ended received");
                if let Err(e) = sqlite_end.end_session(&end_session_id, Utc::now()).await {
                    warn!("session ended record failure: {e}");
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;
    use tokio::sync::watch;

    #[tokio::test]
    async fn startup_delay_returns_when_shutdown_fires() {
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

        let wait_task = tokio::spawn(async move {
            super::wait_for_startup_delay_or_shutdown(Duration::from_secs(3600), &mut shutdown_rx)
                .await
        });

        tokio::time::sleep(Duration::from_millis(10)).await;
        shutdown_tx.send(true).unwrap();

        let shutdown_seen = tokio::time::timeout(Duration::from_millis(200), wait_task)
            .await
            .expect("startup delay must not block shutdown")
            .unwrap();
        assert!(shutdown_seen);
    }

    // The runtime respawn/backoff and clean-shutdown-drain behaviour of the loop
    // supervisor is now owned by `loops/supervisor.rs` (`supervise_loop`) and is
    // unit-tested there against the real production code path, replacing the two
    // former tests here that re-implemented the old JoinSet supervisor inline.
}
