//! Minimal, bounded-cardinality, NON-PII metric instruments (E20-43 #4835).
//!
//! Design mirrors the tracing-layer feature gate: when the `telemetry` feature
//! is OFF every public function compiles to a no-op (see the
//! `#[cfg(not(feature = "telemetry"))]` block at the bottom), so call sites do
//! NOT need their own `#[cfg]`. When the feature is ON the instruments read
//! through `opentelemetry::global` — they record into whatever MeterProvider is
//! currently installed (the OTLP `SdkMeterProvider` while telemetry is enabled,
//! or the global `NoopMeterProvider` after a toggle-off). That means call sites
//! are always safe to invoke regardless of the runtime `telemetry.enabled`
//! state; no extra guarding is required at the call site.
//!
//! ## Cardinality / PII contract (load-bearing — keep it this way)
//! Every instrument here is deliberately low-cardinality and carries NO
//! user-identifying data. Allowed label values are restricted to:
//!   - a fixed, code-defined `loop` name (a small closed set), and
//!   - a bounded, code-defined upload-channel `endpoint` authority — never a
//!     full URL, path, query, or anything derived from user input.
//! There are intentionally NO per-user, per-window-title, per-app, per-session,
//! or per-document labels. Anything that would unbound cardinality or leak PII
//! does NOT belong here.

#[cfg(feature = "telemetry")]
mod imp {
    use opentelemetry::{global, KeyValue};

    /// Instrumentation scope name attached to every meter we create.
    const METER_NAME: &str = "maekon.client";

    // IMPORTANT (binding semantics): per the opentelemetry 0.32 contract, a
    // `Meter`/instrument is bound to whatever global `MeterProvider` was
    // installed *at the moment it was created*, and does NOT follow later
    // `set_meter_provider` swaps (see `global::meter` docs). The telemetry
    // runtime toggle (`telemetry.enabled`) installs a fresh `SdkMeterProvider`
    // on each off->on transition, so we MUST resolve the meter + instrument
    // fresh on every record call — caching a `Counter` in a `OnceLock` would
    // pin it to a stale (possibly shut-down) provider and silently drop every
    // subsequent record. Instrument creation is cheap: the SDK dedups by name
    // within a provider, so re-`build()` returns the same underlying stream.

    pub fn record_heartbeat() {
        global::meter(METER_NAME)
            .u64_counter("maekon.client.heartbeat")
            .with_description("Scheduler heartbeat ticks since process start.")
            .build()
            .add(1, &[]);
    }

    pub fn record_loop_iteration(loop_name: &'static str) {
        global::meter(METER_NAME)
            .u64_counter("maekon.client.scheduler.loop.iterations")
            .with_description("Scheduler loop iterations, labelled by loop name only.")
            .build()
            .add(1, &[KeyValue::new("loop", loop_name)]);
    }

    pub fn record_batch_upload_success(endpoint_authority: &str) {
        global::meter(METER_NAME)
            .u64_counter("maekon.client.batch_upload.success")
            .with_description("Successful batch uploads, labelled by endpoint authority only.")
            .build()
            .add(
                1,
                &[KeyValue::new("endpoint", endpoint_authority.to_string())],
            );
    }

    pub fn record_batch_upload_failure(endpoint_authority: &str) {
        global::meter(METER_NAME)
            .u64_counter("maekon.client.batch_upload.failure")
            .with_description("Failed batch uploads, labelled by endpoint authority only.")
            .build()
            .add(
                1,
                &[KeyValue::new("endpoint", endpoint_authority.to_string())],
            );
    }
}

// No-op shim when the `telemetry` feature is off — mirrors `NoopLayer` so call
// sites stay free of `#[cfg]`. The args are accepted and dropped; nothing is
// allocated and no global state is touched.
#[cfg(not(feature = "telemetry"))]
mod imp {
    #[inline]
    pub fn record_heartbeat() {}

    #[inline]
    pub fn record_loop_iteration(_loop_name: &'static str) {}

    #[inline]
    pub fn record_batch_upload_success(_endpoint_authority: &str) {}

    #[inline]
    pub fn record_batch_upload_failure(_endpoint_authority: &str) {}
}

pub use imp::{
    record_batch_upload_failure, record_batch_upload_success, record_heartbeat,
    record_loop_iteration,
};
