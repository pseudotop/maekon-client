[English](./ADR-014-tauri-managed-state-boundary.md) | [한국어](./ADR-014-tauri-managed-state-boundary.ko.md)

# ADR-014: Tauri Managed State Boundary

**Status**: Accepted
**Date**: 2026-04-02
**Scope**: `src-tauri/`, Tauri managed state, IPC command boundary, and composition-root wiring

---

## Context

ADR-001 correctly establishes the default client rule: cross-crate behavior should flow through
`oneshim-core` ports rather than through concrete adapter types.

However, `oneshim-app` also owns the desktop entry point and Tauri framework boundary. That creates
three pressures that are not fully answered by ADR-001 alone:

1. Tauri managed state is retrieved by exact type. It is a framework-level storage mechanism, not a
   domain port boundary.
2. Some desktop command paths need operational capabilities such as lifecycle control, retries,
   token accounting, or persistence coordination that are meaningful inside `src-tauri` but are not
   reusable business contracts for the wider workspace.
3. Leaving raw implementation types in `AppState` makes the command boundary depend on internal
   service shapes. Pushing every desktop-only helper method into `oneshim-core` would instead
   overfit the core to a specific delivery/runtime framework.

The result is architectural drift in either direction:

- raw implementation objects leak into framework-managed state, or
- `oneshim-core` ports grow delivery-specific and runtime-specific methods only to satisfy Tauri.

This ADR defines the best-practice boundary for that middle ground.

---

## Decisions

### 1. Keep Concrete Composition at the Entry Point

`oneshim-app` remains the single composition root for the desktop binary.

Concrete adapter construction is allowed in:

- `src-tauri/src/main.rs`
- `src-tauri/src/setup*.rs`
- app-layer builders and launch coordinators such as `app_runtime_launch.rs`

Concrete composition is **not** itself a violation. That is the job of the composition root.

### 2. Managed State Uses Ports or Binary-Local Boundary Types

Any state type registered with Tauri and consumed by commands, event handlers, or background
callbacks MUST use one of these forms:

1. `Arc<dyn PortTrait>` from `oneshim-core` when the capability is a real cross-crate contract.
2. A purpose-built binary-local facade / handle type defined in `src-tauri` when the capability is
   desktop-specific, framework-specific, or orchestration-specific.
3. A framework-native runtime handle such as `AppHandle`, window handles, background runtime
   coordination objects, or channel senders/receivers.

Examples of acceptable managed-state boundary types:

```rust
pub struct AiSessionRuntimeHandle {
    pub session_manager: Arc<dyn SessionManager>,
    pub session_storage: Arc<dyn SessionStoragePort>,
    pub token_budget: Arc<TokenBudgetTracker>,
}

pub struct AutomationCommandHandle {
    pub tx: tokio::sync::mpsc::Sender<AutomationCommand>,
}
```

### 3. Raw Implementation Objects Are Not the Default Command Boundary

New managed-state fields MUST NOT expose raw implementation types directly as the command boundary.

This prohibition applies whether the implementation type lives:

- in another workspace crate, or
- in `src-tauri` itself

Examples of prohibited new boundary shapes:

```rust
pub struct AppState {
    pub session_manager: Arc<SessionManagerImpl>;
    pub storage: Arc<SqliteStorage>;
}
```

The problem is not that these types are concrete during composition. The problem is that commands
and framework callbacks become coupled to raw implementation details rather than to an explicit
boundary type.

### 4. Choose the Boundary Form by Responsibility

When adding a new capability to Tauri-managed state, use this decision order:

1. Use a `oneshim-core` port when the capability expresses a stable business or application
   contract that could reasonably be implemented or consumed by more than one crate.
2. Use a `src-tauri` facade / handle when the capability is specific to desktop delivery,
   framework lifecycle, or command orchestration.
3. Use an actor-style handle with message passing when the underlying resource requires serialized
   async access, explicit backpressure, or exclusive ownership of an I/O-heavy runtime object.

This means:

- domain/application contracts belong in `oneshim-core`
- desktop command orchestration belongs in `src-tauri`
- serialized async resource ownership may justify a manager task plus channels

### 5. Prefer Narrow Managed States Over a Growing Mega-State

New desktop features SHOULD prefer narrowly-scoped managed state types instead of extending a
single catch-all `AppState` whenever the feature can be isolated cleanly.

Preferred pattern:

