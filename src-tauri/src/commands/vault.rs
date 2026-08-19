//! Tauri IPC: ADR-033 memory vault mirror commands.
//!
//! `run_vault_mirror_cycle` is the "Export now" surface (ADR-033 §7.5): it
//! runs ONE full mirror cycle (§7.1–§7.3 — day-file fill, claims regen,
//! expiry sweep), not a today-only one-shot export. Internally fail-closed:
//! with the feature disabled or consent absent it returns a no-op stats
//! object with the reason, never an error.
//!
//! `get_vault_mirror_settings` / `set_vault_mirror_path` are the settings
//! surface (§3). The path triple (`custom_path`, `custom_path_acknowledged`,
//! `cloud_provider`) is writable ONLY through `set_vault_mirror_path`, which
//! owns the §3.2 detection and enforces the §3.3 acknowledgement; the generic
//! `update_setting` config-patch surface forbids those three sub-paths
//! (`commands::settings::FORBIDDEN_ALLOWED_SUBPATHS`) so the acknowledgement
//! gate cannot be flipped by a patch that skips the warning flow.
//! `analysis.memory_vault.enabled` and `mirror_window_days` stay on
//! `update_setting` — they carry no overwrite/egress risk of their own, and the
//! writer gates on Tier-13 consent regardless.

use std::path::{Path, PathBuf};

use maekon_core::models::memory_vault::VaultLastCycleSummary;
use maekon_core::ports::vault_mirror_state::VaultMirrorStatePort;
use serde::Serialize;
use tauri::command;

use crate::ipc_error::IpcError;
use crate::runtime_state::{AppState, ConfigRuntimeState};

/// ADR-033 §3 vault mirror settings, as the settings UI needs to render them.
///
/// Paths are absolute and user-chosen, so echoing them back to the user's own
/// settings screen discloses nothing they did not type (the same treatment
/// `audio.whisper_model_path` already gets). Nothing here reaches the egress
/// ledger — §3.4 records the coarse `cloud_provider` label only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VaultMirrorSettings {
    /// `analysis.memory_vault.enabled`. NOT sufficient on its own: the writer
    /// also requires Tier-13 `memory_vault_mirror` consent (§2.3), which the
    /// caller reads through `get_consent`.
    pub enabled: bool,
    /// The root the writer would mirror into right now — the acknowledged
    /// custom path when there is one, else the default (§3.1/§3.3). Mirrors
    /// `VaultMirrorWriter::active_root` so the UI cannot drift from the rule
    /// the writer actually applies. `None` = unresolvable (§2.3 no-op gate).
    pub active_path: Option<String>,
    /// App-owned default root (`<data_dir()>/vault`). `None` when `data_dir()`
    /// cannot be resolved.
    pub default_path: Option<String>,
    /// The configured custom root, acknowledged or not.
    pub custom_path: Option<String>,
    /// §3.3: the user completed the explicit acknowledgement flow. A custom
    /// path without this is inert — the mirror stays on the default location.
    pub custom_path_acknowledged: bool,
    /// §3.2 detection result stored at path-acceptance time (coarse provider
    /// label, never a path). `Some` means every writing cycle records a §3.4
    /// egress-ledger row.
    pub cloud_provider: Option<String>,
    /// §1.4 bounded mirror window in days.
    pub mirror_window_days: u32,
    /// Whether `mirror_window_days` satisfies the §1.5 bound
    /// (1..=`analysis.embedding.retention_days`). False means every cycle is a
    /// complete no-op — no writes AND no deletes — which the UI must surface
    /// rather than let the user believe the mirror is running.
    pub window_within_bound: bool,
    /// §6.4: the persisted summary of the last cycle that ran, or `None` when
    /// none has. Carries the marker conflicts of a **scheduled** cycle too —
    /// `run_vault_mirror_cycle`'s return value only ever described the cycle the
    /// user pressed a button for (#9522). Vault-relative names only, never
    /// content. `None` also when the storage runtime is not up yet (a status
    /// line must not be able to fail the whole settings read).
    pub last_cycle: Option<VaultLastCycleSummary>,
}

