// OOS-TBD: ADR-013 file split (tests are ~60% of the LOC; split candidate: tests.rs)
//! ADR-033 memory vault mirror writer (`MemoryVaultWriterPort` impl).
//!
//! One-way, regenerable, bounded Markdown mirror of digests + Active claims.
//! The writer fetches its own inputs via injected core ports and owns every
//! contract obligation locally: fail-closed §2 gates, §1.4/§1.5 bounds,
//! §5.1 `Active`-only claim selection, §5.2 whole-document post-render
//! sanitizing, §6 marker/containment/atomic-write guards, §7 cycle phases,
//! §3.4 cloud egress-ledger record, and the §4 Art.17 erase surface.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{Local, NaiveDate, TimeZone, Utc};
use maekon_core::config::PiiFilterLevel;
use maekon_core::config_manager::ConfigManager;
use maekon_core::error::CoreError;
use maekon_core::models::daily_digest::DigestExporter;
use maekon_core::models::memory_graph::ClaimStatus;
use maekon_core::models::memory_vault::{
    VaultCycleStats, VaultEraseFailure, VaultEraseReport, VaultLastCycleSummary,
};
use maekon_core::models::storage_records::EgressLedgerRecord;
use maekon_core::ports::consent_manager::ConsentManagerPort;
use maekon_core::ports::egress_ledger::EgressLedgerSink;
use maekon_core::ports::memory_graph_port::MemoryGraphPort;
use maekon_core::ports::memory_vault_writer::MemoryVaultWriterPort;
use maekon_core::ports::pii_sanitizer::PiiSanitizer;
use maekon_core::ports::vault_mirror_state::VaultMirrorStatePort;
use maekon_core::ports::web_storage::DigestStorage;
use tracing::{debug, warn};

/// §6.4 product marker: the fixed first line of every generated file. A
/// pattern-matching file that does NOT start with this line is never
/// overwritten and never deleted.
pub const VAULT_MARKER_LINE: &str =
    "<!-- maekon:vault-mirror generated file (ADR-033) — edits are overwritten, do not edit -->";

const CLAIMS_FILE: &str = "claims.md";
const README_FILE: &str = "README.md";
const DAILY_DIR: &str = "daily";
const TMP_SUFFIX: &str = ".maekon-tmp";

/// Reserved `vault_mirror_state` key holding the canonical root the mirror
/// last generated into. NOT a file name (the `::` prefix cannot collide with
/// vault-relative names): it lets a later cycle detect a `custom_path` change
/// and erase the OLD root's generated files (a config-drift erasure gap of
/// the #4478 class — without this, files mirrored to a previous custom path
/// would survive Art.17 forever). Device-local and ALL_TABLES-erased like
/// every other row in the table. Because Phase-1 destroys this row before
/// Phase-3 runs, the erase orchestrators snapshot it PRE-wipe via
/// `snapshot_generated_roots` and pass the roots into
/// `erase_generated_files` — a post-wipe read would be dead code. Residual:
/// only a failed state read at snapshot time (best-effort fallback to
/// config-derived roots, warn-logged).
const ACTIVE_ROOT_KEY: &str = "::active_root";

/// ADR-033 §3.4: the closed set of coarse `destination` labels permitted in
/// the erase-retained, deliberately-no-PII egress ledger. `cloud_provider` is
/// a free-string config field (hand-editable), so the writer allowlists at
/// the point of use — anything else skips the ledger record rather than
/// persisting an arbitrary (possibly path/username-bearing) string forever.
///
/// Re-exported from `maekon_core::vault_cloud_sync` rather than re-listed here:
/// the §3.2 detector that MINTS these labels lives there, and a label it could
/// store that this allowlist would drop is silent unledgered egress.
use maekon_core::vault_cloud_sync::CLOUD_PROVIDER_LABELS;

/// Per-root cycle/erase serialization (process-global). Writer instances are
/// deliberately interchangeable (see `vault_wiring`), so instance-level state
/// cannot serialize them — two concurrent cycles on one root would race the
/// shared tmp path and could pin a hash row that mismatches the file that
/// actually won the rename. Keyed by canonical root; erase takes the same
/// lock so a cycle can never interleave file writes with Art.17 deletion.
static VAULT_ROOT_LOCKS: std::sync::OnceLock<
    std::sync::Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
> = std::sync::OnceLock::new();

fn root_lock(canonical_root: &Path) -> Arc<tokio::sync::Mutex<()>> {
    let map = VAULT_ROOT_LOCKS.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let mut guard = map.lock().expect("vault root lock map poisoned");
    Arc::clone(
        guard
            .entry(canonical_root.to_path_buf())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
    )
}

/// ADR-033 writer. `consent = None` means "consent authority unavailable"
/// (permanent fail-closed no-op, never a bypass); `default_root = None`
/// means the app data dir could not be resolved at composition time (§2.3
/// unevaluable gate). The egress ledger is required — a cloud-flagged cycle
/// that cannot even attempt a ledger record must not exist by construction.
pub struct VaultMirrorWriter {
    digests: Arc<dyn DigestStorage>,
    memory_graph: Arc<dyn MemoryGraphPort>,
    vault_state: Arc<dyn VaultMirrorStatePort>,
    pii_sanitizer: Arc<dyn PiiSanitizer>,
    egress_ledger: Arc<dyn EgressLedgerSink>,
    consent: Option<Arc<dyn ConsentManagerPort>>,
    config_manager: ConfigManager,
    default_root: Option<PathBuf>,
}

impl VaultMirrorWriter {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        digests: Arc<dyn DigestStorage>,
        memory_graph: Arc<dyn MemoryGraphPort>,
        vault_state: Arc<dyn VaultMirrorStatePort>,
        pii_sanitizer: Arc<dyn PiiSanitizer>,
        egress_ledger: Arc<dyn EgressLedgerSink>,
        consent: Option<Arc<dyn ConsentManagerPort>>,
        config_manager: ConfigManager,
        default_root: Option<PathBuf>,
    ) -> Self {
        Self {
            digests,
            memory_graph,
            vault_state,
            pii_sanitizer,
            egress_ledger,
            consent,
            config_manager,
            default_root,
        }
    }

    /// Resolve the active vault root per §3: acknowledged custom path wins,
    /// otherwise the app-owned default. `None` = unresolvable (§2.3 gate).
    fn active_root(&self) -> Option<PathBuf> {
        let cfg = self.config_manager.get();
        let vault = &cfg.analysis.memory_vault;
        match (&vault.custom_path, vault.custom_path_acknowledged) {
            (Some(path), true) => Some(path.clone()),
            // §3.3: an unacknowledged custom path is rejected; the mirror
            // stays on the default location.
            _ => self.default_root.clone(),
        }
    }

    fn no_op(reason: &str) -> VaultCycleStats {
        debug!(reason, "ADR-033 vault mirror: no-op cycle");
        VaultCycleStats {
            skipped_reason: Some(reason.to_string()),
            ..VaultCycleStats::default()
        }
    }
}

