use chrono::Utc;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

use super::super::shared_regime_state::SharedRegimeState;
use super::super::Scheduler;

impl Scheduler {
    /// Periodic LLM analysis loop — runs `analyze_if_changed()` on each tick
    /// and forces a full `analyze()` every `full_interval_secs`.
    /// Generated suggestions are persisted to SQLite for the web dashboard.
    #[tracing::instrument(skip_all)]
    pub(in crate::scheduler) fn spawn_analysis_loop(
        &self,
        config: maekon_core::config::AnalysisConfig,
        // E20-26 (#4818): shared regime state (monitor loop writes it). Read here to
        // make the local-suggestion enqueue path regime/context-aware.
        shared_regime: Arc<SharedRegimeState>,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        let analyzer = self.context_analyzer.clone();
        let storage_ref = self.storage.clone();
        let sqlite_ref = self.sqlite_storage.clone();
        let config_manager = self.config_manager.clone();
        // D13: 4-term privacy gate DI.
        let consent_mgr_a = self.consent_manager.clone();
        let capture_paused_a = self.capture_paused.clone();
        // #5069: feature-perf recorder (None ⇒ pass-through). Wraps the real
        // analyze() wall-clock as the `local-suggestions` feature execution time.
        #[cfg(feature = "analysis")]
        let perf_recorder: Option<
            Arc<dyn maekon_core::ports::feature_perf::FeaturePerfRecorder>,
        > = self
            .feature_perf
            .clone()
            .map(|u| u as Arc<dyn maekon_core::ports::feature_perf::FeaturePerfRecorder>);
        #[cfg(not(feature = "analysis"))]
        let perf_recorder: Option<
            Arc<dyn maekon_core::ports::feature_perf::FeaturePerfRecorder>,
        > = None;
        // E20-24 (#4816): producer wire — the live suggestion queue (the SAME Arc
        // the IPC `get_pending_suggestions` reads via the manager). Pushing here is
        // what lets the OSS local pipeline surface/score/accept its own suggestions;
        // previously they were only persisted to SQLite and never reached the queue.
        #[cfg(feature = "local-suggestions")]
        let suggestion_queue = self.suggestion_manager.as_ref().map(|m| m.queue().clone());
        // E20-26 (#4818): keep the shared regime handle alive in the spawned task only
        // when the local-suggestion filter actually consumes it.
        #[cfg(feature = "local-suggestions")]
        let shared_regime_for_filter = shared_regime.clone();
        // #5694: surfacing handles — overlay auto-refresh + desktop toast for
        // locally produced suggestions (previously SQLite/queue-only and thus
        // invisible until a manual panel open).
        #[cfg(feature = "local-suggestions")]
        let notif_a = self.notification_manager.clone();
        #[cfg(feature = "local-suggestions")]
        let overlay_a = self.magic_overlay.clone();
        #[cfg(feature = "local-suggestions")]
        let focus_a = self.focus_mode.clone();
        // Drop the unused binding in builds without the local-suggestion pipeline.
        let _ = &shared_regime;

        tokio::spawn(async move {
            let analyzer = match analyzer {
                Some(a) => a,
                None => {
                    let _ = shutdown_rx.changed().await;
                    return;
                }
            };

            // Use initial config for interval timing (changes require restart).
            // Other settings (enabled, min_confidence, max_suggestions, throttle_secs)
            // are read dynamically from ConfigManager on each tick so that
            // changes via the Tauri `update_analysis_config` command propagate
            // immediately without an agent restart.
            //
            // #6177: defense-in-depth — `tokio::time::interval` panics on a zero
            // period. ConfigManager already clamps `interval_secs` to its floor at
            // load (AppConfig::clamp_bounds), but `.max(1)` here guarantees a
            // non-zero period even if this loop is spawned with a config that
            // bypassed that path.
            let mut interval = super::intervals::coalescing_interval(Duration::from_secs(
                config.interval_secs.max(1),
            ));
            let full_interval = Duration::from_secs(config.full_interval_secs.max(1));
            let mut last_full = std::time::Instant::now();

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        // D13: 4-term composite gate (CONS-PC02 / §3.3 A.9).
                        // effective_permissions() returns permissions only in the Valid state — Expired/UpdateRequired
                        // return all-false, so a stale consent record is also handled fail-closed (Task 3).
                        let consent = consent_mgr_a.as_ref()
                            .map(|cm| cm.effective_permissions())
                            .unwrap_or_default();
                        let paused = capture_paused_a.load(Ordering::Relaxed);
                        let permitted = config_manager.as_ref()
                            .map(|cm| crate::scheduler::capture_permitted_now(&cm.snapshot(), &consent, paused))
                            .unwrap_or(false);
                        if !permitted {
                            debug!("analysis loop: capture gate closed (TS/consent/paused) — skipping tick");
                            continue;
                        }

                        // Read current config from ConfigManager (the single source
                        // of truth also written to by update_analysis_config).
                        let current_config = config_manager
                            .as_ref()
                            .map(|cm| cm.get().analysis)
                            .unwrap_or_else(|| config.clone());

                        if !current_config.enabled {
                            debug!("analysis loop: disabled via runtime config, skipping tick");
                            continue;
                        }

                        // Server coexistence: skip local LLM analysis when
                        // the server has recently sent suggestions via SSE.
                        // #6083: `has_recent_server_suggestions` is a SYNCHRONOUS
                        // SqliteStorage read that takes the shared connection mutex;
                        // calling it inline parks the tokio worker on the 1s loop
                        // (and can stall under a concurrent VACUUM/optimize pass).
                        // Offload to the blocking pool — mirrors the sibling
                        // spawn_blocking offloads in this scheduler.
                        let coexist_lookback = current_config.server_coexistence_lookback_secs;
                        let sqlite_coexist = sqlite_ref.clone();
                        let coexist_result = tokio::task::spawn_blocking(move || {
                            sqlite_coexist.has_recent_server_suggestions(coexist_lookback)
                        })
                        .await;
                        match coexist_result {
                            Ok(Ok(true)) => {
                                debug!(
                                    "server suggestions active (last {coexist_lookback}s) — skipping local analysis",
                                );
                                continue;
                            }
                            Ok(Ok(false)) => { /* proceed with local analysis */ }
                            Ok(Err(e)) => {
                                warn!(err.code = %e.code(), "server coexistence check failed: {e}");
                                // Proceed anyway — fail-open
                            }
                            Err(join_err) => {
                                warn!("server coexistence check task panicked: {join_err}");
                                // Proceed anyway — fail-open
                            }
                        }

                        let force_full = last_full.elapsed() >= full_interval;

                        // Wrap the actual analyze() call in a wall-clock span so the
                        // sample reflects real feature execution time (§4 anti-theater),
                        // not the gate checks above.
                        use maekon_core::models::feature_performance::feature_keys::LOCAL_SUGGESTIONS;
                        use maekon_core::ports::feature_perf::time_feature;
                        let result = if force_full {
                            last_full = std::time::Instant::now();
                            time_feature(
                                perf_recorder.as_ref(),
                                LOCAL_SUGGESTIONS,
                                analyzer.analyze(),
                            )
                            .await
                        } else {
                            time_feature(
                                perf_recorder.as_ref(),
                                LOCAL_SUGGESTIONS,
                                analyzer.analyze_if_changed(),
                            )
                            .await
                        };

                        match result {
                            Ok(suggestions) => {
                                if !suggestions.is_empty() {
                                    info!(
                                        count = suggestions.len(),
                                        "LLM analysis produced suggestions"
                                    );
                                }
                                for suggestion in &suggestions {
                                    info!(
                                        id = %suggestion.suggestion_id,
                                        priority = ?suggestion.priority,
                                        content = %maekon_monitor::log_privacy::content_digest(
                                            &suggestion.content
                                        ),
                                        "suggestion produced"
                                    );
                                    if let Err(e) = storage_ref.save_suggestion(suggestion).await {
                                        warn!(err.code = %e.code(), "suggestion save failure: {e}");
                                    }
                                }
                                // E20-24 (#4816): enqueue for live review (dedup via
                                // queue fingerprint; push returns false on duplicate).
                                #[cfg(feature = "local-suggestions")]
                                if let Some(ref q) = suggestion_queue {
                                    // E20-26 (#4818): regime/context-aware gating. In a
                                    // focused regime (e.g. "Deep Focus") low/medium-priority
                                    // suggestions are dropped BEFORE they reach the queue so
                                    // the user is not interrupted. `regime: None` => pass-through.
                                    let mut to_enqueue = suggestions.clone();
                                    let regime = shared_regime_for_filter.snapshot().regime;
                                    maekon_analysis::filter_by_regime(
                                        &mut to_enqueue,
                                        regime.as_ref(),
                                    );
                                    // #5694: shared surfacing funnel — enqueue (dedup) +
                                    // overlay auto-refresh + High+ toast (focus-gated).
                                    let on_changed = overlay_a.as_ref().map(|o| {
                                        let o = o.clone();
                                        move |c: usize| o.emit_suggestions_changed(c)
                                    });
                                    let on_changed_ref: Option<&(dyn Fn(usize) + Send + Sync)> =
                                        on_changed
                                            .as_ref()
                                            .map(|f| f as &(dyn Fn(usize) + Send + Sync));
                                    super::helpers::enqueue_and_surface(
                                        q,
                                        to_enqueue,
                                        on_changed_ref,
                                        notif_a.as_ref(),
                                        focus_a.is_active(),
                                    )
                                    .await;
                                }
                            }
                            Err(e) => {
                                // AnalysisError doesn't expose code() directly; convert
                                // through the existing From<AnalysisError> for CoreError
                                // to surface the wire code to telemetry.
                                let core: maekon_core::error::CoreError = e.into();
                                warn!(err.code = %core.code(), "analysis failure: {core}");
                            }
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        info!("analysis loop ended");
                        break;
                    }
                }
            }
        })
    }

    #[tracing::instrument(skip_all)]
    pub(in crate::scheduler) fn spawn_focus_loop(
        &self,
        // E20-26 (#4818)/#5696: shared regime state for context-aware gating of
        // the rule-suggestion enqueue path (monitor loop writes it each tick).
        shared_regime: Arc<SharedRegimeState>,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        let focus8 = self.focus_analyzer.clone();
        // D13: 4-term privacy gate DI.
        let config_mgr_f = self.config_manager.clone();
        let consent_mgr_f = self.consent_manager.clone();
        let capture_paused_f = self.capture_paused.clone();
        // #5696: rule-suggestion live-queue bridge — the FocusAnalyzer rules
        // already save to SQLite and toast, but never reached the live queue,
        // so the overlay panel stayed empty all session (until a restart's
        // pending-restore). notifier stays None here: the rules send their own
        // one-shot OS notification; a funnel toast would double-notify.
        #[cfg(feature = "local-suggestions")]
        let focus_queue = self.suggestion_manager.as_ref().map(|m| m.queue().clone());
        #[cfg(feature = "local-suggestions")]
        let sqlite_f = self.sqlite_storage.clone();
        #[cfg(feature = "local-suggestions")]
        let overlay_f = self.magic_overlay.clone();
        #[cfg(feature = "local-suggestions")]
        let shared_regime_f = shared_regime.clone();
        let _ = &shared_regime;

        tokio::spawn(async move {
            let focus = match focus8 {
                Some(f) => f,
                None => {
                    let _ = shutdown_rx.changed().await;
                    return;
                }
            };

            let mut interval = super::intervals::coalescing_interval(Duration::from_secs(60)); // 1min
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        // D13: 4-term composite gate (CONS-PC02 / §3.3 A.9).
                        // effective_permissions() returns permissions only in the Valid state — Expired/UpdateRequired
                        // return all-false, so a stale consent record is also handled fail-closed (Task 3).
                        let consent = consent_mgr_f.as_ref()
                            .map(|cm| cm.effective_permissions())
                            .unwrap_or_default();
                        let paused = capture_paused_f.load(Ordering::Relaxed);
                        let permitted = config_mgr_f.as_ref()
                            .map(|cm| crate::scheduler::capture_permitted_now(&cm.snapshot(), &consent, paused))
                            .unwrap_or(false);
                        if !permitted {
                            debug!("focus loop: capture gate closed (TS/consent/paused) — skipping tick");
                            continue;
                        }
                        let rule_suggestions = focus.analyze_periodic().await;
                        let _ = &rule_suggestions;
                        // #5696: bridge produced rule suggestions into the live
                        // review queue (save + OS toast already happened inside
                        // the analyzer). Mirrors the analysis loop: server
                        // coexistence → regime filter → shared funnel.
                        #[cfg(feature = "local-suggestions")]
                        if let Some(ref q) = focus_queue {
                            if !rule_suggestions.is_empty() {
                                let coexist_lookback = config_mgr_f
                                    .as_ref()
                                    .map(|cm| {
                                        cm.get().analysis.server_coexistence_lookback_secs
                                    })
                                    .unwrap_or(300);
                                // #6083: offload the SYNCHRONOUS SqliteStorage read
                                // off the focus loop's tokio worker — it takes the
                                // shared connection mutex and can park under a
                                // concurrent VACUUM/optimize pass.
                                let sqlite_coexist = sqlite_f.clone();
                                let coexist = tokio::task::spawn_blocking(move || {
                                    sqlite_coexist
                                        .has_recent_server_suggestions(coexist_lookback)
                                })
                                .await
                                .unwrap_or_else(|join_err| {
                                    warn!(
                                        "focus coexistence check task panicked: {join_err}"
                                    );
                                    Ok(false) // fail-open
                                })
                                .unwrap_or(false);
                                if !coexist {
                                    let mut to_enqueue = rule_suggestions;
                                    let regime = shared_regime_f.snapshot().regime;
                                    maekon_analysis::filter_by_regime(
                                        &mut to_enqueue,
                                        regime.as_ref(),
                                    );
                                    let on_changed = overlay_f.as_ref().map(|o| {
                                        let o = o.clone();
                                        move |c: usize| o.emit_suggestions_changed(c)
                                    });
                                    let on_changed_ref: Option<&(dyn Fn(usize) + Send + Sync)> =
                                        on_changed
                                            .as_ref()
                                            .map(|f| f as &(dyn Fn(usize) + Send + Sync));
                                    super::helpers::enqueue_and_surface(
                                        q,
                                        to_enqueue,
                                        on_changed_ref,
                                        None, // rules already sent their own toast
                                        false,
                                    )
                                    .await;
                                }
                            }
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        info!("in progress min ended");
                        break;
                    }
                }
            }
        })
    }

    /// 13. Coaching feedback evaluation loop.
    ///
    /// Runs implicit feedback evaluation on pending coaching messages every 30s.
    /// The actual coaching `evaluate()` call is performed inside `spawn_monitor_loop()`
    /// where live regime data is available (Option A from the plan).
    #[tracing::instrument(skip_all)]
    pub(in crate::scheduler) fn spawn_coaching_loop(
        &self,
        shared_regime: Arc<SharedRegimeState>,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        let coaching = self.coaching_engine.clone();
        let _notif = self.notification_manager.clone();
        // D13: 4-term privacy gate DI.
        let config_mgr_c = self.config_manager.clone();
        let consent_mgr_c = self.consent_manager.clone();
        let capture_paused_c = self.capture_paused.clone();

        tokio::spawn(async move {
            let engine = match coaching {
                Some(e) => e,
                None => {
                    let _ = shutdown_rx.changed().await;
                    return;
                }
            };

            let mut interval = super::intervals::coalescing_interval(Duration::from_secs(
                super::super::config::COACHING_INTERVAL_SECS,
            ));

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        // D13: 4-term composite gate (CONS-PC02 / §3.3 A.9).
                        // Coaching during an opt-out window is invasive (R3.I4).
                        // effective_permissions() returns permissions only in the Valid state — Expired/UpdateRequired
                        // return all-false, so a stale consent record is also handled fail-closed (Task 3).
                        let consent = consent_mgr_c.as_ref()
                            .map(|cm| cm.effective_permissions())
                            .unwrap_or_default();
                        let paused = capture_paused_c.load(Ordering::Relaxed);
                        let permitted = config_mgr_c.as_ref()
                            .map(|cm| crate::scheduler::capture_permitted_now(&cm.snapshot(), &consent, paused))
                            .unwrap_or(false);
                        if !permitted {
                            debug!("coaching loop: capture gate closed (TS/consent/paused) — skipping tick");
                            continue;
                        }
                        // Read current regime context from the monitor loop (C1)
                        let snap = shared_regime.snapshot();
                        engine.evaluate_implicit_feedback(
                            snap.regime_id.as_deref(),
                            &snap.current_app,
                            Utc::now(),
                        ).await;
                    }
                    _ = shutdown_rx.changed() => {
                        info!("coaching loop ended");
                        break;
                    }
                }
            }
        })
    }
}

