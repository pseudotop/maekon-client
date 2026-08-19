//! ADR-033 §7.4: the memory vault mirror writer port.
//!
//! The single seam through which the vault is ever written or erased.
//! Implementation lives in `maekon-analysis` and fetches its own inputs via
//! injected core ports (`DigestStorage`, `MemoryGraphPort`,
//! `VaultMirrorStatePort`, `PiiSanitizer`, `EgressLedgerSink`,
//! `ConsentManagerPort`, `ConfigManager`); callers pass nothing but time.
//! `src-tauri` wires it via DI (the ADR-032 placement pattern) and shares
//! ONE instance with the scheduler, the IPC surface, and both Art.17 erase
//! orchestrators.

use crate::error::CoreError;
use crate::models::memory_vault::{VaultCycleStats, VaultEraseReport};

/// One-way, regenerable, bounded vault mirror (ADR-033).
///
/// # Fail-closed contract (ADR-033 §2/§1.5)
/// An unevaluable gate — feature disabled, consent authority unavailable or
/// permission not granted, erase in progress, unresolvable data dir, window
/// bound violation — yields a no-op `Ok` cycle (no writes AND no deletes)
/// with the reason in the stats. Storage failures propagate as `Err`.
#[async_trait::async_trait]
pub trait MemoryVaultWriterPort: Send + Sync {
    /// One full mirror cycle (ADR-033 §7.1–§7.3): day-file fill, claims-file
    /// regen, expiry sweep — all under the §6 marker/containment guards.
    /// `now_secs` is epoch seconds and anchors the mirror window.
    async fn run_mirror_cycle(&self, now_secs: i64) -> Result<VaultCycleStats, CoreError>;

    /// Art.17 pre-wipe step: snapshot every root that may hold generated
    /// files — the default root, an acknowledged custom root, AND the stored
    /// last-active root. MUST be called BEFORE the Phase-1 SQL wipe: the
    /// stored-root row lives in `vault_mirror_state`, which Phase-1 destroys,
    /// so a post-wipe read can never see it (the config-drift #4478 class).
    /// Best-effort on state-read failure (falls back to config-derived roots).
    async fn snapshot_generated_roots(&self) -> Vec<std::path::PathBuf>;

    /// Art.17 Phase-3 (ADR-033 §4): delete every marker-bearing generated
    /// file under each of `roots` (obtained from
    /// [`Self::snapshot_generated_roots`] BEFORE Phase-1). Per-file failures
    /// are reported in the result, never swallowed; callers (both erase
    /// orchestrators) MUST surface an incomplete report in their own outcome.
    /// Runs regardless of the `enabled`/consent gates — erasure must succeed
    /// even after the user revoked everything.
    async fn erase_generated_files(
        &self,
        roots: Vec<std::path::PathBuf>,
    ) -> Result<VaultEraseReport, CoreError>;
}
