[English](./ADR-033-memory-vault-mirror.md) | [한국어](./ADR-033-memory-vault-mirror.ko.md)

# ADR-033: Memory Vault Mirror — User-Owned Local Markdown Surface

**Status**: Accepted — amended 2026-07-29 after 3-loop review (#9465); originally Proposed 2026-07-29
**Date**: 2026-07-29
**Scope**: `maekon-core` (consent Tier 13, `analysis.memory_vault` config, `MemoryVaultWriterPort`), `maekon-analysis` (writer implementation, exporter reuse), `maekon-storage` (`vault_mirror_state` migration, §1.4), `maekon-web` (settings surface, erase orchestrator Phase-3), `src-tauri` (scheduler wiring, IPC, erase orchestrator Phase-3)
**Related**: ADR-023 (memory-graph substrate + digest exporter), ADR-032 (generation-input contract — disclosure taxonomy this ADR sits beside, not under), ADR-028 §P3 (sanitizer-floor precedent), ADR-026 (`ConsentManagerPort`), #4478/#4479 (erasure shadow-copy incident class + fix pattern), #8056 (Art.20 rollup), #9465 (implementation issue)
**Issue**: #9465 (MK-MEM-01.T03)

---

## Context

The memory graph accumulates durable, activity-derived claims (ADR-023), and today the ONLY way a user can hold them in their own hands is `GET /api/digests/daily/export` — a one-shot, user-initiated Markdown download rendered by `DigestExporter::to_markdown_with_claims` (`crates/maekon-core/src/models/daily_digest.rs:150`; the exporter drops `Retracted` claims internally, while the `Active`-only selection that also excludes `Superseded` lives at the caller — `handlers/daily_digest.rs:76-85`). MK-MEM-01 (#9462) wants the next step: a **continuously mirrored local Markdown vault** — files a user can open in Obsidian or any editor, embodying "your data is your files". Hands-on reviews of the closest comparable product identify an inspectable vault as its single most-praised property.

What makes this an architecture decision rather than a feature PR is the risk asymmetry of a *continuous plaintext file tree outside SQLite*:

- **A second copy of activity-derived text is a new erasure surface.** The #4478 incident class is exactly this shape: a shadow copy (then: FTS tables) that erasure did not cover, silently surviving GDPR Article 17. Erasure today is split across **two independent orchestrators** — `src-tauri/src/commands/consent.rs::erase_all_local_data` (SQL phase + `FrameStoragePort` file phase, under `pending_local_erase` crash-recovery) and `crates/maekon-web/src/services/data_web_service.rs::DataCommandService::delete_all_data` (its own SQL/file split) — and the SQLite-layer `delete_all_data_inner` (`maintenance/retention.rs`, the `ALL_TABLES` sweep #4479 extended to the memory tables) is a pure-SQL body under a non-reentrant erase lock. None of them knows about files in a user-chosen directory, and a vault wired into only one orchestrator would recreate #4478 in the other.
- **A cloud-synced target folder is implicit off-device egress.** ADR-023's final audit concluded `egress_safe=true` — activity-derived claim text does not leave the device. If the user points the mirror at an iCloud/Dropbox/OneDrive/Google Drive folder (the natural choice: their existing Obsidian vault), claim text leaves the device continuously with **zero egress-ledger entries** — the ledger (`EgressLedgerSink::record_egress`, deliberately erase-retained and absent from `ALL_TABLES`) is the product's only "what left this device" truth surface, and this path would bypass it wholesale.
- **The mirror's own filenames collide with the user's.** Obsidian's Daily Notes convention is precisely `daily/YYYY-MM-DD.md`. A mirror that unconditionally overwrites or deletes files matching its naming pattern would destroy a user's pre-existing daily notes on the first cycle — a foreseeable data-loss bug inside the headline use case.
- **The existing export applies no export-time masking.** `export_daily_digest` renders stored text as-is. Acceptable for a one-shot download the user explicitly triggers; a different calculus for unattended files that persist at rest and may be indexed, synced, or backed up by third-party tools.
- **Retention divergence.** Claims are pruned at the retention window (`prune_claims_older_than`, driven by `analysis.embedding.retention_days`, default 90). Files have no such lifecycle unless one is decided.

ADR-023 is `Accepted — fully implemented`; grafting an unimplemented forward surface onto it would blur that status (the same reasoning ADR-032 used). Hence a dedicated ADR.

## Decision

Adopt a **one-way, regenerable, bounded mirror** with a fail-closed consent/config gate, orchestrator-level erasure propagation, a header-marker collision guard, and an explicit cloud-sync boundary.

### 1. Product shape: two file classes, derived view, not an archive

1. The vault is a **derived, regenerable view** of the SQLite SSOT, with two file classes matching how the underlying data actually behaves:
   - **Day files** — `vault/daily/YYYY-MM-DD.md`, the digest body only (`DigestExporter::to_markdown`, no claims section). A day's digest row changes rarely after generation (a later LLM-narrative backfill upserts it), so a day file is rewritten only when its rendered content changes (§1.4) and is otherwise only expired (§7) or erased (§4).
   - **Claims file** — `vault/claims.md`, the current `Active` claims rendered through the same claim-rendering logic `to_markdown_with_claims` uses today, extracted in the implementing PR as a pure `DigestExporter::claims_to_markdown(&[MemoryClaim])` in `maekon-core` (same fields, same `Retracted`-dropping invariant — a rendering-logic extraction, not a new disclosure surface). Claims are global, not per-day (`MemoryClaim` carries no day association), so they get ONE file that changes when the graph changes — not a copy pasted into every day file.
   - A generated `vault/README.md` index (third generated file, same header/marker rules).
2. **Strictly one-way.** The product never reads vault file *content* back (the §6.4 collision guard reads only enough to verify the product header). User edits are not merged and are overwritten on regeneration; every generated file begins with a fixed product header line (the marker, §6.4) stating exactly that. There is no file watching.
3. **The vault is not an archive.** It mirrors current DB truth: day files outside the mirror window are expired on regeneration (§7.3), and `claims.md` always reflects the current `Active` set (pruned/retracted/superseded claims disappear at the next cycle). Long-term keeping is the user's act — copying files out of the vault — at which point the copies are user-owned data outside product responsibility (Article 20 data-portability semantics, partially discharging the #8056 gap).
4. **Bounds and change-detection**: `mirror_window_days` (default **90**, must be ≥ 1 and ≤ `analysis.embedding.retention_days`) bounds managed files to window + 2. A file is (re)written when its newly rendered content differs from the **stored per-path content hash**, or when its hash row is absent, **or when the file itself is missing on disk** (a per-cycle existence check — a user-deleted file regenerates, preserving the regenerable-view promise; hash-only comparison must never suppress recreating a missing file). Hashes live in a new **`vault_mirror_state`** SQLite table (a `maekon-storage` migration; path-keyed rows holding the last-rendered content hash — no file content read-back, §1.2). The table joins the erasure `ALL_TABLES` sweep, so hash state can never outlive the files it describes (an orphaned erase-surviving hash row would silently suppress regeneration); rows for files the expiry sweep deletes are removed with them. A quiet day is zero writes; a claims change touches exactly `claims.md`.
5. **Bound violation is an unevaluable gate**, exactly the `MemoryGraphProjectionConfig` sibling semantics: if `mirror_window_days` is 0 or exceeds `analysis.embedding.retention_days` (including when a user later lowers retention below an already-configured window), the cycle is a **complete no-op** — no writes AND no deletes (a misconfiguration must never delete the vault), debug-logged. Never clamped, never widened.

### 2. Consent and configuration (fail-closed)

1. New dedicated `ConsentPermissions.memory_vault_mirror` — **Tier 13** (Tiers 11/12 are name-reserved by ADR-032 for Modes B/C), `#[serde(default)]` false, doc comment citing this ADR, not borrowed from or implied by any sibling permission.
2. New config section `analysis.memory_vault` (`MemoryVaultConfig`): `enabled` (default false), `custom_path: Option<PathBuf>` (default `None` = app-owned default location), `custom_path_acknowledged: bool` (default false, §3.3), `cloud_provider: Option<String>` (detection result stored at acceptance time, §3.2), `mirror_window_days` (default 90).
3. The writer is gated by `enabled` AND `memory_vault_mirror` consent AND the consent `deletion_flag` being clear — the same skip-while-erasing discipline every SQLite writer follows. Any unevaluable gate — including `data_dir()` resolution failure (`config_manager::data_dir` returns `Result`) — ⇒ complete no-op cycle (fail-closed; mirrors ADR-032 §2 semantics).

### 3. Custom-path boundary (the load-bearing clause)

1. **Default location**: `<data_dir()>/vault` — app-owned, platform-local, not under any cloud-sync root. Choosing the default requires only §2 gates.
2. **Cloud-sync detection** runs **once, at path-acceptance time**, against the canonicalized target, and its result is stored as `cloud_provider` (§2.2) — the stored value, not live re-detection, is the per-cycle truth:
   - macOS: path under `~/Library/Mobile Documents/` (iCloud Drive) or `~/Library/CloudStorage/` (the OS mount point for Dropbox/Google Drive/OneDrive/Box provider folders).
   - Windows: path under `%OneDrive%`/`%OneDriveCommercial%`, or a known provider root (`~/Dropbox`, `~/Google Drive`).
   - Linux: best-effort known roots (`~/Dropbox`, provider-named mount dirs).
   Detection's role is deliberately narrow: it **enriches the §3.3 warning copy** with the named provider and it **gates §3.4 ledger recording**. It gates no consent decision by itself.
3. **Every custom path — detected or not — requires a distinct acknowledgement** (`custom_path_acknowledged = true`, set through an explicit UI flow) whose copy states BOTH risks plainly: (a) files matching the mirror's naming pattern in that folder will be overwritten or deleted without merge (subject only to the §6.4 marker guard), and (b) if the folder is synced by any mechanism — the named provider when detected, "any sync tool you run" otherwise — claim text will leave the device through it, and Maekon cannot detect every sync mechanism. Without the acknowledgement the custom path is rejected and the mirror stays on the default location. (The detected/undetected split of the Proposed draft is gone: one unconditional acknowledgement closes the false-confidence gap where an unlisted provider produced no warning at all.)
4. **Egress-ledger visibility**: every regeneration cycle that writes at least one file to a path whose stored `cloud_provider` is set records **one** `EgressLedgerRecord`, fully pinned:
   - `event_type`: `vault_mirror_cloud_sync`
   - `destination`: the coarse provider label only (`icloud` | `cloud_storage` | `onedrive` | `dropbox` | `google_drive`) — **never a filesystem path**, which would embed the OS username into an erase-retained, deliberately-no-PII table (`destination`'s own doc comment: endpoint details are deliberately not recorded)
   - `record_id`: derived deterministically as `vault_mirror|<destination>|<local YYYY-MM-DD>` per the `EgressLedgerSink` dedup convention — replays and multiple cycles in one day collapse to a single audit row (the row means "the vault mirrored to a cloud-synced path on this date")
   - `byte_count`: total bytes written in the cycle that created the record; `recipient_count`: 1; remaining fields follow the existing producer conventions.
   Writes to the default location record nothing — they are device-local, same as the SQLite store itself. Ledger writes follow the port's own non-fatal discipline (log-and-continue on ledger failure; the mirror write itself is not blocked).
5. **Honesty bound**: detection is best-effort (a user can symlink, mount, or run an arbitrary sync daemon over any folder). The §3.3 copy and documentation state plainly that a custom path is the user's responsibility. The contract defends the *default* path absolutely, surfaces *every* custom path explicitly, and enriches what it can detect — and does not pretend to more.

### 4. Article 17 erasure propagation (the #4478 clause)

1. **Layering**: vault erasure is a **shared Phase-3 of both erase orchestrators**, NOT an extension of the SQLite-layer `delete_all_data_inner` (a pure-SQL body under a non-reentrant erase lock that must not gain filesystem I/O). The single implementation lives behind the same `maekon-core` port as the writer (`MemoryVaultWriterPort::erase_generated_files`, §7.4), and **both** orchestrators MUST call it:
   - `src-tauri/src/commands/consent.rs::erase_all_local_data` — as a new phase alongside its existing `FrameStoragePort` file phase, inside the `pending_local_erase` crash-recovery envelope (a crash between phases re-runs vault erasure on recovery);
   - `crates/maekon-web/src/services/data_web_service.rs::DataCommandService::delete_all_data` — as the same new phase.
   A contract test per orchestrator (not one shared test) is mandatory, because the two sites are already-duplicated code — the exact condition that produced #4478.
2. **Scope**: the default vault directory is always erased; a configured `custom_path` is erased by removal of **marker-bearing generated files only** (§6.4 — never a recursive directory delete of a user-chosen folder that may contain their own notes).
3. **Failure is surfaced, not swallowed**: `erase_generated_files` returns a per-file report; an orchestrator that receives failures MUST reflect them in its own outcome (erasure reported incomplete, retried under the crash-recovery envelope where available) — never `warn!`-and-continue. Files containing disclosed claim text are compliance surface, not a best-effort asset class like frame thumbnails.
4. **Regression guard in the #4479 pattern** (fail-before/pass-after) is mandatory in the implementing PR: seed claims → run a mirror cycle → erase → assert all generated files (day files, `claims.md`, index) are gone, in both orchestrator paths.
5. The erase-barrier ordering (deletion flag set before sweeps + the §2.3 writer gate) guarantees no regeneration lands mid-erase or after erase.
6. **What survives, stated plainly**: files the user copied out of the vault (§1.3), and any `vault_mirror_cloud_sync` ledger rows — the egress ledger is erase-retained by design, and those rows contain only the coarse labels of §3.4.

### 5. Content and masking floor

1. Mirror content is exactly the existing export surface, re-partitioned per §1.1: day files carry the digest body; `claims.md` carries the `Active` claims. **The `Active`-only selection (`list_claims_by_status(ClaimStatus::Active)`) is a contract obligation of the writer itself** — it lives inside the `MemoryVaultWriterPort` implementation, not at call sites, with a contract test asserting `Superseded` and `Retracted` text can never reach a generated file. (Today that selection exists only in the maekon-web export handler; the writer is a new call site in a different crate and MUST NOT be wired to any broader claims query.)
2. **Sanitizer floor, stricter than the endpoint, whole-document**: the **entire rendered Markdown of every generated file** — digest narrative, highlights, timeline entries, AND claim text — passes the injected `maekon_core::ports::pii_sanitizer::PiiSanitizer` at `PiiFilterLevel::Standard` minimum, applied **once, post-render, over the full document** before the atomic write. Fail-closed: no sanitizer wired ⇒ no vault writes (ADR-028 §P3 precedent). Post-render application is what makes the floor cover the digest body (rendered by shared `render_body`), not just the claims appendix. This is deliberately stricter than the one-shot HTTP export; the delta is documented rather than papered over, and endpoint parity is a separate decision this ADR does not make.
3. Retraction visibility: a retracted claim disappears from `claims.md` at the next regeneration cycle; the hard guarantee is the next daily cycle. (Cross-crate prompt-regeneration wiring from the retract handler is a Known Follow-up, not a contract term — no scheduler-trigger primitive currently exists between those crates.)

### 6. Filesystem safety

1. **Atomic writes**: temp file + same-directory rename, per file.
2. **Containment**: the vault root is canonicalized once per cycle; every write and every delete re-verifies the target resolves under that root (symlink-escape refuses the operation) — the existing canonicalize + `starts_with` pattern in `data_web_service.rs` is the model.
3. **Bounded**: at most window + 2 files are ever managed; the writer never enumerates or touches anything outside its own naming pattern.
4. **Marker guard (collision safety)**: every generated file begins with a fixed product header line (the marker). The writer NEVER overwrites, and the writer/eraser NEVER deletes, a pattern-matching file that lacks the marker — such a file is skipped, counted as a conflict in the cycle report, and surfaced in the settings/status UI; the mirror does not adopt that filename until the user removes or renames the file. This is what makes pointing `custom_path` at a live Obsidian vault safe: pre-existing user daily notes lack the marker and are untouchable by construction. (Verification reads only the header prefix — this is the §1.2 carve-out, not content read-back.)

**Amendment (#9522, 2026-07-30) — the "surfaced in the settings/status UI" clause is per-cycle, not per-invocation.** The cycle report is returned to whoever invoked the cycle, so as first implemented only the conflicts of a **manual** "Export now" were ever visible; the representative case — a *scheduled* cycle silently skipping a pre-existing Obsidian daily note — stayed invisible until the user happened to press that button. The last cycle that ran therefore persists a summary (timestamp, written/expired counts, and the conflicts as **vault-relative names only**, capped) in `vault_mirror_state` under a reserved `::`-prefixed key, read back with the §3 settings payload. Reserved rows are not file names and ride the same §4 `ALL_TABLES` erasure sweep as the hash rows, so this adds no state that can survive Art.17. Fail-closed no-op cycles deliberately do NOT record — an empty "feature disabled" record would destroy the conflict report this clause exists to show.

### 7. Cycle definition and scheduling

A **mirror cycle** is one invocation of `MemoryVaultWriterPort::run_mirror_cycle` and performs, in order:

1. **Day-file fill**: for each date in the mirror window whose digest row exists (read via the `DigestStorage` port — `crates/maekon-core/src/ports/web_storage.rs:288`) and whose §1.4 staleness condition holds (hash absent, hash stale, or file missing on disk), render and write the day file. This subsumes the digest catch-up seam: the scheduler's existing daily-digest generation (`scheduler/loops/system.rs` aggregation path) runs first and only walks forward (`daily_catchup_dates` short-circuits on existing digests), so the cycle reads digest ROWS rather than piggybacking the generation loop's control flow — the Proposed draft's "piggyback the loop" wording was structurally unable to expire or revisit files.
2. **Claims-file regen**: render `claims.md` from the writer's own `Active` selection (§5.1); rewrite under the same §1.4 condition (hash change or missing file).
3. **Expiry sweep**: enumerate marker-bearing generated files under the canonical root; delete those whose date falls outside the window (marker + pattern + containment checks all apply, §6).
4. The port (in `maekon-core`, implementation in `maekon-analysis`, DI-wired in `src-tauri` — the ADR-032 placement pattern):

```rust
#[async_trait]
pub trait MemoryVaultWriterPort: Send + Sync {
    /// One full mirror cycle (§7.1–§7.3). Fail-closed: any unevaluable
    /// §2 gate or §1.5 bound violation yields a no-op Ok with the reason
    /// in the stats. Storage errors propagate as Err.
    async fn run_mirror_cycle(&self, now_secs: i64) -> Result<VaultCycleStats, CoreError>;

    /// Art.17 Phase-3 (§4): delete every marker-bearing generated file
    /// under the active vault root. Per-file failures are reported in the
    /// result, never swallowed.
    async fn erase_generated_files(&self) -> Result<VaultEraseReport, CoreError>;
}
```

   The implementation fetches its own inputs via injected core ports (`DigestStorage` for digest rows, `MemoryGraphPort` for claims, `PiiSanitizer`, `EgressLedgerSink`, `ConsentManagerPort`, `ConfigManager`) — the scheduler passes nothing but `now_secs`.
5. **Triggers**: the scheduler invokes one cycle after its daily digest generation completes; the "Export now" IPC invokes one cycle (a full §7.1–§7.3 cycle, not a today-only export — naming in the implementing PR should avoid implying the old one-shot semantics).

## Consequences

### Positive
- The most-praised property of the comparable product (inspectable, user-owned vault) lands as a thin layer over an already-audited exporter — same fields as the existing endpoint, with a stricter at-rest floor.
- The three silent-failure classes this surface could introduce (erasure shadow copy across duplicated orchestrators, unledgered cloud egress, collision data-loss in the user's own vault) are named, gated, and test-mandated *before* implementation.
- Art.20 portability gets a standing partial answer (#8056) instead of none.

### Negative
- Cloud-sync detection remains a per-OS maintenance surface, though demoted to warning-enrichment + ledger-gating (its drift no longer affects consent flow).
- The vault-vs-endpoint masking delta (§5.2) is a deliberate inconsistency that documentation must carry until the endpoint decision is made.
- One-way regeneration will overwrite user edits to *marker-bearing* files despite the header warning — a support-burden trade accepted for SSOT integrity (files without the marker are structurally safe, §6.4).

### Neutral
- No runtime change until the implementing PR; defaults keep the feature entirely off (consent false AND enabled false).
- The vault makes user-chosen onward disclosure trivial (that is its purpose); ADR-032 continues to govern what *Maekon's own* generation pipeline may read — the two boundaries are disjoint by design.

## Alternatives Considered

**A. Amend ADR-023.** Rejected — same status-ambiguity reasoning as ADR-032; this is a cross-cutting surface (consent, erasure, ledger, scheduler), not a substrate change.

**B. Encrypted vault.** Rejected — defeats the purpose (user-inspectable files in ordinary editors is the feature). Users wanting encryption at rest have OS-level tools; the default location inherits the app data directory's protections.

**C. Two-way sync (vault edits flow back).** Rejected — turns a display surface into an unauthenticated write path into the memory graph (untrusted-content injection into a store that feeds ADR-032 consumers), plus merge complexity. One-way is a contract, not a phase.

**D. Unbounded archive (never delete day files).** Rejected — an unbounded plaintext accumulation of activity-derived text contradicts the retention posture (GDPR Art. 5(1)(e)) that bounds the SSOT itself; keeping is the user's explicit act (§1.3).

**E. Ledger entry per file write.** Rejected — per-day dedup granularity (§3.4) keeps the ledger signal-dense; per-file entries would flood it without adding decision-relevant information.

**F. Claims section in every day file (the Proposed draft's shape).** Rejected on review — claims are global, so per-day duplication renders ~window identical copies, breaks §1.4 change-detection (one claims change dirties every file), and leaves "which day's claims" undefined. Two file classes match the data's actual shape.

## Known Follow-ups

1. **Implementing PR** (#9465): `MemoryVaultConfig` + Tier-13 consent + `MemoryVaultWriterPort` (§7.4) + `DigestExporter::claims_to_markdown` extraction + writer implementation (`maekon-analysis`) + Phase-3 erasure in **both** orchestrators + §4.4 regression guards (both paths) + §6.4 marker-guard tests + settings surface (`maekon-web`) + scheduler/IPC wiring (`src-tauri`). The §4/§6 tests are acceptance criteria, not suggestions.
2. **Cloud-detection table maintenance** — the §3.2 per-OS list will drift; treat additions as docs+const updates, not ADR amendments.
3. **HTTP export masking parity** — decide separately whether `GET /api/digests/daily/export` adopts the §5.2 floor.
4. **Art.20 full export** (#8056 residue) — the vault covers claims/digests; a complete portability export remains open.
5. **Retract-triggered prompt regeneration** — requires a cross-crate scheduler-trigger primitive that does not exist today; if wanted, design it as its own small seam rather than ad-hoc coupling (§5.3).

## Amendment History

- **2026-07-29 (#9465, 3-loop review: devils-advocate + implementer lens; all BLOCKING/IMPORTANT folded in):**
  1. **Two file classes** (§1.1, Alternative F) — claims moved out of day files into a single `claims.md`; day files are digest-body only. Resolves the global-claims/per-day-file contradiction (identical claims sections in ~91 files, all-files-dirty on one claims change).
  2. **Cycle redefined off the catch-up loop** (§7) — the Proposed "piggyback the daily-digest loop" was structurally unable to expire or revisit files (`daily_catchup_dates` is forward-only and short-circuits on existing digests); the cycle now reads digest rows via `DigestStorage` and owns fill/regen/expiry explicitly.
  3. **Port pinned** (§7.4) — `MemoryVaultWriterPort` signature, input ownership (writer fetches via injected core ports), ADR-032 placement pattern.
  4. **Erasure re-layered** (§4) — off the locked SQL body, into a shared Phase-3 called by **both** already-duplicated erase orchestrators (named), with per-orchestrator contract tests, surfaced (never swallowed) failures, and crash-recovery participation.
  5. **Marker guard** (§6.4) — generated-file header marker; files without it are never overwritten or deleted. Closes the Obsidian daily-notes collision (data loss in the headline use case).
  6. **Custom-path acknowledgement made unconditional** (§3.3) — detected/undetected split removed; detection demoted to warning-enrichment + ledger-gating, run once at acceptance and stored (`cloud_provider`).
  7. **Ledger record fully pinned** (§3.4) — coarse `destination` labels (never a path — erase-retained no-PII table), deterministic `record_id` watermark (`vault_mirror|<destination>|<date>`), `byte_count`/`recipient_count` semantics, non-fatal ledger-failure discipline.
  8. **Active-only selection made a writer contract obligation** (§5.1) with a no-`Superseded`/`Retracted`-text contract test; **sanitizer floor widened to the whole rendered document, post-render** (§5.2), covering digest narrative/highlights/timeline, not just claims.
  9. **Bound-violation semantics pinned** (§1.5) — unevaluable-gate no-op (no writes AND no deletes), never clamp; `data_dir()` fallibility folded into §2.3.
  10. Minors: ledger-row erasure survival stated (§4.6); "no additional disclosure" wording tightened to same-fields; "Export now" scope = full cycle (§7.5); detection cadence = once at acceptance, stored result is per-cycle truth (§3.2); retract-prompt wiring moved to Known Follow-up 5.
- **2026-07-29, second confirmation pass (implementer lens, 2 IMPORTANT + 1 MINOR on the amendment's own hash mechanism):**
  11. **Hash storage pinned** (§1.4) — the "config-state/SQLite" hedge replaced with a named `vault_mirror_state` SQLite table (`maekon-storage` migration, added to Scope), a member of the erasure `ALL_TABLES` sweep so hash state can never outlive the files it describes.
  12. **Missing-file self-healing restored** (§1.4/§7.1/§7.2) — the staleness condition is hash-absent OR hash-stale OR **file missing on disk**; a stored-hash match must never suppress recreating a deleted file (the property the old byte-compare design had implicitly).
  13. §1.1 day-file immutability wording aligned with §7.1 (digest rows are upserted — e.g. LLM-narrative backfill — so day files rewrite on rendered-content change, not "written once").

## Related Docs

- `docs/architecture/ADR-023-local-symbolic-memory-graph.md` — substrate, exporter, erasure history
- `docs/architecture/ADR-032-memory-graph-generation-input-contract.md` — the sibling boundary (pipeline-side disclosure)
- `docs/architecture/ADR-028-durable-task-lifecycle-boundary.md` §P3 — sanitizer-floor precedent
- `crates/maekon-core/src/models/daily_digest.rs` — `DigestExporter` (`to_markdown`, `to_markdown_with_claims`)
- `crates/maekon-core/src/ports/web_storage.rs` — `DigestStorage` (§7.1 read seam)
- `crates/maekon-storage/src/sqlite/maintenance/retention.rs` — `delete_all_data_inner` / `ALL_TABLES` (the layer §4 deliberately does NOT extend)
- `src-tauri/src/commands/consent.rs` / `crates/maekon-web/src/services/data_web_service.rs` — the two erase orchestrators §4.1 binds
- `crates/maekon-core/src/ports/egress_ledger.rs` — `EgressLedgerSink` (§3.4)
