//! Telemetry bootstrap.
//!
//! - **Feature-off (`default`)**: zero-cost no-op layer and handle.
//! - **Feature-on (`telemetry`)**: OpenTelemetry OTLP pipeline wrapped in
//!   `tracing_subscriber::reload::Layer<Option<OtelLayer>, _>` so runtime
//!   toggles (`telemetry.enabled`) can attach and detach the layer without
//!   restarting the process.
//!
//! See `docs/guides/telemetry.md` and client ADR-016 for the public runtime
//! toggle design.

use maekon_core::config::TelemetryConfig;
use maekon_core::ports::consent_manager::ConsentManagerPort;

/// Consent-gated effective telemetry config (#5056 — consent→exporter bridge).
///
/// The OTLP exporter must be active ONLY when BOTH conditions hold:
///   1. the user opted telemetry in (`config.telemetry.enabled == true`), AND
///   2. the GDPR consent record grants the `telemetry` permission.
///
/// This is the single fail-closed chokepoint that both the config-change
/// reconcile task (`setup::spawn_telemetry_toggle_task`) and the consent-change
/// IPC commands (`commands::consent::{set_consent, withdraw_consent}`) call, so
/// the two activation paths can never diverge.
///
/// Fail-closed by construction: the consent term is read through
/// [`ConsentManagerPort::effective_permissions`], which returns an all-false
/// permission set whenever the consent status is anything other than `Valid`
/// (absent / revoked / expired / update-required). Any ambiguity therefore
/// collapses to `telemetry = false` ⇒ exporter OFF, regardless of config.
///
/// The returned config is a clone of `config` with ONLY the `enabled` field
/// overridden to the AND of the two terms; every other field (endpoint,
/// sample_rate, service_name, …) is preserved verbatim so a later re-enable
/// reuses the user's settings.
pub fn consent_gated_telemetry_config(
    config: &TelemetryConfig,
    consent: &dyn ConsentManagerPort,
) -> TelemetryConfig {
    // effective_permissions() is the fail-closed accessor: zeroed unless Valid.
    let consent_grants_telemetry = consent.effective_permissions().telemetry;
    TelemetryConfig {
        enabled: config.enabled && consent_grants_telemetry,
        ..config.clone()
    }
}

/// Metric instruments (E20-43 #4835). Declared under BOTH feature states: the
/// module's own no-op shim handles feature-off, so scheduler/upload call sites
/// can record metrics without their own `#[cfg]`.
pub mod metrics;

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

/// Consent→exporter bridge helper tests (#5056). Feature-agnostic: the gated
/// helper is pure logic over `TelemetryConfig` + `ConsentManager` and does not
/// touch the OTLP pipeline, so these run under BOTH feature states (they prove
/// the bridge compiles and is fail-closed even under the no-op telemetry shim).
#[cfg(test)]
mod consent_gate_tests {
    use super::*;
    use maekon_core::consent::{ConsentManager, ConsentPermissions, ConsentStatus};

    /// Build a `ConsentManager` over a temp consent file with the given
    /// telemetry-permission bit (other permissions left default-false).
    fn consent_with_telemetry(telemetry: bool) -> (ConsentManager, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let cm = ConsentManager::new(dir.path().join("consent.json"));
        cm.grant_consent(
            ConsentPermissions {
                telemetry,
                ..Default::default()
            },
            30,
        )
        .expect("grant_consent must succeed in a writable temp dir");
        assert_eq!(cm.check_consent(), ConsentStatus::Valid);
        (cm, dir)
    }

    /// config.enabled=true + consent.telemetry=false ⇒ effective enabled=false.
    /// The decorative-consent gap this issue closes: telemetry must NOT emit
    /// when the user toggled config on but never granted the telemetry consent.
    #[test]
    fn config_on_consent_off_yields_disabled() {
        let (cm, _dir) = consent_with_telemetry(false);
        let cfg = TelemetryConfig {
            enabled: true,
            ..Default::default()
        };
        let gated = consent_gated_telemetry_config(&cfg, &cm);
        assert!(
            !gated.enabled,
            "config on + consent off MUST gate the exporter OFF"
        );
    }

    /// config.enabled=true + consent.telemetry=true ⇒ effective enabled=true.
    /// The default intent may be on, but export still requires consent.
    #[test]
    fn config_on_consent_on_yields_enabled() {
        let (cm, _dir) = consent_with_telemetry(true);
        let cfg = TelemetryConfig {
            enabled: true,
            ..Default::default()
        };
        let gated = consent_gated_telemetry_config(&cfg, &cm);
        assert!(
            gated.enabled,
            "config on + consent on must permit the exporter"
        );
    }

