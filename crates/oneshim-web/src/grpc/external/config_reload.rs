//! `ConfigReloadTask` — watches `ConfigManager` for changes and swaps
//! `LiveExternalConfig`'s snapshot atomically.
//!
//! Spec §5.4. Partial-apply semantics per D23: if LoadPolicy::try_new
//! rejects new thresholds, the previous policy is carried forward while
//! streaming_enabled (trivially valid) still updates. D21's single atomic
//! swap makes this visible as one consistent transition.
//!
//! Spawn site: `build_external_spawn_config` (NOT inside `serve_external`)
//! per D30 — matches cert-watcher/expiry-monitor precedent, avoids
//! supervisor-respawn duplicate-task hazard.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use oneshim_core::config::AppConfig;
use tokio::sync::watch;

use super::live_config::{LiveExternalConfig, LiveSnapshot};
use super::metrics::ExternalMetrics;
use crate::grpc::load_policy::LoadPolicy;

pub async fn run_config_reload(
    live: Arc<LiveExternalConfig>,
    metrics: Arc<ExternalMetrics>,
    mut config_rx: watch::Receiver<Arc<AppConfig>>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    metrics
        .config_reload_task_alive
        .store(true, Ordering::Relaxed);
    tracing::debug!("external_grpc: config reload task started");

    loop {
        tokio::select! {
            biased;
            _ = shutdown_rx.changed() => {
                tracing::debug!("external_grpc: config reload task shutting down (signalled)");
                break;
            }
            res = config_rx.changed() => {
                if res.is_err() {
                    tracing::warn!(
                        "external_grpc: ConfigManager sender dropped; exiting reload task"
                    );
                    break;
                }
                apply_config(&live, &config_rx.borrow_and_update());
                // Ref dropped at end of statement; no await held across borrow.
                metrics.config_reload_total.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    metrics
        .config_reload_task_alive
        .store(false, Ordering::Relaxed);
}

fn apply_config(live: &LiveExternalConfig, cfg: &AppConfig) {
    let current = live.snapshot();

    // streaming_enabled: external override with fallback to shared web field.
    let new_streaming = cfg
        .external_grpc
        .streaming_enabled
        .unwrap_or(cfg.web.grpc_streaming_enabled);

    // load_policy: try_new fallible; preserve started_at across reloads (D27).
    let new_thresholds = cfg.web.grpc_load_thresholds.clone().unwrap_or_default();
    let old_started_at = current.load_policy.started_at();
    let new_load_policy = match LoadPolicy::try_new_with_started_at(new_thresholds, old_started_at)
    {
        Ok(p) => Arc::new(p),
        Err(e) => {
            tracing::error!(
                err = %e,
                "external_grpc: invalid LoadThresholds in reloaded config; keeping previous load_policy"
            );
            current.load_policy.clone()
        }
    };

    // Single atomic store — no torn reads (D21).
    live.store(LiveSnapshot {
        streaming_enabled: new_streaming,
        load_policy: new_load_policy,
    });

    tracing::info!(
        streaming_enabled = new_streaming,
        "external_grpc: live config applied"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use oneshim_core::config::{AppConfig, LoadThresholds};

    fn fixture_policy() -> Arc<LoadPolicy> {
        Arc::new(LoadPolicy::new(LoadThresholds {
            cpu_low_pct: 30.0,
            cpu_medium_pct: 60.0,
            cpu_high_pct: 85.0,
            min_free_mem_gb: 1.0,
        }))
    }

    fn fixture_live() -> Arc<LiveExternalConfig> {
        Arc::new(LiveExternalConfig::new(LiveSnapshot {
            streaming_enabled: true,
            load_policy: fixture_policy(),
        }))
    }

    fn fixture_cfg() -> AppConfig {
        AppConfig::default_config()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn applies_config_change_to_live() {
        let live = fixture_live();
        let metrics = Arc::new(ExternalMetrics::new());
        let mut cfg0 = fixture_cfg();
        cfg0.web.grpc_streaming_enabled = true;
        let (config_tx, config_rx) = watch::channel(Arc::new(cfg0));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let handle = tokio::spawn(run_config_reload(
            live.clone(),
            metrics.clone(),
            config_rx,
            shutdown_rx,
        ));

        // Wait briefly for task to start + set alive=true.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(metrics.config_reload_task_alive.load(Ordering::Relaxed));

        // Fire config change.
        let mut cfg1 = fixture_cfg();
        cfg1.web.grpc_streaming_enabled = false;
        config_tx.send_replace(Arc::new(cfg1));
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let snap = live.snapshot();
        assert!(!snap.streaming_enabled);
        assert_eq!(metrics.config_reload_total.load(Ordering::Relaxed), 1);

        // Clean shutdown
        shutdown_tx.send_replace(true);
        handle.await.expect("task joined");
        assert!(!metrics.config_reload_task_alive.load(Ordering::Relaxed));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn external_override_wins_over_web_field() {
        let live = fixture_live();
        let metrics = Arc::new(ExternalMetrics::new());
        let mut cfg = fixture_cfg();
        cfg.web.grpc_streaming_enabled = false; // shared field says off
        cfg.external_grpc.streaming_enabled = Some(true); // override says on
        let (config_tx, config_rx) = watch::channel(Arc::new(cfg));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let handle = tokio::spawn(run_config_reload(
            live.clone(),
            metrics,
            config_rx,
            shutdown_rx,
        ));
        // Force a change event so apply_config runs.
        config_tx.send_modify(|c| {
            Arc::make_mut(c);
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let snap = live.snapshot();
        assert!(snap.streaming_enabled, "external override must win");

        shutdown_tx.send_replace(true);
        handle.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fallback_to_web_field_when_external_none() {
        let live = fixture_live();
        let metrics = Arc::new(ExternalMetrics::new());
        let mut cfg = fixture_cfg();
        cfg.web.grpc_streaming_enabled = false;
        cfg.external_grpc.streaming_enabled = None; // fall back
        let (config_tx, config_rx) = watch::channel(Arc::new(cfg));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let handle = tokio::spawn(run_config_reload(
            live.clone(),
            metrics,
            config_rx,
            shutdown_rx,
        ));
        config_tx.send_modify(|c| {
            Arc::make_mut(c);
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!live.snapshot().streaming_enabled);
        shutdown_tx.send_replace(true);
        handle.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn malformed_thresholds_partial_apply() {
        let live = fixture_live();
        let initial_policy = live.snapshot().load_policy.clone();
        let metrics = Arc::new(ExternalMetrics::new());
        let mut cfg = fixture_cfg();
        cfg.web.grpc_streaming_enabled = false;
        // Invalid: low > medium
        cfg.web.grpc_load_thresholds = Some(LoadThresholds {
            cpu_low_pct: 99.0,
            cpu_medium_pct: 50.0,
            cpu_high_pct: 85.0,
            min_free_mem_gb: 1.0,
        });
        let (config_tx, config_rx) = watch::channel(Arc::new(cfg));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let handle = tokio::spawn(run_config_reload(
            live.clone(),
            metrics,
            config_rx,
            shutdown_rx,
        ));
        config_tx.send_modify(|c| {
            Arc::make_mut(c);
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let snap = live.snapshot();
        assert!(!snap.streaming_enabled, "streaming update applied");
        assert!(
            Arc::ptr_eq(&snap.load_policy, &initial_policy),
            "invalid policy rejected; previous preserved"
        );
        shutdown_tx.send_replace(true);
        handle.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn biased_shutdown_preempts_config_change() {
        let live = fixture_live();
        let metrics = Arc::new(ExternalMetrics::new());
        let (config_tx, config_rx) = watch::channel(Arc::new(fixture_cfg()));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let handle = tokio::spawn(run_config_reload(
            live,
            metrics.clone(),
            config_rx,
            shutdown_rx,
        ));
        // Fire both nearly simultaneously with shutdown signalled first.
        shutdown_tx.send_replace(true);
        config_tx.send_modify(|c| {
            Arc::make_mut(c);
        });
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        handle.await.unwrap();
        // Either zero or one reload applied — shutdown preempted further work.
        let count = metrics.config_reload_total.load(Ordering::Relaxed);
        assert!(
            count <= 1,
            "biased ordering bounds apply_config calls during shutdown"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exits_on_config_sender_drop() {
        let live = fixture_live();
        let metrics = Arc::new(ExternalMetrics::new());
        let (config_tx, config_rx) = watch::channel(Arc::new(fixture_cfg()));
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let handle = tokio::spawn(run_config_reload(
            live,
            metrics.clone(),
            config_rx,
            shutdown_rx,
        ));
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        drop(config_tx);
        handle.await.unwrap();
        assert!(!metrics.config_reload_task_alive.load(Ordering::Relaxed));
    }
}
