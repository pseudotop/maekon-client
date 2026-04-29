[English](./ADR-015-frame-storage-port.md) | [한국어](./ADR-015-frame-storage-port.ko.md)

# ADR-015: Frame Storage Port Abstraction

**Status**: Accepted
**Date**: 2026-04-03
**Scope**: `oneshim-core` ports, `oneshim-storage` adapter, `src-tauri` composition root

---

## Context

`FrameFileStorage` is a concrete type in `oneshim-storage` that handles frame image
persistence (WebP files organized by date directories with retention policies).

Currently, 10+ files in `src-tauri` reference `Arc<FrameFileStorage>` directly,
bypassing the hexagonal port abstraction that all other storage operations follow.

While this is acceptable per ADR-014 (composition root may reference concrete types
for wiring), the widespread use of `FrameFileStorage` across scheduler loops, capture
services, automation runtime, and agent support creates tight coupling that hinders:

1. **Testability** — Unit tests cannot mock frame storage without the full filesystem
2. **Replaceability** — Switching to in-memory or cloud storage requires 10+ file changes
3. **Dependency clarity** — Consumers declare dependency on an implementation rather than a capability

## Decision

Introduce a `FrameStoragePort` trait in `oneshim-core::ports` that abstracts the
frame storage operations actually used by consumers:

```rust
#[async_trait]
pub trait FrameStoragePort: Send + Sync {
    async fn save_frame(&self, timestamp: DateTime<Utc>, data: Vec<u8>)
        -> Result<PathBuf, CoreError>;
    async fn save_frames_batch(&self, frames: Vec<(DateTime<Utc>, Vec<u8>)>)
        -> Result<Vec<PathBuf>, CoreError>;
    async fn enforce_retention(&self) -> Result<usize, CoreError>;
    async fn enforce_storage_limit(&self) -> Result<usize, CoreError>;
}
```

`FrameFileStorage` in `oneshim-storage` implements this trait.

Consumers in `src-tauri` receive `Arc<dyn FrameStoragePort>` instead of
`Arc<FrameFileStorage>`.

## Rationale

- **ADR-001 §2 alignment**: Port traits use `#[async_trait]` with `&self` receivers
- **ADR-001 §3 alignment**: DI via `Arc<dyn T>` constructor injection
- **Minimal surface**: Only 4 methods that are actually consumed; diagnostic methods
  (`frames_dir`, `buffer_pool_stats`, `disk_status`) remain on the concrete type
  for composition-root-only access
- **SOLID compliance**: Consumers depend on the capability they need, not the implementation

## Consequences

### Positive
- Frame storage consumers become testable with mock implementations
- Future storage backends (in-memory, cloud) require zero consumer changes
- Dependency graph is clearer — `oneshim-storage` is only referenced in wiring code

### Negative
- Small runtime overhead from dynamic dispatch (negligible for I/O-bound operations)
- Composition root still needs `Arc<FrameFileStorage>` for diagnostic methods

### Migration
- `CaptureContext.frame_storage` changes from `Option<Arc<FrameFileStorage>>`
  to `Option<Arc<dyn FrameStoragePort>>`
- Scheduler, automation runtime, agent support follow the same pattern
- Wiring code (`capture_services.rs`, `agent_runtime_support.rs`) creates
  `Arc<FrameFileStorage>` and passes it as `Arc<dyn FrameStoragePort>`