/// FNV-1a 64-bit content hash (hex). Change detection only — not security.
fn content_hash(text: &str) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    format!("{hash:016x}")
}

/// §6.2 containment: the target's parent must canonicalize under the
/// canonical vault root (symlink escape refuses the operation).
fn contained(canonical_root: &Path, target: &Path) -> bool {
    match target.parent().and_then(|p| p.canonicalize().ok()) {
        Some(parent) => parent.starts_with(canonical_root),
        None => false,
    }
}

/// §6.4 marker guard: true when the file starts with the product marker.
/// Reads ONLY the header prefix (the §1.2 carve-out — never whole-file
/// read-back, and bounded memory on user-owned folders whose contents we do
/// not control). A read error is treated as "no marker" (never
/// overwrite/delete on doubt).
async fn has_marker(path: &Path) -> bool {
    use tokio::io::AsyncReadExt;
    let Ok(file) = tokio::fs::File::open(path).await else {
        return false;
    };
    let marker = VAULT_MARKER_LINE.as_bytes();
    let mut prefix = Vec::with_capacity(marker.len());
    let mut handle = file.take(marker.len() as u64);
    if handle.read_to_end(&mut prefix).await.is_err() {
        return false;
    }
    prefix == marker
}

/// Parse `YYYY-MM-DD` from a `daily/` file name (`2026-07-29.md`).
fn day_file_date(file_name: &str) -> Option<NaiveDate> {
    let stem = file_name.strip_suffix(".md")?;
    NaiveDate::parse_from_str(stem, "%Y-%m-%d").ok()
}

/// One guarded, atomic write of a generated file. Returns
/// `Ok(Some(bytes))` when written, `Ok(None)` on a §6.4 conflict skip.
async fn write_generated(
    canonical_root: &Path,
    rel_name: &str,
    content: &str,
) -> Result<Option<u64>, CoreError> {
    let target = canonical_root.join(rel_name);
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(io_err)?;
    }
    if !contained(canonical_root, &target) {
        warn!(
            rel_name,
            "ADR-033 §6.2: write target escapes vault root; refused"
        );
        return Ok(None);
    }
    if tokio::fs::try_exists(&target).await.map_err(io_err)? && !has_marker(&target).await {
        // §6.4: pre-existing user file at a generated path — never overwrite.
        return Ok(None);
    }
    let tmp = canonical_root.join(format!("{rel_name}{TMP_SUFFIX}"));
    tokio::fs::write(&tmp, content.as_bytes())
        .await
        .map_err(io_err)?;
    tokio::fs::rename(&tmp, &target).await.map_err(io_err)?;
    Ok(Some(content.len() as u64))
}

fn io_err(e: std::io::Error) -> CoreError {
    CoreError::Storage {
        code: maekon_core::error_codes::StorageCode::Failed,
        message: format!("vault mirror io: {e}"),
    }
}

