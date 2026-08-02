//! ADR-033 memory vault mirror result models.
//!
//! Returned by `MemoryVaultWriterPort` (§7.4). These carry counts and coarse
//! reasons only — never file contents and never absolute filesystem paths
//! (generated files are identified by their vault-relative names, e.g.
//! `daily/2026-07-29.md`, so no OS-username-bearing path ever leaves the
//! writer).

use serde::{Deserialize, Serialize};

/// Outcome of one mirror cycle (ADR-033 §7.1–§7.3).
///
/// A fail-closed no-op cycle (unevaluable §2 gate or §1.5 bound violation)
/// is `Ok` with `skipped_reason = Some(..)` and every counter zero.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct VaultCycleStats {
    /// Why the cycle was a no-op, when it was (coarse, log-safe reason).
    pub skipped_reason: Option<String>,
    /// Day files written or rewritten this cycle (§7.1).
    pub day_files_written: usize,
    /// Whether `claims.md` was (re)written this cycle (§7.2).
    pub claims_file_written: bool,
    /// Generated files deleted by the expiry sweep (§7.3).
    pub files_expired: usize,
    /// Pattern-matching files skipped because they lack the product marker
    /// (§6.4 collision guard) — surfaced to the settings/status UI.
    pub conflicts: usize,
    /// Vault-relative names of the §6.4 conflicts, capped at
    /// [`VAULT_CONFLICT_PATHS_MAX`] (`conflicts` stays authoritative and may
    /// be larger). Names only — the writer never reads a conflicting file's
    /// content, so nothing here can carry user text, and a vault-relative name
    /// carries no OS username the way an absolute path would. Mutate only
    /// through [`VaultCycleStats::record_conflict`] so the count and the list
    /// cannot drift apart.
    pub conflict_paths: Vec<String>,
    /// Total bytes written this cycle (feeds the §3.4 ledger `byte_count`).
    pub bytes_written: u64,
    /// Whether a `vault_mirror_cloud_sync` ledger record was submitted
    /// this cycle (§3.4 — cloud-flagged custom path with ≥ 1 write).
    pub cloud_ledger_recorded: bool,
}

/// Cap on [`VaultCycleStats::conflict_paths`] and the persisted conflict list.
///
/// The last-cycle summary is one persisted row (§1.4) and a user folder could
/// in principle hold a whole window of pattern-matching notes; the count stays
/// exact while the listed sample stays bounded.
pub const VAULT_CONFLICT_PATHS_MAX: usize = 20;

impl VaultCycleStats {
    /// Record one §6.4 marker conflict: bump `conflicts` and remember the
    /// vault-relative name while the capped sample still has room.
    pub fn record_conflict(&mut self, rel_name: &str) {
        self.conflicts += 1;
        if self.conflict_paths.len() < VAULT_CONFLICT_PATHS_MAX {
            self.conflict_paths.push(rel_name.to_string());
        }
    }
}

/// Summary of the last mirror cycle that actually ran, persisted so the
/// settings surface can report a **scheduled** cycle's §6.4 conflicts (#9522).
///
/// `VaultCycleStats` is per-invocation and unpersisted, so before this only the
/// conflicts of a cycle the user triggered by hand ("Export now") were ever
/// visible — the representative case (a scheduled cycle silently skipping a
/// pre-existing Obsidian daily note) stayed invisible until the user happened
/// to press that button. Persisted in `vault_mirror_state` under a reserved
/// key, so the §4 erasure `ALL_TABLES` pass sweeps it like every sibling row.
///
/// Fail-closed no-op cycles are deliberately NOT recorded: replacing a real
/// conflict report with an empty "feature disabled" record would destroy the
/// very information §6.4 requires the UI to show.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct VaultLastCycleSummary {
    /// Epoch seconds the recorded cycle was anchored at (`now_secs`).
    pub finished_at: i64,
    /// Day files written or rewritten (§7.1).
    pub day_files_written: usize,
    /// Generated files deleted by the expiry sweep (§7.3).
    pub files_expired: usize,
    /// Total §6.4 conflicts — may exceed `conflict_paths.len()`.
    pub conflicts: usize,
    /// Capped vault-relative names of those conflicts (see
    /// [`VaultCycleStats::conflict_paths`] — names only, never content).
    pub conflict_paths: Vec<String>,
}

impl VaultLastCycleSummary {
    /// Project a finished cycle's stats into the persisted summary.
    pub fn from_cycle(stats: &VaultCycleStats, finished_at: i64) -> Self {
        Self {
            finished_at,
            day_files_written: stats.day_files_written,
            files_expired: stats.files_expired,
            conflicts: stats.conflicts,
            conflict_paths: stats.conflict_paths.clone(),
        }
    }
}

/// One file the Art.17 vault erase failed to delete (ADR-033 §4.3).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VaultEraseFailure {
    /// Vault-relative file name (never an absolute path).
    pub file_name: String,
    /// Coarse error description for the orchestrator's outcome report.
    pub message: String,
}

/// Outcome of `erase_generated_files` (ADR-033 §4).
///
/// Orchestrators MUST reflect a non-empty `failures` list in their own
/// erasure outcome — never log-and-continue (§4.3).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct VaultEraseReport {
    /// Marker-bearing generated files successfully deleted.
    pub deleted: usize,
    /// Files that could not be deleted; empty means complete.
    pub failures: Vec<VaultEraseFailure>,
}

impl VaultEraseReport {
    /// True when every targeted generated file was deleted.
    pub fn is_complete(&self) -> bool {
        self.failures.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_conflict_caps_the_list_but_never_the_count() {
        // The count is what the UI reports ("N files skipped"); the list is a
        // bounded sample so the persisted row cannot grow with the user's
        // folder. Truncating the count instead would under-report the skip.
        let mut stats = VaultCycleStats::default();
        for i in 0..(VAULT_CONFLICT_PATHS_MAX + 5) {
            stats.record_conflict(&format!("daily/2026-01-{i:02}.md"));
        }
        assert_eq!(stats.conflicts, VAULT_CONFLICT_PATHS_MAX + 5);
        assert_eq!(stats.conflict_paths.len(), VAULT_CONFLICT_PATHS_MAX);
        assert_eq!(stats.conflict_paths[0], "daily/2026-01-00.md");
    }

    #[test]
    fn summary_carries_the_conflict_names_and_the_cycle_anchor() {
        let mut stats = VaultCycleStats {
            day_files_written: 3,
            files_expired: 1,
            ..VaultCycleStats::default()
        };
        stats.record_conflict("daily/2026-07-29.md");

        let summary = VaultLastCycleSummary::from_cycle(&stats, 1_753_000_000);
        assert_eq!(summary.finished_at, 1_753_000_000);
        assert_eq!(summary.day_files_written, 3);
        assert_eq!(summary.files_expired, 1);
        assert_eq!(summary.conflicts, 1);
        assert_eq!(summary.conflict_paths, vec!["daily/2026-07-29.md"]);
    }
}
