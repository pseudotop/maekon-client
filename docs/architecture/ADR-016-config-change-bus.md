[English](./ADR-016-config-change-bus.md) | [한국어](./ADR-016-config-change-bus.ko.md)

# ADR-016: Config Change Bus

**Status**: Accepted
**Date**: 2026-04-17
**Scope**: `oneshim-core::config_manager::ConfigManager`, all runtime consumers of `AppConfig`

---

## Context

Before this ADR, `ConfigManager` (in `oneshim-core`) held `Arc<RwLock<AppConfig>>` and
exposed only polled reads via `get()`. Every consumer that needed to react to a
user-driven settings change cached its own previous snapshot and re-read on its own
tick. The scheduler loops in `src-tauri/src/scheduler/loops/` each reimplemented the
dirty-check pattern differently; some consumers (`oneshim-vision::privacy`,
`oneshim-analysis::regime_manager`) cached sections at init and never saw later
changes at all. A toggle in the settings UI took 1–30 s to reach each consumer.

The full inventory and the feature-gap analysis that originated this decision are archived as internal implementation records.

This coupling also blocked the telemetry exporter work (X2 in the same gap analysis):
the OTel layer lifecycle has to swap on a runtime `telemetry.enabled` change, and
polling every second from inside `main.rs` was an unattractive plumbing story.

## Decision

`ConfigManager` now owns a `tokio::sync::watch::Sender<Arc<AppConfig>>` inside a
private `Arc<Inner>`, plus a `parking_lot::Mutex<()>` writer-lock that linearises
concurrent mutations. Two new public methods land on `ConfigManager`:

```rust
/// Subscribe to whole-config change notifications.
pub fn subscribe(&self) -> watch::Receiver<Arc<AppConfig>>;

/// Cheap `Arc` read without registering a subscriber.
pub fn snapshot(&self) -> Arc<AppConfig>;
```

Existing callers of `get()` / `update()` / `update_with()` / `reload()` are
**unchanged**. `ConfigManager: Clone` is preserved (clones share the `Arc<Inner>`),
so the 20+ existing call sites across `src-tauri`, `oneshim-web`, and the scheduler
loops continue to work without edits.

## Consequences

### Positive

- **Wake-up on change**: subscribers react within one async tick; the "toggle
  takes 30 seconds to propagate" problem is gone for any consumer that migrates to
  `subscribe()`.
- **No per-consumer polling scaffold**: the pattern is one `select!` arm, documented
  in the `subscribe()` doc comment.
- **Readers never block writers**: `watch::Sender::borrow()` and the writer-lock are
  independent.
- **Additive API**: zero migration cost for existing callers; new consumers can opt
  in.

### Negative — audit-coalescing hazard

`tokio::sync::watch` has **latest-wins** semantics. If rapid `A→B→A` updates occur
between two `changed().await` wake-ups, the subscriber sees only the final value
`A`, not every intermediate transition.

**Consumers whose correctness depends on observing every intermediate transition
must keep their existing poll-and-diff structure, or publish their own per-mutation
signal through a separate channel.** Concretely, `src-tauri/src/scheduler/loops/helpers.rs::audit_consent_and_pii_changes`
emits a compliance audit-log entry on every `PiiFilterLevel` transition. That
callsite is deliberately NOT migrated in Phase 2; a naïve `subscribe-and-diff`
rewrite would silently drop audit events under user-driven rapid toggling.

Every consumer migrated to `subscribe()` in later phases must pass a review question:
*"If A→B→A happens between wake-ups, is it OK that the subscriber never sees B?"*
If the answer is "no", the consumer stays on the tick-based pattern or adopts a
`broadcast` channel instead.

### Neutral

- Subscribers who just need the latest state can call `snapshot()` instead of
  `subscribe()` — no async, no diff, same cheap read.
- `ConfigManager::get()` is now implemented on top of `snapshot()`; its semantics
  and cost are identical to before.

## Alternatives considered

- **`tokio::sync::broadcast`** — rejected. Adds `Lagged` handling and per-subscriber
  queue sizing for no capability gain in a latest-wins world.
- **Per-section watch channels** (one per top-level section of `AppConfig`) —
  rejected. `AppConfig` has 16 top-level sections; API explosion. Diffing in the
  consumer is cheap.
- **`arc_swap::ArcSwap<AppConfig>` + polling** — rejected. Avoids lock contention
  but gives no wake-up signal; consumers would still poll.
- **Panic-in-`Clone`** (force all callers to wrap in `Arc<ConfigManager>`) —
  explicitly rejected during planning. 20+ existing call sites would have broken at
  runtime; the `Arc<Inner>` approach keeps `Clone` cheap and correct.

## References

- Implementation record: internal config telemetry spec, plan, and feature-gap analysis notes
- ADR-001: Rust client architecture patterns (Hexagonal boundary compliance)
- ADR-007: Async runtime safety patterns (`parking_lot::Mutex` is never held across `.await`)