#[async_trait::async_trait]
impl MemoryVaultWriterPort for VaultMirrorWriter {
    async fn run_mirror_cycle(&self, now_secs: i64) -> Result<VaultCycleStats, CoreError> {
        let cfg = self.config_manager.get();
        let vault_cfg = cfg.analysis.memory_vault.clone();
        let retention_days = cfg.analysis.embedding.retention_days;

        // ── §2 gates (each miss: fail-closed no-op) ─────────────────────
        if !vault_cfg.enabled {
            return Ok(Self::no_op("disabled"));
        }
        let Some(consent) = self.consent.as_ref() else {
            return Ok(Self::no_op("consent_unavailable"));
        };
        if !consent.memory_vault_mirror_permitted() {
            return Ok(Self::no_op("consent_denied"));
        }
        // #4928 round-3 discipline: skip on `deletion_flag || erasing`, the
        // SAME predicate every SQLite/frame writer uses. `grant_consent` can
        // flip `deletion_flag` back mid-erase, but `erasing` is RAII-held for
        // the WHOLE erase span (Phase-1..Phase-3) and only erase touches it —
        // gating on the pair is what §4.5's "no regeneration mid-erase"
        // guarantee actually rests on.
        if consent
            .deletion_flag()
            .load(std::sync::atomic::Ordering::SeqCst)
            || consent.erasing().load(std::sync::atomic::Ordering::SeqCst)
        {
            return Ok(Self::no_op("erase_in_progress"));
        }
        // §1.5: bound violation = complete no-op (no writes AND no deletes).
        if vault_cfg.mirror_window_days == 0 || vault_cfg.mirror_window_days > retention_days {
            return Ok(Self::no_op("window_invalid"));
        }
        let Some(root) = self.active_root() else {
            return Ok(Self::no_op("data_dir_unresolved"));
        };

        // Root setup + §6.2 canonical containment anchor (once per cycle).
        tokio::fs::create_dir_all(root.join(DAILY_DIR))
            .await
            .map_err(io_err)?;
        let canonical_root = root.canonicalize().map_err(io_err)?;

        // Serialize with any concurrent cycle/erase on this root (see
        // VAULT_ROOT_LOCKS — instances are interchangeable, so the lock is
        // process-global and root-keyed).
        let lock = root_lock(&canonical_root);
        let _cycle_guard = lock.lock().await;

        let mut stats = VaultCycleStats::default();
        let stored_hashes = self.vault_state.vault_hashes().await?;

        // B3 (#4478 config-drift class): if the active root CHANGED since the
        // last cycle (custom_path edited/acknowledged/cleared), erase the OLD
        // root's generated files FIRST — otherwise they would sit outside
        // every future erase sweep forever. The reserved row is updated only
        // after the old root is cleaned, so a crash retries the cleanup.
        let current_root_str = canonical_root.to_string_lossy().into_owned();
        let mut old_root_clean = true;
        if let Some(previous_root) = stored_hashes.get(ACTIVE_ROOT_KEY) {
            if *previous_root != current_root_str {
                let prev = PathBuf::from(previous_root);
                let (_deleted, failures) =
                    erase_root_generated(&prev, self.vault_state.as_ref()).await;
                if !failures.is_empty() {
                    // Keep the old row so the NEXT cycle retries the cleanup.
                    old_root_clean = false;
                    warn!(
                        failed = failures.len(),
                        "vault mirror: old-root cleanup incomplete; will retry next cycle"
                    );
                }
            }
        }
        if old_root_clean && stored_hashes.get(ACTIVE_ROOT_KEY) != Some(&current_root_str) {
            self.vault_state
                .upsert_vault_hash(ACTIVE_ROOT_KEY, &current_root_str, now_secs)
                .await?;
        }

        let today = Local
            .timestamp_opt(now_secs, 0)
            .single()
            .map(|t| t.date_naive())
            .unwrap_or_else(|| Local::now().date_naive());
        let window_days = i64::from(vault_cfg.mirror_window_days);
        let oldest_allowed = today - chrono::Duration::days(window_days - 1);

        // ── §7.1 day-file fill ──────────────────────────────────────────
        let digests = self
            .digests
            .list_daily_digests(vault_cfg.mirror_window_days as usize + 2)
            .await?;
        for digest in digests
            .iter()
            .filter(|d| d.date >= oldest_allowed && d.date <= today)
        {
            let rel_name = format!("{DAILY_DIR}/{}.md", digest.date);
            let rendered = DigestExporter::to_markdown(digest);
            // §5.2: whole-document, post-render, Standard-floor sanitize;
            // the marker line is ours and is prepended after.
            let sanitized = self
                .pii_sanitizer
                .sanitize_text(&rendered, PiiFilterLevel::Standard);
            let content = format!("{VAULT_MARKER_LINE}\n\n{sanitized}");
            self.write_if_stale(
                &canonical_root,
                &rel_name,
                &content,
                &stored_hashes,
                now_secs,
                &mut stats,
                false,
            )
            .await?;
        }

        // ── §7.2 claims-file regen (§5.1 Active-only is OUR obligation) ─
        let claims = self
            .memory_graph
            .list_claims_by_status(ClaimStatus::Active)
            .await?;
        let rendered = DigestExporter::claims_to_markdown(&claims);
        let sanitized = self
            .pii_sanitizer
            .sanitize_text(&rendered, PiiFilterLevel::Standard);
        let content = format!("{VAULT_MARKER_LINE}\n\n{sanitized}");
        self.write_if_stale(
            &canonical_root,
            CLAIMS_FILE,
            &content,
            &stored_hashes,
            now_secs,
            &mut stats,
            true,
        )
        .await?;

        // Generated index (no per-file list — content only changes when the
        // window bounds move, keeping churn at one small write per day).
        let readme = format!(
            "{VAULT_MARKER_LINE}\n\n# Maekon Memory Vault\n\n\
             This folder is a one-way, regenerable mirror of your local Maekon \
             data (ADR-033). Files carrying the marker line above are rewritten \
             and expired automatically — copy them elsewhere to keep them. \
             Files without the marker are never touched.\n\n\
             - `{DAILY_DIR}/` — one file per day ({oldest_allowed} … {today})\n\
             - `{CLAIMS_FILE}` — your current accumulated claims\n"
        );
        self.write_if_stale(
            &canonical_root,
            README_FILE,
            &readme,
            &stored_hashes,
            now_secs,
            &mut stats,
            false,
        )
        .await?;

        // ── §7.3 expiry sweep (marker + pattern + containment guarded) ──
        let daily_dir = canonical_root.join(DAILY_DIR);
        let mut entries = tokio::fs::read_dir(&daily_dir).await.map_err(io_err)?;
        while let Some(entry) = entries.next_entry().await.map_err(io_err)? {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()).map(String::from) else {
                continue;
            };
            // I1: reap orphaned tmp artifacts (crash/rename-failure leftovers)
            // — they carry the full marker + content, so leaving them would be
            // the exact #4478 shadow-copy class. Marker + containment guarded
            // like every other delete.
            if name.ends_with(TMP_SUFFIX) {
                if contained(&canonical_root, &path) && has_marker(&path).await {
                    let _ = tokio::fs::remove_file(&path).await;
                }
                continue;
            }
            let Some(date) = day_file_date(&name) else {
                continue; // not our naming pattern — never touched
            };
            if date >= oldest_allowed && date <= today {
                continue;
            }
            if !contained(&canonical_root, &path) {
                continue;
            }
            let rel_name = format!("{DAILY_DIR}/{name}");
            if !has_marker(&path).await {
                stats.record_conflict(&rel_name);
                continue; // §6.4: user file matching our pattern — never deleted
            }
            tokio::fs::remove_file(&path).await.map_err(io_err)?;
            self.vault_state.delete_vault_hash(&rel_name).await?;
            stats.files_expired += 1;
        }

        // ── §3.4 cloud egress-ledger record (per-cycle, per-day dedup) ──
        if stats.bytes_written > 0 {
            if let Some(provider) = vault_cfg.cloud_provider.as_deref() {
                // I2: the ledger is erase-retained and deliberately no-PII, so
                // only the §3.4 closed label set may ever land in it. A
                // hand-edited config with an arbitrary string (worst case: a
                // path bearing the OS username) skips the record instead.
                if !CLOUD_PROVIDER_LABELS.contains(&provider) {
                    warn!(
                        "vault mirror: cloud_provider is not a known coarse label; \
                         skipping egress-ledger record (ADR-033 §3.4 allowlist)"
                    );
                } else {
                    let record = EgressLedgerRecord {
                        record_id: format!("vault_mirror|{provider}|{today}"),
                        event_type: "vault_mirror_cloud_sync".to_string(),
                        event_id: None,
                        byte_count: stats.bytes_written as i64,
                        recipient_count: 1,
                        destination: provider.to_string(),
                        disposition: "uploaded".to_string(),
                        consent_state: "lawful_basis:memory_vault_mirror_opt_in".to_string(),
                        occurred_at: Utc::now().to_rfc3339(),
                    };
                    let ledger = Arc::clone(&self.egress_ledger);
                    match tokio::task::spawn_blocking(move || ledger.record_egress(&record)).await {
                        Ok(Ok(())) => stats.cloud_ledger_recorded = true,
                        Ok(Err(e)) => {
                            // Port contract: ledger failure is non-fatal for
                            // the data flow it audits — log and continue.
                            warn!(err.code = %e.code(), "vault mirror ledger record failed: {e}");
                        }
                        Err(e) => warn!("vault mirror ledger task join failed: {e}"),
                    }
                }
            }
        }

        // ── §6.4 visibility: persist THIS cycle's summary (#9522) ───────
        // Every cycle reaching here records — SCHEDULED ones included, which is
        // the whole point: `stats` is per-invocation, so a scheduled cycle's
        // marker conflicts used to vanish and only a hand-pressed "Export now"
        // could reveal them. Still inside `_cycle_guard`, so the row describes
        // the cycle that won the root lock. Errors propagate like every sibling
        // state write; see `VaultLastCycleSummary` for why the fail-closed
        // no-op returns above deliberately record nothing.
        self.vault_state
            .put_last_cycle_summary(&VaultLastCycleSummary::from_cycle(&stats, now_secs))
            .await?;

