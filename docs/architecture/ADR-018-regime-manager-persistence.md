[English](./ADR-018-regime-manager-persistence.md) | [한국어](./ADR-018-regime-manager-persistence.ko.md)

# ADR-018: RegimeManager Persistence

**Status**: Accepted
**Date**: 2026-04-18
**Scope**: `oneshim-core::ports::regime_storage`, `oneshim-storage::regime_manager_state_store`, `oneshim-analysis::RegimeManager::hydrate_from`, `src-tauri::main::RunEvent::Exit`

---

## Context

`RegimeManager` was purely in-memory — every restart lost user-curated regime names, merges, deletes. The existing `regimes` SQL table is touched only by the cross-device sync path (`sync_merger.rs`); it does NOT carry RegimeManager's full state (centroid, RegimeStatus enum, name override).

See the 2026-04-16 gap analysis X6.

## Decision

A new `RegimeStoragePort` in `oneshim-core` and `SqliteRegimeManagerStateStore` in `oneshim-storage`. State is a JSON blob in a new **dedicated** `regime_manager_state` singleton table (v31 migration), not the existing `regimes` table.

On startup the composition root calls `store.load_all()` → `RegimeManager::hydrate_from(regimes)`. On graceful shutdown the `RunEvent::Exit` handler in `main.rs` calls `store.save_all(&regime_manager.all_regimes())` with a 4 s watchdog.

On parse failure, `load_all` quarantines the corrupt payload to `payload_backup` with `payload_backup_at` timestamp, logs `error!`, and returns `Ok(vec![])` so the app starts fresh. User-curated state is preserved for later recovery.

## Consequences

### Positive

- Regimes survive restart; the "new regime discovered" notification stops firing for the same cluster on every cold boot.
- Vector `regime_id` filter (C3a) becomes meaningful across sessions — regime IDs are now stable.
- sync_merger's use of the existing `regimes` table is untouched.

### Negative / Constraints

- JSON blob evolves with `Regime` struct. The current struct carries NO `#[serde(default)]` attributes, so *any* schema evolution — additive, removed, or renamed — triggers the quarantine path. Adding `#[serde(default)]` to future additive fields is a deliberate per-field decision; do NOT add it blanket because silent default-substitution across versions hides real migration intent. Schema mismatches are never silent wipes — the quarantine preserves the old payload.
- `load_all` is not read-only in the quarantine edge case. Doc warns callers; all call sites are single-shot at startup.
- Shutdown save is best-effort. The watchdog is two-layered and each layer has limits:
  1. `tokio::time::timeout(4s)` wraps the save future. But `SqliteRegimeManagerStateStore::save_all` takes the `std::sync::Mutex<Connection>` and calls `rusqlite::Connection::execute` — both blocking sync, with no `.await` once inside. tokio's timeout polls at await boundaries; it cannot preempt the in-flight SQL. The timeout only fires if the save yields before the mutex lock (e.g., waiting for the runtime thread) or if the inner channel machinery awaits.
  2. `std::sync::mpsc::recv_timeout(4.5s)` on the main thread. This *does* fire at 4.5s and lets shutdown proceed. A genuinely stalled save thread will outlive this wait; the OS reaps it when the process exits.
  In practice the SQL is a small JSON blob + `INSERT OR REPLACE` and completes in <50 ms on a healthy disk. SQLite's journal guarantees there is no torn-write risk: `execute` either commits (data durable in WAL) or does not (journal rolls back on next open).
- **Signal-driven shutdown bypasses the save entirely.** `lifecycle.rs::wait_second_signal` calls `std::process::exit(0)` after `FORCE_EXIT_GRACE_SECS`, running before Tauri's `RunEvent::Exit` closure. `kill -TERM <pid>`, `launchctl unload`, or any non-tray-quit termination therefore skips both the regime save and the suggestion-queue save. This is pre-existing behavior (same constraint on suggestion-queue save) not introduced by this ADR; it is called out here because a strict reading of "graceful shutdown" would obscure it. Mid-life periodic save (Neutral, below) is the follow-up remedy.
- **Shutdown ordering note.** `RunEvent::Exit` runs the WAL checkpoint BEFORE the regime save. If the order were reversed, a stalled save holding the connection mutex would block the checkpoint on the same `Arc<Mutex<Connection>>`, leaving the WAL un-truncated. Running the checkpoint first gives it an unblocked window; the save that follows writes into a fresh WAL, idempotently replayed on next startup if the process dies mid-write.

### Neutral

- Mid-life periodic save is OUT OF SCOPE for this phase. Shutdown-only is sufficient for routine restart survival; a follow-up phase can add periodic save after `run_maintenance` ticks if cold-kill data loss becomes a concern.

## Alternatives considered

- **Reuse the existing `regimes` table** — rejected. Its schema is partial (no centroid, no RegimeStatus enum, no user-name override) and it is owned by sync_merger. Extending it would require migration + write-path update to keep sync consistent. New dedicated table avoids that blast radius.
- **Per-regime rows instead of JSON blob** — rejected. RegimeManager's regime count is bounded (`max_active + archive_days`), so a single blob is simpler and negligible cost. Diff-API is a backward-compatible follow-up if it ever matters.
- **"Start fresh on parse failure"** — explicitly rejected during spec review. Wiping months of user-curated names silently is a regression. Quarantine preserves recovery path.

## References

- Implementation record: internal regime feedback learning spec and feature-gap analysis notes
- ADR-016 ConfigChangeBus (shutdown-watchdog pattern)
