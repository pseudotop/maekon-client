/**
 * ADR-033 memory vault mirror IPC wrappers (#9465).
 *
 * The vault is a ONE-WAY, regenerable mirror of the SQLite source of truth:
 * digest day files + the current Active claims, written as plain Markdown into
 * a folder the user owns. The product never reads vault file *content* back
 * (only the header marker, to avoid overwriting a file it did not generate), so
 * user edits to generated files are not merged and are overwritten on the next
 * cycle. The UI copy in `MemoryVaultSettings` has to say exactly that.
 *
 * Rust counterpart: `src-tauri/src/commands/vault.rs`.
 */

/** Coarse cloud-provider labels the §3.2 detector can return. */
export type VaultCloudProvider = 'icloud' | 'cloud_storage' | 'onedrive' | 'dropbox' | 'google_drive'

async function invokeDesktop<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import('@tauri-apps/api/core')
  return invoke<T>(cmd, args)
}

/** Mirrors the Rust `VaultCycleStats` (ADR-033 §7.1–§7.3). */
export interface VaultCycleStats {
  /**
   * Why the cycle was a no-op, when it was — e.g. `feature_disabled`,
   * `consent_missing`, `window_invalid`, `erase_in_progress`. A fail-closed
   * skip is a successful call with every counter zero, NOT an error, so the UI
   * must branch on this rather than reporting "exported" unconditionally.
   */
  readonly skipped_reason: string | null
  readonly day_files_written: number
  readonly claims_file_written: boolean
  readonly files_expired: number
  /**
   * Files matching the mirror's naming pattern that were skipped because they
   * lack the product header marker (§6.4 collision guard) — the user's own
   * notes. Surfaced so the user knows why a file did not appear.
   */
  readonly conflicts: number
  /**
   * Vault-relative names of those conflicts, capped (so it can be shorter than
   * `conflicts`). Names only — the writer never reads a conflicting file's
   * content back, so nothing here carries user text.
   */
  readonly conflict_paths: readonly string[]
  readonly bytes_written: number
  readonly cloud_ledger_recorded: boolean
}

/**
 * Mirrors the Rust `VaultLastCycleSummary` — the PERSISTED summary of the last
 * cycle that ran (#9522).
 *
 * `VaultCycleStats` describes one invocation, so before this the §6.4 marker
 * conflicts of a **scheduled** cycle were never visible: only pressing "Export
 * now" could ever reveal them. This is read back with the settings, so the
 * settings screen reports the last cycle whoever (or whatever) ran it.
 */
export interface VaultLastCycleSummary {
  /** Epoch SECONDS the recorded cycle was anchored at (not milliseconds). */
  readonly finished_at: number
  readonly day_files_written: number
  readonly files_expired: number
  /** Total §6.4 conflicts — may exceed `conflict_paths.length`. */
  readonly conflicts: number
  /** Capped vault-relative names of those conflicts (never content). */
  readonly conflict_paths: readonly string[]
}

/** Mirrors the Rust `VaultMirrorSettings` (ADR-033 §3). */
export interface VaultMirrorSettings {
  /**
   * `analysis.memory_vault.enabled`. Not sufficient on its own — the writer
   * also requires Tier-13 `memory_vault_mirror` consent (§2.3).
   */
  readonly enabled: boolean
  /** The root the writer would mirror into right now (§3.1/§3.3). */
  readonly active_path: string | null
  /** App-owned default root (`<data dir>/vault`). */
  readonly default_path: string | null
  /** Configured custom root, acknowledged or not. */
  readonly custom_path: string | null
  /** §3.3: the acknowledgement flow was completed for `custom_path`. */
  readonly custom_path_acknowledged: boolean
  /** §3.2 stored provider label; `null` = nothing recognized. */
  readonly cloud_provider: string | null
  readonly mirror_window_days: number
  /**
   * §1.5: false means the window violates its bound and EVERY cycle is a
   * complete no-op (no writes and no deletes).
   */
  readonly window_within_bound: boolean
  /**
   * §6.4 status: the last cycle that ran, scheduled or manual. `null` = none
   * has (fresh install, or everything was erased) — render that as "not run
   * yet", never as a zero-count cycle.
   */
  readonly last_cycle: VaultLastCycleSummary | null
}

/** Mirrors the Rust `VaultMirrorPathOutcome`. */
export interface VaultMirrorPathOutcome {
  readonly cloud_provider: string | null
  /**
   * `false` is the §3.3 rejection — config was NOT written. The caller must
   * show the overwrite + sync warning and re-submit with `acknowledged: true`.
   */
  readonly applied: boolean
  readonly settings: VaultMirrorSettings
}

/** Reads the current vault mirror settings. */
export function getVaultMirrorSettings(): Promise<VaultMirrorSettings> {
  return invokeDesktop<VaultMirrorSettings>('get_vault_mirror_settings')
}

/**
 * Runs ONE full mirror cycle — "Export now" (ADR-033 §7.5).
 *
 * Deliberately a full cycle (day-file fill + claims regen + expiry sweep), not
 * a today-only one-shot export.
 */
export function runVaultMirrorCycle(): Promise<VaultCycleStats> {
  return invokeDesktop<VaultCycleStats>('run_vault_mirror_cycle')
}

/**
 * Proposes a custom vault path WITHOUT writing config (§3.3 step 1).
 *
 * Returns the §3.2 detection so the warning copy can name the provider. The
 * outcome's `applied` is always false here.
 */
export function previewVaultMirrorPath(path: string): Promise<VaultMirrorPathOutcome> {
  return invokeDesktop<VaultMirrorPathOutcome>('set_vault_mirror_path', {
    path,
    acknowledged: false,
  })
}

/**
 * Commits a custom vault path together with the §3.3 acknowledgement and the
 * §3.2 detection result (which the backend re-runs at acceptance time — the
 * frontend never supplies the provider label).
 */
export function acceptVaultMirrorPath(path: string): Promise<VaultMirrorPathOutcome> {
  return invokeDesktop<VaultMirrorPathOutcome>('set_vault_mirror_path', {
    path,
    acknowledged: true,
  })
}

/** Clears the custom path and reverts to the app-owned default root (§3.1). */
export function clearVaultMirrorPath(): Promise<VaultMirrorPathOutcome> {
  return invokeDesktop<VaultMirrorPathOutcome>('set_vault_mirror_path', {
    path: null,
    acknowledged: false,
  })
}

/**
 * Toggles `analysis.memory_vault.enabled` through the sanctioned WebView
 * config-patch chokepoint (`update_setting`), which applies the MDM-policy and
 * bounds checks every other settings write goes through.
 *
 * The custom-path triple is deliberately NOT reachable this way — those three
 * sub-paths are on `FORBIDDEN_ALLOWED_SUBPATHS`, so the §3.3 acknowledgement
 * gate cannot be flipped by a config patch. Use `acceptVaultMirrorPath`.
 */
export function setVaultMirrorEnabled(enabled: boolean): Promise<void> {
  return invokeDesktop<void>('update_setting', {
    configJson: JSON.stringify({ analysis: { memory_vault: { enabled } } }),
  })
}