        debug!(
            day_files = stats.day_files_written,
            claims = stats.claims_file_written,
            expired = stats.files_expired,
            conflicts = stats.conflicts,
            bytes = stats.bytes_written,
            "ADR-033 vault mirror cycle complete"
        );
        Ok(stats)
    }

    async fn snapshot_generated_roots(&self) -> Vec<PathBuf> {
        // Pre-wipe root discovery (MUST run before Phase-1 destroys the
        // vault_mirror_state row): default root, acknowledged custom root,
        // and the stored last-active root. Best-effort on state-read failure.
        let cfg = self.config_manager.get();
        let vault_cfg = &cfg.analysis.memory_vault;
        let mut roots: Vec<PathBuf> = Vec::new();
        if let Some(default_root) = &self.default_root {
            roots.push(default_root.clone());
        }
        if let (Some(custom), true) = (&vault_cfg.custom_path, vault_cfg.custom_path_acknowledged) {
            if !roots.contains(custom) {
                roots.push(custom.clone());
            }
        }
        match self.vault_state.vault_hashes().await {
            Ok(hashes) => {
                if let Some(stored) = hashes.get(ACTIVE_ROOT_KEY) {
                    let stored = PathBuf::from(stored);
                    if !roots.contains(&stored) {
                        roots.push(stored);
                    }
                }
            }
            Err(e) => warn!(
                err.code = %e.code(),
                "vault root snapshot: state read failed; using config-derived roots only: {e}"
            ),
        }
        roots
    }

    async fn erase_generated_files(
        &self,
        roots: Vec<PathBuf>,
    ) -> Result<VaultEraseReport, CoreError> {
        // §4: runs regardless of enabled/consent gates, over the PRE-WIPE
        // root snapshot (see snapshot_generated_roots — reading state here
        // would be dead code, Phase-1 has already emptied the table).
        // Marker-bearing generated files only; never a recursive delete of a
        // user folder.
        let mut report = VaultEraseReport::default();
        for root in roots {
            let (deleted, failures) = erase_root_generated(&root, self.vault_state.as_ref()).await;
            report.deleted += deleted;
            report.failures.extend(failures);
            // Tidy empty generated dirs on the default root only.
            if Some(&root) == self.default_root.as_ref() {
                if let Ok(canonical_root) = root.canonicalize() {
                    let _ = tokio::fs::remove_dir(canonical_root.join(DAILY_DIR)).await;
                    let _ = tokio::fs::remove_dir(&canonical_root).await;
                }
            }
        }
        Ok(report)
    }
}

impl VaultMirrorWriter {
    /// §1.4 staleness + write + hash upkeep for one generated file.
    #[allow(clippy::too_many_arguments)]
    async fn write_if_stale(
        &self,
        canonical_root: &Path,
        rel_name: &str,
        content: &str,
        stored_hashes: &HashMap<String, String>,
        now_secs: i64,
        stats: &mut VaultCycleStats,
        is_claims: bool,
    ) -> Result<(), CoreError> {
        let hash = content_hash(content);
        let target = canonical_root.join(rel_name);
        let file_exists = tokio::fs::try_exists(&target).await.unwrap_or(false);
        let hash_fresh = stored_hashes.get(rel_name) == Some(&hash);
        // §1.4: hash-absent OR hash-stale OR file missing on disk.
        if hash_fresh && file_exists {
            return Ok(());
        }
        match write_generated(canonical_root, rel_name, content).await? {
            Some(bytes) => {
                self.vault_state
                    .upsert_vault_hash(rel_name, &hash, now_secs)
                    .await?;
                stats.bytes_written += bytes;
                if is_claims {
                    stats.claims_file_written = true;
                } else if rel_name != README_FILE {
                    stats.day_files_written += 1;
                }
            }
            None => stats.record_conflict(rel_name),
        }
        Ok(())
    }
}

