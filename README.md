<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="./assets/brand/logo-full-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="./assets/brand/logo-full-light.svg">
    <img alt="Maekon" src="./assets/brand/logo-full-light.svg" width="400">
  </picture>
</p>

<p align="center">
  <a href="./README.md">English</a> | <a href="./README.ko.md">한국어</a> | <a href="./README.ja.md">日本語</a> | <a href="./README.zh-CN.md">简体中文</a> | <a href="./README.es.md">Español</a>
</p>

# Maekon

> **Local work signals, policy-gated action paths.**
> Maekon organizes local work signals into a focus timeline, next-action candidates, and policy-gated automation paths.

Maekon is an Apache-2.0 local-first desktop agent that can be used independently without ONESHIM. It provides local context capture, user-reviewed next-action candidates, policy-gated automation, and a built-in dashboard. Built with Rust and Tauri v2 (WebView shell around a React frontend) for native performance across macOS, Windows, and Linux.

## Table of Contents

- [Source Build Quick Start](#source-build-quick-start)
- [Why Maekon](#why-maekon)
- [Who It's For](#who-its-for)
- [2-Minute Quickstart](#2-minute-quickstart)
- [Safety and Privacy at a Glance](#safety-and-privacy-at-a-glance)
- [Features](#features)
- [Requirements](#requirements)
- [Developer Quick Start](#developer-quick-start-build-from-source)
- [Installation](#installation)
- [Configuration](#configuration)
- [Architecture](#architecture)
- [Development](#development)
- [License](#license)
- [Contributing](#contributing)

## Source Build Quick Start

The public repository is live, and `v0.0.1-rc.5` is available as the current
public prerelease. Because GitHub's `latest` release endpoint excludes
prereleases, use the version-pinned installer commands in the install guide for
release-binary testing. For monorepo development and debug builds, run Maekon
from a local source checkout:

```bash
git clone https://github.com/pseudotop/maekon-client.git
cd maekon-client
./scripts/cargo-cache.sh run -p maekon-app -- --offline
```

Release installer commands are documented below. For prerelease version pinning,
signature enforcement, and uninstall:
- English: [`docs/install.md`](./docs/install.md)
- Korean: [`docs/install.ko.md`](./docs/install.ko.md)

## Why Maekon

- **Turn activity into governed work insight**: Track context, timeline, focus trends, interruptions, and approved automation paths in one place.
- **Stay lightweight on-device**: Edge processing (delta encoding, thumbnailing, OCR) reduces transfer volume and keeps response fast.
- **Use a production-ready desktop stack**: Cross-platform binary, auto-update, system tray integration, and local web dashboard.

### Market positioning (2026)

Google DeepMind (AI Pointer, 2026-05) and OpenAI (Codex Chronicle, 2026-04) both entered the same problem space — **AI that understands screen context and acts on natural pointing/typing intent**. Maekon differentiates on four axes:

1. **Local-first by default** — pixels, OCR, and signals stay on-device; cloud round-trips are opt-in
2. **Source-first audit** — every signal has a traceable origin, retention policy, and PII filter step
3. **Policy-gated automation** — natural intent ("summarize this", "organize that") resolves to **next-action candidates** with explicit review/approval boundaries, not direct execution
4. **App- and OS-crossing** — works across Chrome, native apps, terminals, and OS-level workflows (3 OS: macOS, Windows, Linux), not bound to a single vendor's ecosystem

See [`docs/market-positioning-references.md`](./docs/market-positioning-references.md) for the full positioning matrix and references.

## Who It's For

- Individual contributors who want visibility into focus patterns and work context
- Teams building AI-assisted workflow tooling on top of rich desktop signals
- Developers who want a modular, high-performance client with clear architecture boundaries

## 2-Minute Quickstart

```bash
# 1) Run in standalone mode (recommended for security-sensitive environments)
./scripts/cargo-cache.sh run -p maekon-app -- --offline

# 2) Open local dashboard
# http://localhost:10090
```

Standalone mode is available now.

Connected mode is available only as an opt-in preview path.
Standalone mode remains the production-ready default path for release use.

## Safety and Privacy at a Glance

- PII filtering levels (Off/Basic/Standard/Strict) are applied in the vision pipeline
- Local data is stored in SQLite and managed with retention controls
- Automation requires policy validation, sandbox profiles, and local audit logging
- Security reporting and response policy: [SECURITY.md](./SECURITY.md)
- Standalone integrity baseline: [docs/security/standalone-integrity-baseline.md](./docs/security/standalone-integrity-baseline.md)
- Integrity operation runbook: [docs/security/integrity-runbook.md](./docs/security/integrity-runbook.md)
- Documentation index: [docs/README.md](./docs/README.md)
- Automation playbook templates: [docs/guides/automation-playbook-templates.md](./docs/guides/automation-playbook-templates.md)
- Standalone adoption runbook: [docs/guides/standalone-adoption-runbook.md](./docs/guides/standalone-adoption-runbook.md)
- First 5 minutes guide: [docs/guides/first-5-minutes.md](./docs/guides/first-5-minutes.md)
- Automation event contract: [docs/contracts/automation-event-contract.md](./docs/contracts/automation-event-contract.md)
- AI provider contract: [docs/contracts/ai-provider-contract.md](./docs/contracts/ai-provider-contract.md)

## Features

### Core Features
- **Real-time Context Monitoring**: Tracks active windows, system resources, and user activity
- **Edge Image Processing**: Screenshot capture, delta encoding, thumbnails, and OCR
- **Policy-Gated Automation**: Routes approved actions through policy checks, sandbox isolation, and audit logging
- **Connected Server Features (Preview / Opt-in)**: Real-time suggestions and feedback sync are available for staged validation and are not the default production path
- **System Tray**: Runs in the background with quick access
- **Auto-Update**: Automatic updates based on GitHub Releases
- **Cross-Platform**: Supports macOS, Windows, and Linux

### Local Web Dashboard (http://localhost:10090)
- **Dashboard**: Real-time system metrics, CPU/memory charts, app usage time
- **Timeline**: Screenshot timeline, tag filtering, lightbox viewer
- **Reports**: Weekly/monthly activity reports, productivity analysis
- **Session Replay**: Session replay with app segment visualization
- **Focus Analytics**: Focus analysis, interruption tracking, local suggestions
- **Settings**: Configuration management, data export/backup

### Desktop Notifications
- **Idle Notification**: Triggered after 30+ minutes of inactivity
- **Long Session Notification**: Triggered after 60+ minutes of continuous work
- **High Usage Notification**: Triggered when CPU/memory exceeds 90%
- **Focus Suggestions**: Break reminders, focus time scheduling, context restoration

## Requirements

- Rust 1.77.1 or later
- macOS 10.15+ / Windows 10+ / Linux (X11/Wayland)

## Developer Quick Start (Build from Source)

This is the normal local debug and development path for Maekon Client from a
source checkout. Internal maintainers use the same commands before exporting
public snapshots; public contributors can use them directly in this repository.

### Build

```bash
# Build embedded web dashboard assets (required before packaging/release builds)
./scripts/build-frontend.sh

# Development build
./scripts/cargo-cache.sh build -p maekon-app

# Release build
./scripts/cargo-cache.sh build --release -p maekon-app

# Build desktop app (Tauri v2, v0.1.5+)
cd src-tauri && cargo tauri build

# Start dev server with frontend HMR (v0.1.5+)
cd src-tauri && cargo tauri dev
```

### Build Cache (Recommended for Local Development)

```bash
# Optional: install sccache
brew install sccache

# Use cached Rust builds via helper wrapper
./scripts/cargo-cache.sh check --workspace
./scripts/cargo-cache.sh test -p maekon-web
./scripts/cargo-cache.sh build -p maekon-app
```

If `sccache` is not installed, the wrapper falls back to normal `cargo`.

`cargo-cache.sh` also enforces target-size guardrails to prevent local disk bloat:
- Soft limit (`MAEKON_TARGET_SOFT_LIMIT_MB`, default `8192`): prunes `target/debug/incremental`, then `target/debug/deps` if still large
- Hard limit (`MAEKON_TARGET_HARD_LIMIT_MB`, default `12288`): additionally prunes `target/debug/build`
- Auto prune toggle: `MAEKON_TARGET_AUTO_PRUNE=1` (default) / `0` (disable)
- Current cache status: `./scripts/cargo-cache.sh --status`

Example custom limits:
```bash
MAEKON_TARGET_SOFT_LIMIT_MB=4096 \
MAEKON_TARGET_HARD_LIMIT_MB=6144 \
./scripts/cargo-cache.sh test --workspace
```

### Run

```bash
# Standalone mode (recommended)
./scripts/cargo-cache.sh run -p maekon-app -- --offline
```

Connected mode is preview-only and intentionally gated behind explicit server/auth configuration.
Use standalone mode as the default production path unless your environment has validated connected mode.

For headless CI/remote debug sessions where macOS tray bootstrap can fail due missing WindowServer:
```bash
MAEKON_DISABLE_TRAY=1 ./scripts/cargo-cache.sh run -p maekon-app -- --offline --gui
```
Use this only for non-interactive smoke/debug paths.

### Test

```bash
# Rust tests
./scripts/cargo-cache.sh test --workspace

# E2E tests — web dashboard
cd crates/maekon-web/frontend && pnpm test:e2e

# Lint (policy: zero warnings in CI)
./scripts/cargo-cache.sh clippy --workspace

# Format check
./scripts/cargo-cache.sh fmt --check

# Language / i18n quality checks
./scripts/check-language.sh
# i18n-only check
./scripts/check-language.sh i18n
# scope-limited scan (example)
./scripts/check-language.sh non-english --path crates/maekon-web/frontend/src
# Optional: strict mode (fails on hardcoded UI copy warnings too)
./scripts/check-language.sh --strict-i18n
```

### macOS WindowServer Smoke (Self-hosted)

For real macOS GUI bootstrap verification with a live WindowServer session, run:
- Workflow: `.github/workflows/macos-windowserver-gui-smoke.yml`
- Runner labels: `self-hosted`, `macOS`, `windowserver`

## Installation

Full install guide:
- English: [`docs/install.md`](./docs/install.md)
- Korean: [`docs/install.ko.md`](./docs/install.ko.md)

### Quick Install (Terminal)

> The current public binary release is the prerelease `v0.0.1-rc.5`. GitHub's
> `latest` stable URL is not available until the first stable release, so the
> commands below pin the prerelease explicitly.

macOS / Linux:
```bash
curl -fsSL -o /tmp/maekon-install.sh \
  https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.sh
MAEKON_VERSION=v0.0.1-rc.5 bash /tmp/maekon-install.sh
```

Windows (PowerShell):
```powershell
$tmp = Join-Path $env:TEMP "maekon-install.ps1"
Invoke-WebRequest -UseBasicParsing `
  -Uri "https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.ps1" `
  -OutFile $tmp
powershell -ExecutionPolicy Bypass -File $tmp -Version v0.0.1-rc.5
```

### Release Assets

Download from [Releases](https://github.com/pseudotop/maekon-client/releases):

The current published prerelease is `v0.0.1-rc.5`. This table documents the
expected release asset names used by the installer, updater, checksum, and
signature flows.

Maekon is the app display name. Current release filenames intentionally retain
`maekon-*` for installer, updater, and checksum compatibility.

| Platform | File |
|--------|------|
| macOS Universal (DMG installer) | `maekon-macos-universal.dmg` |
| macOS Universal (PKG installer) | `maekon-macos-universal.pkg` |
| macOS Universal | `maekon-macos-universal.tar.gz` |
| macOS Apple Silicon | `maekon-macos-arm64.tar.gz` |
| macOS Intel | `maekon-macos-x64.tar.gz` |
| Windows x64 (zip) | `maekon-windows-x64.zip` |
| Windows x64 (MSI) | `maekon-app-*.msi` |
| Linux x64 (DEB package) | `maekon-*.deb` |
| Linux x64 | `maekon-linux-x64.tar.gz` |

## Configuration

### Environment Variables

Compatibility note: `MAEKON_*` environment variables, the `maekon` CLI command,
`com.maekon.app`, and existing config/data paths remain stable technical
identifiers for this release line.

| Variable | Description | Default |
|------|------|--------|
| `MAEKON_EMAIL` | Login email (connected mode only) | (optional in standalone) |
| `MAEKON_PASSWORD` | Login password (connected mode only) | (optional in standalone) |
| `MAEKON_TESSDATA` | Tesseract data path | (optional) |
| `MAEKON_DISABLE_TRAY` | Skip system tray initialization (headless CI/remote GUI smoke only) | `0` |
| `RUST_LOG` | Log level | `info` |

### Config File

`~/.config/maekon/config.json` (Linux) / `~/Library/Application Support/com.maekon.app/config.json` (macOS) / `%APPDATA%\maekon\agent\config.json` (Windows):

```json
{
  "server": {
    "base_url": "https://api.example.com",
    "request_timeout_ms": 30000,
    "sse_max_retry_secs": 30
  },
  "monitor": {
    "poll_interval_ms": 1000,
    "sync_interval_ms": 10000,
    "heartbeat_interval_ms": 30000
  },
  "storage": {
    "retention_days": 30,
    "max_storage_mb": 500
  },
  "vision": {
    "capture_throttle_ms": 5000,
    "thumbnail_width": 480,
    "thumbnail_height": 270,
    "ocr_enabled": false
  },
  "update": {
    "enabled": true,
    "repo_owner": "pseudotop",
    "repo_name": "maekon-client",
    "check_interval_hours": 24,
    "include_prerelease": false
  },
  "web": {
    "enabled": true,
    "port": 10090,
    "allow_external": false
  },
  "notification": {
    "enabled": true,
    "idle_threshold_mins": 30,
    "long_session_threshold_mins": 60,
    "high_usage_threshold_percent": 90
  }
}
```

## Architecture

A Cargo workspace with adapter crates following Hexagonal Architecture (Ports & Adapters). Since v0.1.5 the main binary entry point is `src-tauri/` (Tauri v2), which hosts the existing React dashboard in a WebView shell.

```
maekon-client/
├── src-tauri/              # Tauri v2 binary entry point (main binary, v0.1.5+)
│   ├── src/
│   │   ├── main.rs         # Tauri app builder + DI wiring
│   │   ├── tray.rs         # System tray menu
│   │   ├── commands/       # Tauri IPC commands (directory module)
│   │   └── scheduler/      # 16-loop background scheduler (monitor, metrics, process, sync, heartbeat, aggregation, notification, focus, event_snapshot, oauth_refresh, analysis, cross_device_sync, coaching + conditional: health_check, suggestion_sse, suggestion_maintenance)
│   └── tauri.conf.json     # Tauri configuration
├── crates/
│   ├── maekon-core/       # Domain models + port traits + errors + config
│   ├── maekon-network/    # HTTP/SSE/WebSocket/gRPC, compression, auth
│   ├── maekon-suggestion/ # Suggestion reception and processing
│   ├── maekon-storage/    # SQLite local storage + schema migration
│   ├── maekon-monitor/    # System metrics, active window, activity tracking
│   ├── maekon-vision/     # Screen capture, delta encoding, OCR, PII filter
│   ├── maekon-web/        # Local web dashboard (Axum REST + React frontend)
│   ├── maekon-automation/ # Automation control, policy, audit logging
│   ├── maekon-analysis/   # LLM analysis pipeline, regime classification
│   ├── maekon-embedding/  # Vector embedding + INT8 quantization
│   ├── maekon-audio/      # Audio capture (cpal) + STT (Whisper + cloud)
│   ├── maekon-sandbox-worker/ # Out-of-process sandboxed automation action executor
│   ├── maekon-api-contracts/ # Shared API type contracts
│   └── maekon-lint/       # Workspace lint tool (language-check binary)
└── docs/
    ├── crates/             # Per-crate detailed documentation
    ├── architecture/       # ADR documents (ADR-001~ADR-019; see docs/architecture/ADR-*.md)
    └── migration/          # Migration documents
```

### Crate Documentation

| Crate | Role | Docs |
|----------|------|------|
| maekon-core | Domain models, port interfaces, config | [Details](./docs/crates/maekon-core.md) |
| maekon-network | HTTP/SSE/WebSocket/gRPC, compression, auth | [Details](./docs/crates/maekon-network.md) |
| maekon-vision | Screen capture, delta encoding, OCR, PII filter | [Details](./docs/crates/maekon-vision.md) |
| maekon-monitor | System metrics, active windows, activity tracking | [Details](./docs/crates/maekon-monitor.md) |
| maekon-storage | SQLite storage, schema migration | [Details](./docs/crates/maekon-storage.md) |
| maekon-suggestion | Suggestion queue, SSE reception, feedback | [Details](./docs/crates/maekon-suggestion.md) |
| maekon-web | Local web dashboard (Axum REST + React) | [Details](./docs/crates/maekon-web.md) |
| maekon-automation | Automation control, policy, audit logging | [Details](./docs/crates/maekon-automation.md) |
| maekon-analysis | LLM analysis pipeline, regime classification | — |
| maekon-embedding | Vector embedding, INT8 quantization | — |
| maekon-audio | Audio capture, STT pipeline | — |
| maekon-sandbox-worker | Sandboxed automation action executor | — |
| maekon-api-contracts | Shared API type contracts | — |
| maekon-lint | Workspace lint tool (language-check) | — |

Full documentation index: [docs/crates/README.md](./docs/crates/README.md)

For contribution workflow, see [CONTRIBUTING.md](./CONTRIBUTING.md).

Documentation language and consistency rules are defined in [docs/DOCUMENTATION_POLICY.md](./docs/DOCUMENTATION_POLICY.md).
Translations: [한국어](./README.ko.md) | [日本語](./README.ja.md) | [简体中文](./README.zh-CN.md) | [Español](./README.es.md).
Korean companion policy doc: [docs/DOCUMENTATION_POLICY.ko.md](./docs/DOCUMENTATION_POLICY.ko.md).

## Development

### Code Style

- **Language**: English-first documentation with Korean companion docs for key public guides
- **Format**: `cargo fmt` default settings
- **Lint**: `cargo clippy` with 0 warnings

### Adding New Features

1. Define port traits in `maekon-core`
2. Implement adapters in the relevant crate
3. Wire up DI in `src-tauri/src/main.rs`
4. Add tests

### Building Installers

macOS .app bundle:
```bash
./scripts/cargo-cache.sh install cargo-bundle
./scripts/cargo-cache.sh bundle --release -p maekon-app
```

Windows .msi:
```bash
./scripts/cargo-cache.sh install cargo-wix
./scripts/cargo-cache.sh wix -p maekon-app
```

## License

Apache License 2.0 — see [LICENSE](./LICENSE)

- [Contributing Guide](./CONTRIBUTING.md)
- [Code of Conduct](./CODE_OF_CONDUCT.md)
- [Security Policy](./SECURITY.md)

## Contributing

1. Fork
2. Create a feature branch (`git checkout -b feature/amazing`)
3. Commit your changes (`git commit -m 'Add amazing feature'`)
4. Push the branch (`git push origin feature/amazing`)
5. Open a Pull Request
