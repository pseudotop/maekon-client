# Crate Implementation Docs

Detailed implementation reference for the ONESHIM Rust client's current 15-package workspace
(14 packages under `crates/` plus the `src-tauri` binary package; `cargo metadata --no-deps`
is the source of truth).

## Crate Dependency Graph

```
┌──────────────────────────────────────────────────────────────────────┐
│      src-tauri/ (package: oneshim-app, composition root)            │
│  runtime wiring, scheduler, desktop lifecycle, web server startup   │
└──────────────────────────────────────────────────────────────────────┘
          │
          ├── runtime adapters: analysis / audio / automation / embedding / monitor
          ├── runtime adapters: network / storage / suggestion / vision / web
          └── shared contracts: oneshim-core / oneshim-api-contracts

oneshim-core
  └── domain models, configuration, errors, and cross-crate ports

oneshim-api-contracts
  └── shared HTTP/integration DTO contract crate used by oneshim-web and oneshim-network

Runtime adapter baseline (normal dependencies only)
  ├── oneshim-analysis   -> oneshim-core
  ├── oneshim-audio      -> oneshim-core
  ├── oneshim-automation -> oneshim-core
  ├── oneshim-embedding  -> oneshim-core
  ├── oneshim-monitor    -> oneshim-core
  ├── oneshim-storage    -> oneshim-core
  ├── oneshim-suggestion -> oneshim-core
  ├── oneshim-vision     -> oneshim-core
  ├── oneshim-network    -> oneshim-core + oneshim-api-contracts
  └── oneshim-web        -> oneshim-core + oneshim-api-contracts

Out-of-process isolated executor (spawned by oneshim-app)
  └── oneshim-sandbox-worker -> oneshim-core
      (standalone binary; stdin SandboxRequest JSON → stdout SandboxResponse JSON under
       platform sandbox — Job Object on Windows, seccomp+Landlock on Linux, App Sandbox on macOS)

Tooling package
  └── oneshim-lint (workspace-local lint/test helper, not part of the runtime graph)
```

## Active Workspace Packages

| Package | Location | Role | Docs |
|--------|----------|------|------|
| **oneshim-core** | `crates/oneshim-core` | Foundation layer: models, ports, errors, config | [Details](./oneshim-core.md) |
| **oneshim-api-contracts** | `crates/oneshim-api-contracts` | Shared transport contract SSOT for web/integration DTOs | [Details](./oneshim-api-contracts.md) |
| **oneshim-audio** | `crates/oneshim-audio` | Audio capture, STT providers, model download helpers | Pending dedicated crate doc |
| **oneshim-monitor** | `crates/oneshim-monitor` | System monitoring adapter | [Details](./oneshim-monitor.md) |
| **oneshim-vision** | `crates/oneshim-vision` | Edge capture, OCR, privacy filter, accessibility helpers | [Details](./oneshim-vision.md) |
| **oneshim-network** | `crates/oneshim-network` | HTTP/SSE/WebSocket/gRPC/network adapters | [Details](./oneshim-network.md) |
| **oneshim-storage** | `crates/oneshim-storage` | SQLite persistence, retention, sync extraction/merge | [Details](./oneshim-storage.md) |
| **oneshim-suggestion** | `crates/oneshim-suggestion` | Suggestion queue, history, feedback pipeline | [Details](./oneshim-suggestion.md) |
| **oneshim-web** | `crates/oneshim-web` | Local web delivery layer: Axum + embedded frontend | [Details](./oneshim-web.md) |
| **oneshim-automation** | `crates/oneshim-automation` | Policy, sandbox, audit, GUI automation execution | [Details](./oneshim-automation.md) |
| **oneshim-analysis** | `crates/oneshim-analysis` | Analysis pipeline, coaching, regime/tiered-memory logic | Pending dedicated crate doc |
| **oneshim-embedding** | `crates/oneshim-embedding` | Local embedding provider adapter | Pending dedicated crate doc |
| **oneshim-lint** | `crates/oneshim-lint` | Workspace-local tooling and language/lint helpers | Pending dedicated crate doc |
| **oneshim-sandbox-worker** | `crates/oneshim-sandbox-worker` | Out-of-process sandboxed automation action executor (stdin JSON → stdout JSON under platform sandbox) | Pending dedicated crate doc |
| **oneshim-app** | `src-tauri` | Binary package / composition root / desktop runtime orchestration | [Details](./oneshim-app.md) |