/// Shared per-root Art.17 / root-change erase pass (ADR-033 §4): delete every
/// marker-bearing generated file (claims/README/day files AND orphaned
/// `.maekon-tmp` artifacts) under `root`, marker + containment guarded, and
/// drop their hash rows. Returns (deleted_count, failures).
async fn erase_root_generated(
    root: &Path,
    vault_state: &dyn VaultMirrorStatePort,
) -> (usize, Vec<VaultEraseFailure>) {
    let Ok(canonical_root) = root.canonicalize() else {
        return (0, Vec::new()); // root does not exist — nothing generated
    };
    // Serialize against any in-flight cycle on THIS root (the erase half of
    // the VAULT_ROOT_LOCKS contract): Phase-3's "erasure complete" must be a
    // real synchronization point, not a scan racing a concurrent writer. No
    // lock-order inversion is possible: both the cycle (current root) and its
    // old-root cleanup derive roots from the same global config, and erase
    // acquires one root at a time.
    let lock = root_lock(&canonical_root);
    let _erase_guard = lock.lock().await;
    let mut targets: Vec<(String, PathBuf)> = vec![
        (CLAIMS_FILE.to_string(), canonical_root.join(CLAIMS_FILE)),
        (README_FILE.to_string(), canonical_root.join(README_FILE)),
    ];
    let daily_dir = canonical_root.join(DAILY_DIR);
    if let Ok(mut entries) = tokio::fs::read_dir(&daily_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if day_file_date(name).is_some() || name.ends_with(TMP_SUFFIX) {
                    targets.push((format!("{DAILY_DIR}/{name}"), path));
                }
            }
        }
    }
    // Root-level tmp orphans (claims.md/README.md writes).
    if let Ok(mut entries) = tokio::fs::read_dir(&canonical_root).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.ends_with(TMP_SUFFIX) {
                    targets.push((name.to_string(), path));
                }
            }
        }
    }

    let mut deleted = 0usize;
    let mut failures = Vec::new();
    for (rel_name, path) in targets {
        if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
            continue;
        }
        if !contained(&canonical_root, &path) || !has_marker(&path).await {
            continue; // §6.4: never delete a non-marker file
        }
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {
                deleted += 1;
                // Best-effort: the SQL erase sweep clears the whole table
                // anyway in the full Art.17 flow.
                let _ = vault_state.delete_vault_hash(&rel_name).await;
            }
            Err(e) => failures.push(VaultEraseFailure {
                file_name: rel_name,
                message: e.to_string(),
            }),
        }
    }
    (deleted, failures)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use maekon_core::config::AppConfig;
    use maekon_core::consent::{ConsentPermissions, ConsentRecord, ConsentStatus};
    use maekon_core::models::daily_digest::{DailyDigest, DailyStatistics};
    use maekon_core::models::memory_graph::{ClaimKind, EdgeType, MemoryClaim, MemoryEdge};
    use std::sync::atomic::AtomicBool;
    use std::sync::Mutex;

    pub(super) const NOW: i64 = 1_753_000_000;

    // ---- manual fakes (ADR-001 §5) ----

    struct FakeDigests(Vec<DailyDigest>);
    #[async_trait::async_trait]
    impl DigestStorage for FakeDigests {
        async fn list_daily_digests(&self, limit: usize) -> Result<Vec<DailyDigest>, CoreError> {
            Ok(self.0.iter().take(limit).cloned().collect())
        }
        async fn list_weekly_digests(
            &self,
            _limit: usize,
        ) -> Result<Vec<maekon_core::models::weekly_digest::WeeklyDigest>, CoreError> {
            Ok(vec![])
        }
        async fn get_current_week_digest(
            &self,
        ) -> Result<Option<maekon_core::models::weekly_digest::WeeklyDigest>, CoreError> {
            Ok(None)
        }
        async fn save_weekly_digest(
            &self,
            _digest: &maekon_core::models::weekly_digest::WeeklyDigest,
        ) -> Result<(), CoreError> {
            Ok(())
        }
    }

    struct FakeGraph(Vec<MemoryClaim>);
    #[async_trait::async_trait]
    impl MemoryGraphPort for FakeGraph {
        async fn save_claim(&self, _c: &MemoryClaim) -> Result<(), CoreError> {
            unreachable!()
        }
        async fn get_claim(&self, _id: &str) -> Result<Option<MemoryClaim>, CoreError> {
            unreachable!()
        }
        async fn list_claims_by_status(
            &self,
            status: ClaimStatus,
        ) -> Result<Vec<MemoryClaim>, CoreError> {
            Ok(self
                .0
                .iter()
                .filter(|c| c.status == status)
                .cloned()
                .collect())
        }
        async fn set_claim_status(
            &self,
            _id: &str,
            _s: ClaimStatus,
            _t: i64,
        ) -> Result<(), CoreError> {
            unreachable!()
        }
        async fn add_edge(&self, _e: &MemoryEdge) -> Result<(), CoreError> {
            unreachable!()
        }
        async fn edges_from(
            &self,
            _s: &str,
            _t: Option<EdgeType>,
        ) -> Result<Vec<MemoryEdge>, CoreError> {
            unreachable!()
        }
        async fn edges_from_many(
            &self,
            _s: &[String],
        ) -> Result<HashMap<String, Vec<MemoryEdge>>, CoreError> {
            unreachable!()
        }
        async fn prune_claims_older_than(&self, _c: i64) -> Result<u64, CoreError> {
            unreachable!()
        }
        async fn prune_orphan_evidence_edges(&self) -> Result<u64, CoreError> {
            unreachable!()
        }
        async fn supersede_claim(
            &self,
            _l: &str,
            _e: &MemoryEdge,
            _t: i64,
        ) -> Result<(), CoreError> {
            unreachable!()
        }
    }

    #[derive(Default)]
    pub(super) struct FakeVaultState {
        hashes: Mutex<HashMap<String, String>>,
        last_cycle: Mutex<Option<VaultLastCycleSummary>>,
    }
    #[async_trait::async_trait]
    impl VaultMirrorStatePort for FakeVaultState {
        async fn vault_hashes(&self) -> Result<HashMap<String, String>, CoreError> {
            Ok(self.hashes.lock().unwrap().clone())
        }
        async fn upsert_vault_hash(
            &self,
            file_name: &str,
            content_hash: &str,
            _updated_at: i64,
        ) -> Result<(), CoreError> {
            self.hashes
                .lock()
                .unwrap()
                .insert(file_name.to_string(), content_hash.to_string());
            Ok(())
        }
        async fn delete_vault_hash(&self, file_name: &str) -> Result<(), CoreError> {
            self.hashes.lock().unwrap().remove(file_name);
            Ok(())
        }
        async fn last_cycle_summary(&self) -> Result<Option<VaultLastCycleSummary>, CoreError> {
            Ok(self.last_cycle.lock().unwrap().clone())
        }
        async fn put_last_cycle_summary(
            &self,
            summary: &VaultLastCycleSummary,
        ) -> Result<(), CoreError> {
            *self.last_cycle.lock().unwrap() = Some(summary.clone());
            Ok(())
        }
    }

    struct MarkerSanitizer;
    impl PiiSanitizer for MarkerSanitizer {
        fn sanitize_text(&self, text: &str, _level: PiiFilterLevel) -> String {
            text.replace("SECRET", "[MASKED]")
        }
    }

    #[derive(Default)]
    pub(super) struct RecordingLedger(pub(super) Mutex<Vec<EgressLedgerRecord>>);
    impl EgressLedgerSink for RecordingLedger {
        fn record_egress(&self, record: &EgressLedgerRecord) -> Result<(), CoreError> {
            self.0.lock().unwrap().push(record.clone());
            Ok(())
        }
    }

    pub(super) struct FakeConsent {
        pub(super) granted: bool,
        pub(super) deleting: Arc<AtomicBool>,
        pub(super) erasing_flag: Arc<AtomicBool>,
    }
    impl FakeConsent {
        pub(super) fn granted() -> Self {
            Self {
                granted: true,
                deleting: Arc::new(AtomicBool::new(false)),
                erasing_flag: Arc::new(AtomicBool::new(false)),
            }
        }
    }
    impl ConsentManagerPort for FakeConsent {
        fn check_consent(&self) -> ConsentStatus {
            if self.granted {
                ConsentStatus::Valid
            } else {
                ConsentStatus::NotGranted
            }
        }
        fn current_consent(&self) -> Option<ConsentRecord> {
            None
        }
        fn effective_permissions(&self) -> ConsentPermissions {
            ConsentPermissions {
                memory_vault_mirror: self.granted,
                ..Default::default()
            }
        }
        fn status_and_permissions(&self) -> (ConsentStatus, ConsentPermissions) {
            (self.check_consent(), self.effective_permissions())
        }
        fn grant_consent(&self, _p: ConsentPermissions, _d: u32) -> Result<(), CoreError> {
            unreachable!()
        }
        fn revoke_consent(&self) -> Result<(), CoreError> {
            unreachable!()
        }
        fn has_pending_deletion(&self) -> bool {
            false
        }
        fn pending_erasure_id(&self) -> Option<String> {
            None
        }
        fn clear_pending_deletion(&self) {}
        fn deletion_flag(&self) -> Arc<AtomicBool> {
            Arc::clone(&self.deleting)
        }
        fn erasing(&self) -> Arc<AtomicBool> {
            Arc::clone(&self.erasing_flag)
        }
    }

    fn digest_for(date: NaiveDate) -> DailyDigest {
        DailyDigest {
            date,
            insight: None,
            timeline: vec![],
            statistics: DailyStatistics::default(),
            generated_at: Utc::now(),
        }
    }

    pub(super) fn claim(id: &str, text: &str, status: ClaimStatus) -> MemoryClaim {
        MemoryClaim {
            claim_id: id.to_string(),
            kind: ClaimKind::Episodic,
            text: text.to_string(),
            source: "digest_highlight".to_string(),
            confidence: 0.9,
            status,
            created_at: NOW,
            updated_at: NOW,
        }
    }

    pub(super) fn today() -> NaiveDate {
        Local
            .timestamp_opt(NOW, 0)
            .single()
            .map(|t| t.date_naive())
            .unwrap()
    }

    pub(super) struct Harness {
        pub(super) writer: VaultMirrorWriter,
        pub(super) root: PathBuf,
        pub(super) ledger: Arc<RecordingLedger>,
        /// Same Arc the writer holds — lets a test read back what the cycle
        /// persisted (§1.4 hashes, §6.4 last-cycle summary).
        pub(super) state: Arc<FakeVaultState>,
        _dirs: (tempfile::TempDir, tempfile::TempDir),
    }

    pub(super) fn harness(
        digests: Vec<DailyDigest>,
        claims: Vec<MemoryClaim>,
        consent: Option<FakeConsent>,
        mutate: impl FnOnce(&mut AppConfig),
    ) -> Harness {
        let cfg_dir = tempfile::tempdir().unwrap();
        let vault_dir = tempfile::tempdir().unwrap();
        let root = vault_dir.path().join("vault");
        let manager = ConfigManager::with_path(cfg_dir.path().join("config.json")).unwrap();
        let mut cfg = manager.get();
        cfg.analysis.memory_vault.enabled = true;
        mutate(&mut cfg);
        manager.update(cfg).unwrap();
        let ledger = Arc::new(RecordingLedger::default());
        let state = Arc::new(FakeVaultState::default());
        let writer = VaultMirrorWriter::new(
            Arc::new(FakeDigests(digests)),
            Arc::new(FakeGraph(claims)),
            state.clone() as Arc<dyn VaultMirrorStatePort>,
            Arc::new(MarkerSanitizer),
            ledger.clone() as Arc<dyn EgressLedgerSink>,
            consent.map(|c| Arc::new(c) as Arc<dyn ConsentManagerPort>),
            manager,
            Some(root.clone()),
        );
        Harness {
            writer,
            root,
            ledger,
            state,
            _dirs: (cfg_dir, vault_dir),
        }
    }

    // ---- §2 gates: fail-closed no-op ----

    #[tokio::test]
    async fn disabled_consent_missing_denied_and_bad_window_all_no_op() {
        for (label, consent, mutate) in [
            (
                "disabled",
                Some(FakeConsent::granted()),
                Box::new(|c: &mut AppConfig| c.analysis.memory_vault.enabled = false)
                    as Box<dyn FnOnce(&mut AppConfig)>,
            ),
            (
                "consent_unavailable",
                None,
                Box::new(|_: &mut AppConfig| {}),
            ),
            (
                "consent_denied",
                Some(FakeConsent {
                    granted: false,
                    deleting: Arc::new(AtomicBool::new(false)),
                    erasing_flag: Arc::new(AtomicBool::new(false)),
                }),
                Box::new(|_: &mut AppConfig| {}),
            ),
            (
                "window_invalid",
                Some(FakeConsent::granted()),
                Box::new(|c: &mut AppConfig| {
                    c.analysis.memory_vault.mirror_window_days =
                        c.analysis.embedding.retention_days + 1
                }),
            ),
        ] {
            let h = harness(vec![digest_for(today())], vec![], consent, mutate);
            let stats = h.writer.run_mirror_cycle(NOW).await.unwrap();
            assert!(stats.skipped_reason.is_some(), "{label}: must be a no-op");
            assert!(!h.root.exists(), "{label}: no-op must not create the vault");
        }
    }

    #[tokio::test]
    async fn erase_in_progress_is_no_op() {
        let consent = FakeConsent::granted();
        consent
            .deleting
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let h = harness(vec![digest_for(today())], vec![], Some(consent), |_| {});
        let stats = h.writer.run_mirror_cycle(NOW).await.unwrap();
        assert_eq!(stats.skipped_reason.as_deref(), Some("erase_in_progress"));
    }

    // ---- happy path: files, marker, sanitizing, idempotence ----

    #[tokio::test]
    async fn cycle_writes_marked_sanitized_files_and_is_idempotent() {
        let h = harness(
            vec![digest_for(today())],
            vec![
                claim("clm_a", "user did SECRET things", ClaimStatus::Active),
                claim("clm_b", "superseded text", ClaimStatus::Superseded),
                claim("clm_c", "retracted text", ClaimStatus::Retracted),
            ],
            Some(FakeConsent::granted()),
            |_| {},
        );
        let stats = h.writer.run_mirror_cycle(NOW).await.unwrap();
        assert_eq!(stats.skipped_reason, None);
        assert_eq!(stats.day_files_written, 1);
        assert!(stats.claims_file_written);

        let claims_md =
            std::fs::read_to_string(h.root.join("claims.md")).expect("claims.md written");
        assert!(
            claims_md.starts_with(VAULT_MARKER_LINE),
            "§6.4 marker first"
        );
        // §5.2 whole-doc sanitize + §5.1 Active-only selection.
        assert!(claims_md.contains("[MASKED]"));
        assert!(!claims_md.contains("SECRET"));
        assert!(!claims_md.contains("superseded text"));
        assert!(!claims_md.contains("retracted text"));
        let day_md = std::fs::read_to_string(h.root.join(format!("daily/{}.md", today())))
            .expect("day file written");
        assert!(day_md.starts_with(VAULT_MARKER_LINE));

        // Second cycle: nothing changed → zero writes (§1.4).
        let stats2 = h.writer.run_mirror_cycle(NOW).await.unwrap();
        assert_eq!(stats2.day_files_written, 0);
        assert!(!stats2.claims_file_written);
        assert_eq!(stats2.bytes_written, 0);
    }

    #[tokio::test]
    async fn missing_file_regenerates_despite_fresh_hash() {
        let h = harness(
            vec![digest_for(today())],
            vec![claim("clm_a", "text", ClaimStatus::Active)],
            Some(FakeConsent::granted()),
            |_| {},
        );
        h.writer.run_mirror_cycle(NOW).await.unwrap();
        std::fs::remove_file(h.root.join("claims.md")).unwrap();
        // §1.4: stored hash matches, but the file is gone → regenerate.
        let stats = h.writer.run_mirror_cycle(NOW).await.unwrap();
        assert!(stats.claims_file_written, "deleted file must self-heal");
        assert!(h.root.join("claims.md").exists());
    }

    // ---- §6.4 marker guard (collision safety) ----

    #[tokio::test]
    async fn pre_existing_user_file_is_never_overwritten_and_counted_as_conflict() {
        let h = harness(
            vec![digest_for(today())],
            vec![],
            Some(FakeConsent::granted()),
            |_| {},
        );
        let day_rel = format!("daily/{}.md", today());
        std::fs::create_dir_all(h.root.join("daily")).unwrap();
        std::fs::write(h.root.join(&day_rel), "my precious obsidian daily note").unwrap();

        let stats = h.writer.run_mirror_cycle(NOW).await.unwrap();
        assert_eq!(stats.day_files_written, 0);
        assert_eq!(stats.conflict_paths, vec![day_rel.clone()]);
        let preserved = std::fs::read_to_string(h.root.join(&day_rel)).unwrap();
        assert_eq!(preserved, "my precious obsidian daily note");
        // #9522: `stats` dies with the invocation — what makes a SCHEDULED
        // cycle's conflict reachable is this persisted row.
        let recorded = h.state.last_cycle_summary().await.unwrap();
        let persisted = recorded.expect("recorded");
        assert_eq!(persisted.finished_at, NOW);
        assert_eq!(persisted.conflicts, stats.conflicts);
        assert_eq!(persisted.conflict_paths, vec![day_rel]);
    }

    #[tokio::test]
    async fn a_later_fail_closed_no_op_keeps_the_recorded_conflicts() {
        // An empty "feature off" record would destroy the very names §6.4 asks
        // the settings screen to show, so no-op cycles must not overwrite.
        let h = harness(vec![], vec![], Some(FakeConsent::granted()), |_| {});
        std::fs::create_dir_all(h.root.join("daily")).unwrap();
        std::fs::write(h.root.join("daily/2020-01-02.md"), "user note").unwrap();
        h.writer.run_mirror_cycle(NOW).await.unwrap();

        let mut cfg = h.writer.config_manager.get();
        cfg.analysis.memory_vault.enabled = false;
        h.writer.config_manager.update(cfg).unwrap();
        let stats = h.writer.run_mirror_cycle(NOW + 60).await.unwrap();
        assert_eq!(stats.skipped_reason.as_deref(), Some("disabled"));

        let kept = h.state.last_cycle_summary().await.unwrap().expect("kept");
        assert_eq!(
            kept.finished_at, NOW,
            "the no-op must not re-anchor the row"
        );
        assert_eq!(kept.conflict_paths, vec!["daily/2020-01-02.md"]);
    }

    // ---- §7.3 expiry sweep ----

    #[tokio::test]
    async fn expiry_deletes_marked_out_of_window_files_but_never_user_files() {
        let h = harness(vec![], vec![], Some(FakeConsent::granted()), |c| {
            c.analysis.memory_vault.mirror_window_days = 7;
        });
        std::fs::create_dir_all(h.root.join("daily")).unwrap();
        let old_generated = h.root.join("daily/2020-01-01.md");
        std::fs::write(&old_generated, format!("{VAULT_MARKER_LINE}\nold")).unwrap();
        let old_user = h.root.join("daily/2020-01-02.md");
        std::fs::write(&old_user, "user note that merely matches the pattern").unwrap();

        let stats = h.writer.run_mirror_cycle(NOW).await.unwrap();
        assert_eq!(stats.files_expired, 1);
        assert!(!old_generated.exists(), "marked out-of-window file expired");
        assert!(old_user.exists(), "§6.4: user file never deleted");
        assert!(stats.conflicts >= 1);
    }

    // ---- §3.4 cloud egress ledger ----

    #[tokio::test]
    async fn cloud_provider_cycle_records_one_pinned_ledger_row() {
        let h = harness(
            vec![digest_for(today())],
            vec![],
            Some(FakeConsent::granted()),
            |c| {
                c.analysis.memory_vault.cloud_provider = Some("icloud".to_string());
            },
        );
        let stats = h.writer.run_mirror_cycle(NOW).await.unwrap();
        assert!(stats.cloud_ledger_recorded);
        let records = h.ledger.0.lock().unwrap();
        assert_eq!(records.len(), 1);
        let r = &records[0];
        assert_eq!(r.event_type, "vault_mirror_cloud_sync");
        assert_eq!(r.destination, "icloud");
        assert_eq!(r.record_id, format!("vault_mirror|icloud|{}", today()));
        assert!(r.byte_count > 0);
        // §3.4: never a filesystem path in the erase-retained table.
        assert!(!r.destination.contains('/'));
    }

    #[tokio::test]
    async fn default_path_records_nothing() {
        let h = harness(
            vec![digest_for(today())],
            vec![],
            Some(FakeConsent::granted()),
            |_| {},
        );
        let stats = h.writer.run_mirror_cycle(NOW).await.unwrap();
        assert!(!stats.cloud_ledger_recorded);
        assert!(h.ledger.0.lock().unwrap().is_empty());
    }

    // ---- §4 erase ----

    #[tokio::test]
    async fn erase_deletes_generated_files_even_when_disabled_and_reports_complete() {
        let h = harness(
            vec![digest_for(today())],
            vec![claim("clm_a", "text", ClaimStatus::Active)],
            Some(FakeConsent::granted()),
            |_| {},
        );
        h.writer.run_mirror_cycle(NOW).await.unwrap();
        let user_file = h.root.join("daily/keep-me.md.txt");
        std::fs::write(h.root.join("daily/2019-01-01.md"), "user file no marker").unwrap();
        std::fs::write(&user_file, "unrelated").unwrap();

        // Erase must run even after the feature is turned off (§4).
        let manager = h.writer.config_manager.clone();
        let mut cfg = manager.get();
        cfg.analysis.memory_vault.enabled = false;
        manager.update(cfg).unwrap();

        let roots = h.writer.snapshot_generated_roots().await;
        let report = h.writer.erase_generated_files(roots).await.unwrap();
        assert!(report.is_complete());
        assert_eq!(report.deleted, 3, "day file + claims.md + README.md");
        assert!(!h.root.join("claims.md").exists());
        assert!(!h.root.join(format!("daily/{}.md", today())).exists());
        assert!(
            h.root.join("daily/2019-01-01.md").exists(),
            "§6.4: marker-less user file survives Art.17 vault sweep"
        );
    }

    // ---- unacknowledged custom path stays on default ----

    #[tokio::test]
    async fn unacknowledged_custom_path_falls_back_to_default_root() {
        let elsewhere = tempfile::tempdir().unwrap();
        let h = harness(
            vec![digest_for(today())],
            vec![],
            Some(FakeConsent::granted()),
            |c| {
                c.analysis.memory_vault.custom_path = Some(elsewhere.path().to_path_buf());
                c.analysis.memory_vault.custom_path_acknowledged = false;
            },
        );
        h.writer.run_mirror_cycle(NOW).await.unwrap();
        assert!(h.root.join("claims.md").exists(), "default root used");
        assert!(
            !elsewhere.path().join("claims.md").exists(),
            "§3.3: unacknowledged custom path rejected"
        );
    }
}

