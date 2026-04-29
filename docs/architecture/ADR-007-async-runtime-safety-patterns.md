# ADR-007: Async Runtime Safety Patterns

**Status**: Accepted (promoted from Proposed 2026-04-20; three decisions — `spawn_blocking` boundary, subprocess execution, lock-poisoning handling — are all implemented across the workspace; referenced from `src-tauri/src/feedback_sink/mod.rs:40`)
**Date**: 2026-03-09
**Scope**: All crates using tokio async runtime

> **Note on CoreError syntax in example snippets**: Examples below use the
> pre-[ADR-019](./ADR-019-error-code-infrastructure.md) tuple-variant syntax
> `CoreError::Internal(String)`. After ADR-019 these must be written as
> struct variants with a typed `code` field:
> ```rust
> CoreError::Internal {
>     code: oneshim_core::error_codes::InternalCode::Generic,
>     message: format!("..."),
> }
> ```
> The patterns (spawn_blocking wrapping, subprocess timeout, lock-poison
> map_err) are unchanged — only the construction call sites need the new
> struct shape. See ADR-019 for the wire-code contract.

---

## Context

The client-rust workspace runs on a tokio multi-threaded runtime. The 1-second scheduler loop (defined in `src-tauri/src/scheduler/`) requires consistent, low-latency task completion across all 9 background loops.

Three recurring problems threaten that latency guarantee:

1. **Blocking I/O inside async tasks** — `rusqlite` in `oneshim-storage`, `xcap` screen capture in `oneshim-vision`, and `std::fs` calls block tokio worker threads for the full duration of the operation. When the pool of worker threads stalls, unrelated async tasks queue behind them.

2. **Synchronous subprocess invocation** — `std::process::Command` blocks the calling thread until the child process exits. `osascript` calls on macOS (via `oneshim-monitor/src/macos.rs`) and `xdotool`/`xprintidle` calls on Linux (via `oneshim-monitor/src/linux.rs`) are currently synchronous. A hung or slow subprocess freezes an entire worker thread.

3. **Panic-on-lock-poison** — `.expect()` on `Mutex::lock()` or `RwLock::read()` propagates a panic through the entire spawned task, which terminates it silently. For a 24/7 desktop agent that must survive subprocess failures and hardware anomalies, silent task death is worse than degraded operation.

### Pivot Evidence

Three commits establish the direct lineage of these issues:

| Commit | Date | Path | Relevance |
|--------|------|------|-----------|
| `1e8c918` | 2026-02-26 | `crates/oneshim-monitor/src/macos.rs`, `crates/oneshim-monitor/src/linux.rs` | Initial codebase introduced `std::process::Command` for all subprocess calls |
| `aa03871` | 2026-02-28 | `crates/oneshim-vision/src/trigger.rs` | Interior-mutability refactor (`&mut self` → `&self`) introduced `Mutex::lock().expect(...)` as the locking pattern |
| `e633ac5` | 2026-03-08 | `crates/oneshim-vision/src/trigger.rs`, `crates/oneshim-monitor/src/input_activity.rs` | Partial unwrap cleanup replaced `unwrap()` with `.expect()` — correct for documented invariants, but `.expect()` still panics on lock poison; the remaining cases need graceful handling |

---

## Decisions

### 1. Blocking I/O Boundary (`spawn_blocking`)

**Rule**: Any operation that may block a thread for more than ~1 ms inside an async context MUST be offloaded to `tokio::task::spawn_blocking`. This applies to:

- All `rusqlite` database methods in `oneshim-storage/src/sqlite/`
- Screen capture via `xcap::Monitor::capture_image()` in `oneshim-vision/src/capture.rs`
- File system operations using `std::fs` (not `tokio::fs`) when called from async functions

**Preferred pattern for SQLite — `with_conn` helper**:

```rust
// 동기 Connection을 소유한 구조체에 이 헬퍼를 추가한다
async fn with_conn<F, T>(&self, f: F) -> Result<T, CoreError>
where
    F: FnOnce(&Connection) -> Result<T, CoreError> + Send + 'static,
    T: Send + 'static,
{
    // Arc<Mutex<Connection>>을 복제하여 클로저로 이동시킨다
    let conn = self.conn.clone();
    tokio::task::spawn_blocking(move || {
        let guard = conn.lock().map_err(|e| {
            CoreError::Internal(format!("SQLite lock poisoned: {e}"))
        })?;
        f(&guard)
    })
    .await
    .map_err(|e| CoreError::Internal(format!("spawn_blocking join error: {e}")))?
}
```

Callers use it as a thin wrapper:

```rust
// 호출 측 — 동기 rusqlite 코드를 클로저 안에 작성한다
let count = self
    .with_conn(|conn| {
        conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .map_err(|e| CoreError::Internal(e.to_string()))
    })
    .await?;
```

**Why not `tokio::sync::Mutex`?** `tokio::sync::Mutex` still blocks the underlying system thread during the actual SQL execution. The `spawn_blocking` boundary moves the blocking work to a dedicated thread pool that tokio sizes separately from its async worker pool, preventing head-of-line blocking.

---

### 2. Subprocess Execution Pattern

**Rule**: Use `tokio::process::Command` instead of `std::process::Command` in all code that runs inside an async context. Every subprocess call MUST have an explicit timeout.

**Affected files**:
- `oneshim-monitor/src/macos.rs` — `osascript`, `ioreg` (currently uses `std::process::Command`)
- `oneshim-monitor/src/linux.rs` — `xdotool`, `xprintidle` (currently uses `std::process::Command`)

**Migration pattern**:

