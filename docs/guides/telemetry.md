[English](./telemetry.md) | [한국어](./telemetry.ko.md)

# Telemetry

> **Opt-in. Disabled by default. Private by construction.**

ONESHIM's Rust client can ship distributed-trace spans to an OpenTelemetry collector for production triage. This document covers what is collected, how to enable or disable it, how to point it at your own collector, and how to wipe the identifier the collector sees.

## What is collected

When telemetry is enabled and the `telemetry` Cargo feature is compiled in:

- **`tracing` spans** emitted by the Rust crates (timestamps, span names, parent/child links, numeric attributes). No PII. No screen contents. No keystrokes.
- **OpenTelemetry Resource attributes** attached to every span:
  - `service.name` — defaults to `oneshim-client`; identifies the binary, not the user.
  - `service.instance.id` — a per-install UUIDv4 generated on first opt-in. Not derived from any user identifier. Stored in `telemetry_instance_id` inside the app data directory (see below). Lets the collector group spans from the same install without identifying who is running it.

Crash reports, usage analytics, and performance metrics are reserved fields in the config but **not wired** in the current release. The telemetry feature covers span export only.

## What is NOT collected

- Screen captures, OCR text, accessibility-tree contents.
- Chat messages, file contents, configuration values.
- User identifiers, email addresses, or any data that has not been cleared by the existing `PiiFilterLevel` pipeline before reaching a tracing call.

## How to enable

Telemetry is **off** by default. To enable:

1. Open Preferences → Privacy → Telemetry in the app.
2. Toggle **Enable telemetry** on.

Changes take effect within a few seconds — you do not need to restart the client. The first opt-in creates the `telemetry_instance_id` file described above.

Advanced users can edit `config.json` directly:

```json
{
  "telemetry": {
    "enabled": true,
    "otlp_endpoint": null,
    "sample_rate": 1.0,
    "service_name": "oneshim-client"
  }
}
```

The config file lives under the same platform-specific directory the client uses for all its settings:
- **macOS**: `~/Library/Application Support/oneshim/config.json`
- **Linux**: `~/.config/oneshim/config.json`
- **Windows**: `%APPDATA%/oneshim/config.json`

## How to disable

Set `telemetry.enabled` to `false` (UI toggle or edit `config.json`). Export stops within one async tick. The `telemetry_instance_id` file is intentionally left in place so toggling back on re-uses the same identifier — see [Erase identity](#erase-identity).

## How to point at a custom collector

Three ways, listed in precedence order (highest wins):

1. **Explicit config**: set `telemetry.otlp_endpoint` in `config.json` to the full `/v1/traces` URL of your collector.
2. **Environment variable**: `OTEL_EXPORTER_OTLP_ENDPOINT=https://otel.example.com` — the client appends `/v1/traces` per the OpenTelemetry specification.
3. **Default**: `http://localhost:4318/v1/traces` (OTLP/HTTP-proto default). Useful when you run an `otel/opentelemetry-collector-contrib` container locally for debugging.

The client uses OTLP over HTTP/proto. No gRPC fallback is exposed today.

## Compile-time gating

Even with `telemetry.enabled = true`, the exporter does nothing unless the binary was built with the `telemetry` Cargo feature. Default release builds ship with the feature **off** so users who never want telemetry pay zero binary-size or dependency cost. Packagers who want the feature must build with `cargo build --release --features telemetry -p oneshim-app`.

## Erase identity

The `telemetry_instance_id` file holds a UUIDv4 that the collector uses to group spans from the same install. To erase it without uninstalling:

1. Disable telemetry (so no spans reference the old ID).
2. Delete `telemetry_instance_id` from the app data directory:
   - **macOS**: `~/Library/Application Support/oneshim/data/telemetry_instance_id`
   - **Linux**: `~/.local/share/oneshim/telemetry_instance_id`
   - **Windows**: `%LOCALAPPDATA%/oneshim/data/telemetry_instance_id`
3. Re-enable telemetry. A fresh UUIDv4 is generated and written with `0600` permissions (Unix).

A dedicated `telemetry reset-instance-id` CLI command ships in a later release; the manual step above is the current supported path.

## Troubleshooting

- **"My spans aren't reaching the collector"** — confirm the collector is listening on the endpoint you configured, and that it accepts OTLP/HTTP-proto at `/v1/traces`. A quick local smoke test: `docker run -p 4318:4318 otel/opentelemetry-collector-contrib:latest` with the default config.
- **"Telemetry turned off but my app is still sending data"** — it isn't, but a buffered batch may be mid-flight. Exports stop accepting new spans within one async tick; any in-flight HTTP POST completes or times out within 4 s (the shutdown watchdog).
- **"Where do I see what was sent?"** — the client logs the exporter's warn-level failures to the same tracing subscriber the rest of the app uses (the `warn` macro in `src-tauri/src/telemetry/otlp.rs::shutdown`). Enable debug logs with `RUST_LOG=opentelemetry=debug,oneshim=debug`.

## References

- Spec: internal config telemetry implementation record
- ADR-016 ConfigChangeBus: [`docs/architecture/ADR-016-config-change-bus.md`](../architecture/ADR-016-config-change-bus.md)
- OpenTelemetry specification — Resource semantics, OTLP/HTTP transport.