#[cfg(test)]
mod review_fix_tests {
    use super::tests::*;
    use super::*;
    use maekon_core::models::daily_digest::{DailyDigest, DailyStatistics};
    use maekon_core::models::memory_graph::ClaimStatus;

    // B1: `erasing` alone (deletion_flag re-cleared by a mid-erase re-grant)
    // must still gate the cycle — the #4928 round-3 predicate.
    #[tokio::test]
    async fn erasing_flag_alone_gates_the_cycle() {
        let consent = FakeConsent::granted();
        consent
            .erasing_flag
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let h = harness(vec![digest_for(today())], vec![], Some(consent), |_| {});
        let stats = h.writer.run_mirror_cycle(NOW).await.unwrap();
        assert_eq!(stats.skipped_reason.as_deref(), Some("erase_in_progress"));
        assert!(!h.root.exists());
    }

    // B2: concurrent cycles on one root serialize — after both complete, the
    // stored hash matches the file actually on disk (no torn-write desync).
    #[tokio::test]
    async fn concurrent_cycles_serialize_and_hash_matches_disk() {
        let h = harness(
            vec![digest_for(today())],
            vec![claim("clm_a", "text", ClaimStatus::Active)],
            Some(FakeConsent::granted()),
            |_| {},
        );
        let w = &h.writer;
        let (a, b) = tokio::join!(w.run_mirror_cycle(NOW), w.run_mirror_cycle(NOW));
        a.unwrap();
        b.unwrap();
        // A third cycle must be a zero-write no-change pass — hash state and
        // disk agree (a desync would force a rewrite here).
        let stats = w.run_mirror_cycle(NOW).await.unwrap();
        assert_eq!(stats.day_files_written, 0);
        assert!(!stats.claims_file_written);
        assert_eq!(stats.bytes_written, 0);
    }

