//! OTLP span pipeline. Feature-gated at the `mod.rs` declaration.
//!
//! - `build_initial_handle(cfg, data_dir) -> Result<(Layer, Handle)>`
//!   Builds the pipeline ONCE when `cfg.enabled == true`, wraps the layer in
//!   `reload::Layer`, stashes the `TracerProvider` in `Inner.active` so we
//!   can shut it down on toggle-off.
//! - `Inner::apply(cfg)` drives off↔on transitions in place.
//! - `resolve_endpoint(cfg)` implements the precedence from spec §3.2.

use crate::telemetry::{Handle, Layer};
use maekon_core::config::TelemetryConfig;
use opentelemetry::KeyValue;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::trace::{self as sdktrace, SdkTracerProvider};
use opentelemetry_sdk::Resource;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::{reload, Registry};

pub(super) type OtelLayer = OpenTelemetryLayer<Registry, sdktrace::Tracer>;

pub(super) struct Inner {
    /// Handle to swap the `Option<OtelLayer>` baked into the subscriber at init.
    reload_handle: reload::Handle<Option<OtelLayer>, Registry>,
    /// Currently-active provider. `None` when disabled. `shutdown()` on
    /// toggle-off; rebuilt via `build_pipeline` on toggle-on.
    active: Option<SdkTracerProvider>,
    /// Currently-active meter provider (E20-43 #4835). `None` when disabled.
    /// Shut down on toggle-off via the SAME dedicated-thread 4 s watchdog as the
    /// tracer; rebuilt on toggle-on. The global meter provider is reset to a
    /// noop on toggle-off so post-disable `metrics::record_*` calls are silently
    /// dropped, mirroring the tracing-layer `None` reload.
    active_meter: Option<SdkMeterProvider>,
    /// Last config we applied. Used to detect transitions and avoid redundant work.
    last_cfg: TelemetryConfig,
    /// Captured from boot so the off→on transition can regenerate the pipeline
    /// (including `service.instance.id` — Task 9) without re-plumbing it in.
    data_dir: std::path::PathBuf,
}

impl Inner {
    pub(super) fn apply(&mut self, cfg: &TelemetryConfig) -> anyhow::Result<()> {
        match (self.last_cfg.enabled, cfg.enabled) {
            (false, true) => {
                // off -> on: build a new pipeline and swap in.
                let (provider, layer, meter) = build_pipeline(cfg, &self.data_dir)?;
                self.reload_handle
                    .modify(|opt| *opt = Some(layer))
                    .map_err(|e| anyhow::anyhow!("reload modify failed: {e:?}"))?;
                self.active = Some(provider);
                // Install the meter provider globally so `metrics::record_*` call
                // sites route into the OTLP exporter, mirroring the tracing-layer
                // reload-to-Some(...) above.
                opentelemetry::global::set_meter_provider(meter.clone());
                self.active_meter = Some(meter);
            }
            (true, false) => {
                // on -> off: detach and shut down.
                self.reload_handle
                    .modify(|opt| *opt = None)
                    .map_err(|e| anyhow::anyhow!("reload modify failed: {e:?}"))?;
                if let Some(provider) = self.active.take() {
                    shutdown(provider);
                }
                // Reset the global meter provider to a noop FIRST so in-flight
                // `metrics::record_*` calls stop recording into the exporter,
                // then shut the provider down via the same 4 s watchdog. This
                // mirrors the tracing-layer reload-to-None above.
                if let Some(meter) = self.active_meter.take() {
                    opentelemetry::global::set_meter_provider(
                        opentelemetry::metrics::noop::NoopMeterProvider::new(),
                    );
                    shutdown_meter(meter);
                }
            }
            _ => {
                // No transition. Idempotent.
            }
        }
        self.last_cfg = cfg.clone();
        Ok(())
    }
}