```rust
use tokio::process::Command;
use tokio::time::{timeout, Duration};

// osascript 호출 예시 — 5초 타임아웃 적용
async fn get_active_window_macos() -> Result<Option<WindowInfo>, CoreError> {
    let output = timeout(
        Duration::from_secs(5),
        Command::new("osascript")
            .arg("-e")
            .arg(APPLESCRIPT)
            .output(),
    )
    .await
    .map_err(|_| CoreError::Internal("osascript timed out".into()))?
    .map_err(|e| CoreError::Internal(format!("subprocess failed: {e}")))?;

    if !output.status.success() {
        return Ok(None);
    }
    // ... parse output
}
```

**Default timeout values**:

| Context | Timeout |
|---------|---------|
| Monitor commands (`osascript`, `xdotool`, `ioreg`) | 5 seconds |
| OCR subprocess (Tesseract via `oneshim-vision`) | 30 seconds |
| Any other subprocess | 10 seconds (default) |

Timeouts are not configurable at runtime; they are compile-time constants in each module. If a subprocess consistently times out, the correct fix is to replace it with a native Rust API, not to raise the timeout.

---

### 3. Lock Poisoning Handling

**Rule**: NEVER use `.expect()` or `.unwrap()` on `Mutex::lock()`, `RwLock::read()`, or `RwLock::write()`. Always propagate lock-poison errors as `CoreError::Internal` using `.map_err()`.

**Current violations** (to be migrated incrementally):

| File | Line | Violation |
|------|------|-----------|
| `crates/oneshim-vision/src/trigger.rs` | 88–89 | `.expect("SmartCaptureTrigger state lock was poisoned...")` |
| `crates/oneshim-monitor/src/input_activity.rs` | 114–115 | `.expect("InputActivityCollector period_start lock was poisoned")` |

**Pattern**:

```rust
// ❌ Wrong — panics if a previous task panicked while holding the lock
let guard = self.state.lock().expect("lock poisoned");

// ✅ Correct — degrades gracefully; logs the event and returns an error
let guard = self.state.lock().map_err(|e| {
    tracing::error!(
        target: "oneshim::runtime",
        "mutex lock poisoned — previous task may have panicked: {e}"
    );
    CoreError::Internal(format!("lock poisoned: {e}"))
})?;
```

**When `.expect()` is acceptable**: On values that are `PoisonError`-immune by construction (e.g., `AtomicU32`, `AtomicU64`) or on `Mutex` guards that are only ever acquired in contexts where a panic is impossible (e.g., a `Mutex<Vec<_>>` that is never mutated by fallible code). In those cases, document the invariant in a comment above the `.expect()` call.

**Rationale**: When a tokio task panics while holding a `Mutex`, the lock enters a poisoned state. A subsequent `.lock().expect()` in a different task will panic too, cascading the failure. For a desktop agent that runs 24/7 and monitors system state, the correct behavior is to log the poisoned-lock event, skip the current operation, and continue collecting data on the next tick. The agent must be resilient to partial failures in individual monitoring tasks.

---

## Consequences

### Positive

- Tokio worker threads remain free for async scheduling; blocking work is isolated to the `spawn_blocking` pool.
- A hung subprocess no longer freezes a worker thread beyond the configured timeout.
- A single panicking task can no longer cascade lock-poison failures to sibling tasks.
- The 1-second scheduler latency guarantee is protected for non-blocking tasks even when SQLite or screen capture is slow.

### Negative / Trade-offs

- `spawn_blocking` adds one context-switch overhead per SQLite call. This is acceptable given that SQLite latency already dominates the operation.
- `tokio::process::Command` is not available in pure synchronous contexts. Non-async callers must spawn a small async block or restructure to call from an async boundary. In practice, all affected monitor functions are already called from async scheduler loops.
- `with_conn` requires `Arc<Mutex<Connection>>` rather than a plain `Connection`. Existing `SqliteStorage` implementations must be reviewed and updated.

### Migration Path

New code must follow these patterns from the date this ADR is accepted.

Existing violations are migrated incrementally in the following priority order:

1. **High** — `oneshim-monitor/src/macos.rs` and `oneshim-monitor/src/linux.rs`: subprocess calls affect every monitor loop tick.
2. **Medium** — `oneshim-vision/src/trigger.rs` and `oneshim-monitor/src/input_activity.rs`: lock-poison handling (these are low-contention locks; risk is lower but the pattern must be corrected for consistency).
3. **Low** — `oneshim-storage/src/sqlite/`: already runs inside dedicated scheduler loop tasks; migrate to `with_conn` alongside any future schema changes to avoid pure churn.

### Code Review Checklist

Add the following checks to pull request review for any file under `crates/`:

- [ ] Does the diff introduce `std::process::Command` in an async function? If so, replace with `tokio::process::Command` + `timeout`.
- [ ] Does the diff call `std::fs` functions directly from an async function? If so, use `tokio::fs` or `spawn_blocking`.
- [ ] Does the diff call `.lock()`, `.read()`, or `.write()` on a `std::sync` primitive? Verify the result uses `.map_err(...)`, not `.expect()` or `.unwrap()`.
- [ ] Are all new `spawn_blocking` closures `Send + 'static`? Verify no borrowed references escape into the closure.

---

## Related ADRs

- [ADR-001: Rust Client Architecture Patterns](ADR-001-rust-client-architecture-patterns.md) — error type strategy (`thiserror` / `anyhow`), async trait pattern
- [ADR-002: OS/GUI Interaction Boundary and Runtime Split](ADR-002-os-gui-interaction-boundary.md) — async runtime topology; this ADR refines the blocking-I/O boundary within that topology