    // I1: an orphaned tmp artifact (crash between write and rename) is reaped
    // by the next cycle's sweep and never survives Art.17 erase.
    #[tokio::test]
    async fn orphaned_tmp_artifacts_are_reaped() {
        let h = harness(
            vec![digest_for(today())],
            vec![],
            Some(FakeConsent::granted()),
            |_| {},
        );
        h.writer.run_mirror_cycle(NOW).await.unwrap();
        let orphan = h.root.join("daily/2020-01-01.md.maekon-tmp");
        std::fs::write(&orphan, format!("{VAULT_MARKER_LINE}\norphan")).unwrap();

        h.writer.run_mirror_cycle(NOW).await.unwrap();
        assert!(
            !orphan.exists(),
            "cycle sweep must reap marker-bearing tmp orphans"
        );

        // And a marker-less tmp-suffixed user file is never touched.
        let user_tmp = h.root.join("daily/notes.maekon-tmp");
        std::fs::write(&user_tmp, "user file").unwrap();
        h.writer.run_mirror_cycle(NOW).await.unwrap();
        assert!(
            user_tmp.exists(),
            "§6.4: marker-less tmp-named file survives"
        );
    }

    // I2: a non-allowlisted cloud_provider string never reaches the
    // erase-retained ledger.
    #[tokio::test]
    async fn non_allowlisted_cloud_provider_skips_ledger() {
        let h = harness(
            vec![digest_for(today())],
            vec![],
            Some(FakeConsent::granted()),
            |c| {
                c.analysis.memory_vault.cloud_provider =
                    Some("/Users/jsmith/Library/CloudStorage".to_string());
            },
        );
        let stats = h.writer.run_mirror_cycle(NOW).await.unwrap();
        assert!(stats.bytes_written > 0, "writes still happen");
        assert!(!stats.cloud_ledger_recorded);
        assert!(
            h.ledger.0.lock().unwrap().is_empty(),
            "arbitrary provider strings must never land in the ledger"
        );
    }

