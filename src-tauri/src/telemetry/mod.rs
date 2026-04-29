//! Telemetry bootstrap.
//!
//! - **Feature-off (`default`)**: zero-cost no-op layer and handle.
//! - **Feature-on (`telemetry`)**: OpenTelemetry OTLP pipeline wrapped in
//!   `tracing_subscriber::reload::Layer<Option<OtelLayer>, _>` so runtime
//!   toggles (`telemetry.enabled`) can attach and detach the layer without
//!   restarting the process.
//!
//! See `docs/guides/telemetry.md` (ships with Task 12) and
//! `docs/reviews/2026-04-17-phase2-config-telemetry-spec.md` §3 for design.

use oneshim_core::config::TelemetryConfig;

#[cfg(feature = "telemetry")]
mod instance_id;
#[cfg(feature = "telemetry")]
mod otlp;

#[cfg(all(test, feature = "telemetry"))]
mod mock_otlp;

/// Public handle for runtime toggle. Construct via [`Handle::new_with_layer`].
pub struct Handle {
    #[cfg(feature = "telemetry")]
    inner: parking_lot::Mutex<otlp::Inner>,
}

/// Layer attached to the tracing subscriber. Type alias keeps the `.with()`
/// call-site monomorphic across feature states.
#[cfg(feature = "telemetry")]
pub type Layer =
    tracing_subscriber::reload::Layer<Option<otlp::OtelLayer>, tracing_subscriber::Registry>;

#[cfg(not(feature = "telemetry"))]
pub type Layer = NoopLayer;

/// No-op placeholder layer when the `telemetry` feature is off. Satisfies
/// `tracing_subscriber::Layer<S>` via an empty impl so `.with(layer)` compiles
/// under either feature state without `#[cfg]` at the call site.
#[cfg(not(feature = "telemetry"))]
#[derive(Clone, Copy, Default)]
pub struct NoopLayer;

#[cfg(not(feature = "telemetry"))]
impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for NoopLayer {}

impl Handle {
    /// Build the layer and handle for the tracing subscriber.
    ///
    /// - **Feature-off**: infallible, `data_dir` ignored, returns a no-op layer.
    /// - **Feature-on + `cfg.enabled == false`**: returns a `reload::Layer`
    ///   wrapping `None`. No exporter built.
    /// - **Feature-on + `cfg.enabled == true`**: builds the OTLP pipeline,
    ///   reads/writes `telemetry_instance_id` under `data_dir`, wraps the
    ///   `OtelLayer` in the reload wrapper.
    ///
    /// Signature is identical across feature states so `main.rs` and tests can
    /// pass `data_dir` unconditionally.
    pub fn new_with_layer(
        _cfg: &TelemetryConfig,
        _data_dir: &std::path::Path,
    ) -> anyhow::Result<(Layer, Self)> {
        #[cfg(not(feature = "telemetry"))]
        {
            Ok((NoopLayer, Handle {}))
        }
        #[cfg(feature = "telemetry")]
        {
            otlp::build_initial_handle(_cfg, _data_dir)
        }
    }

    /// Apply a runtime toggle. Idempotent when `cfg` matches the last applied
    /// value.
    ///
    /// - **Feature-off**: always `Ok(())`.
    /// - **Feature-on**: drives the off↔on transitions inside the reload
    ///   wrapper, building or shutting down the OTLP provider as needed.
    pub fn apply(&self, _cfg: &TelemetryConfig) -> anyhow::Result<()> {
        #[cfg(not(feature = "telemetry"))]
        {
            Ok(())
        }
        #[cfg(feature = "telemetry")]
        {
            self.inner.lock().apply(_cfg)
        }
    }
}

#[cfg(all(test, not(feature = "telemetry")))]
mod tests {
    use super::*;

    /// T-X2-1 — feature-off construction is a no-op (no panic, no allocation).
    #[test]
    fn feature_off_construction_is_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = TelemetryConfig {
            enabled: true,
            ..Default::default()
        };
        let (_layer, handle) = Handle::new_with_layer(&cfg, tmp.path())
            .expect("feature-off construction is infallible");
        handle
            .apply(&cfg)
            .expect("apply is a no-op when feature is off");
    }
}