pub(super) fn build_initial_handle(
    cfg: &TelemetryConfig,
    data_dir: &std::path::Path,
) -> anyhow::Result<(Layer, Handle)> {
    // Build the pipeline at most ONCE at boot. If enabled, the provider lives
    // in Inner.active so we can shut it down on toggle-off; the layer is moved
    // into the reload wrapper.
    let (initial_layer, active, active_meter) = if cfg.enabled {
        let (provider, layer, meter) = build_pipeline(cfg, data_dir)?;
        // Install the meter provider globally at boot so call sites record into
        // the OTLP exporter immediately, matching the tracer's Some(layer) state.
        opentelemetry::global::set_meter_provider(meter.clone());
        (Some(layer), Some(provider), Some(meter))
    } else {
        (None, None, None)
    };

    let (reload_layer, reload_handle) = reload::Layer::new(initial_layer);

    let handle = Handle {
        inner: parking_lot::Mutex::new(Inner {
            reload_handle,
            active,
            active_meter,
            last_cfg: cfg.clone(),
            data_dir: data_dir.to_path_buf(),
        }),
    };

    Ok((reload_layer, handle))
}

fn build_pipeline(
    cfg: &TelemetryConfig,
    data_dir: &std::path::Path,
) -> anyhow::Result<(SdkTracerProvider, OtelLayer, SdkMeterProvider)> {
    let endpoint = resolve_endpoint(cfg);
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(endpoint)
        .build()?;

    // Ensure the per-install UUID exists and attach it as `service.instance.id`.
    // Failure here is non-fatal for the overall pipeline — we still ship spans
    // without the instance attribute rather than refusing to export at all.
    let instance_id = super::instance_id::ensure_instance_id(data_dir)
        .map_err(|e| anyhow::anyhow!("telemetry_instance_id: {e}"))?;

    // SAME Resource (service.name + instance.id) is shared by the tracer and the
    // meter provider so both signals correlate to the same install (E20-43).
    let resource = Resource::builder()
        .with_service_name(cfg.service_name.clone())
        .with_attribute(KeyValue::new("service.instance.id", instance_id))
        .build();

    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource.clone())
        .build();

    use opentelemetry::trace::TracerProvider as _;
    let tracer = provider.tracer("maekon");
    let layer = tracing_opentelemetry::layer().with_tracer(tracer);

    // Metrics pipeline (E20-43 #4835). Separate signal-specific endpoint
    // (/v1/metrics, NOT /v1/traces) + a periodic push reader.
    let metrics_endpoint = resolve_metrics_endpoint(cfg);
    let metric_exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_http()
        .with_endpoint(metrics_endpoint)
        .build()?;
    let meter = SdkMeterProvider::builder()
        .with_periodic_exporter(metric_exporter)
        .with_resource(resource)
        .build();

    Ok((provider, layer, meter))
}

/// Endpoint resolution precedence (§3.2):
/// 1. Explicit `config.telemetry.otlp_endpoint` (Some) — passed through
///    verbatim so power users who terminate traces at a non-standard path can
///    override.
/// 2. Env var `OTEL_EXPORTER_OTLP_ENDPOINT` — treated as a base URL per the
///    OpenTelemetry spec; `/v1/traces` is appended for the traces exporter.
/// 3. `http://localhost:4318/v1/traces` (OTLP/HTTP default, full path).
///
/// The `opentelemetry-otlp` 0.27 HTTP builder does NOT append the signal path
/// when passed to `.with_endpoint(...)`, so we compose the full URL here.
pub(crate) fn resolve_endpoint(cfg: &TelemetryConfig) -> String {
    if let Some(ref explicit) = cfg.otlp_endpoint {
        return explicit.clone();
    }
    if let Ok(env) = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT") {
        if !env.is_empty() {
            return append_signal_path(&env);
        }
    }
    "http://localhost:4318/v1/traces".to_string()
}

/// Append `/v1/traces` to a base endpoint URL, handling trailing slashes.
fn append_signal_path(base: &str) -> String {
    let trimmed = base.trim_end_matches('/');
    format!("{trimmed}/v1/traces")
}

