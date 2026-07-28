[English](./telemetry.md) | [한국어](./telemetry.ko.md)

# Telemetry

> **Configuration default: off. Effective export requires every runtime gate.**

MAEKON's Rust client can ship distributed-trace spans and a small set of bounded, non-PII metrics to an OpenTelemetry collector for prerelease triage. This document covers what is collected, how to enable or disable it, how to point it at your own collector, and how to wipe the identifier the collector sees.

## Four separate controls

Do not collapse the following controls into a single “telemetry on/off” claim:

1. **Configuration default** — persisted `telemetry.enabled` defaults to `false`.
2. **Effective runtime state** — export is active only when the configuration is
   enabled and valid **feature consent** exists.
3. **Build capability** — the binary must include the `telemetry Cargo feature`;
   default release builds omit it.
4. **Diagnostic export consent** — a support bundle is a separate, locally
   generated user action. Runtime logs default to excluded and are shared only
   after the user reviews and explicitly sends the bundle.

Changing one control does not imply that the others are enabled.

## What is collected

When telemetry is enabled and the `telemetry` Cargo feature is compiled in, the
OpenTelemetry path may export:

- **`tracing` spans** emitted by the Rust crates (timestamps, span names, parent/child links, numeric attributes). No PII. No screen contents. No keystrokes.
- **OpenTelemetry metrics** — a minimal, bounded-cardinality, NON-PII instrument set exported over OTLP/HTTP to `/v1/metrics`:
  - `maekon.client.heartbeat` — counter; scheduler heartbeat ticks. No labels.
  - `maekon.client.scheduler.loop.iterations` — counter; the only label is a fixed, code-defined loop name (a small closed set).
  - `maekon.client.batch_upload.success` / `maekon.client.batch_upload.failure` — counters; the only label is a bounded, code-defined upload-channel authority. Never a full URL, path, query, or anything derived from user input.
  - There are **no** per-user, per-window-title, per-app, per-session, or per-document labels by design.
- **OpenTelemetry Resource attributes** attached to every span and metric (the same Resource is shared by both signals):
  - `service.name` — defaults to `maekon-client`; identifies the binary, not the user.
  - `service.instance.id` — a per-install UUIDv4 generated on first consent-approved exporter activation. Not derived from any user identifier. Stored in `telemetry_instance_id` inside the app data directory (see below). Lets the collector group telemetry from the same install without identifying who is running it.

Crash reports and usage analytics are reserved fields in the config but **not wired** in the current release. The telemetry feature covers span and bounded-metric export only.

## Feature performance samples

Feature performance samples are separate from the OpenTelemetry collector path.
They are emitted only by explicit instrumentation around real feature
executions and are flushed to the MAEKON server's feature-performance endpoint
when telemetry consent is currently allowed.

The sample contract is intentionally narrow:

- `feature_key`: one of the client-defined canonical feature keys such as
  `local-suggestions` or `sync`. Keys are a small code-defined set, not user
  input.
- `response_time_ms`: measured wall-clock duration of the feature execution.
  It is not flag-evaluation latency and not a host CPU/memory snapshot.
- `timestamp`: completion time of the measured invocation.
- `total_requests` and `error_count`: bounded counters for that measured
  invocation or batch. They must satisfy `error_count <= total_requests`.

Samples do **not** contain user identifiers, organization identifiers, feature
ids, document ids, raw content, prompts, OCR text, screen contents, window
titles, or derived health fields such as `success_rate`, `availability`,
`error_rate`, `status`, or `health_score`.

The client buffers samples per `feature_key` with a bounded in-memory queue.
When consent is off, samples are discarded locally and recorded as blocked
egress if an egress ledger is configured. When consent is on, each upload is
audited through the egress ledger with destination
`server.feature_performance`.

## What is NOT collected

- Screen captures, OCR text, accessibility-tree contents.
- Chat messages, file contents, configuration values.
- User identifiers, email addresses, or any data that has not been cleared by the existing `PiiFilterLevel` pipeline before reaching a tracing call.
- Public feature-performance payloads never include server-side identifiers or
  derived health fields; the server derives organization context from its own
  authenticated request context.

## How to enable

On fresh installs, `telemetry.enabled` defaults to `false`. To make the effective runtime state eligible for export, all three telemetry gates must be open:

1. The user changes the persisted configuration to `telemetry.enabled=true`.
2. The user has granted valid feature consent for `telemetry`.
3. The binary was built with the `telemetry` Cargo feature.

