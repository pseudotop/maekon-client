# ADR-009: Client Architecture Baseline

**Status**: Accepted
**Date**: 2026-03-17
**Scope**: `client-rust/`, with emphasis on the `oneshim-app` package (now lives in `src-tauri/` — the former `crates/oneshim-app/` directory was removed by ADR-004 Tauri v2 migration; the package name `oneshim-app` is preserved), `oneshim-web`, integration runtime, and AI provider surfaces

---

## Context

The client architecture has undergone several rounds of structural cleanup across:

- provider surface modeling
- AI runtime wiring
- integration plane runtime design
- `oneshim-app` composition-root structure
- `oneshim-web` delivery/service layering

Those changes are no longer experimental cleanup. They now represent the intended baseline for future development. Without an explicit ADR, there is a high risk that later work will gradually reintroduce:

- handler fattening
- `AppState` leakage into delivery code
- service-entry drift
- composition-root growth in `setup.rs`
- non-spec-driven AI provider behavior
- accidental collapse of the external integration plane back into the local control plane

This ADR freezes the current shape as the standard to maintain and improve from.

---

## Decisions

### 1. Core Layering Remains Stable

The following layer roles are now fixed:

1. `oneshim-core` is the domain contract layer.
2. Adapter crates implement ports from `oneshim-core`.
3. `oneshim-app` is the composition root and runtime orchestrator.
4. `oneshim-web` is a delivery layer only.
5. External integration remains distinct from the local desktop control plane.
6. AI provider behavior remains surface-driven and contract-driven.

This ADR does not replace ADR-001 or ADR-002. It operationalizes them as the accepted baseline for the current client shape.

### 2. `oneshim-web` Uses a Fixed Delivery Pattern

`oneshim-web` MUST keep the following structure:

1. Handlers stay thin.
2. Narrow delivery substates are represented as `WebContext` structs.
3. `WebContext` definitions live in [web_contexts/mod.rs](../../crates/oneshim-web/src/services/web_contexts/mod.rs).
4. Services are the public orchestration entrypoints at the delivery boundary.
5. Assemblers and helper modules own DTO shaping and pure transformation logic.
6. Handlers and services MUST NOT pull `AppState` directly except for explicit cross-cutting exceptions such as middleware.

Required handler flow:

```rust
State(WebContext) -> QueryService/CommandService -> Assembler/Helper
```

Forbidden drift:

1. defining new `WebContext` structs inside feature service files
2. reintroducing `context.queries()` and `context.commands()` factory helpers
3. moving domain invariants into handlers
4. letting web-only delivery concerns leak into `oneshim-core`

Preferred handler boundary:

```rust
XxxQueryService::new(context)
XxxCommandService::new(context)
```

### 3. `oneshim-app` Keeps Builder/Coordinator Composition

`oneshim-app` MUST preserve the current app-layer composition style.

Required shape:

1. `setup.rs` remains close to a pure assembly script.
2. Runtime bootstrap belongs in app-layer builders and coordinators.
3. Long-running orchestration belongs in bundles, runtime coordinators, or launch builders.

This applies to the runtime modules already in use, including:

- `integration_runtime`
- `agent_runtime`
- `web_server_runtime`
- `background_runtime`
- `storage_runtime`
- `update_runtime`

Forbidden drift:

1. growing `setup.rs` back into a feature-implementation file
2. embedding runtime-specific orchestration directly in Tauri setup wiring
3. bypassing builders for new runtime slices without a clear architectural reason

### 4. Integration Plane Remains Separate

The integration architecture is accepted as the correct direction and MUST be preserved.

Required shape:

1. Local `/api` remains a first-party control plane.
2. External integration remains a separate plane with its own auth and runtime model.
3. The integration runtime remains outbound and client-initiated.
4. Privacy, policy, and audit gates remain mandatory for all external egress.

Required modeling split:

1. `session/auth`
2. `egress/outbox`
3. `inbox`
4. `policy/audit`

These concerns MUST NOT be collapsed into one generic controller.

### 5. AI Provider Runtime Remains Spec-Driven

The AI/provider architecture is also accepted as baseline.

Required shape:

1. Provider behavior is driven from provider surface contracts and catalog specs.
2. `managed_oauth`, `direct_http`, `subprocess_cli`, and self-hosted surfaces remain modeled as explicit surfaces.
3. Settings, runtime, and UI consume the same surface contracts.
4. New providers should extend the spec-driven path before introducing special-case logic.

Forbidden drift:

1. ad hoc vendor branching where a surface contract already exists
2. delivery-layer AI behavior that bypasses provider surface resolution
3. reintroducing mismatched settings/runtime/provider interpretations

### 6. Explicit Exceptions Are Allowed but Narrow

The following exceptions are allowed and are not considered violations:

1. Middleware may still use `State<AppState>` directly for cross-cutting auth and boundary enforcement.
2. Pure specification helper modules may remain function-oriented when they are not orchestration entrypoints.
3. Test modules may construct `AppState` directly for fixtures.

These are explicit exceptions and should not be generalized into broader patterns.

---

## Consequences

### Positive

1. The client now has a clear architectural baseline for future work.
2. New work can be judged against stable rules instead of local style preference.
3. `oneshim-web` is significantly easier to review because handlers, contexts, services, and assemblers have clearer roles.
4. `oneshim-app` is less likely to regress into an oversized composition root.
5. Integration and AI/provider work can evolve without reopening already-solved structural problems.

### Negative

1. Some contributors may view the boundary rules as stricter than necessary.
2. Small features may require one extra service or helper type compared with a shortcut implementation.
3. Middleware and pure helper modules remain explicit exceptions, which requires some judgment in review.

### Operational Impact

Future refactors should optimize for real architectural wins, not cosmetic splitting.

This ADR does **not** mean:

1. every helper must become its own type
2. every utility must become a service
3. refactoring should continue indefinitely

From this point, the default is to extend the product on top of this baseline rather than repeatedly redesign the baseline itself.

---

## Review Checklist

Any substantial change that touches client architecture should still answer:

1. Does it preserve DDD and Hexagonal dependency direction?
2. Does it keep `oneshim-web` as a delivery layer?
3. Does it preserve the `WebContext -> service -> assembler/helper` flow?
4. Does it preserve privacy, policy, and audit gates for integration and automation?
5. Does it preserve the spec-driven AI/provider architecture?
6. Does it keep runtime wiring in builders/coordinators instead of regrowing `setup.rs`?

---

## Related

- [ADR-001: Rust Client Architecture Patterns](./ADR-001-rust-client-architecture-patterns.md)
- [ADR-002: OS GUI Interaction Boundary and Runtime Split](./ADR-002-os-gui-interaction-boundary.md)
- [ADR-003: Directory Module Pattern for Large Source Files](./ADR-003-directory-module-pattern.md)
- [ADR-007: Async Runtime Safety Patterns](./ADR-007-async-runtime-safety-patterns.md)
- [ADR-008: Network Resilience Patterns](./ADR-008-network-resilience-patterns.md)
- [ADR-014: Tauri Managed State Boundary](./ADR-014-tauri-managed-state-boundary.md)