    fn digest_for(date: chrono::NaiveDate) -> DailyDigest {
        DailyDigest {
            date,
            insight: None,
            timeline: vec![],
            statistics: DailyStatistics::default(),
            generated_at: Utc::now(),
        }
    }
}

#[cfg(test)]
mod b3_root_change_tests {
    use super::tests::*;
    use super::*;
    use maekon_core::models::daily_digest::{DailyDigest, DailyStatistics};

    fn digest_for(date: chrono::NaiveDate) -> DailyDigest {
        DailyDigest {
            date,
            insight: None,
            timeline: vec![],
            statistics: DailyStatistics::default(),
            generated_at: Utc::now(),
        }
    }

    // B3: changing custom_path erases the OLD root's generated files on the
    // next cycle — nothing generated is ever stranded outside erase reach.
    #[tokio::test]
    async fn root_change_cleans_previous_root() {
        let h = harness(
            vec![digest_for(today())],
            vec![],
            Some(FakeConsent::granted()),
            |_| {},
        );
        h.writer.run_mirror_cycle(NOW).await.unwrap();
        let old_claims = h.root.join("claims.md");
        assert!(old_claims.exists());

        // User points the mirror at a new acknowledged custom path.
        let new_vault = tempfile::tempdir().unwrap();
        let manager = h.writer.config_manager.clone();
        let mut cfg = manager.get();
        cfg.analysis.memory_vault.custom_path = Some(new_vault.path().join("vault"));
        cfg.analysis.memory_vault.custom_path_acknowledged = true;
        manager.update(cfg).unwrap();

        h.writer.run_mirror_cycle(NOW).await.unwrap();
        assert!(
            !old_claims.exists(),
            "old root's generated files must be cleaned on root change (#4478 drift class)"
        );
        assert!(new_vault.path().join("vault/claims.md").exists());
    }

    // B3 crash-window coverage: erase_generated_files also sweeps the STORED
    // active root even when the current config no longer points at it.
    #[tokio::test]
    async fn erase_sweeps_stored_active_root() {
        let custom = tempfile::tempdir().unwrap();
        let custom_root = custom.path().join("vault");
        let h = harness(
            vec![digest_for(today())],
            vec![],
            Some(FakeConsent::granted()),
            |c| {
                c.analysis.memory_vault.custom_path = Some(custom_root.clone());
                c.analysis.memory_vault.custom_path_acknowledged = true;
            },
        );
        h.writer.run_mirror_cycle(NOW).await.unwrap();
        assert!(custom_root.join("claims.md").exists());

        // Config reverts to default WITHOUT a cycle running in between
        // (the crash window): erase must still find the stored root.
        let manager = h.writer.config_manager.clone();
        let mut cfg = manager.get();
        cfg.analysis.memory_vault.custom_path = None;
        cfg.analysis.memory_vault.custom_path_acknowledged = false;
        manager.update(cfg).unwrap();

        // Real orchestrator ordering (IMPORTANT#2): snapshot BEFORE the SQL
        // wipe would run, then erase over the snapshot.
        let roots = h.writer.snapshot_generated_roots().await;
        // The stored row holds the CANONICALIZED root (macOS tempdirs resolve
        // /var → /private/var), so compare canonical forms.
        let canonical_custom = custom_root
            .canonicalize()
            .expect("canonicalize custom root");
        assert!(
            roots.iter().any(|r| r == &canonical_custom),
            "pre-wipe snapshot must include the stored active root"
        );
        let report = h.writer.erase_generated_files(roots).await.unwrap();
        assert!(report.is_complete());
        assert!(
            !custom_root.join("claims.md").exists(),
            "stored active root must be swept by Art.17 erase"
        );
    }
}