/// Metrics endpoint resolution (E20-43 #4835). Mirrors [`resolve_endpoint`] but
/// the signal path is `/v1/metrics` (NOT `/v1/traces`):
/// 1. Explicit `config.telemetry.otlp_endpoint` (Some) — the SAME single config
///    field [`resolve_endpoint`] reads as the full *traces* path. We derive the
///    metrics signal path so one endpoint serves both signals: a `/v1/traces`
///    suffix becomes `/v1/metrics`; an already-`/v1/metrics` path is kept; any
///    other value is treated as a base and gets `/v1/metrics` appended. (Returning
///    it verbatim — the old behavior — sent metrics to `/v1/traces`, the wrong
///    signal path, for every explicit-endpoint config; #4835 pre-merge fix.)
/// 2. Env var `OTEL_EXPORTER_OTLP_ENDPOINT` — base URL, `/v1/metrics` appended.
/// 3. `http://localhost:4318/v1/metrics` (OTLP/HTTP default, full path).
///
/// The `opentelemetry-otlp` 0.32 HTTP builder does NOT append the signal path
/// when given an explicit `.with_endpoint(...)`, so we compose the full URL here.
pub(crate) fn resolve_metrics_endpoint(cfg: &TelemetryConfig) -> String {
    if let Some(ref explicit) = cfg.otlp_endpoint {
        let trimmed = explicit.trim_end_matches('/');
        return if let Some(base) = trimmed.strip_suffix("/v1/traces") {
            format!("{base}/v1/metrics")
        } else if trimmed.ends_with("/v1/metrics") {
            trimmed.to_string()
        } else {
            append_metrics_path(trimmed)
        };
    }
    if let Ok(env) = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT") {
        if !env.is_empty() {
            return append_metrics_path(&env);
        }
    }
    "http://localhost:4318/v1/metrics".to_string()
}

/// Append `/v1/metrics` to a base endpoint URL, handling trailing slashes.
fn append_metrics_path(base: &str) -> String {
    let trimmed = base.trim_end_matches('/');
    format!("{trimmed}/v1/metrics")
}

/// Run `provider.shutdown()` on a dedicated thread with a 4 s watchdog so a
/// wedged exporter (collector down, network partition) cannot block the app
/// on toggle-off or exit. Past the deadline we log a warning and proceed; the
/// SDK may retain a zombie I/O task but the caller is never blocked.
fn shutdown(provider: SdkTracerProvider) {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = provider.shutdown();
        let _ = tx.send(());
    });
    if rx.recv_timeout(std::time::Duration::from_secs(4)).is_err() {
        tracing::warn!("otel provider shutdown exceeded 4s; continuing without waiting");
    }
}

/// Shut down the meter provider with the SAME dedicated-thread 4 s watchdog as
/// [`shutdown`] (E20-43 #4835). A wedged metrics exporter (collector down,
/// network partition) must not block the app on toggle-off or exit; past the
/// deadline we warn and proceed. `shutdown()` performs a final flush so any
/// buffered counters are exported before the deadline if the collector is alive.
fn shutdown_meter(provider: SdkMeterProvider) {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = provider.shutdown();
        let _ = tx.send(());
    });
    if rx.recv_timeout(std::time::Duration::from_secs(4)).is_err() {
        tracing::warn!("otel meter provider shutdown exceeded 4s; continuing without waiting");
    }
}

#[cfg(test)]
mod metrics_endpoint_tests {
    use super::*;

    fn cfg_with(ep: Option<&str>) -> TelemetryConfig {
        TelemetryConfig {
            enabled: true,
            otlp_endpoint: ep.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn explicit_traces_path_maps_to_metrics_path() {
        // #4835 fix: the explicit (traces-convention) endpoint must yield the
        // /v1/metrics path, not the /v1/traces path verbatim.
        assert_eq!(
            resolve_metrics_endpoint(&cfg_with(Some("http://c:4318/v1/traces"))),
            "http://c:4318/v1/metrics"
        );
        assert_eq!(
            resolve_metrics_endpoint(&cfg_with(Some("http://c:4318/v1/traces/"))),
            "http://c:4318/v1/metrics"
        );
    }

    #[test]
    fn explicit_base_gets_metrics_path() {
        assert_eq!(
            resolve_metrics_endpoint(&cfg_with(Some("http://c:4318"))),
            "http://c:4318/v1/metrics"
        );
    }

    #[test]
    fn explicit_metrics_path_kept() {
        assert_eq!(
            resolve_metrics_endpoint(&cfg_with(Some("http://c:4318/v1/metrics"))),
            "http://c:4318/v1/metrics"
        );
    }
}
