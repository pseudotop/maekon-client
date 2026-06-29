use chrono::Utc;
use maekon_monitor::input_activity::InputActivityCollector;
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

            // Startup delay: wait 10 seconds before first sync
            tokio::time::sleep(Duration::from_secs(10)).await;

            let mut interval = super::intervals::coalescing_interval(sync_interval);

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        // D13: 4-term composite gate (CONS-PC02 / §3.3 A.9).
                        // effective_permissions() returns permissions only in the Valid state — Expired/UpdateRequired
                        // return all-false, so a stale consent record is also handled fail-closed (Task 3).
                        let consent = consent_mgr_s.as_ref()
                            .map(|cm| cm.effective_permissions())
                            .unwrap_or_default();
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
        // #4805: bind egress to telemetry consent — inject the shared ConsentManager.
        let egress_policy = Arc::new(
            PlatformEgressPolicy::new(&self.config)
                .with_consent_manager(self.consent_manager.clone()),
        );

        info!(
            platform_sync_enabled = egress_policy.is_enabled(),
            "platform egress policy applied"
        );

        self.initialize_session(&session_id).await;

        let shared_input_collector = Arc::new(InputActivityCollector::new());

        // -- Phase 1.5: Platform key-category hook --
        // Spawns a passive OS keyboard observer that classifies key events
        // into KeyCategory and feeds them into InputActivityCollector.
        // Gated by text_intelligence.input_pattern_detail config flag AND the
        // activity_pattern_learning consent (review4 monitor): installing a
        // system-wide OS key observer is GDPR Tier 4 (analysis.rs documents
        // "input_pattern_detail requires activity_pattern_learning consent"), and
        // the sibling accessibility extractor / tiered-memory paths already gate on
        // it. effective_permissions() is Valid-only, so stale (Expired/UpdateRequired)
        // consent fails closed; a missing ConsentManager also fails closed.
        let _key_hook = {
            let text_intel_config = self
                .config_manager
                .as_ref()
                .map(|cm| cm.get().analysis.text_intelligence.clone())
                .unwrap_or_default();

            let consent_ok = self
                .consent_manager
                .as_ref()
                .map(|cm| cm.effective_permissions().activity_pattern_learning)
                .unwrap_or(false);

            if text_intel_config.enabled && text_intel_config.input_pattern_detail && consent_ok {
                maekon_monitor::key_hook::KeyHook::start(shared_input_collector.clone())
            } else {
                debug!(
                    consent_ok,
                    "key-category hook disabled \
                     (text_intelligence.input_pattern_detail = false, \
                     text_intelligence.enabled = false, \
                     or activity_pattern_learning consent not granted)"
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

        // ── Spawn monitoring loops under one supervisor ───────────────────────
        // Every long-lived loop's JoinHandle is wrapped in `supervisor_set` so the
        // supervisor task (spawned below) can:
        //   • log error! immediately if any loop exits during normal runtime
        //     (#5989 — mandatory loops; #38 — the previously-unwatched optional
        //     oauth_refresh / health_check / suggestion loops), and
        //   • on clean shutdown, give every loop a BOUNDED window to run its own
        //     graceful-drain arm (event/sync flush, push drain) before aborting
        //     the stragglers (#10 — no more hard-abort racing the drain).
        //
        // Aborting is owned entirely by the supervisor via `supervisor_set`: the
        // JoinSet's `abort_all()` cancels any loop still running past the drain
        // window, so no separate AbortHandles need to be retained out here.

        // Maximum time the supervisor waits for loops to drain their final
        // flush/push on shutdown before force-aborting the stragglers (#10).
        const GRACEFUL_DRAIN: Duration = Duration::from_secs(5);

        // #6266: collect each inner loop's AbortHandle. Aborting the supervisor
        // JoinSet only cancels the WRAPPER tasks; the wrapper's `h.await` future,
        // when dropped, DETACHES the inner loop (a dropped JoinHandle does not
        // abort its task) instead of cancelling it. To make the documented
        // "force-abort the stragglers" real, we abort these inner handles too.
        let mut inner_aborts: Vec<tokio::task::AbortHandle> = Vec::new();

        // Helper macro: moves a JoinHandle<()> into the supervisor JoinSet. The
        // wrapper task returns (&'static str, inner_result) so the supervisor can
        // log the loop name on unexpected exit. The inner loop's AbortHandle is
        // recorded in `inner_aborts` so the shutdown path can truly cancel it.
        macro_rules! supervise {
            ($set:expr, $name:literal, $handle:expr) => {{
                let h = $handle;
                inner_aborts.push(h.abort_handle());
                $set.spawn(async move { ($name, h.await) });
            }};
        }

        // Variant for optional loops returning Option<JoinHandle<()>>. When the
        // loop is wired (`Some`) it is supervised exactly like a mandatory loop so
        // an unexpected exit logs error!(loop_name) (#38); a `None` (loop not
        // configured for this build/runtime) is simply skipped.
        macro_rules! supervise_opt {
            ($set:expr, $name:literal, $handle:expr) => {{
                if let Some(h) = $handle {
                    inner_aborts.push(h.abort_handle());
                    $set.spawn(async move { ($name, h.await) });
                }
            }};
        }

        // JoinSet<(&'static str, Result<(), JoinError>)> — one wrapper per loop.
        let mut supervisor_set: tokio::task::JoinSet<(
            &'static str,
            Result<(), tokio::task::JoinError>,
        )> = tokio::task::JoinSet::new();

        supervise!(
            supervisor_set,
            "monitor",
            self.spawn_monitor_loop(
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
            )
        );

        supervise!(
            supervisor_set,
            "metrics",
            self.spawn_metrics_loop(metrics_interval, shutdown_rx.clone())
        );

        supervise!(
            supervisor_set,
            "process",
            self.spawn_process_loop(process_interval, shutdown_rx.clone())
        );

        supervise!(
            supervisor_set,
            "sync",
            self.spawn_sync_loop(sync, egress_policy.clone(), shutdown_rx.clone())
        );

        supervise!(
            supervisor_set,
            "heartbeat",
            self.spawn_heartbeat_loop(
                heartbeat,
                session_id.clone(),
                egress_policy.clone(),
                shutdown_rx.clone(),
            )
        );

        supervise!(
            supervisor_set,
            "aggregation",
            self.spawn_aggregation_loop(
                aggregation,
                llm_summarizer_for_digest,
                shutdown_rx.clone()
            )
        );

        supervise!(
            supervisor_set,
            "notification",
            self.spawn_notification_loop(self.focus_mode.clone(), shutdown_rx.clone())
        );

        supervise!(
            supervisor_set,
            "focus",
            self.spawn_focus_loop(shared_regime.clone(), shutdown_rx.clone())
        );

        supervise!(
            supervisor_set,
            "event_snapshot",
            self.spawn_event_snapshot_loop(
                detailed_process_interval,
                input_activity_interval,
                egress_policy.clone(),
                shared_input_collector.clone(),
                shutdown_rx.clone(),
            )
        );

        // 10. OAuth token refresh (conditional — returns None if no coordinator).
        //     #38: supervised so an unexpected exit no longer dies silently.
        #[cfg(feature = "analysis")]
        supervise_opt!(
            supervisor_set,
            "oauth_refresh",
            self.spawn_oauth_refresh_loop(shutdown_rx.clone(), app_handle)
        );

        // 11. LLM analysis loop (periodic + change-detection)
        let analysis_config = self.config.analysis_config.clone();
        // E20-26 (#4818): thread shared regime state so the local-suggestion
        // enqueue path inside the analysis loop is regime/context-aware.
        supervise!(
            supervisor_set,
            "analysis",
            self.spawn_analysis_loop(analysis_config, shared_regime.clone(), shutdown_rx.clone())
        );

        // #5069: feature-performance flush loop (Option — None unless the emitter
        // is wired in an analysis build). 5-min cadence matches cross-device sync.
        #[cfg(feature = "analysis")]
        supervise_opt!(
            supervisor_set,
            "feature_perf_flush",
            self.spawn_feature_perf_flush_loop(Duration::from_secs(300), shutdown_rx.clone())
        );

        // 12. Cross-device sync loop (P3 Phase 3a-2)
        supervise!(
            supervisor_set,
            "cross_device_sync",
            self.spawn_cross_device_sync_loop(
                self.config.cross_device_sync_interval,
                shutdown_rx.clone(),
            )
        );

        // 13. Coaching feedback evaluation loop
        supervise!(
            supervisor_set,
            "coaching",
            self.spawn_coaching_loop(shared_regime.clone(), shutdown_rx.clone())
        );

        // 14. Health check loop — reads adapter health flags and updates connection
        //     flags. #38: previously spawned untracked; now supervised so an
        //     unexpected exit logs error!(loop_name) like the mandatory loops.
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
        supervise_opt!(supervisor_set, "health_check", health_task);

        // 15. Suggestion SSE + maintenance loops (server feature only). #38: both
        //     are now supervised so an unexpected exit no longer dies silently.
        //     #7099: the SSE stream runs under a dedicated supervisor that
        //     respawns the consumer after a permanent outage (or on server
        //     reconnect) instead of leaving the session suggestion-less until a
        //     full scheduler restart. The supervisor itself only returns on
        //     shutdown, so the generic JoinSet wrapper still catches any panic.
        #[cfg(feature = "server")]
        {
            let suggestion_sse_task = if self.suggestions_enabled {
                self.suggestion_receiver.as_ref().map(|receiver| {
                    super::suggestions::spawn_suggestion_sse_supervisor(
                        receiver.clone(),
                        session_id.clone(),
                        self.server_connected.clone(),
                        shutdown_rx.clone(),
                    )
                })
            } else {
                None
            };
            supervise_opt!(supervisor_set, "suggestion_sse", suggestion_sse_task);
        }

        #[cfg(feature = "local-suggestions")]
        {
            let suggestion_maintenance_task = if self.suggestions_enabled {
                self.suggestion_manager.as_ref().map(|mgr| {
                    super::suggestions::spawn_suggestion_maintenance_loop(
                        mgr.queue().clone(),
                        mgr.deferred().clone(),
                        mgr.retry_queue().clone(),
                        mgr.feedback().clone(),
                        mgr.storage().clone(),
                        // #5694: resurfaced (deferred) suggestions refresh the overlay.
                        // NOTE: this loop only spawns when config.suggestions.enabled is
                        // true (default false) — inert under stock config; wired so the
                        // surface is correct the moment the flag is enabled.
                        self.magic_overlay.as_ref().map(|o| {
                            let o = o.clone();
                            std::sync::Arc::new(move |count: usize| {
                                o.emit_suggestions_changed(count)
                            })
                                as std::sync::Arc<dyn Fn(usize) + Send + Sync>
                        }),
                        shutdown_rx.clone(),
                    )
                })
            } else {
                None
            };
            supervise_opt!(
                supervisor_set,
                "suggestion_maintenance",
                suggestion_maintenance_task
            );
        }

        // ── Live supervisor ───────────────────────────────────────────────────
        // Owns every long-lived loop via the JoinSet wrapper tasks (mandatory +
        // optional). Two responsibilities:
        //
        //   • Runtime: if any loop exits unexpectedly before shutdown, log error!
        //     with the loop name so the failure is visible in Loki/OTel rather
        //     than silently dying until the next restart (#5989, #38).
        //
        //   • Shutdown (#10): when shutdown_rx fires, give the loops a BOUNDED
        //     window (GRACEFUL_DRAIN) to run their own graceful-drain arms — each
        //     loop holds its own shutdown_rx.clone() and flushes/pushes on
        //     shutdown_rx.changed() — by continuing to drain join_next(). Only the
        //     loops still running past the timeout are force-aborted via
        //     abort_all(). This replaces the previous hard-abort that could cut an
        //     in-flight event/sync flush short.
        let mut supervisor_shutdown_rx = shutdown_rx.clone();
        let supervisor = tokio::spawn(async move {
            // Phase 1: normal runtime — watch for unexpected exits until shutdown.
            loop {
                tokio::select! {
                    biased; // check shutdown first to avoid processing stale entries
                    _ = supervisor_shutdown_rx.changed() => break,
                    Some(outcome) = supervisor_set.join_next() => {
                        match outcome {
                            // Wrapper task itself panicked (should never happen).
                            Err(join_err) => {
                                error!("supervisor wrapper task panicked: {join_err}");
                            }
                            // Inner loop returned Ok(()) — unexpectedly exited during
                            // runtime. All loops are designed to run until shutdown.
                            Ok((name, Ok(()))) => {
                                error!(
                                    loop_name = name,
                                    "scheduler loop exited unexpectedly during runtime"
                                );
                            }
                            // Inner loop panicked.
                            Ok((name, Err(ref e))) if !e.is_cancelled() => {
                                error!(
                                    loop_name = name,
                                    "scheduler loop panicked during runtime: {e}"
                                );
                            }
                            // Cancelled — expected when abort_all() fires on shutdown.
                            // A genuine panic that races with the shutdown abort is also
                            // surfaced as Cancelled here and is intentionally not reported
                            // separately: the process is already tearing down, so a loop
                            // dying at the exact shutdown boundary needs no runtime recovery.
                            // Runtime panics BEFORE shutdown are caught by the
                            // `!e.is_cancelled()` arm above and logged with the loop name.
                            Ok((_, Err(_))) => {}
                        }
                        // If all wrapper tasks have been consumed, the supervisor
                        // has nothing left to watch — exit so the outer loop does
                        // not spin on join_next() returning Poll::Ready(None).
                        if supervisor_set.is_empty() {
                            return;
                        }
                    }
                }
            }

            // Phase 2: bounded graceful drain (#10). The loops have all observed
            // shutdown_rx and are running their flush/push arms; wait for them to
            // finish on their own, up to GRACEFUL_DRAIN.
            let drain = async { while supervisor_set.join_next().await.is_some() {} };
            if tokio::time::timeout(GRACEFUL_DRAIN, drain).await.is_err() {
                // Stragglers still running past the deadline — force-abort them.
                warn!(
                    timeout_secs = GRACEFUL_DRAIN.as_secs(),
                    "scheduler loops did not drain within graceful window — aborting stragglers"
                );
                // #6266: abort the INNER loop tasks first — aborting only the
                // supervisor JoinSet detaches them (a dropped JoinHandle does not
                // cancel its task), leaving the loops running past shutdown.
                for inner in &inner_aborts {
                    inner.abort();
                }
                supervisor_set.abort_all();
                while supervisor_set.join_next().await.is_some() {}
            }
        });

        let _ = shutdown_rx.changed().await;
        info!("ended received");

        let sqlite_end = self.sqlite_storage.clone();
        if let Err(e) = sqlite_end.end_session(&session_id, Utc::now()).await {
            warn!("session ended record failure: {e}");
        }

        // The supervisor owns the graceful-drain-then-abort lifecycle for all
        // loops; just wait for it to finish.
        if let Err(e) = supervisor.await {
            if !e.is_cancelled() {
                error!("supervisor task panicked during shutdown: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::watch;

    /// Verify that the supervisor pattern detects a loop that exits unexpectedly
    /// during runtime (before shutdown_rx fires).
    ///
    /// We build a minimal JoinSet with one wrapper task whose inner JoinHandle
    /// returns immediately (simulating a loop that exits without panicking), and
    /// one wrapper that stays alive. The test confirms:
    ///   1. The supervisor's join_next() resolves with Ok("early_exit").
    ///   2. The supervisor does NOT resolve for the long-lived loop before shutdown.
    ///   3. After the shutdown signal fires the supervisor terminates cleanly.
    #[tokio::test]
    async fn supervisor_detects_early_exit() {
        let (shutdown_tx, mut supervisor_shutdown_rx) = watch::channel(false);

        let mut supervisor_set: tokio::task::JoinSet<(
            &'static str,
            Result<(), tokio::task::JoinError>,
        )> = tokio::task::JoinSet::new();

        // Loop that exits immediately — simulates a subsystem silently dying.
        let early_handle = tokio::spawn(async move {
            // returns right away
        });
        supervisor_set.spawn(async move { ("early_exit", early_handle.await) });

        // Loop that stays alive until explicitly aborted.
        let long_handle = tokio::spawn(async move {
            futures::future::pending::<()>().await;
        });
        let long_abort = long_handle.abort_handle();
        supervisor_set.spawn(async move { ("long_lived", long_handle.await) });

        // Run the supervisor inline (mirroring the production logic).
        let mut detected: Vec<&'static str> = Vec::new();
        loop {
            tokio::select! {
                biased;
                _ = supervisor_shutdown_rx.changed() => {
                    supervisor_set.abort_all();
                    break;
                }
                Some(outcome) = supervisor_set.join_next() => {
                    match outcome {
                        Ok((name, Ok(()))) => {
                            // Unexpected early exit — this is what we are testing.
                            detected.push(name);
                            // Signal shutdown so the test terminates.
                            let _ = shutdown_tx.send(true);
                        }
                        Ok((_, Err(ref e))) if e.is_cancelled() => {}
                        Ok((name, Err(ref e))) => {
                            panic!("unexpected panic from {name}: {e}");
                        }
                        Err(e) => panic!("wrapper panicked: {e}"),
                    }
                }
            }
        }

        // Cleanup: ensure the long-lived handle is also aborted.
        long_abort.abort();

        assert_eq!(
            detected,
            vec!["early_exit"],
            "supervisor must detect the early-exit loop"
        );
    }

    /// Verify that a clean shutdown (shutdown_rx fires before any loop exits)
    /// does not produce spurious detections.
    #[tokio::test]
    async fn supervisor_clean_shutdown_no_spurious_detection() {
        let (shutdown_tx, mut supervisor_shutdown_rx) = watch::channel(false);

        let mut supervisor_set: tokio::task::JoinSet<(
            &'static str,
            Result<(), tokio::task::JoinError>,
        )> = tokio::task::JoinSet::new();

        for name in ["loop_a", "loop_b", "loop_c"] {
            let handle = tokio::spawn(async move {
                futures::future::pending::<()>().await;
            });
            supervisor_set.spawn(async move { (name, handle.await) });
        }

        // Trigger shutdown immediately.
        let _ = shutdown_tx.send(true);

        let mut unexpected_exits: Vec<&str> = Vec::new();
        loop {
            tokio::select! {
                biased;
                _ = supervisor_shutdown_rx.changed() => {
                    supervisor_set.abort_all();
                    break;
                }
                Some(outcome) = supervisor_set.join_next() => {
                    match outcome {
                        Ok((name, Ok(()))) => unexpected_exits.push(name),
                        Ok((_, Err(ref e))) if e.is_cancelled() => {}
                        Ok((name, Err(ref e))) => panic!("unexpected panic from {name}: {e}"),
                        Err(e) => panic!("wrapper panicked: {e}"),
                    }
                }
            }
        }

        assert!(
            unexpected_exits.is_empty(),
            "clean shutdown must not produce spurious detections; got: {unexpected_exits:?}"
        );
    }
}
