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
        config: oneshim_core::config::AnalysisConfig,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        let analyzer = self.context_analyzer.clone();
        let storage_ref = self.storage.clone();
        let sqlite_ref = self.sqlite_storage.clone();
        let config_manager = self.config_manager.clone();
        // D13: 4-term privacy gate DI.
        let consent_mgr_a = self.consent_manager.clone();
        let capture_paused_a = self.capture_paused.clone();

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
            let mut interval = tokio::time::interval(Duration::from_secs(config.interval_secs));
            let full_interval = Duration::from_secs(config.full_interval_secs);
            let mut last_full = std::time::Instant::now();

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        // D13: 4-term composite gate (CONS-PC02 / §3.3 A.9).
                        let consent = consent_mgr_a.as_ref()
                            .and_then(|cm| cm.current_consent().map(|r| r.permissions.clone()))
                            .unwrap_or_default();
                        let paused = capture_paused_a.load(Ordering::Relaxed);
                        let permitted = config_manager.as_ref()
                            .map(|cm| crate::scheduler::capture_permitted_now(&cm.snapshot(), &consent, paused))
                            .unwrap_or(!paused);
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
                        match sqlite_ref.has_recent_server_suggestions(
                            current_config.server_coexistence_lookback_secs,
                        ) {
                            Ok(true) => {
                                debug!(
                                    "server suggestions active (last {}s) — skipping local analysis",
                                    current_config.server_coexistence_lookback_secs,
                                );
                                continue;
                            }
                            Ok(false) => { /* proceed with local analysis */ }
                            Err(e) => {
                                warn!(err.code = %e.code(), "server coexistence check failed: {e}");
                                // Proceed anyway — fail-open
                            }
                        }

                        let force_full = last_full.elapsed() >= full_interval;

                        let result = if force_full {
                            last_full = std::time::Instant::now();
                            analyzer.analyze().await
                        } else {
                            analyzer.analyze_if_changed().await
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
                                        "suggestion: {}",
                                        suggestion.content
                                    );
                                    if let Err(e) = storage_ref.save_suggestion(suggestion).await {
                                        warn!(err.code = %e.code(), "suggestion save failure: {e}");
                                    }
                                }
                            }
                            Err(e) => {
                                // AnalysisError doesn't expose code() directly; convert
                                // through the existing From<AnalysisError> for CoreError
                                // to surface the wire code to telemetry.
                                let core: oneshim_core::error::CoreError = e.into();
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
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        let focus8 = self.focus_analyzer.clone();
        // D13: 4-term privacy gate DI.
        let config_mgr_f = self.config_manager.clone();
        let consent_mgr_f = self.consent_manager.clone();
        let capture_paused_f = self.capture_paused.clone();

        tokio::spawn(async move {
            let focus = match focus8 {
                Some(f) => f,
                None => {
                    let _ = shutdown_rx.changed().await;
                    return;
                }
            };

            let mut interval = tokio::time::interval(Duration::from_secs(60)); // 1min
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        // D13: 4-term composite gate (CONS-PC02 / §3.3 A.9).
                        let consent = consent_mgr_f.as_ref()
                            .and_then(|cm| cm.current_consent().map(|r| r.permissions.clone()))
                            .unwrap_or_default();
                        let paused = capture_paused_f.load(Ordering::Relaxed);
                        let permitted = config_mgr_f.as_ref()
                            .map(|cm| crate::scheduler::capture_permitted_now(&cm.snapshot(), &consent, paused))
                            .unwrap_or(!paused);
                        if !permitted {
                            debug!("focus loop: capture gate closed (TS/consent/paused) — skipping tick");
                            continue;
                        }
                        focus.analyze_periodic().await;
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

            let mut interval = tokio::time::interval(Duration::from_secs(
                super::super::config::COACHING_INTERVAL_SECS,
            ));

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        // D13: 4-term composite gate (CONS-PC02 / §3.3 A.9).
                        // Coaching during an opt-out window is invasive (R3.I4).
                        let consent = consent_mgr_c.as_ref()
                            .and_then(|cm| cm.current_consent().map(|r| r.permissions.clone()))
                            .unwrap_or_default();
                        let paused = capture_paused_c.load(Ordering::Relaxed);
                        let permitted = config_mgr_c.as_ref()
                            .map(|cm| crate::scheduler::capture_permitted_now(&cm.snapshot(), &consent, paused))
                            .unwrap_or(!paused);
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