/// Outcome of a `set_vault_mirror_path` attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VaultMirrorPathOutcome {
    /// §3.2 detection against the canonicalized target. Populated for BOTH an
    /// applied write and an unacknowledged rejection — the §3.3 warning copy
    /// names the detected provider, so the caller needs it before it can ask
    /// for the acknowledgement.
    pub cloud_provider: Option<String>,
    /// Whether the path was persisted. `false` is the §3.3 rejection: the
    /// caller must present the overwrite + sync warning and re-submit with
    /// `acknowledged = true`. Config is untouched in that case.
    pub applied: bool,
    /// Settings after the call — unchanged when `applied` is false.
    pub settings: VaultMirrorSettings,
}

/// Run one full ADR-033 vault mirror cycle and return its stats.
#[command]
pub async fn run_vault_mirror_cycle(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<maekon_core::models::memory_vault::VaultCycleStats, IpcError> {
    use tauri::Manager;
    let Some(config_state) = app.try_state::<crate::runtime_state::ConfigRuntimeState>() else {
        return Err(IpcError::new(
            "internal.generic",
            "config runtime state unavailable",
        ));
    };
    // Stateless writer over the same shared SqliteStorage Arc (see
    // vault_wiring — instances interchangeable).
    let writer = crate::vault_wiring::build_vault_writer(
        state.storage.clone(),
        state.capture.consent_manager.clone(),
        config_state.config_manager().clone(),
    );
    writer
        .run_mirror_cycle(chrono::Utc::now().timestamp())
        .await
        .map_err(IpcError::from)
}

/// Read the ADR-033 §3 vault mirror settings plus the §6.4 last-cycle summary.
#[command]
pub async fn get_vault_mirror_settings(
    app: tauri::AppHandle,
    config_state: tauri::State<'_, ConfigRuntimeState>,
) -> Result<VaultMirrorSettings, IpcError> {
    let config = config_state.config_manager().get();
    Ok(build_settings(&config, read_last_cycle(&app).await))
}

/// Best-effort read of the persisted §6.4 last-cycle summary (#9522).
///
/// Degrades to `None` rather than failing the read: the settings screen also
/// carries the §3.3 path controls and the enable gate, and losing those over an
/// informational status line would be a worse failure than showing no status.
/// A read error is warn-logged so it is not silent.
async fn read_last_cycle(app: &tauri::AppHandle) -> Option<VaultLastCycleSummary> {
    use tauri::Manager;
    let state = app.try_state::<AppState>()?;
    match state.storage.last_cycle_summary().await {
        Ok(summary) => summary,
        Err(e) => {
            tracing::warn!(
                err.code = %e.code(),
                "vault settings: last-cycle summary read failed: {e}"
            );
            None
        }
    }
}

/// Set (or clear) the vault mirror custom path, enforcing the ADR-033 §3.3
/// acknowledgement gate.
///
/// - `path = None` clears the custom path and reverts to the app-owned default
///   (§3.1) — no acknowledgement needed, since the default location is the one
///   the contract defends absolutely.
/// - `path = Some(..)` with `acknowledged = false` performs §3.2 detection and
///   returns `applied = false` WITHOUT writing config, so the caller can render
///   the §3.3 warning naming the detected provider.
/// - `path = Some(..)` with `acknowledged = true` persists the path, the
///   acknowledgement, and the detection result together (§3.2: detection runs
///   once, at acceptance time, and the stored value is the per-cycle truth).
#[command]
pub async fn set_vault_mirror_path(
    app: tauri::AppHandle,
    config_state: tauri::State<'_, ConfigRuntimeState>,
    path: Option<String>,
    acknowledged: bool,
) -> Result<VaultMirrorPathOutcome, IpcError> {
    let config_manager = config_state.config_manager().clone();
    // The echoed `settings` must be the same shape the read command returns, or
    // a caller that trusts the outcome instead of re-reading would see the §6.4
    // status blank itself out on every path edit.
    let last_cycle = read_last_cycle(&app).await;

    // Clearing needs no detection and no acknowledgement (§3.1).
    let Some(raw) = path else {
        let config = tokio::task::spawn_blocking(move || {
            config_manager.update_with(|config| {
                let vault = &mut config.analysis.memory_vault;
                vault.custom_path = None;
                vault.custom_path_acknowledged = false;
                vault.cloud_provider = None;
                Ok(())
            })
        })
        .await
        .map_err(join_error)??;
        return Ok(VaultMirrorPathOutcome {
            cloud_provider: None,
            applied: true,
            settings: build_settings(&config, last_cycle),
        });
    };

    let target = validate_custom_path(&raw)?;

    // Canonicalize for detection (§3.2) and persist, both blocking.
    tokio::task::spawn_blocking(move || {
        apply_custom_path(
            &config_manager,
            &target,
            acknowledged,
            detect_provider_for_target,
            last_cycle,
        )
    })
    .await
    .map_err(join_error)?
}

/// The ADR-033 §3.3 acknowledgement gate, extracted so it is unit-testable
/// without a Tauri `State` (which cannot be constructed in unit tests).
///
/// `acknowledged = false` performs §3.2 detection and returns it, but writes
/// NOTHING — not config, and not a directory. A preview must not have side
/// effects, and an unacknowledged custom path must leave the mirror on the
/// default location.
///
/// `detect` is injected rather than called directly so tests can supply an
/// explicit home directory. The host detector reads `$HOME`, and mutating a
/// process-global env var from a test races every sibling test in the same
/// binary (`bootstrap_runtime.rs` carries an `env_lock()` for exactly this
/// reason) — injection removes the need for the lock instead of taking it.
fn apply_custom_path(
    config_manager: &maekon_core::config_manager::ConfigManager,
    target: &Path,
    acknowledged: bool,
    detect: impl Fn(&Path) -> Option<&'static str>,
    last_cycle: Option<VaultLastCycleSummary>,
) -> Result<VaultMirrorPathOutcome, IpcError> {
    let provider = detect(target);
    if !acknowledged {
        let config = config_manager.get();
        return Ok(VaultMirrorPathOutcome {
            cloud_provider: provider.map(str::to_string),
            applied: false,
            settings: build_settings(&config, last_cycle),
        });
    }
    let config = config_manager.update_with(|config| {
        let vault = &mut config.analysis.memory_vault;
        vault.custom_path = Some(target.to_path_buf());
        vault.custom_path_acknowledged = true;
        vault.cloud_provider = provider.map(str::to_string);
        Ok(())
    })?;
    Ok(VaultMirrorPathOutcome {
        cloud_provider: provider.map(str::to_string),
        applied: true,
        settings: build_settings(&config, last_cycle),
    })
}

fn join_error(join_err: tokio::task::JoinError) -> IpcError {
    IpcError::new(
        "internal.generic",
        format!("vault settings task join failed: {join_err}"),
    )
}

/// Reject a custom path that cannot be a vault root before any detection or
/// write happens. A relative path would resolve against the agent's working
/// directory rather than anything the user meant.
fn validate_custom_path(raw: &str) -> Result<PathBuf, IpcError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(IpcError::new(
            "validation.invalid_field",
            "vault path must not be empty",
        ));
    }
    let path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        return Err(IpcError::new(
            "validation.invalid_field",
            "vault path must be absolute",
        ));
    }
    Ok(path)
}