```rust
app.manage(AiSessionRuntimeHandle::new(...));
app.manage(AudioRuntimeHandle::new(...));
```

This keeps Tauri's exact-type state retrieval explicit and prevents unrelated capabilities from
accumulating in one global struct.

### 6. Do Not Pollute `oneshim-core` to Satisfy a Framework

`oneshim-core` ports MUST NOT gain methods solely because a Tauri command wants a convenience API.

Examples of operations that are often better kept in a binary-local facade than forced into a core
port:

- framework shutdown coordination
- desktop-only token display aggregation
- UI retry/recovery helpers
- command-specific event emission coordination

If such operations later become true multi-crate contracts, they can be promoted into
`oneshim-core` intentionally. They should not start there by default.

### 7. Existing Raw Fields Are Legacy and Should Be Migrated Opportunistically

Some current `src-tauri` state still contains raw implementation objects. Those fields are treated
as legacy transitional debt, not as the target pattern.

Rules for legacy fields:

1. They may remain temporarily when required for ongoing delivery work.
2. New features MUST NOT copy the pattern.
3. When a legacy field is touched for meaningful feature work or refactoring, prefer replacing it
   with a facade / handle or a port-backed boundary.

This ADR sets the forward-looking rule without requiring a destabilizing repo-wide rewrite in one
change.

---

## Alternatives Considered

### A. Put Every Needed Method on `oneshim-core` Ports

Rejected as the default.

This keeps command code fully trait-based, but it pushes Tauri-specific lifecycle and convenience
operations into the core even when they are not reusable business contracts. That weakens the
semantic role of `oneshim-core`.

### B. Keep Raw Implementation Types in `AppState`

Rejected as the default.

This is the simplest short-term implementation, but it couples commands and callbacks to internal
service shapes and makes later replacement, testing, and review harder.

### C. Use Binary-Local Facades / Handles

Accepted as the default.

This preserves a concrete framework-facing type for Tauri while keeping the boundary explicit and
crate-local. It also allows the facade to combine multiple ports and local helpers without forcing
them into `oneshim-core`.

### D. Use Actor / Message-Passing Handles Everywhere

Rejected as the universal rule.

Actor-style handles are strong when a resource needs exclusive async ownership or bounded queuing.
They are unnecessary overhead for every command boundary, especially when a simple facade around
port-backed collaborators is enough.

---

## Consequences

### Positive

1. `oneshim-core` stays focused on reusable contracts instead of Tauri convenience APIs.
2. Tauri state remains explicit and framework-friendly without normalizing raw implementation leaks.
3. Commands become easier to review because they depend on named boundary handles instead of wide
   service internals.
4. Future refactors can move from a legacy mega-state toward feature-scoped state without changing
   the composition-root rule.

### Negative

1. Some features will require an extra facade / handle type in `src-tauri`.
2. Contributors must decide whether a capability belongs in a core port, a local facade, or an
   actor handle.
3. Existing legacy state fields may coexist with the preferred pattern during migration.

---

## Review Checklist

For any PR that adds or changes Tauri-managed state:

1. Is the new state entry a core port, a binary-local facade / handle, or a framework-native
   runtime handle?
2. If the change introduces a raw implementation type, is there a documented reason it cannot be a
   facade / handle instead?
3. Does the change avoid adding framework-only convenience methods to `oneshim-core`?
4. Would an actor handle be more appropriate because the resource is async, exclusive, or
   backpressure-sensitive?
5. Could the feature use a narrow managed state instead of growing `AppState` further?

---

## Research Notes

This decision follows the repository's needs and is informed by these primary references:

- Alistair Cockburn, *Hexagonal architecture the original 2005 article*:
  https://alistair.cockburn.us/hexagonal-architecture
- Mark Seemann, *Composition Root*:
  https://blog.ploeh.dk/2011/07/28/CompositionRoot/
- Tauri v2 official docs, *State Management*:
  https://v2.tauri.app/develop/state-management/
- Tokio official tutorial, *Shared state*:
  https://tokio.rs/tokio/tutorial/shared-state
- Tokio official tutorial, *Channels*:
  https://tokio.rs/tokio/tutorial/channels

---

## Related

- [ADR-001: Rust Client Architecture Patterns](./ADR-001-rust-client-architecture-patterns.md)
- [ADR-007: Async Runtime Safety Patterns](./ADR-007-async-runtime-safety-patterns.md)
- [ADR-009: Client Architecture Baseline](./ADR-009-client-architecture-baseline.md)