## Historical Package Docs

| Package | Status | Docs |
|--------|--------|------|
| **oneshim-ui** | Removed from the workspace during the iced -> Tauri migration; kept only as historical reference | [Historical](./oneshim-ui.md) |

## Architecture Principles

### Hexagonal Architecture (Ports & Adapters)

- **Core**: `oneshim-core` defines all ports (traits) and domain models.
- **Transport contract**: `oneshim-api-contracts` holds shared delivery/integration DTOs.
- **Adapters**: Runtime adapter crates depend on `oneshim-core`; delivery/network crates may also depend on `oneshim-api-contracts`.
- **Composition root**: `oneshim-app` (package in `src-tauri/`) is the only package that aggregates multiple runtime adapters directly.

### Cross-Crate Communication Rules

1. Normal runtime dependencies must target `oneshim-core`, or `oneshim-api-contracts` when sharing transport DTOs.
2. Direct adapter aggregation is reserved for `oneshim-app` in `src-tauri/`.
3. Current non-core normal dependency exceptions are `oneshim-network -> oneshim-api-contracts` and `oneshim-web -> oneshim-api-contracts`; `oneshim-audio` remains a core-only adapter.
4. Dev/build-only dependencies are tracked separately and are not treated as runtime architecture edges.
5. CI enforces the current runtime baseline via `scripts/check-architecture-deps.sh`.

### DI Pattern

- Constructor injection with `Arc<dyn T>`
- No DI framework; manual wiring
- Wiring is handled in `src-tauri/src/main.rs`, `src-tauri/src/setup.rs`, and app-layer builders such as `app_runtime_launch.rs`, `agent_runtime.rs`, and `web_server_runtime.rs`

### Two-Layer Automation Action Model

- **AutomationIntent** (server -> client): High-level intent (e.g., ClickElement, TypeIntoElement)
- **AutomationAction** (internal client): Low-level action (e.g., MouseMove, MouseClick, KeyType)
- **IntentResolver**: Converts intent into executable action sequence (with OCR + LLM assistance)

## Main Flows

### Monitoring Flow (1-second interval)

```
SystemMonitor -> ProcessMonitor -> ActivityMonitor
       │              │               │
       └──────────────┴───────────────┘
                      │
                      ▼
               ContextEvent
                      │
          ┌───────────┴───────────┐
          ▼                       ▼
    CaptureTrigger            Storage
          │                       │
          ▼                       │
    FrameProcessor                │
          │                       │
          └───────────┬───────────┘
                      ▼
               BatchUploader
                      │
                      ▼
                    Server
```

### Suggestion Reception Flow

```
Server (SSE) -> SseClient -> SuggestionReceiver -> PriorityQueue
                                    │                 │
                                    ▼                 ▼
                            DesktopNotifier    MainWindow (UI)
                                                      │
                                                      ▼
                                              FeedbackSender
                                                      │
                                                      ▼
                                               Server (REST)
```

### Automation Execution Flow

```
Server (AutomationIntent)
          │
          ▼
  AutomationController
          │
    ┌─────┴──────┐
    ▼            ▼
PolicyClient  AuditLogger
(validate)     (record)
    │
    ▼
IntentResolver
    │
    ├── ElementFinder (OCR)
    ├── LlmProvider
    └── PrivacyGateway
          │
          ▼
  AutomationAction[]
          │
          ▼
    ┌─────┴──────┐
    ▼            ▼
InputDriver   Sandbox
(execute)     (isolate)
```

## Test and Quality Status

This file intentionally avoids hard-coded totals for test counts, warning counts, and pass/fail status. Use the current GitHub Actions run pages as the live source of truth.

## References

- [Documentation Index](../README.md)
- [ADR-001: Rust Client Architecture Patterns](../architecture/ADR-001-rust-client-architecture-patterns.md)
- [ADR-002: OS GUI Interaction Boundary and Runtime Split](../architecture/ADR-002-os-gui-interaction-boundary.md)
- [ADR-009: Client Architecture Baseline](../architecture/ADR-009-client-architecture-baseline.md)
- [CONTRIBUTING.md](../../CONTRIBUTING.md) - Contribution workflow
- [Contributing Guide](../../CONTRIBUTING.md)
- [Code of Conduct](../../CODE_OF_CONDUCT.md)
- [Security Policy](../../SECURITY.md)