    /// config.enabled=false ⇒ effective enabled=false regardless of consent.
    /// An explicit opt-out keeps telemetry off even when consent happens to
    /// grant the permission.
    #[test]
    fn config_off_yields_disabled_regardless_of_consent() {
        for consent_bit in [false, true] {
            let (cm, _dir) = consent_with_telemetry(consent_bit);
            let cfg = TelemetryConfig {
                enabled: false,
                ..Default::default()
            };
            let gated = consent_gated_telemetry_config(&cfg, &cm);
            assert!(
                !gated.enabled,
                "config off must keep the exporter OFF (consent.telemetry={consent_bit})"
            );
        }
    }

    /// Fail-closed: absent consent record (NotGranted) ⇒ effective enabled=false
    /// even if config.enabled=true. No consent ever recorded.
    #[test]
    fn absent_consent_yields_disabled_even_when_config_on() {
        let dir = tempfile::tempdir().unwrap();
        let cm = ConsentManager::new(dir.path().join("consent.json"));
        assert_eq!(cm.check_consent(), ConsentStatus::NotGranted);
        let cfg = TelemetryConfig {
            enabled: true,
            ..Default::default()
        };
        let gated = consent_gated_telemetry_config(&cfg, &cm);
        assert!(
            !gated.enabled,
            "absent consent (NotGranted) MUST gate the exporter OFF (fail-closed)"
        );
    }

    /// Fail-closed: revoked consent ⇒ effective enabled=false even if config
    /// granted telemetry AND config.enabled=true. Revocation must shut it down.
    #[test]
    fn revoked_consent_yields_disabled_even_when_config_on() {
        let (cm, _dir) = consent_with_telemetry(true);
        // Sanity: before revoke the gate is open under config-on.
        let cfg = TelemetryConfig {
            enabled: true,
            ..Default::default()
        };
        assert!(consent_gated_telemetry_config(&cfg, &cm).enabled);

        cm.revoke_consent().expect("revoke must succeed");
        assert_eq!(cm.check_consent(), ConsentStatus::NotGranted);
        let gated = consent_gated_telemetry_config(&cfg, &cm);
        assert!(
            !gated.enabled,
            "revoked consent MUST gate the exporter OFF (fail-closed)"
        );
    }

