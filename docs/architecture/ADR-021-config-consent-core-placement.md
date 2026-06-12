[English](./ADR-021-config-consent-core-placement.md) | [한국어](./ADR-021-config-consent-core-placement.ko.md)

# ADR-021: Config and Consent Core Placement

**Status**: Accepted
**Date**: 2026-05-28
**Scope**: `crates/maekon-core/src/config_manager`, `crates/maekon-core/src/consent.rs`, runtime wiring
**Related**: ADR-001, ADR-014, ADR-016, ADR-019
**Implementation**: `crates/maekon-core/src/config_manager/`, `crates/maekon-core/src/consent.rs`, `src-tauri/src/app_runtime_launch/capture_wiring.rs`

---

## Context

The local consolidation work for privacy gates and runtime hardening raised a placement question: should file-backed `ConfigManager` and `ConsentManager` remain in `maekon-core`, or should they move behind adapter ports because they read and write local files?

The usual Hexagonal rule says domain contracts must not depend on infrastructure. At the same time, Maekon client has two cross-cutting local authorities:

- configuration gates such as `vision.capture_enabled`, active hours, and tracking schedule;
- privacy consent gates such as screen capture and full-text extraction consent.

Both authorities are consumed by many adapters and runtime loops. Moving their public API into an adapter crate would force core-facing code to depend on a concrete runtime adapter or create parallel DTOs for the same policy state.

## Decision

### 1. Keep ConfigManager and ConsentManager in maekon-core

`ConfigManager` and `ConsentManager` stay in `maekon-core` as local state managers because they define product policy state, validation, migrations, defaults, and consent semantics used across the workspace.

This is an approved boundary exception: their file-backed persistence is treated as local product state persistence, not remote or platform infrastructure.

### 2. Do not put external side effects in those managers

The managers may read and write their own local JSON state files. They must not perform provider calls, network egress, native automation, screen capture, notification delivery, or OS permission mutation.

Those effects remain behind ports or runtime adapters such as provider catalog, frame storage, notification, capture, and automation ports.

### 3. Use runtime composition to pass managers downward

`src-tauri` owns composition. It may construct the managers and pass clones to web services, scheduler loops, capture wiring, and provider guards. Consumers should read snapshots or subscribe through the existing change bus instead of creating independent file-backed managers.

### 4. Add a port only when the persistence backend becomes replaceable

If configuration or consent state gains a second production backend, such as encrypted cloud sync or OS keychain-backed consent records, introduce a `maekon-core` port at that point. Until then, an adapter port would add indirection without reducing real coupling.

## Consequences

### Positive

- A single source of truth controls capture, consent, schedule, and privacy gates.
- Runtime loops can enforce fail-closed gates before capture, AX extraction, GUI analysis, or provider calls.
- Public API stability is preserved for existing web, Tauri, and scheduler consumers.

### Negative

- `maekon-core` continues to contain a small amount of local file I/O.
- Tests must keep validating that the managers do not grow external side effects.

### Neutral

- `ConfigManager` remains a core local-state service, while network/provider discovery still moves behind core/network ports.

## Alternatives Considered

**A. Move both managers to a storage adapter crate.** Rejected because consumers throughout core-facing runtime code would need an adapter dependency or an equivalent duplicated trait boundary for policy state.

**B. Introduce ports immediately for the existing JSON files.** Rejected because there is only one production backend today. The added indirection would not remove side effects that actually matter for privacy or testability.

**C. Split file I/O into storage but keep DTOs in core.** Rejected for now because it would make startup and migration semantics harder to reason about while preserving nearly the same persistence coupling.

## Known Follow-ups

1. Keep `ConfigManager` and `ConsentManager` tests focused on local persistence, migration, snapshots, and consent semantics.
2. If a second backend appears, add a core port before wiring that backend into `src-tauri`.
3. Keep external egress, native GUI mutation, and OS permission changes outside both managers.

## Related Docs

- `docs/architecture/ADR-001-rust-client-architecture-patterns.md`
- `docs/architecture/ADR-014-tauri-managed-state-boundary.md`
- `docs/architecture/ADR-016-config-change-bus.md`
- `docs/architecture/ADR-019-error-code-infrastructure.md`
