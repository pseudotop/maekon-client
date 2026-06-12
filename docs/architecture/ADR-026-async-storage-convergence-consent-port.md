[English](./ADR-026-async-storage-convergence-consent-port.md) | [한국어](./ADR-026-async-storage-convergence-consent-port.ko.md)

# ADR-026: Async Storage Convergence + Object-Safe ConsentManagerPort

**Status**: Accepted
**Date**: 2026-06-04
**Scope**: `crates/maekon-core/src/ports/focus_storage.rs`, `crates/maekon-core/src/ports/web_storage.rs`, `crates/maekon-core/src/ports/annotation_storage.rs`, `crates/maekon-core/src/consent.rs`, `crates/maekon-storage/src/sqlite/`, `crates/maekon-web/`, `src-tauri/src/focus_analyzer/`
**Related**: ADR-001 (§2 async-trait, §6 dependency direction, §7 port placement), ADR-007 (async runtime safety), ADR-021 (config/consent core placement), ADR-024 (conversation content guard port)
**Implementation**: complete — see the "Implementation complete" note below.

> **Issue**: E20-19 / #4811 (XL). Deferred behind #4928 (consent-erasure drain barrier), which has since **merged and closed** — the dependency is resolved, so this ADR can be authored.

---

## Implementation complete

The sliced migration of §Decision 2 has landed across **PR-1..PR-9**, converting the entire sync storage surface (FocusStorage + the 14 `WebStorage` sub-traits + AnnotationStorage) to `#[async_trait]` and routing every method through the `with_conn`/`with_conn_mut`/`with_conn_read` funnel (`spawn_blocking`), so the single-connection `parking_lot` guard is never held across an `.await` (#4928 erase barrier preserved):

- **PR-1** — object-safe `ConsentManagerPort` (§1) + `impl for ConsentManager` (additive; no call-site churn).
- **PR-2** — `FocusStorage` → async; `.await` at the 20 `focus_analyzer` call sites.
- **PR-3** — `AnnotationStorage` → async (web smallest sub-trait; proved the per-sub-trait recipe).
- **PR-4/5/6** — `FrameQueryStorage`/`EventQueryStorage`/`StorageMaintenanceStorage` + `TagStorage`/`ActivityStatsStorage`/`FocusQueryStorage` + `SuggestionQueryStorage`/`DigestStorage` → async; lone `suggestion_digest_storage.rs` `block_in_place` removed.
- **PR-7** — `BackupStorage`/`SegmentQueryStorage`/`GuiInteractionStorage` → async.
- **PR-8** — `CoachingQueryStorage`/`HabitStorage` → async.
- **PR-9** — `DashboardStreamingStorage` → async; the `WebStorage` supertrait + blanket impl are now **uniformly async** (ADR-001 §2 satisfied for the whole storage surface).
- **PR-final** — this documentation sweep + a `maekon-storage` current-thread `#[tokio::test]` guarding the `DigestStorage` async body against a future `block_in_place` reintroduction (it has no `maekon-web` entrypoint, so no handler test covers it).

**Follow-up #1 (still open)**: the **41-consumer migration** of the concrete `Arc<ConsentManager>` sites to `Arc<dyn ConsentManagerPort>` remains out of scope here — PR-1 only introduced the port. It is tracked as Known Follow-up #1 below.

---

## Context

Two intertwined design problems block the storage layer from satisfying ADR-001 §2 ("all port traits are async") and ADR-001 §7 ("cross-crate contracts are ports, not concrete types"). They are intertwined because the consent authority (`ConsentManager`) is the GDPR-critical sibling of the storage layer: both feed the #4928 erasure barrier, so converting storage to async without also reasoning about consent risks regressing that barrier.

### Problem 1 — synchronous storage ports

`StorageService` and `MetricsStorage` (`crates/maekon-core/src/ports/storage.rs`) are **already `#[async_trait]`**. But three storage port families remain **synchronous** (plain `fn`, no `#[async_trait]`):

| Port family | File | Methods | Async? |
|-------------|------|--------:|--------|
| `FocusStorage` | `crates/maekon-core/src/ports/focus_storage.rs` | 12 | sync |
| `WebStorage` sub-traits (14 traits) | `crates/maekon-core/src/ports/web_storage.rs` | 65 | sync |
| `AnnotationStorage` | `crates/maekon-core/src/ports/annotation_storage.rs` | 3 | sync |
| **Total sync surface** | | **80** | |

`WebStorage` is a **composed supertrait**: it inherits the *already-async* `StorageService` + `MetricsStorage` **and** the 14 sync sub-traits (`TagStorage`, `FrameQueryStorage`, `EventQueryStorage`, `StorageMaintenanceStorage`, `ActivityStatsStorage`, `FocusQueryStorage`, `SuggestionQueryStorage`, `DigestStorage`, `BackupStorage`, `GuiInteractionStorage`, `SegmentQueryStorage`, `CoachingQueryStorage`, `HabitStorage`, `DashboardStreamingStorage`) plus `AnnotationStorage`. So `WebStorage` is today a **mixed sync/async composition** — a self-inconsistent surface that ADR-001 §2 forbids.

**Why this is a runtime defect, not cosmetic.** The single SQLite `Connection` is guarded by a `parking_lot::Mutex` inside `GuardedConnection` (`crates/maekon-storage/src/sqlite/guarded_connection.rs`). The async impls (`StorageService`/`MetricsStorage`) route through `SqliteStorage::with_conn` / `with_conn_read`, which offload to `tokio::task::spawn_blocking` so the parking_lot guard is acquired **on a blocking-pool thread** and never held across `.await` (the #4928 design). The sync impls do **not**: e.g. `SqliteStorage::increment_focus_metrics` (`crates/maekon-storage/src/sqlite/edge_intelligence/focus_metrics.rs:146`) calls `self.conn.write_lock().run(...)` directly, holding the lock **on whatever thread called it**. When that caller is an async context, the SQLite I/O blocks a tokio worker thread.

Two caller shapes hit this:

1. **`focus_analyzer` (src-tauri)** — `FocusAnalyzer` holds `Arc<dyn FocusStorage>` and calls it from `async fn` (`on_app_switch`, `analyze_periodic`, `on_idle_resume`). **20 call sites** (11 in `src-tauri/src/focus_analyzer/mod.rs`, 9 in `src-tauri/src/focus_analyzer/suggestions.rs`) invoke sync `FocusStorage` methods directly inside async fns — each one blocks the runtime thread for the duration of the SQLite write.
2. **`maekon-web` handlers** — Axum handlers consume `Arc<dyn WebStorage>` (`crates/maekon-web/src/app_state.rs:47`, threaded through `web_contexts`, `grpc/*`, `services/*`). The handler surface holds **~56 `storage.<method>(...)` call expressions** across `crates/maekon-web/src`. The web crate is **mid-migration**: newer handlers already wrap the sync call in `tokio::task::spawn_blocking` (e.g. `handlers/annotations.rs`, `handlers/coaching.rs`, `services/data_web_service.rs`, `services/search_service.rs`), while a residual minority still call sync methods directly or via the legacy `block_in_place` bridge. There is exactly **1** storage-related `block_in_place` in the storage crate itself (`crates/maekon-storage/src/sqlite/web_storage_impl/suggestion_digest_storage.rs`), plus a separate, **unrelated** `block_in_place` family in `CoachingPort` (`crates/maekon-core/src/ports/coaching.rs`, `crates/maekon-analysis/src/coaching_engine/port_impl.rs`) that bridges a `tokio::sync::RwLock`, **not** SQLite — those already have non-blocking async variants (F-RR-C37-01) and are **out of scope** for this ADR.

> **Honest blast-radius re-measurement.** The issue/earlier scope estimated 39–64 sites. The corrected figures: **80 sync trait methods** to gain `async`, **20 `FocusStorage` call sites** (all in `focus_analyzer`), **~56 `WebStorage` call-site expressions** in `maekon-web/src`, **80 sync impl methods** in `maekon-storage` (`focus_storage_impl.rs` + `web_storage_impl/*` + `annotation_storage_impl.rs`), and **1** in-scope storage `block_in_place`. The "39–64" range under-counted the web sub-trait method total (80 > 64) and over-weighted `block_in_place` (most web callers already moved to `spawn_blocking`). The dominant cost is **method-signature churn** (80 sigs × N impls), not call-site `.await` insertion.

### Problem 2 — `ConsentManager` is a concrete type, not a port

`ConsentManager` (`crates/maekon-core/src/consent.rs`, ~1183 lines) is consumed as a **concrete type** across **41** `Arc<ConsentManager>` / `&ConsentManager` sites in `src-tauri` and adapter crates (scheduler loops, sync engine, vision privacy gateway, provider guards, web). ADR-001 §7 wants cross-crate contracts behind a port. ADR-021 deliberately keeps `ConsentManager` **in `maekon-core`** (a boundary exception: local product-policy state, not infrastructure) — so this ADR does **not** move it; it adds a port *alongside* it, in core, satisfying ADR-001 §7's "consumed by >1 crate ⇒ core" rule.

**The blocker.** A prior spec proposed `ConsentManagerPort` but its central method was:

```rust
fn is_permitted(&self, check: impl Fn(&ConsentPermissions) -> bool) -> bool;
```

This is **not object-safe** (Rust 2024: "not dyn compatible"): a generic type parameter (`impl Fn`) on a trait method means the method cannot appear in a vtable, so `Arc<dyn ConsentManagerPort>` will not compile. Empirically confirmed for this ADR — adding that exact signature to a candidate trait yields:

```
error[E0038]: the trait `ConsentManagerPort` is not dyn compatible
note: for a trait to be dyn compatible it needs to allow building a vtable
      ...method `is_permitted` has generic type parameters
```

Without `Arc<dyn ConsentManagerPort>` the port is useless for the DI pattern (ADR-001 §3).

**Decisive measurement.** `is_permitted` is **production-public but called only from tests**: all 6 call sites live in the `#[cfg(test)] mod tests` block of `consent.rs` (after line 414). Production consent gating already goes through `effective_permissions()` (fail-closed validity-checked) and `status_and_permissions()` — never `is_permitted`. So `is_permitted` does not need to be on the port at all.

### Why now

#4928 (consent-erasure drain barrier + `GuardedConnection` chokepoint + `deletion_flag`/`erasing` signals) is **merged and closed**. That removed the original deferral reason: the erasure barrier is now a stable, single chokepoint (`SqliteStorage::with_conn`/`write_lock`, `ConsentManager::deletion_flag()`/`erasing()`), so an async conversion can be designed *on top of* a known-good barrier rather than racing against an in-flight one.

## Decision

### 1. Object-safe `ConsentManagerPort` in `maekon-core`

Introduce `crates/maekon-core/src/ports/consent_manager.rs` with an **object-safe** (`dyn`-compatible) port. `ConsentManager` (which stays in core per ADR-021) implements it. The trait is **synchronous** — `ConsentManager` is pure in-memory `parking_lot::RwLock` state + local JSON file I/O (no `.await` anywhere today), and ADR-021 forbids it from growing async external side effects. ADR-001 §2's `#[async_trait]` rule targets I/O-bound ports; a consent **policy** authority with no async surface is correctly a sync port (consistent with the existing sync policy accessors).

```rust
/// Object-safe (dyn-compatible) consent authority port.
/// Implemented by `ConsentManager` (kept in maekon-core per ADR-021).
pub trait ConsentManagerPort: Send + Sync {
    fn check_consent(&self) -> ConsentStatus;
    fn current_consent(&self) -> Option<ConsentRecord>;
    /// Fail-closed: returns permissions ONLY when consent is currently Valid,
    /// else `ConsentPermissions::default()` (all false). This is the canonical
    /// gating accessor — use it instead of the removed generic `is_permitted`.
    fn effective_permissions(&self) -> ConsentPermissions;
    /// Atomic (status, raw-permissions) snapshot for UI (NOT fail-closed-gated).
    fn status_and_permissions(&self) -> (ConsentStatus, ConsentPermissions);
    fn grant_consent(
        &self,
        permissions: ConsentPermissions,
        data_retention_days: u32,
    ) -> Result<(), CoreError>;
    fn revoke_consent(&self) -> Result<(), CoreError>;
    fn has_pending_deletion(&self) -> bool;
    fn clear_pending_deletion(&self);
    /// #4928 erasure-barrier signals (shared `Arc` installed into storage adapters).
    fn deletion_flag(&self) -> Arc<AtomicBool>;
    fn erasing(&self) -> Arc<AtomicBool>;

    /// Convenience: non-generic, object-safe replacements for the most common
    /// `is_permitted(|p| p.<field>)` test idioms. Default-implemented on top of
    /// `effective_permissions()` so impls get them for free, and they remain in
    /// the vtable (no generic params).
    fn telemetry_permitted(&self) -> bool {
        self.effective_permissions().telemetry
    }
    fn screen_capture_permitted(&self) -> bool {
        self.effective_permissions().screen_capture
    }
}
```

**The `is_permitted` fix (and why the original wasn't object-safe).** The original `fn is_permitted(&self, check: impl Fn(&ConsentPermissions) -> bool) -> bool` carries a generic type parameter (`impl Fn` desugars to `<F: Fn(...)>`). A `dyn Trait` vtable has one entry per method with a *fixed* signature; a generic method would need a distinct vtable entry per instantiating closure type, which is impossible, so the compiler rejects the trait as not dyn-compatible (`E0038`). **Fix: drop `is_permitted` from the trait entirely.** It is production-public-but-test-only, so:

- Keep `ConsentManager::is_permitted` as an **inherent method** on the concrete type (unchanged — the 6 in-crate tests keep calling it directly on `ConsentManager`, not through `dyn`). No production code and no port consumer ever needed it.
- For any future caller that wants "is permission X currently granted", they call the **object-safe** `effective_permissions()` and read the field, or a non-generic `*_permitted()` default helper. The caller inspects a `ConsentPermissions` snapshot (it is `Clone`), which is strictly more flexible than a single-predicate closure and is fully `dyn`-compatible.

**Verification.** A scratch trait file with this exact surface was wired into `crates/maekon-core/src/ports/mod.rs`, implemented for `ConsentManager`, and `cargo check -p maekon-core` confirmed (a) the trait + the `Arc<ConsentManager> -> Arc<dyn ConsentManagerPort>` coercion compile, and (b) re-adding the generic `is_permitted` produces `E0038 ... is not dyn compatible`. The scratch file was then removed so this ADR commit is **doc-only**.

**Rationale**: object-safety is mandatory for `Arc<dyn _>` DI (ADR-001 §3). A `ConsentPermissions` snapshot accessor is the minimal, non-generic, future-proof shape; the closure form bought nothing over it except a marginally terser test idiom, which the inherent method preserves.

### 2. Async storage convergence — sliced migration

Convert the 80 sync methods to `#[async_trait]` `async fn`, deleting the blocking call shapes, in **independently shippable slices**. Each slice keeps a single `cargo check -p <crate>` green and is reviewable in isolation. Ordering minimizes lock-ordering risk by converting **leaf consumers before shared supertraits**, and by leaving `WebStorage` (the composed supertrait) last so its blanket impl flips only once all sub-traits are async.

| PR | Scope | Verifiable headless? | Notes |
|----|-------|----------------------|-------|
| **PR-1** | Add `ConsentManagerPort` (§1) + `impl for ConsentManager`. Additive only — **zero** call-site churn (existing 41 concrete consumers untouched). | ✅ `cargo check -p maekon-core` + new unit test asserting `Arc<dyn ConsentManagerPort>` coercion | Lands the object-safe port first; decouples Problem 2 from the storage churn. |
| **PR-2** | Convert `FocusStorage` (12 methods) → `#[async_trait]`. Update `focus_storage_impl.rs` to `async fn` delegating to async `with_conn`/`with_conn_read`. Add `.await` at the **20** `focus_analyzer` call sites (already in `async fn`). | ✅ `cargo check -p maekon-storage -p maekon-app` + existing `focus_analyzer` `#[tokio::test]`s | Self-contained: `FocusStorage`'s only consumer is `focus_analyzer`. No `block_in_place` to remove here (direct sync calls). |
| **PR-3** | Convert `AnnotationStorage` (3 methods) → async. Update `annotation_storage_impl.rs` + `handlers/annotations.rs` (already `spawn_blocking` — replace with direct `.await`). | ✅ `cargo check -p maekon-web` + annotation handler tests | Smallest web sub-trait; proves the per-sub-trait recipe before scaling. |
| **PR-4..N** | One PR **per remaining `WebStorage` sub-trait** (14 traits, ~65 methods total — e.g. PR-4 `TagStorage`, PR-5 `FrameQueryStorage`, … grouped into ~6–8 PRs of cohesive sub-traits to keep each ≤ ~10 methods). Each: flip the sub-trait to `#[async_trait]`, convert its `web_storage_impl/*` block to `async fn` over `with_conn`/`with_conn_read`, replace the matching handler `spawn_blocking` wrappers with direct `.await`, and remove the lone `suggestion_digest_storage.rs` `block_in_place`. | ✅ per-PR `cargo check -p maekon-storage -p maekon-web` + sub-trait contract test (ADR-001 §8) | The blanket `impl<T> WebStorage for T` stays valid throughout because the supertrait bound list is unchanged; only each sub-trait's method bodies become async. Because `StorageService`/`MetricsStorage` are *already* async, no mixed-ness is introduced mid-flight. |
| **PR-final** | Documentation sweep: mark `WebStorage` fully-async in its doc comment; flip ADR-026 Status → Accepted; update ADR-001 §2 "Scope" note (FocusStorage/WebStorage now async). | ✅ `cargo doc` / `cargo check` | No behavior change. |

**Ordering rationale (lock-ordering safety).** Today the sync impls take `write_lock()`/`read_lock()` **synchronously on the caller thread**; after conversion they take it **inside `spawn_blocking`** via the existing `with_conn*` funnel (same primitive, different thread). Because every converted method routes through the **single** `GuardedConnection` parking_lot mutex (there is exactly one lock; no second lock is introduced), there is **no new lock-ordering edge** — the conversion cannot create a deadlock cycle that did not already exist. Converting leaf consumers first (FocusStorage → AnnotationStorage → other sub-traits → WebStorage doc) means each PR's blast radius is one impl block + its direct callers.

**Headless vs runtime.** Compile-correctness, contract tests (ADR-001 §8), and the existing `#[tokio::test]` handler/analyzer suites are all **verifiable headless** (`cargo check`/`cargo test`). What is **NOT** verifiable headless: whether the conversion actually *relieves* tokio-worker starvation under load — that is a scheduler-contention / `spawn_blocking`-pool-saturation property observable only on a running client under realistic capture+web load. This ADR explicitly does not claim a measured throughput win; the justification is correctness (ADR-001 §2 + no-blocking-the-runtime per ADR-007), not a benchmark.

## Consequences

### Positive

- `WebStorage` becomes a uniformly-async port; ADR-001 §2 is satisfied for the entire storage surface.
- The runtime-thread-blocking defect in `focus_analyzer` (20 sync SQLite calls inside `async fn`) is removed — SQLite work moves onto the `spawn_blocking` pool (ADR-007 alignment).
- `ConsentManagerPort` lets the 41 concrete `ConsentManager` consumers migrate to `Arc<dyn ConsentManagerPort>` over time, and makes consent **mockable** in adapter tests without a real file-backed manager.
- The migration is shippable in ~10 small PRs, each green and independently revertible — no big-bang storage rewrite.

### Negative

- 80 method signatures + their impls churn (`async fn` + `.await`); a large mechanical diff spread over many PRs.
- `spawn_blocking` adds a thread hop + (under saturation) potential pool back-pressure; **not** verifiable headless — must be watched in staging.
- A residual `is_permitted` inherent method stays on `ConsentManager` for tests only; a future reader must understand it is intentionally *not* on the port.
- Two consent access styles coexist during PR-1→migration: concrete `Arc<ConsentManager>` and `Arc<dyn ConsentManagerPort>`. Intentional (additive rollout) but a transient divergence.

### Neutral

- `ConsentManager`/`ConsentPermissions`/`ConsentRecord` stay in `maekon-core` (ADR-021 unchanged). The port is additive, not a move.
- `CoachingPort`'s `block_in_place` bridge is untouched (separate `tokio::sync::RwLock` concern, already has async variants).

### Risks & mitigations (GDPR-critical)

- **#4928 erasure-barrier sensitivity.** `consent.rs` is GDPR-critical: the `deletion_flag`/`erasing` signals and the `GuardedConnection` write-skip predicate (`deletion_flag || erasing`) are the right-to-erasure backstop. The async conversion **must not** alter the predicate or the `with_conn`/`write_lock` chokepoint — it only changes *which thread* holds the parking_lot guard (already `spawn_blocking` for the async impls). **Mitigation**: each storage-conversion PR must keep routing every write through `with_conn`/`with_conn_mut` (never a bare `write_lock` on the caller thread), and the existing #4928 erase-barrier tests (`commands/consent.rs` erase tests + ptr-eq `deletion_flag`/`erasing` install tests) must stay green — they are the regression gate.
- **Lock-ordering audit.** Single `GuardedConnection` mutex ⇒ no new ordering edge (see §2 rationale). Still, each PR must confirm no method holds the parking_lot guard across an `.await` (it cannot, since the guard lives inside the `spawn_blocking` closure) — this is the exact B2 invariant #4928 established.
- **Test strategy.** (a) ADR-001 §8 contract tests per converted sub-trait; (b) keep all `focus_analyzer` + web handler `#[tokio::test]`s green; (c) PR-1 adds a `dyn ConsentManagerPort` coercion + fail-closed `effective_permissions()` test; (d) #4928 erase-barrier tests are the GDPR regression gate on every storage PR.
- **Rollback.** Per-slice: revert the single PR (each is independent). `ConsentManagerPort` (PR-1) is purely additive, so reverting it cannot break existing concrete consumers.
- **Honest limit.** Scheduler-contention / `spawn_blocking`-pool-saturation behavior is **not** verifiable headless and is therefore an accepted post-merge staging-observation item, not a pre-merge gate.

## Alternatives Considered

**A. Keep `is_permitted(&self, check: impl Fn(...))` on the port.** Rejected: not object-safe (`E0038`, empirically confirmed) ⇒ `Arc<dyn ConsentManagerPort>` will not compile ⇒ unusable for DI (ADR-001 §3). The whole point of the port is the `dyn` boundary.

**B. Make `is_permitted` object-safe via boxed closure: `fn is_permitted(&self, check: Box<dyn Fn(&ConsentPermissions) -> bool>) -> bool`.** Object-safe, but rejected: it forces every caller to box a closure and ships a heap allocation per gate check for **zero** production callers (all 6 are tests). A `ConsentPermissions` snapshot accessor is simpler, allocation-light, and strictly more expressive.

**C. Big-bang single PR converting all 80 methods.** Rejected: an XL diff touching 3 ports + ~80 impls + ~76 call sites in one PR is unreviewable and un-bisectable, and a single mistake near the GDPR erase barrier would be hard to isolate. The sliced plan keeps each diff small and the erase-barrier tests green at every step.

**D. Make `ConsentManagerPort` async (`#[async_trait]`) for ADR-001 §2 uniformity.** Rejected: `ConsentManager` has no async surface (in-memory `parking_lot::RwLock` + sync local file I/O), ADR-021 forbids it growing async external side effects, and async would force pointless `.await` on 41 synchronous gate checks. ADR-001 §2's intent is I/O ports; a pure policy authority is correctly sync.

**E. Move `ConsentManager` into an adapter crate behind the port.** Rejected: directly contradicts ADR-021 (consent state is core product policy, deliberately kept in `maekon-core`). This ADR adds a core-local port instead.

## Known Follow-ups

1. **Migrate the 41 concrete `Arc<ConsentManager>` consumers to `Arc<dyn ConsentManagerPort>`** — out of scope here (PR-1 only introduces the port); track as a separate sweep once the port lands.
2. **`spawn_blocking` pool sizing** — if staging shows pool saturation after convergence, evaluate a dedicated blocking pool or the read-only second-connection path already sketched in `SqliteStorage` docs.
3. **Sub-trait grouping for PR-4..N** — finalize which sub-traits pair into each PR (target ≤ ~10 methods/PR) during implementation; the table above is indicative, not binding.

## Related Docs

- `docs/architecture/ADR-001-rust-client-architecture-patterns.md` — §2 async-trait, §3 DI, §6 dependency direction, §7 port placement, §8 contract tests
- `docs/architecture/ADR-007-async-runtime-safety-patterns.md` — not blocking the async runtime
- `docs/architecture/ADR-021-config-consent-core-placement.md` — consent stays in core (boundary exception)
- `docs/architecture/ADR-024-conversation-content-guard-port.md` — recent port-introduction precedent