    /// Non-`enabled` fields are preserved verbatim through the gate, so a later
    /// re-enable reuses the user's endpoint / sample_rate / service_name.
    #[test]
    fn non_enabled_fields_are_preserved() {
        let (cm, _dir) = consent_with_telemetry(false);
        let cfg = TelemetryConfig {
            enabled: true,
            otlp_endpoint: Some("http://collector.example:4318".into()),
            sample_rate: 0.25,
            service_name: "maekon-client-custom".into(),
            crash_reports: true,
            usage_analytics: true,
            performance_metrics: true,
        };
        let gated = consent_gated_telemetry_config(&cfg, &cm);
        assert!(!gated.enabled, "gated off by consent");
        assert_eq!(gated.otlp_endpoint, cfg.otlp_endpoint);
        assert_eq!(gated.sample_rate, cfg.sample_rate);
        assert_eq!(gated.service_name, cfg.service_name);
        assert_eq!(gated.crash_reports, cfg.crash_reports);
        assert_eq!(gated.usage_analytics, cfg.usage_analytics);
        assert_eq!(gated.performance_metrics, cfg.performance_metrics);
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
    use serial_test::serial;

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
    #[serial]
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
    #[serial]
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
    #[serial]
    async fn config_bus_delivers_telemetry_toggle() {
        use maekon_core::config_manager::ConfigManager;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc as StdArc;

        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("config.json");
        let mgr = StdArc::new(ConfigManager::with_path(cfg_path).unwrap());
        mgr.update_with(|c| {
            c.telemetry = TelemetryConfig::disabled();
            Ok(())
        })
        .expect("test must seed telemetry off before the toggle");

        // Build the handle seeded off — matches main.rs wiring.
        let (_layer, handle) = Handle::new_with_layer(&TelemetryConfig::disabled(), tmp.path())
            .expect("feature-on disabled construction is infallible");
        let handle = StdArc::new(handle);

        let observed = StdArc::new(AtomicBool::new(false));
        let observed_for_task = observed.clone();
        let handle_for_task = handle.clone();
        let mut rx = mgr.subscribe();
        let mock = mock_otlp::start().await;
        let endpoint = mock.endpoint.clone();

        tokio::spawn(async move {
            let mut prev = TelemetryConfig::disabled();
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
    #[serial]
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
            if let Ok(Some((signal, _body))) =
                tokio::time::timeout(std::time::Duration::from_millis(250), mock.rx.recv()).await
            {
                // Only a TRACES POST satisfies this span-spine test; a metrics
                // POST (the meter also flushes on shutdown) must not pass it.
                if signal == mock_otlp::Signal::Traces {
                    seen = true;
                    break;
                }
            }
        }
        assert!(
            seen,
            "no OTLP traces POST reached the mock collector within 5s after shutdown"
        );
    }

    /// T-X2-8 — shutdown-with-unreachable-collector watchdog. Toggle on
    /// against a dead port, emit a few spans, toggle off (drives `shutdown`).
    /// Must complete within 5 s (watchdog is 4 s + 1 s overhead budget) and
    /// never panic.
    #[tokio::test]
    #[serial]
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
        // SAFETY: same single-threaded per-test assumption as the set_var above;
        // this test does not depend on concurrent env access and restores state
        // at the end.
        unsafe { std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", "http://from-env:4318/") };
        assert_eq!(
            otlp::resolve_endpoint(&TelemetryConfig::default()),
            "http://from-env:4318/v1/traces"
        );

        // Default (no env, no cfg) is the full URL.
        // SAFETY: same single-threaded per-test assumption as the set_var above;
        // no other thread is reading the environment during this test.
        unsafe { std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT") };
        assert_eq!(
            otlp::resolve_endpoint(&TelemetryConfig::default()),
            "http://localhost:4318/v1/traces"
        );

        // Restore pre-existing env so other tests are unaffected.
        if let Some(value) = prev {
            // SAFETY: same single-threaded per-test assumption as the set_var
            // above; this restores the captured pre-existing value with no
            // concurrent env access.
            unsafe { std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", value) };
        }
    }

    /// T-metrics-1 (E20-43 #4835) — a recorded counter reaches the mock OTLP
    /// collector as a `/v1/metrics` POST after a shutdown-driven flush.
    ///
    /// This test drives the meter provider *directly* rather than through
    /// `global::set_meter_provider`. Rationale: the global meter provider is a
    /// single piece of process-wide state, and the other `#[tokio::test]`
    /// telemetry tests run concurrently and each swap/reset it — so recording
    /// through `global::meter()` here would race those swaps and flake. Building
    /// the provider inline exercises the SAME production path
    /// (`MetricExporter::builder().with_http()` + `with_periodic_exporter` +
    /// shared Resource) that `otlp::build_pipeline` constructs, and the shutdown
    /// flush mirrors the production toggle-off behaviour, without depending on
    /// global state.
    #[tokio::test]
    #[serial]
    async fn mock_collector_receives_metric() {
        use opentelemetry::metrics::MeterProvider as _;
        use opentelemetry::KeyValue;
        use opentelemetry_otlp::WithExportConfig;
        use opentelemetry_sdk::metrics::SdkMeterProvider;
        use opentelemetry_sdk::Resource;

        let mut mock = mock_otlp::start().await;
        // The mock's `endpoint` field is the traces path; swap the suffix to the
        // metrics signal path the production resolver would use.
        let metrics_endpoint = mock.endpoint.replace("/v1/traces", "/v1/metrics");

        let metric_exporter = opentelemetry_otlp::MetricExporter::builder()
            .with_http()
            .with_endpoint(metrics_endpoint)
            .build()
            .expect("metric exporter must build against the mock");
        let resource = Resource::builder()
            .with_service_name("maekon-client-test")
            .with_attribute(KeyValue::new("service.instance.id", "t-metrics-1"))
            .build();
        let meter = SdkMeterProvider::builder()
            .with_periodic_exporter(metric_exporter)
            .with_resource(resource)
            .build();

        // Record a bounded, NON-PII counter via the provider's own meter.
        let counter = meter
            .meter("maekon.client")
            .u64_counter("maekon.client.heartbeat")
            .build();
        counter.add(1, &[]);

        // Shutdown performs a final export of buffered counters on a dedicated
        // thread bounded by the 4 s watchdog (production toggle-off path).
        let meter_for_shutdown = meter.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = meter_for_shutdown.shutdown();
            let _ = tx.send(());
        });
        let _ = rx.recv_timeout(std::time::Duration::from_secs(4));

        // After shutdown returns, give the exporter's in-flight HTTP request a
        // brief window to land. Filter for METRICS so a stray POST cannot
        // satisfy the assertion.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut seen_metric = false;
        while std::time::Instant::now() < deadline {
            if let Ok(Some((signal, _body))) =
                tokio::time::timeout(std::time::Duration::from_millis(250), mock.rx.recv()).await
            {
                if signal == mock_otlp::Signal::Metrics {
                    seen_metric = true;
                    break;
                }
            }
        }
        assert!(
            seen_metric,
            "no OTLP metrics POST reached the mock collector within 5s after shutdown"
        );
    }

    /// T-metrics-2 (E20-43 #4835) — toggle-off shuts the meter provider within
    /// the 5 s watchdog budget (4 s watchdog + 1 s overhead), even against an
    /// unreachable collector, and never panics. Mirrors the tracer's T-X2-8.
    #[tokio::test]
    #[serial]
    async fn meter_shutdown_completes_within_watchdog_budget() {
        let tmp = tempfile::tempdir().unwrap();
        // Port 1 is reliably unreachable for TCP; the metrics exporter can never
        // flush but the meter shutdown watchdog must still let `apply` return.
        let cfg_on = TelemetryConfig {
            enabled: true,
            otlp_endpoint: Some("http://127.0.0.1:1".into()),
            ..Default::default()
        };
        let (_layer, handle) = Handle::new_with_layer(&cfg_on, tmp.path())
            .expect("metrics exporter builds even with an unreachable endpoint");

        // Push a few increments so the periodic reader has buffered data.
        for _ in 0..5 {
            crate::telemetry::metrics::record_heartbeat();
        }

        let cfg_off = TelemetryConfig {
            enabled: false,
            ..cfg_on
        };
        let start = std::time::Instant::now();
        handle
            .apply(&cfg_off)
            .expect("toggle off must not hang even against a dead metrics collector");
        assert!(
            start.elapsed() < std::time::Duration::from_secs(5),
            "meter shutdown watchdog exceeded 5s (elapsed: {:?})",
            start.elapsed()
        );
    }

    /// T-metrics-3 (E20-43 #4835 review) — GLOBAL fresh-resolve guard. After an
    /// OFF→ON toggle installs a NEW meter provider, a record via the GLOBAL path
    /// (`record_heartbeat`) must reach the NEW collector. Guards against a future
    /// regression that caches the instrument (e.g. `OnceLock`) and pins it to the
    /// stale/shut-down provider — the exact failure metrics.rs documents. `#[serial]`
    /// with the other global-meter tests so a concurrent `set_meter_provider`
    /// cannot swap the provider mid-record.
    #[tokio::test]
    #[serial]
    async fn metric_delivery_survives_off_on_toggle_via_global_path() {
        let tmp = tempfile::tempdir().unwrap();

        // Boot ON against mock A (installs the global meter provider = A).
        let _mock_a = mock_otlp::start().await;
        let cfg_on_a = TelemetryConfig {
            enabled: true,
            otlp_endpoint: Some(_mock_a.endpoint.replace("/v1/traces", "")),
            ..Default::default()
        };
        let (_layer, handle) =
            Handle::new_with_layer(&cfg_on_a, tmp.path()).expect("boot pipeline A");

        let cfg_off = TelemetryConfig {
            enabled: false,
            ..Default::default()
        };
        handle.apply(&cfg_off).expect("toggle off A");

        // Toggle ON against mock B — apply's (off→on) arm builds a NEW meter
        // provider and calls set_meter_provider(B).
        let mut mock_b = mock_otlp::start().await;
        let cfg_on_b = TelemetryConfig {
            enabled: true,
            otlp_endpoint: Some(mock_b.endpoint.replace("/v1/traces", "")),
            ..Default::default()
        };
        handle.apply(&cfg_on_b).expect("toggle on B");

        // Record via the GLOBAL path (the production call site). A cached instrument
        // pinned to A's shut-down provider would silently drop this.
        crate::telemetry::metrics::record_heartbeat();

        // Toggle off to flush B's final export on the watchdog thread.
        handle.apply(&cfg_off).expect("toggle off B (flush)");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut seen_on_b = false;
        while std::time::Instant::now() < deadline {
            if let Ok(Some((signal, _body))) =
                tokio::time::timeout(std::time::Duration::from_millis(250), mock_b.rx.recv()).await
            {
                if signal == mock_otlp::Signal::Metrics {
                    seen_on_b = true;
                    break;
                }
            }
        }
        assert!(
            seen_on_b,
            "post-toggle metric did not reach the NEW provider's collector (mock B) — \
             instrument fresh-resolve regressed (likely cached to the stale provider)"
        );
    }
}
