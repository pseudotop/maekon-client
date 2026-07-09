# Crate Implementation Docs

Detailed implementation reference for the MAEKON Rust client's current 15-package workspace
(14 packages under `crates/` plus the `src-tauri` binary package; `cargo metadata --no-deps`
is the source of truth).

## Crate Dependency Graph

```
┌──────────────────────────────────────────────────────────────────────┐
│      src-tauri/ (package: maekon-app, composition root)            │
│  runtime wiring, scheduler, desktop lifecycle, web server startup   │
└──────────────────────────────────────────────────────────────────────┘
          │
          ├── runtime adapters: analysis / audio / automation / embedding / monitor
          ├── runtime adapters: network / storage / suggestion / vision / web
          └── shared contracts: maekon-core / maekon-api-contracts

maekon-core
  └── domain models, configuration, errors, and cross-crate ports

maekon-api-contracts
  └── shared HTTP/integration DTO contract crate used by maekon-web and maekon-network

Runtime adapter baseline (normal dependencies only)
  ├── maekon-analysis   -> maekon-core
  ├── maekon-audio      -> maekon-core
  ├── maekon-automation -> maekon-core
  ├── maekon-embedding  -> maekon-core
  ├── maekon-monitor    -> maekon-core
  ├── maekon-storage    -> maekon-core
  ├── maekon-suggestion -> maekon-core
  ├── maekon-vision     -> maekon-core
  ├── maekon-network    -> maekon-core + maekon-api-contracts
  └── maekon-web        -> maekon-core + maekon-api-contracts

Out-of-process isolated executor (spawned by maekon-app)
  └── maekon-sandbox-worker -> maekon-core
      (standalone binary; stdin SandboxRequest JSON → stdout SandboxResponse JSON under
       platform sandbox — Job Object on Windows, seccomp+Landlock on Linux, App Sandbox on macOS)

Tooling package
  └── maekon-lint (workspace-local lint/test helper, not part of the runtime graph)
```

## Active Workspace Packages

| Package | Location | Role | Docs |
|--------|----------|------|------|
| **maekon-core** | `crates/maekon-core` | Foundation layer: models, ports, errors, config | [Details](./maekon-core.md) |
| **maekon-api-contracts** | `crates/maekon-api-contracts` | Shared transport contract SSOT for web/integration DTOs | [Details](./maekon-api-contracts.md) |
| **maekon-audio** | `crates/maekon-audio` | Audio capture, STT providers, model download helpers | Pending dedicated crate doc |
| **maekon-monitor** | `crates/maekon-monitor` | System monitoring adapter | [Details](./maekon-monitor.md) |
| **maekon-vision** | `crates/maekon-vision` | Edge capture, OCR, privacy filter, accessibility helpers | [Details](./maekon-vision.md) |
| **maekon-network** | `crates/maekon-network` | HTTP/SSE/WebSocket/gRPC/network adapters | [Details](./maekon-network.md) |
| **maekon-storage** | `crates/maekon-storage` | SQLite persistence, retention, sync extraction/merge | [Details](./maekon-storage.md) |
| **maekon-suggestion** | `crates/maekon-suggestion` | Suggestion queue, history, feedback pipeline | [Details](./maekon-suggestion.md) |
| **maekon-web** | `crates/maekon-web` | Local web delivery layer: Axum + embedded frontend | [Details](./maekon-web.md) |
| **maekon-automation** | `crates/maekon-automation` | Policy, sandbox, audit, GUI automation execution | [Details](./maekon-automation.md) |
| **maekon-analysis** | `crates/maekon-analysis` | Analysis pipeline, coaching, regime/tiered-memory logic, focus/workflow intelligence (#7735 E-2) | [Details](./maekon-analysis.md) |
| **maekon-embedding** | `crates/maekon-embedding` | Local embedding provider adapter | Pending dedicated crate doc |
| **maekon-lint** | `crates/maekon-lint` | Workspace-local tooling and language/lint helpers | Pending dedicated crate doc |
| **maekon-sandbox-worker** | `crates/maekon-sandbox-worker` | Out-of-process sandboxed automation action executor (stdin JSON → stdout JSON under platform sandbox) | Pending dedicated crate doc |
| **maekon-app** | `src-tauri` | Binary package / composition root / desktop runtime orchestration | [Details](./maekon-app.md) |

## Architecture Principles

### Hexagonal Architecture (Ports & Adapters)

- **Core**: `maekon-core` defines all ports (traits) and domain models.
- **Transport contract**: `maekon-api-contracts` holds shared delivery/integration DTOs.
- **Adapters**: Runtime adapter crates depend on `maekon-core`; delivery/network crates may also depend on `maekon-api-contracts`.
- **Composition root**: `maekon-app` (package in `src-tauri/`) is the only package that aggregates multiple runtime adapters directly.

### Cross-Crate Communication Rules

1. Normal runtime dependencies must target `maekon-core`, or `maekon-api-contracts` when sharing transport DTOs.
2. Direct adapter aggregation is reserved for `maekon-app` in `src-tauri/`.
3. Current non-core normal dependency exceptions are `maekon-network -> maekon-api-contracts` and `maekon-web -> maekon-api-contracts`; `maekon-audio` remains a core-only adapter.
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