Existing config files that persist `"enabled": false` remain opted out. To request telemetry on an eligible build, open Preferences → Privacy → Telemetry and toggle **Enable telemetry** on, or edit `config.json`; this does not bypass feature consent or compile-time gating.

Changes take effect within a few seconds — you do not need to restart the client. The first consent-approved activation creates the `telemetry_instance_id` file described above.

Diagnostic bundle generation and sharing are not part of this toggle. The
diagnostic request defaults `include_logs=false`; a user must separately choose
the bundle contents, review the generated artifact, and send it through an
explicit support path.

Advanced users can edit `config.json` directly:

```json
{
  "telemetry": {
    "enabled": true,
    "otlp_endpoint": null,
    "sample_rate": 1.0,
    "service_name": "maekon-client"
  }
}
```

The config file lives under the same platform-specific directory the client uses for all its settings:
- **macOS**: `~/Library/Application Support/maekon/config.json`
- **Linux**: `~/.config/maekon/config.json`
- **Windows**: `%APPDATA%/maekon/config.json`

## How to disable

Set `telemetry.enabled` to `false` (UI toggle or edit `config.json`). Export stops within one async tick. The `telemetry_instance_id` file is intentionally left in place so toggling back on re-uses the same identifier — see [Erase identity](#erase-identity).

## How to point at a custom collector

Three ways, listed in precedence order (highest wins):

1. **Explicit config**: set `telemetry.otlp_endpoint` in `config.json`. It is passed through verbatim for **both** signals, so point it at a collector that accepts the signal-specific paths your exporter uses.
2. **Environment variable**: `OTEL_EXPORTER_OTLP_ENDPOINT=https://otel.example.com` — treated as a base URL per the OpenTelemetry specification; the client appends `/v1/traces` for spans and `/v1/metrics` for metrics.
3. **Default**: `http://localhost:4318` (OTLP/HTTP-proto default) — spans go to `/v1/traces`, metrics to `/v1/metrics`. Useful when you run an `otel/opentelemetry-collector-contrib` container locally for debugging.

The client uses OTLP over HTTP/proto. No gRPC fallback is exposed today.

## Compile-time gating

Even with `telemetry.enabled = true`, the exporter does nothing unless the binary was built with the `telemetry` Cargo feature. Default release builds ship with the feature **off** so users who never want telemetry pay zero binary-size or dependency cost. Packagers who want the feature must build with `cargo build --release --features telemetry -p maekon-app`.

## Erase identity

The `telemetry_instance_id` file holds a UUIDv4 that the collector uses to group spans from the same install. To erase it without uninstalling:

1. Disable telemetry (so no spans reference the old ID).
2. Delete `telemetry_instance_id` from the app data directory:
   - **macOS**: `~/Library/Application Support/maekon/data/telemetry_instance_id`
   - **Linux**: `~/.local/share/maekon/telemetry_instance_id`
   - **Windows**: `%LOCALAPPDATA%/maekon/data/telemetry_instance_id`
3. Re-enable telemetry. A fresh UUIDv4 is generated and written with `0600` permissions (Unix).

A dedicated `telemetry reset-instance-id` CLI command ships in a later release; the manual step above is the current supported path.

## Troubleshooting

- **"My spans aren't reaching the collector"** — confirm the collector is listening on the endpoint you configured, and that it accepts OTLP/HTTP-proto at `/v1/traces`. A quick local smoke test: `docker run -p 4318:4318 otel/opentelemetry-collector-contrib:latest` with the default config.
- **"Telemetry turned off but my app is still sending data"** — it isn't, but a buffered batch may be mid-flight. Exports stop accepting new spans/metrics within one async tick; the meter provider is reset to a no-op and both the span and meter providers are shut down on a dedicated thread, with any in-flight HTTP POST completing or timing out within 4 s (the shutdown watchdog applies to both signals).
- **"Where do I see what was sent?"** — the client logs the exporter's warn-level failures to the same tracing subscriber the rest of the app uses (the `warn` macro in `src-tauri/src/telemetry/otlp.rs::shutdown`). Enable debug logs with `RUST_LOG=opentelemetry=debug,maekon=debug`.

## References

- Feature performance contract: see [Feature performance samples](#feature-performance-samples)
- ADR-016 ConfigChangeBus: [`docs/architecture/ADR-016-config-change-bus.md`](../architecture/ADR-016-config-change-bus.md)
- OpenTelemetry specification — Resource semantics, OTLP/HTTP transport.