/// §3.2 detection against the canonicalized target.
///
/// The target may not exist yet (a new subfolder inside the user's vault), and
/// `canonicalize` requires existence — so resolve the deepest EXISTING ancestor
/// and re-attach the remainder. That keeps symlink resolution (the property
/// canonicalization is here for) while never creating a directory as a side
/// effect of detection. A target under `~/Dropbox/new-subdir` is still under
/// Dropbox whether or not `new-subdir` exists yet.
fn detect_provider_for_target(target: &Path) -> Option<&'static str> {
    maekon_core::vault_cloud_sync::detect_cloud_provider(&resolve_existing_prefix(target))
}

fn resolve_existing_prefix(target: &Path) -> PathBuf {
    let mut remainder: Vec<&std::ffi::OsStr> = Vec::new();
    let mut cursor = target;
    loop {
        if let Ok(canonical) = cursor.canonicalize() {
            let mut resolved = canonical;
            for part in remainder.iter().rev() {
                resolved.push(part);
            }
            return resolved;
        }
        match (cursor.file_name(), cursor.parent()) {
            (Some(name), Some(parent)) => {
                remainder.push(name);
                cursor = parent;
            }
            // Nothing on this path exists (or we ran out of ancestors) — fall
            // back to the literal target rather than claiming "not synced".
            _ => return target.to_path_buf(),
        }
    }
}