/// #6083: server-coexistence offload fail-open contract tests.
///
/// The analysis/focus/event loops offload the SYNCHRONOUS
/// `has_recent_server_suggestions` SqliteStorage read via `spawn_blocking` so it
/// never parks the scheduler's tokio worker on the 1s hot path. This module pins
/// the fail-open mapping the call sites rely on: when the offloaded read panics
/// (JoinError) or returns an error, the loops must treat the result as
/// "no recent server suggestions" (`false`) and proceed with local analysis
/// rather than silently skipping it.
#[cfg(test)]
mod coexistence_offload_tests {
    use maekon_core::error::CoreError;

    /// A panicking offload closure surfaces as a JoinError on `.await`; the
    /// `unwrap_or(Ok(false))` + `unwrap_or(false)` chain used at the
    /// focus/event call sites must collapse it to fail-open `false`.
    #[tokio::test]
    async fn panicking_offload_fails_open_to_false() {
        let handle = tokio::task::spawn_blocking(|| -> Result<bool, CoreError> {
            panic!("intentional coexistence read panic");
        });
        let coexist = handle.await.unwrap_or(Ok(false)).unwrap_or(false);
        assert!(
            !coexist,
            "a panicking coexistence offload must fail open to false (proceed with local analysis)"
        );
    }

    /// A storage error from the offloaded read must also fail open to `false`.
    #[tokio::test]
    async fn errored_offload_fails_open_to_false() {
        let handle = tokio::task::spawn_blocking(|| -> Result<bool, CoreError> {
            Err(CoreError::Storage {
                message: "simulated sqlite read failure".to_string(),
                code: maekon_core::error_codes::StorageCode::Failed,
            })
        });
        let coexist = handle.await.unwrap_or(Ok(false)).unwrap_or(false);
        assert!(
            !coexist,
            "an errored coexistence offload must fail open to false"
        );
    }

    /// A successful `true` read is preserved through the mapping chain so the
    /// loops correctly suppress local analysis when the server is active.
    #[tokio::test]
    async fn successful_true_offload_is_preserved() {
        let handle = tokio::task::spawn_blocking(|| -> Result<bool, CoreError> { Ok(true) });
        let coexist = handle.await.unwrap_or(Ok(false)).unwrap_or(false);
        assert!(
            coexist,
            "a successful true coexistence read must be preserved (skip local analysis)"
        );
    }
}
