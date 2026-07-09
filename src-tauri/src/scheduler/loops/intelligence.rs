use chrono::Utc;
use maekon_core::ports::consent_manager::ConsentGate;
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
        // #7652: shared runtime slot (Arc<RwLock<Option<Arc<ContextAnalyzer>>>>).
        // This loop is the SOLE writer — it installs the analyzer on a runtime
        // enable transition and tears it down on a runtime disable transition
        // (see `reconcile_analyzer_slot` below), so a Settings flip (or a BYOK
        // key saved after boot) takes effect WITHOUT an app restart.
        let analyzer_slot = self.context_analyzer.clone();
        #[cfg(feature = "analysis")]
        let context_analyzer_factory = self.context_analyzer_factory.clone();
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
        // #7914: shared FeedbackScorer handle (SAME Arc the feedback command
        // records into). Plumbed so the periodic-LLM producer applies the SAME
        // learned relevance gate as the server SSE path — previously the scorer
        // was write-only off the SSE stream. #7913 (T2.1) will back it with
        // persisted state; this handle stays the injection point.
        #[cfg(feature = "local-suggestions")]
        let scorer_a = self.suggestion_manager.as_ref().map(|m| m.scorer().clone());
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
            // #7652: builds WITHOUT the `analysis` feature never populate the
            // shared slot (`build_context_analyzer` is a `None`-returning stub
            // in that build) and have no rebuild factory either, so an empty
            // slot here can never change — park exactly like the pre-#7652
            // code did. `analysis` builds always carry a rebuild factory
            // (installed by the composition root regardless of the
            // startup-time enabled/provider state — see
            // `AgentSupportContextBuilder::build`), so this loop keeps
            // ticking below and reconciles the slot against the LIVE config
            // every interval instead of exiting early.
            #[cfg(not(feature = "analysis"))]
            if analyzer_slot.read().is_none() {
                let _ = shutdown_rx.changed().await;
                return;
            }

            // Use initial config for interval timing (changes require restart).
            // Other settings (enabled, min_confidence, max_suggestions, throttle_secs)
            // are read dynamically from ConfigManager on each tick so that
            // changes made via the embedded HTTP `PUT /settings` endpoint
            // (#7600: the Tauri `update_analysis_config` IPC duplicate was
            // removed) propagate immediately without an agent restart.
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
                        // ConsentGate is fail-closed both on a stale (Expired/UpdateRequired)
                        // consent record AND on a missing ConsentManager (#7728).
                        let consent = ConsentGate::from_ref(consent_mgr_a.as_ref()).permissions_snapshot();
                        let paused = capture_paused_a.load(Ordering::Relaxed);
                        let permitted = config_manager.as_ref()
                            .map(|cm| crate::scheduler::capture_permitted_now(&cm.snapshot(), &consent, paused))
                            .unwrap_or(false);
                        if !permitted {
                            debug!("analysis loop: capture gate closed (TS/consent/paused) — skipping tick");
                            continue;
                        }

                        // Read current config from ConfigManager (the single source
                        // of truth also written to by the HTTP `PUT /settings` endpoint).
                        let current_config = config_manager
                            .as_ref()
                            .map(|cm| cm.get().analysis)
                            .unwrap_or_else(|| config.clone());

                        // #7652: reconcile the shared analyzer slot against the LIVE
                        // `enabled` flag before deciding whether to skip this tick —
                        // this is the runtime-enable/runtime-disable mechanism itself.
                        // Before this fix, a `None` slot at spawn time made this whole
                        // task exit permanently (see the `#[cfg(not(feature = "analysis"))]`
                        // guard above for the equivalent-behavior fast path this
                        // superseded), so a later `analysis.enabled = true` Settings
                        // save was never observed without an app restart.
                        #[cfg(feature = "analysis")]
                        reconcile_analyzer_slot(
                            &analyzer_slot,
                            current_config.enabled,
                            context_analyzer_factory.as_ref(),
                            config_manager.as_ref().map(|cm| cm.snapshot()),
                        ).await;

                        if !current_config.enabled {
                            debug!("analysis loop: disabled via runtime config, skipping tick");
                            continue;
                        }

                        let analyzer = match analyzer_slot.read().clone() {
                            Some(a) => a,
                            None => {
                                debug!("analysis loop: enabled via runtime config but no analyzer is available yet (no BYOK provider configured) — waiting");
                                continue;
                            }
                        };

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
                                    // #7914: regime Deep-Focus filter + learned per-regime
                                    // acceptance + FeedbackScorer now ALL apply inside
                                    // `enqueue_and_surface` (the ONE seam every LOCAL
                                    // producer shares). The periodic-LLM path previously
                                    // applied only the static regime filter; it now runs
                                    // the SAME decision function. The learned acceptance
                                    // rate lives on `adaptive_trigger_state`, owned by the
                                    // monitor loop and not reachable here, so that gate
                                    // stays pass-through (`None`) on this path — the
                                    // scorer (the headline signal) applies fully.
                                    let regime = shared_regime_for_filter.snapshot().regime;
                                    let gates = super::helpers::relevance_gates(
                                        scorer_a.as_ref(),
                                        regime.as_ref(),
                                        None,
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
                                        suggestions.clone(),
                                        gates,
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
        // #7914: shared FeedbackScorer handle so the rule producer (FocusAnalyzer
        // periodic playbook flushes) applies the SAME learned gate as every other
        // producer — this is the exact "reject a rule nudge 10× → it goes quiet"
        // path the issue calls out.
        #[cfg(feature = "local-suggestions")]
        let scorer_f = self.suggestion_manager.as_ref().map(|m| m.scorer().clone());
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
                        // ConsentGate is fail-closed both on a stale (Expired/UpdateRequired)
                        // consent record AND on a missing ConsentManager (#7728).
                        let consent = ConsentGate::from_ref(consent_mgr_f.as_ref()).permissions_snapshot();
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
                                    // #7914: the rule producer now runs the SAME gate
                                    // seam as every other LOCAL producer — regime
                                    // Deep-Focus filter + FeedbackScorer inside
                                    // `enqueue_and_surface`. Learned acceptance rate is
                                    // pass-through here (classifier owned by the monitor
                                    // loop); the scorer applies, so repeated rejection of
                                    // a rule nudge now suppresses it.
                                    let regime = shared_regime_f.snapshot().regime;
                                    let gates = super::helpers::relevance_gates(
                                        scorer_f.as_ref(),
                                        regime.as_ref(),
                                        None,
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
                                        rule_suggestions,
                                        gates,
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
                        // ConsentGate is fail-closed both on a stale (Expired/UpdateRequired)
                        // consent record AND on a missing ConsentManager (#7728).
                        let consent = ConsentGate::from_ref(consent_mgr_c.as_ref()).permissions_snapshot();
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

/// #7652: reconcile the shared analyzer slot against the LIVE `enabled` flag —
/// the sole mechanism that lets a Settings → `analysis.enabled` flip (or a
/// BYOK `ai_provider.llm_api` key saved after boot) take effect WITHOUT an
/// app restart. Only `spawn_analysis_loop` calls this (single writer to the
/// slot; `spawn_monitor_loop` only reads it).
///
/// - `enabled=true` + slot empty  → attempt a build via `factory`, using the
///   CURRENT `live_config` snapshot (so a freshly-saved BYOK key is honored).
///   On success the built analyzer is installed; on failure (still no usable
///   provider) the slot stays empty and the next tick retries.
/// - `enabled=false` + slot occupied → tear down (drop the analyzer) so a
///   later re-enable rebuilds fresh instead of resurrecting stale state.
/// - `enabled=true` + slot already occupied → no-op. Hot-swapping BYOK
///   settings WHILE already enabled (as opposed to on the disable→enable
///   transition) is intentionally out of scope for #7652 — flip
///   `analysis.enabled` off/on (or restart) to pick up a provider change
///   made while analysis was already live.
/// - `enabled=false` + slot empty → no-op.
#[cfg(feature = "analysis")]
async fn reconcile_analyzer_slot(
    analyzer_slot: &Arc<parking_lot::RwLock<Option<Arc<maekon_analysis::ContextAnalyzer>>>>,
    enabled: bool,
    factory: Option<&crate::agent_runtime_support::ContextAnalyzerFactory>,
    live_config: Option<Arc<maekon_core::config::AppConfig>>,
) {
    let has_analyzer = analyzer_slot.read().is_some();

    if !enabled {
        if has_analyzer {
            *analyzer_slot.write() = None;
            info!(
                "analysis loop: disabled via runtime config — analyzer torn down (no restart needed)"
            );
        }
        return;
    }

    if has_analyzer {
        return;
    }

    let (Some(factory), Some(live_config)) = (factory, live_config) else {
        return;
    };

    match factory(live_config).await {
        Some(built) => {
            *analyzer_slot.write() = Some(built);
            info!("analysis loop: runtime-enabled — analyzer built without restart");
        }
        None => {
            debug!(
                "analysis loop: enabled via runtime config but no LLM provider is configured yet — waiting"
            );
        }
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

/// #7652: regression tests for the runtime analysis-enable trap.
///
/// BEFORE this fix, `spawn_analysis_loop` matched the injected analyzer ONCE,
/// before ever entering its `tokio::select!` loop:
/// ```ignore
/// let analyzer = match analyzer {
///     Some(a) => a,
///     None => { let _ = shutdown_rx.changed().await; return; }
/// };
/// ```
/// If `Scheduler.context_analyzer` was `None` at spawn time (analysis
/// disabled at startup, or no BYOK provider configured yet), the spawned task
/// exited BEFORE the loop body ever ran — so no later Settings save
/// (`analysis.enabled = true`, or a freshly-configured `ai_provider.llm_api`
/// key) could ever be observed by that task; the only way to activate
/// analysis was an app restart. `reconcile_analyzer_slot` is the per-tick
/// decision function that replaces that one-shot match; these tests exercise
/// the ACTUAL production function directly (not a reimplementation), only
/// substituting the factory/analyzer instances — the intentional DI seam.
#[cfg(feature = "analysis")]
#[cfg(test)]
mod analyzer_slot_reconcile_tests {
    use super::*;
    use maekon_core::error::CoreError;
    use maekon_core::models::suggestion::Suggestion;
    use maekon_core::ports::analysis_provider::AnalysisProvider;
    use maekon_core::ports::storage::StorageService;

    /// Minimal manual mock (no mockall, per project convention) — only the
    /// two non-defaulted trait methods.
    struct NoopAnalysisProvider;

    #[async_trait::async_trait]
    impl AnalysisProvider for NoopAnalysisProvider {
        async fn analyze(
            &self,
            _context_json: &str,
            _system_prompt: &str,
        ) -> Result<Vec<Suggestion>, CoreError> {
            Ok(vec![])
        }

        fn provider_name(&self) -> &str {
            "noop-test-provider"
        }
    }

    fn test_analyzer() -> Arc<maekon_analysis::ContextAnalyzer> {
        let storage: Arc<dyn StorageService> = Arc::new(
            maekon_storage::sqlite::SqliteStorage::open_in_memory(30)
                .expect("in-memory sqlite storage"),
        );
        let provider: Arc<dyn AnalysisProvider> = Arc::new(NoopAnalysisProvider);
        Arc::new(maekon_analysis::ContextAnalyzer::new(
            storage,
            provider,
            maekon_analysis::PatternMiner::new(),
            maekon_analysis::ContextAssembler::new(Box::new(|text: &str| text.to_string())),
            maekon_core::config::AnalysisConfig::default(),
        ))
    }

    fn empty_slot() -> Arc<parking_lot::RwLock<Option<Arc<maekon_analysis::ContextAnalyzer>>>> {
        Arc::new(parking_lot::RwLock::new(None))
    }

    /// `enabled=false` + empty slot was the ORIGINAL trap's steady state: the
    /// old code never re-entered its tick loop at all here, so this is the
    /// baseline the fix must preserve (still nothing to do while disabled).
    #[tokio::test]
    async fn disabled_with_empty_slot_stays_empty() {
        let slot = empty_slot();
        reconcile_analyzer_slot(&slot, false, None, None).await;
        assert!(slot.read().is_none());
    }

    /// THE regression case: a runtime false→true flip (a Settings save) with
    /// a provider now configured must install an analyzer WITHOUT an app
    /// restart. Before #7652 this transition was unreachable — the task had
    /// already permanently exited at spawn time whenever the slot started
    /// empty, so no per-tick code (this function included) ever ran again.
    #[tokio::test]
    async fn enable_transition_with_provider_builds_analyzer_without_restart() {
        let slot = empty_slot();
        let factory: crate::agent_runtime_support::ContextAnalyzerFactory =
            Arc::new(|_config: Arc<maekon_core::config::AppConfig>| {
                Box::pin(async { Some(test_analyzer()) })
            });
        let live_config = Arc::new(maekon_core::config::AppConfig::default_config());

        reconcile_analyzer_slot(&slot, true, Some(&factory), Some(live_config)).await;

        assert!(
            slot.read().is_some(),
            "a runtime enable with a configured provider must install an analyzer \
             into the shared slot without an app restart"
        );
    }

    /// A runtime enable with NO usable provider yet (factory returns `None`,
    /// e.g. BYOK not configured) must leave the slot empty and must not
    /// panic — the next tick simply retries.
    #[tokio::test]
    async fn enable_transition_without_provider_stays_empty() {
        let slot = empty_slot();
        let factory: crate::agent_runtime_support::ContextAnalyzerFactory =
            Arc::new(|_config: Arc<maekon_core::config::AppConfig>| Box::pin(async { None }));
        let live_config = Arc::new(maekon_core::config::AppConfig::default_config());

        reconcile_analyzer_slot(&slot, true, Some(&factory), Some(live_config)).await;

        assert!(slot.read().is_none());
    }

    /// Teardown on disable: a live analyzer must be dropped from the slot the
    /// moment `analysis.enabled` flips to `false`, so a later re-enable
    /// rebuilds fresh instead of resurrecting stale provider/session state.
    #[tokio::test]
    async fn disable_transition_tears_down_live_analyzer() {
        let slot = empty_slot();
        *slot.write() = Some(test_analyzer());
        assert!(slot.read().is_some(), "precondition: slot starts occupied");

        reconcile_analyzer_slot(&slot, false, None, None).await;

        assert!(
            slot.read().is_none(),
            "a runtime disable must tear down the live analyzer without an app restart"
        );
    }

    /// Already-enabled + already-occupied must be a no-op — reconcile must
    /// NOT rebuild while the analyzer is already live (avoid double-spawn /
    /// discarding in-flight state). Verified via `Arc::ptr_eq` on the
    /// installed instance: the factory below would hand back a DIFFERENT
    /// instance if (incorrectly) invoked.
    #[tokio::test]
    async fn already_enabled_with_analyzer_is_a_noop() {
        let slot = empty_slot();
        let original = test_analyzer();
        *slot.write() = Some(original.clone());

        let factory: crate::agent_runtime_support::ContextAnalyzerFactory =
            Arc::new(|_config: Arc<maekon_core::config::AppConfig>| {
                Box::pin(async { Some(test_analyzer()) })
            });
        let live_config = Arc::new(maekon_core::config::AppConfig::default_config());

        reconcile_analyzer_slot(&slot, true, Some(&factory), Some(live_config)).await;

        let current = slot.read().clone().expect("still occupied");
        assert!(
            Arc::ptr_eq(&original, &current),
            "an already-live analyzer must not be rebuilt while still enabled \
             (no double-spawn/leak)"
        );
    }
}