fn build_settings(
    config: &maekon_core::config::AppConfig,
    last_cycle: Option<VaultLastCycleSummary>,
) -> VaultMirrorSettings {
    let vault = &config.analysis.memory_vault;
    let default_path = maekon_core::config_manager::ConfigManager::data_dir()
        .map(|dir| dir.join("vault"))
        .ok();
    // Mirrors `VaultMirrorWriter::active_root`: an unacknowledged custom path
    // is rejected and the mirror stays on the default location (§3.3).
    let active_path = match (&vault.custom_path, vault.custom_path_acknowledged) {
        (Some(path), true) => Some(path.clone()),
        _ => default_path.clone(),
    };
    let retention_days = config.analysis.embedding.retention_days;
    VaultMirrorSettings {
        enabled: vault.enabled,
        active_path: active_path.map(display_path),
        default_path: default_path.map(display_path),
        custom_path: vault.custom_path.clone().map(display_path),
        custom_path_acknowledged: vault.custom_path_acknowledged,
        cloud_provider: vault.cloud_provider.clone(),
        mirror_window_days: vault.mirror_window_days,
        window_within_bound: vault.mirror_window_days >= 1
            && vault.mirror_window_days <= retention_days,
        last_cycle,
    }
}

fn display_path(path: PathBuf) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::config::AppConfig;
    use maekon_core::config_manager::ConfigManager;

    fn test_config_manager(dir: &tempfile::TempDir) -> ConfigManager {
        ConfigManager::with_path(dir.path().join("config.json")).expect("config manager")
    }

    /// Detector stand-in that never touches the process environment.
    ///
    /// The production detector resolves `$HOME`; a test that set it would race
    /// every sibling test in this binary. Injecting an explicit home keeps the
    /// per-OS table under test in `maekon_core::vault_cloud_sync` (where it is
    /// covered directly) and keeps THESE tests about the gate.
    fn detect_with_home(home: PathBuf) -> impl Fn(&Path) -> Option<&'static str> {
        move |target: &Path| {
            maekon_core::vault_cloud_sync::detect_cloud_provider_with(
                &resolve_existing_prefix(target),
                Some(&home),
                &[],
            )
        }
    }

    /// Detector stand-in for the gate tests that do not exercise detection.
    fn detect_none(_target: &Path) -> Option<&'static str> {
        None
    }

    // ADR-033 §3.3, the backend half of the gate. The frontend disables its
    // confirm control until the box is ticked, but the write must be refused
    // here too — a caller that skips the UI (or a future control that loses its
    // `disabled` prop) must not be able to point the mirror at an arbitrary
    // folder.
    #[test]
    fn unacknowledged_custom_path_writes_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let manager = test_config_manager(&dir);
        let target = dir.path().join("some-vault");

        let outcome = apply_custom_path(&manager, &target, false, detect_none, None)
            .expect("preview is a successful no-write");

        assert!(
            !outcome.applied,
            "an unacknowledged path must not be applied"
        );
        let stored = manager.get().analysis.memory_vault;
        assert_eq!(stored.custom_path, None, "config must be untouched");
        assert!(!stored.custom_path_acknowledged);
        assert_eq!(stored.cloud_provider, None);
    }

    #[test]
    fn acknowledged_custom_path_persists_the_path_and_the_acknowledgement() {
        let dir = tempfile::tempdir().expect("tempdir");
        let manager = test_config_manager(&dir);
        let target = dir.path().join("some-vault");

        let outcome = apply_custom_path(&manager, &target, true, detect_none, None)
            .expect("acknowledged write");

        assert!(outcome.applied);
        let stored = manager.get().analysis.memory_vault;
        assert_eq!(stored.custom_path.as_deref(), Some(target.as_path()));
        assert!(stored.custom_path_acknowledged);
    }

    #[test]
    fn preview_does_not_create_the_target_directory() {
        // A detection probe that created folders would litter the user's disk
        // on every keystroke-driven preview.
        let dir = tempfile::tempdir().expect("tempdir");
        let manager = test_config_manager(&dir);
        let target = dir.path().join("not-created").join("vault");

        apply_custom_path(&manager, &target, false, detect_none, None).expect("preview");

        assert!(!target.exists(), "preview must not create {target:?}");
        assert!(!dir.path().join("not-created").exists());
    }

    #[test]
    fn acknowledged_write_stores_the_detected_provider_not_a_caller_supplied_one() {
        // §3.2: detection runs at acceptance time inside this function and its
        // result is what gets persisted — the caller supplies only the path, so
        // the stored label can never be spoofed from the frontend.
        let home = tempfile::tempdir().expect("tempdir");
        let dir = tempfile::tempdir().expect("tempdir");
        let manager = test_config_manager(&dir);
        let dropbox = home.path().join("Dropbox");
        std::fs::create_dir_all(&dropbox).expect("create Dropbox root");
        let canonical_home = home.path().canonicalize().expect("canonical home");

        let outcome = apply_custom_path(
            &manager,
            &dropbox.join("vault"),
            true,
            detect_with_home(canonical_home),
            None,
        )
        .expect("acknowledged write");
        assert_eq!(outcome.cloud_provider.as_deref(), Some("dropbox"));
        assert_eq!(
            manager
                .get()
                .analysis
                .memory_vault
                .cloud_provider
                .as_deref(),
            Some("dropbox"),
            "the detection result is what gates the §3.4 ledger record"
        );
    }

    #[test]
    fn empty_and_relative_custom_paths_are_rejected_with_a_field_code() {
        let empty = validate_custom_path("   ").expect_err("empty path must be rejected");
        assert_eq!(empty.code, "validation.invalid_field");
        let relative =
            validate_custom_path("relative/vault").expect_err("relative path must be rejected");
        assert_eq!(relative.code, "validation.invalid_field");
    }

    #[test]
    fn absolute_custom_path_is_trimmed_and_accepted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let expected = dir.path().join("vault");
        let input = format!("  {}  ", expected.display());
        let path = validate_custom_path(&input).expect("absolute path accepted");
        assert_eq!(path, expected);
    }

    fn recorded_cycle() -> VaultLastCycleSummary {
        VaultLastCycleSummary {
            finished_at: 1_753_000_000,
            day_files_written: 4,
            files_expired: 1,
            conflicts: 2,
            conflict_paths: vec![
                "daily/2026-07-28.md".to_string(),
                "daily/2026-07-29.md".to_string(),
            ],
        }
    }

    #[test]
    fn the_settings_payload_carries_the_persisted_last_cycle_summary() {
        // #9522: this field IS the §6.4 visibility path for a SCHEDULED cycle —
        // the settings screen has no other source for its conflicts.
        let settings = build_settings(&AppConfig::default_config(), Some(recorded_cycle()));
        let last = settings.last_cycle.expect("summary must reach the UI");
        assert_eq!(last.conflicts, 2);
        assert_eq!(
            last.conflict_paths,
            vec!["daily/2026-07-28.md", "daily/2026-07-29.md"]
        );
        assert_eq!(last.day_files_written, 4);
        assert_eq!(last.finished_at, 1_753_000_000);
    }

    #[test]
    fn a_path_write_echoes_the_last_cycle_summary_rather_than_blanking_it() {
        // The outcome's `settings` is the same type the read command returns; a
        // path edit does not run a cycle, so it must carry the summary through
        // instead of reporting "no cycle has ever run".
        let dir = tempfile::tempdir().expect("tempdir");
        let manager = test_config_manager(&dir);
        let outcome = apply_custom_path(
            &manager,
            &dir.path().join("some-vault"),
            true,
            detect_none,
            Some(recorded_cycle()),
        )
        .expect("acknowledged write");
        assert_eq!(
            outcome.settings.last_cycle.map(|c| c.conflicts),
            Some(2),
            "the §6.4 status must survive a §3.3 path write"
        );
    }

    #[test]
    fn no_recorded_cycle_is_reported_as_absent_not_as_an_empty_cycle() {
        // A fresh install (or a post-Art.17 wipe) must render "not run yet",
        // never a zero-count cycle that claims the mirror already ran.
        assert_eq!(
            build_settings(&AppConfig::default_config(), None).last_cycle,
            None
        );
    }

    #[test]
    fn default_config_reports_default_root_as_active_and_no_acknowledgement() {
        let config = AppConfig::default_config();
        let settings = build_settings(&config, None);
        assert!(!settings.enabled, "ADR-033 §2.2 default is off");
        assert!(!settings.custom_path_acknowledged);
        assert_eq!(settings.custom_path, None);
        assert_eq!(settings.cloud_provider, None);
        assert_eq!(
            settings.active_path, settings.default_path,
            "with no acknowledged custom path the active root IS the default root"
        );
        assert_eq!(settings.mirror_window_days, 90);
    }

    #[test]
    fn unacknowledged_custom_path_does_not_become_the_active_root() {
        // §3.3: the load-bearing clause. The writer's `active_root` falls back
        // to the default here, and the UI must report the same thing.
        let mut config = AppConfig::default_config();
        config.analysis.memory_vault.custom_path = Some(PathBuf::from("/srv/notes/vault"));
        config.analysis.memory_vault.custom_path_acknowledged = false;
        let settings = build_settings(&config, None);
        assert_eq!(
            settings.custom_path.as_deref(),
            Some("/srv/notes/vault"),
            "the configured path is still reported so the UI can show it as pending"
        );
        assert_eq!(settings.active_path, settings.default_path);
    }

    #[test]
    fn acknowledged_custom_path_becomes_the_active_root() {
        let mut config = AppConfig::default_config();
        config.analysis.memory_vault.custom_path = Some(PathBuf::from("/srv/notes/vault"));
        config.analysis.memory_vault.custom_path_acknowledged = true;
        let settings = build_settings(&config, None);
        assert_eq!(settings.active_path.as_deref(), Some("/srv/notes/vault"));
        assert_ne!(settings.active_path, settings.default_path);
    }

    #[test]
    fn window_bound_violation_is_reported_rather_than_clamped() {
        // §1.5: a violation makes every cycle a complete no-op. The value is
        // reported verbatim — never clamped — and flagged out-of-bound.
        let mut config = AppConfig::default_config();
        config.analysis.embedding.retention_days = 30;
        config.analysis.memory_vault.mirror_window_days = 90;
        let settings = build_settings(&config, None);
        assert_eq!(settings.mirror_window_days, 90);
        assert!(!settings.window_within_bound);

        config.analysis.memory_vault.mirror_window_days = 0;
        assert!(!build_settings(&config, None).window_within_bound);

        config.analysis.memory_vault.mirror_window_days = 30;
        assert!(build_settings(&config, None).window_within_bound);
    }

    #[test]
    fn cloud_provider_is_detected_for_a_target_whose_leaf_does_not_exist_yet() {
        // The "point at a new subfolder of my Obsidian vault" case: the leaf
        // does not exist, so a plain `canonicalize` would fail and the
        // detection would silently report "not synced".
        let home = tempfile::tempdir().expect("tempdir");
        let dropbox = home.path().join("Dropbox");
        std::fs::create_dir_all(&dropbox).expect("create Dropbox root");
        let target = dropbox.join("not-created-yet").join("vault");

        let resolved = resolve_existing_prefix(&target);
        assert!(
            resolved.ends_with("not-created-yet/vault"),
            "the non-existent remainder must be preserved: {resolved:?}"
        );
        assert_eq!(
            maekon_core::vault_cloud_sync::detect_cloud_provider_with(
                &resolved,
                Some(&home.path().canonicalize().expect("canonical home")),
                &[],
            ),
            Some("dropbox")
        );
    }

    #[cfg(unix)]
    #[test]
    fn resolve_existing_prefix_resolves_symlinks_on_the_existing_ancestor() {
        // Canonicalization is here precisely so a symlink into a synced folder
        // cannot hide behind an unresolved path.
        let base = tempfile::tempdir().expect("tempdir");
        let real = base.path().join("real-dropbox");
        std::fs::create_dir_all(&real).expect("create real dir");
        let link = base.path().join("link");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");

        let resolved = resolve_existing_prefix(&link.join("vault"));
        assert!(
            resolved.starts_with(real.canonicalize().expect("canonical real")),
            "symlink must resolve to the real directory: {resolved:?}"
        );
    }
}