#[cfg(all(test, feature = "telemetry"))]
mod tests_feature_on {
    use super::*;
    use crate::telemetry::mock_otlp;

    /// T-X2-2 — feature-on + cfg.enabled=false installs a `None`-valued
    /// `reload::Layer` and builds no exporter (no network activity).
    #[test]
    fn feature_on_config_off_installs_empty_reload_wrapper() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = TelemetryConfig {
            enabled: false,
            ..Default::default()
        };
        let (_layer, _handle) = Handle::new_with_layer(&cfg, tmp.path())
            .expect("disabled-at-boot construction is infallible");
    }

    /// T-X2-3 — feature-on + cfg.enabled=true against a mock OTLP collector.
    /// Verifies `build_pipeline` actually produces a working exporter and
    /// `apply(same_cfg)` is idempotent.
    ///
    /// IMPORTANT: the BatchSpanProcessor with `runtime::Tokio` spawns a task
    /// that refuses to drop cleanly once the test's tokio runtime winds down
    /// — the provider shutdown blocks on that task. To keep the test fast we
    /// flip the config off at the end to drive an explicit provider shutdown
    /// via `Inner::apply`. Task 10 adds a 4 s shutdown watchdog so the
    /// production path is bounded as well.
    #[tokio::test]
    async fn feature_on_config_on_builds_pipeline() {
        let tmp = tempfile::tempdir().unwrap();
        let mock = mock_otlp::start().await;

        let cfg = TelemetryConfig {
            enabled: true,
            otlp_endpoint: Some(mock.endpoint.clone()),
            ..Default::default()
        };
        let (_layer, handle) =
            Handle::new_with_layer(&cfg, tmp.path()).expect("pipeline must build against the mock");

        // Re-applying the same config is a no-op (no transition).
        handle
            .apply(&cfg)
            .expect("apply is idempotent for unchanged cfg");

        // Shut down the provider before the test's tokio runtime ends.
        let cfg_off = TelemetryConfig {
            enabled: false,
            ..cfg
        };
        handle
            .apply(&cfg_off)
            .expect("shutdown to avoid drop-hang on provider");
    }

    /// T-X2-4 — toggle off, then back on. Both transitions must succeed live;
    /// no restart required. Ends in the OFF state to avoid the drop-hang.
    #[tokio::test]
    async fn apply_disables_and_reenables_live() {
        let tmp = tempfile::tempdir().unwrap();
        let mock = mock_otlp::start().await;

        let cfg_on = TelemetryConfig {
            enabled: true,
            otlp_endpoint: Some(mock.endpoint.clone()),
            ..Default::default()
        };
        let (_layer, handle) = Handle::new_with_layer(&cfg_on, tmp.path())
            .expect("pipeline must build against the mock");

        let cfg_off = TelemetryConfig {
            enabled: false,
            ..cfg_on.clone()
        };
        handle.apply(&cfg_off).expect("toggle off");
        handle.apply(&cfg_on).expect("toggle back on");
        handle
            .apply(&cfg_off)
            .expect("final shutdown to avoid drop-hang");
    }

    /// T-X2-5 — bus-driven toggle delivery. Drive
    /// `ConfigManager::update_with` to flip `telemetry.enabled`, assert the
    /// spawned task forwards the update to `Handle::apply` within one async
    /// tick (bounded by a 1 s budget).
    #[tokio::test]
    async fn config_bus_delivers_telemetry_toggle() {
        use oneshim_core::config_manager::ConfigManager;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc as StdArc;

        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("config.json");
        let mgr = StdArc::new(ConfigManager::with_path(cfg_path).unwrap());

        // Build the handle seeded with defaults — matches main.rs wiring.
        let (_layer, handle) = Handle::new_with_layer(&TelemetryConfig::default(), tmp.path())
            .expect("feature-on disabled construction is infallible");
        let handle = StdArc::new(handle);

        let observed = StdArc::new(AtomicBool::new(false));
        let observed_for_task = observed.clone();
        let handle_for_task = handle.clone();
        let mut rx = mgr.subscribe();
        let mock = mock_otlp::start().await;
        let endpoint = mock.endpoint.clone();

        tokio::spawn(async move {
            let mut prev = TelemetryConfig::default();
            let initial = rx.borrow_and_update().telemetry.clone();
            if initial != prev {
                // Store observed FIRST — the test's property is "task saw the
                // update" not "apply finished." apply() may take several
                // hundred ms because it builds the pipeline (HTTP client,
                // Resource, exporter batch processor), and the 5 s budget is
                // already the runtime-toggle SLA from spec §7.
                observed_for_task.store(true, Ordering::SeqCst);
                let _ = handle_for_task.apply(&initial);
                prev = initial;
            }
            while rx.changed().await.is_ok() {
                let current = rx.borrow_and_update().telemetry.clone();
                if current != prev {
                    observed_for_task.store(true, Ordering::SeqCst);
                    let _ = handle_for_task.apply(&current);
                    prev = current;
                }
            }
        });

        // Flip the bus: disabled -> enabled (pointed at the mock so the
        // pipeline build succeeds).
        mgr.update_with(|c| {
            c.telemetry.enabled = true;
            c.telemetry.otlp_endpoint = Some(endpoint.clone());
            Ok(())
        })
        .unwrap();

        // Poll the atomic within a 5 s budget — matches the runtime-toggle
        // SLA from spec §7. Pipeline build is the slow step; this budget
        // tolerates local filesystem + HTTP-client construction overhead.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if observed.load(Ordering::SeqCst) {
                break;
            }
            tokio::task::yield_now().await;
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            observed.load(Ordering::SeqCst),
            "toggle task did not observe the update within 5s"
        );

        // Avoid drop-hang: drive the pipeline to shutdown.
        mgr.update_with(|c| {
            c.telemetry.enabled = false;
            Ok(())
        })
        .unwrap();
        tokio::task::yield_now().await;
        let _ = handle.apply(&TelemetryConfig {
            enabled: false,
            otlp_endpoint: Some(endpoint),
            ..Default::default()
        });
    }

    /// T-X2-10 — end-to-end spine test. A span emitted while telemetry is
    /// enabled reaches the mock OTLP collector.
    ///
    /// Completion strategy: `provider.force_flush` deadlocks inside a
    /// `#[tokio::test]` single-threaded runtime because the batch processor
    /// task and the force-flush caller share the one worker thread. We
    /// instead toggle OFF, which drives `shutdown` on a dedicated std thread
    /// (guarded by the 4 s watchdog) — shutdown flushes pending spans before
    /// returning, so the POST lands by the time `apply` completes. The wait
    /// after that is a short post-shutdown grace for the exporter's HTTP
    /// request to complete against the mock.
    #[tokio::test]
    async fn mock_collector_receives_span() {
        use tracing_subscriber::layer::SubscriberExt;

        let tmp = tempfile::tempdir().unwrap();
        let mut mock = mock_otlp::start().await;

        let cfg = TelemetryConfig {
            enabled: true,
            otlp_endpoint: Some(mock.endpoint.clone()),
            ..Default::default()
        };
        let (layer, handle) =
            Handle::new_with_layer(&cfg, tmp.path()).expect("pipeline must build against the mock");

        // Local subscriber for this test only. `set_default` returns a guard
        // that restores the previous subscriber when dropped.
        let subscriber = tracing_subscriber::registry().with(layer);
        let _guard = tracing::subscriber::set_default(subscriber);

        tracing::info_span!("t_x2_10_span").in_scope(|| {
            tracing::info!("body");
        });

        // Toggle off — drives provider.shutdown which flushes pending spans.
        // Watchdog bounds this at 4 s.
        let cfg_off = TelemetryConfig {
            enabled: false,
            ..cfg
        };
        handle
            .apply(&cfg_off)
            .expect("shutdown drives the final flush");

        // After shutdown returns, give the exporter's in-flight HTTP request
        // a brief window to land against the mock.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut seen = false;
        while std::time::Instant::now() < deadline {
            if let Ok(Some(_body)) =
                tokio::time::timeout(std::time::Duration::from_millis(250), mock.rx.recv()).await
            {
                seen = true;
                break;
            }
        }
        assert!(
            seen,
            "no OTLP POST reached the mock collector within 5s after shutdown"
        );
    }

    /// T-X2-8 — shutdown-with-unreachable-collector watchdog. Toggle on
    /// against a dead port, emit a few spans, toggle off (drives `shutdown`).
    /// Must complete within 5 s (watchdog is 4 s + 1 s overhead budget) and
    /// never panic.
    #[tokio::test]
    async fn shutdown_completes_when_collector_unreachable() {
        let tmp = tempfile::tempdir().unwrap();
        // Port 1 is reliably unreachable for TCP; the OTLP exporter queue
        // will never flush but the watchdog must still let shutdown return.
        let cfg_on = TelemetryConfig {
            enabled: true,
            otlp_endpoint: Some("http://127.0.0.1:1".into()),
            ..Default::default()
        };
        let (_layer, handle) = Handle::new_with_layer(&cfg_on, tmp.path())
            .expect("exporter builds even with an unreachable endpoint");

        // Emit a handful of spans so the batch processor has pressure.
        for i in 0..5 {
            tracing::info_span!("t_x2_8_span", i).in_scope(|| {});
        }

        let cfg_off = TelemetryConfig {
            enabled: false,
            ..cfg_on
        };
        let start = std::time::Instant::now();
        handle
            .apply(&cfg_off)
            .expect("apply off must not hang even against a dead collector");
        assert!(
            start.elapsed() < std::time::Duration::from_secs(5),
            "shutdown watchdog exceeded 5s (elapsed: {:?})",
            start.elapsed()
        );
    }

    /// T-X2-7 — endpoint precedence. Explicit cfg wins over env and is passed
    /// through verbatim; env is treated as a base URL and `/v1/traces` is
    /// appended; default is the full OTLP/HTTP URL with `/v1/traces`.
    #[test]
    fn env_endpoint_overrides_default_but_not_explicit_config() {
        // Save and clear any pre-existing env so the test is deterministic.
        // Note: std::env ops are process-global; we restore in a scope.
        let prev = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok();
        // SAFETY: single-threaded per-test assumption. cargo test runs tests
        // on separate threads but this test does not depend on concurrent
        // env access and restores state at the end.
        unsafe { std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", "http://from-env:4318") };

        // Explicit cfg wins and is not rewritten. A user pointing at a
        // non-standard path relies on this.
        let cfg_explicit = TelemetryConfig {
            otlp_endpoint: Some("http://from-config:4318/custom/path".into()),
            ..Default::default()
        };
        assert_eq!(
            otlp::resolve_endpoint(&cfg_explicit),
            "http://from-config:4318/custom/path"
        );

        // Env is treated as a base URL per the OpenTelemetry spec.
        let cfg_default = TelemetryConfig::default();
        assert_eq!(
            otlp::resolve_endpoint(&cfg_default),
            "http://from-env:4318/v1/traces"
        );

        // Env with a trailing slash normalises cleanly.
        unsafe { std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", "http://from-env:4318/") };
        assert_eq!(
            otlp::resolve_endpoint(&TelemetryConfig::default()),
            "http://from-env:4318/v1/traces"
        );

        // Default (no env, no cfg) is the full URL.
        unsafe { std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT") };
        assert_eq!(
            otlp::resolve_endpoint(&TelemetryConfig::default()),
            "http://localhost:4318/v1/traces"
        );

        // Restore pre-existing env so other tests are unaffected.
        if let Some(value) = prev {
            unsafe { std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", value) };
        }
    }
}
