//! Tauri IPC: production consent read/write (GDPR).
//!
//! Provides 3 IPC commands so the Phase A.2 UI toggle grants consent and makes it
//! immediately observable by the scheduler through `Arc<dyn ConsentManagerPort>`.
//!
//! # GDPR Art. 17 local data erasure (#4801)
//!
//! `withdraw_consent` erases local data in 2 phases after the revoke persists:
//! - Phase-1: atomic deletion of all SQLite tables (`delete_all_data`).
//! - Phase-2: deletion of frame image files (`delete_all_frames`).
//!
//! If Phase-2 fails, a `pending_local_erase=1` marker is written to `app_meta` so
//! that on the next app launch `retry_pending_local_erase` retries it (R2/R3).
//!
//! Remote propagation (DeletionEvent) is delegated to its sole owner, sync_engine (R4).
use std::sync::Arc;

use maekon_core::consent::{ConsentPermissions, ConsentStatus};
use maekon_core::models::audit::{AuditEntry, AuditStatus};
use maekon_core::ports::consent_manager::ConsentManagerPort;
use maekon_core::ports::frame_storage::FrameStoragePort;
use maekon_storage::sqlite::SqliteStorage;
use serde::{Deserialize, Serialize};
use tauri::command;

use crate::ipc_error::IpcError;
use crate::runtime_state::AppState;

/// Consent retention policy in days (matches the storage retention policy;
/// `expires_at` stays None).
const RETENTION_DAYS: u32 = 30;

/// #4686: `app_meta` key recording whether the one-time microphone upgrade notice
/// has already been shown.
const MIC_UPGRADE_NOTICE_FLAG: &str = "microphone_split_notice_shown";

/// #4801: `app_meta` key that signals, even across restarts, that frame-file
/// deletion did not complete.
///
/// On Phase-2 (frame deletion) failure this marker is written via `set_meta_checked`.
/// On the next app launch `retry_pending_local_erase` detects the marker and retries
/// the deletion. Once both phases succeed, the marker is cleared via `delete_meta_checked`.
pub(crate) const PENDING_LOCAL_ERASE_KEY: &str = "pending_local_erase";

/// Snapshot DTO of the current consent state.
///
/// Delivers the status (`status`) and the permission set (`permissions`) to the
/// frontend in one shot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsentSnapshot {
    /// Validity status (Valid / NotGranted / Expired / UpdateRequired).
    pub status: ConsentStatus,
    /// Currently granted permission set.
    pub permissions: ConsentPermissions,
}

// ---------------------------------------------------------------------------
// Pure helpers — unit logic testable without AppState
// ---------------------------------------------------------------------------

/// Reads the current state from `ConsentManager` and returns a `ConsentSnapshot`.
///
/// `status_and_permissions()` reads the status and permissions together within a
/// **single read guard**, so there is no TOCTOU window where the two values point
/// to different moments (F5). The permissions are the **raw granted permissions**,
/// not masked to zero even in a non-Valid status — because the UI must show "what
/// was granted" alongside the status (e.g., Expired).
pub(crate) fn read_consent_snapshot(cm: &dyn ConsentManagerPort) -> ConsentSnapshot {
    let (status, permissions) = cm.status_and_permissions();
    ConsentSnapshot {
        status,
        permissions,
    }
}

/// #4686: pure function that decides whether the one-time microphone upgrade notice
/// should be shown.
///
/// Returns `true` if `audio.enabled` (audio capture ON) but the `microphone` consent
/// is not yet present (default OFF since the #4568 split) and the notice has not been
/// shown yet. Unit-testable without AppState.
fn should_show_microphone_upgrade_notice(
    audio_enabled: bool,
    microphone_granted: bool,
    already_shown: bool,
) -> bool {
    audio_enabled && !microphone_granted && !already_shown
}

/// Records an audit log entry into SQLite.
///
/// - `action`: `"consent_granted"` or `"consent_revoked"`
/// - `consent_id`: ConsentRecord.consent_id on grant; empty string on revoke.
fn audit_consent(
    storage: &SqliteStorage,
    action: &str,
    perms: &ConsentPermissions,
    consent_id: &str,
) {
    // Record the resulting permission set, policy version, and consent_id as JSON
    // in the details field.
    let details = serde_json::json!({
        "permissions": perms,
        "version": maekon_core::consent::CURRENT_POLICY_VERSION,
        "consent_id": consent_id,
    });

    storage.save_audit_entry(&AuditEntry {
        entry_id: maekon_core::id_generation::generate_id("audit"),
        timestamp: chrono::Utc::now(),
        // #4685: a consent change is a system-level event not tied to any tracking
        // session or automation command. Use a distinct "system.consent" sentinel
        // (not the bare "consent" string, which collided with per-session audit
        // correlation by reusing the same value for both session_id and command_id).
        session_id: "system.consent".into(),
        command_id: "system.consent".into(),
        action_type: action.into(),
        // Treated as success — there is no Granted/Revoked variant, so use Completed.
        status: AuditStatus::Completed,
        details: Some(details.to_string()),
        execution_time_ms: None,
    });
}

/// Grants consent + records the audit entry and returns the resulting snapshot.
///
/// A `grant_consent` file-I/O failure is converted to `IpcError` and propagated to
/// the caller. The audit log is recorded only after the file write succeeds — a
/// failed grant is not audited.
pub(crate) fn apply_set_consent(
    cm: &dyn ConsentManagerPort,
    storage: &SqliteStorage,
    permissions: ConsentPermissions,
) -> Result<ConsentSnapshot, IpcError> {
    // On file-I/O failure, return Err immediately (GDPR compliance: do not return Ok
    // before persisting).
    cm.grant_consent(permissions.clone(), RETENTION_DAYS)
        .map_err(IpcError::from)?;
    let consent_id = cm
        .current_consent()
        .map(|r| r.consent_id)
        .unwrap_or_default();
    // Record the audit only after persisting succeeds.
    audit_consent(storage, "consent_granted", &permissions, &consent_id);
    Ok(read_consent_snapshot(cm))
}

/// Revokes consent + records the audit entry and returns the resulting snapshot.
///
/// A `revoke_consent` file-I/O failure is converted to `IpcError` and propagated to
/// the caller. Returning Ok to the UI on a revoke failure would violate GDPR Art.
/// 7§3 (right to withdraw), so it must be propagated. The audit log is recorded only
/// after the revoke succeeds.
pub(crate) fn apply_withdraw_consent(
    cm: &dyn ConsentManagerPort,
    storage: &SqliteStorage,
) -> Result<ConsentSnapshot, IpcError> {
    // Capture the permission set and consent_id before revoking so the audit log
    // records "what was revoked" (GDPR Art. 7§1 demonstrability — the revoked
    // permissions must be reconstructable from the revoke event alone).
    let prior = cm.current_consent();
    let prior_permissions = prior
        .as_ref()
        .map(|r| r.permissions.clone())
        .unwrap_or_default();
    let prior_consent_id = prior.map(|r| r.consent_id).unwrap_or_default();
    // On file-I/O failure, return Err immediately (hiding a revoke-persist failure
    // behind Ok is a GDPR violation).
    cm.revoke_consent().map_err(IpcError::from)?;
    // Record the audit only after persisting succeeds. Record the revoked (prior)
    // permission set — not the all-false default.
    audit_consent(
        storage,
        "consent_revoked",
        &prior_permissions,
        &prior_consent_id,
    );
    Ok(read_consent_snapshot(cm))
}

/// #5056: consent-change → telemetry exporter re-apply (consent→exporter bridge).
///
/// After a consent grant/revoke succeeds, re-evaluate the consent gate and push
/// the result to the telemetry `Handle` immediately, rather than waiting for the
/// next config change. This is what makes a telemetry-consent revoke shut the
/// OTLP exporter down at once, and a grant (with config already enabled) start
/// it.
///
/// Reads the live state through `AppHandle`:
///   - `Arc<telemetry::Handle>` (managed in `main.rs`),
///   - `ConfigRuntimeState` → current `config.telemetry` (the raw user setting),
///   - `AppState.capture.consent_manager` → the live consent record.
///
/// Fail-closed by construction: the gate is computed via
/// `consent_gated_telemetry_config`, which reads consent through the fail-closed
/// `effective_permissions()` accessor — any non-Valid status (absent / revoked /
/// expired) collapses the telemetry term to false ⇒ exporter OFF.
///
/// Best-effort: any missing managed state or `Handle::apply` error is logged and
/// swallowed — a telemetry re-apply failure MUST NOT fail the consent write
/// (GDPR Art. 7§3 / Art. 6: the consent change itself already persisted). Under
/// the no-op telemetry shim (`--no-default-features`) `apply` is an infallible
/// no-op, so this is a cheap, safe call there too.
pub(crate) fn reapply_telemetry_gate(app: &tauri::AppHandle) {
    use tauri::Manager;

    // Telemetry handle — managed in main.rs. Absent only in degraded builds.
    let Some(handle) = app.try_state::<Arc<crate::telemetry::Handle>>() else {
        tracing::warn!("telemetry gate re-apply skipped: no telemetry Handle in managed state");
        return;
    };

    // Live raw telemetry config from the ConfigRuntimeState.
    let Some(config_state) = app.try_state::<crate::runtime_state::ConfigRuntimeState>() else {
        tracing::warn!("telemetry gate re-apply skipped: no ConfigRuntimeState in managed state");
        return;
    };
    let telemetry_config = config_state.config_manager().get().telemetry;

    // Live consent manager from AppState's capture context.
    let Some(app_state) = app.try_state::<AppState>() else {
        tracing::warn!("telemetry gate re-apply skipped: no AppState in managed state");
        return;
    };
    let Some(consent_manager) = app_state.capture.consent_manager.as_ref() else {
        // No consent wired → fail-closed: drive the exporter OFF explicitly so a
        // stale enabled state cannot linger.
        tracing::warn!(
            "telemetry gate re-apply: no ConsentManager wired — forcing exporter OFF (fail-closed)"
        );
        let off = maekon_core::config::TelemetryConfig {
            enabled: false,
            ..telemetry_config
        };
        if let Err(e) = handle.apply(&off) {
            tracing::warn!(error = %e, "telemetry gate re-apply (forced OFF) failed");
        }
        return;
    };

    // Compute the consent-gated config and apply it. apply() is idempotent when
    // the gated config matches the last-applied value.
    let gated = crate::telemetry::consent_gated_telemetry_config(
        &telemetry_config,
        consent_manager.as_ref(),
    );
    if let Err(e) = handle.apply(&gated) {
        tracing::warn!(error = %e, "telemetry gate re-apply failed");
    } else {
        tracing::debug!(
            telemetry_enabled = gated.enabled,
            "telemetry gate re-applied after consent change"
        );
    }
}

/// Extracts the `Arc<dyn ConsentManagerPort>` from `AppState`.
///
/// Returns a `service.unavailable` IpcError if `capture.consent_manager` is None.
fn consent_mgr(state: &AppState) -> Result<&Arc<dyn ConsentManagerPort>, IpcError> {
    state
        .capture
        .consent_manager
        .as_ref()
        .ok_or_else(|| IpcError::new("service.unavailable", "consent manager not available"))
}

// ---------------------------------------------------------------------------
// Tauri IPC commands
// ---------------------------------------------------------------------------

/// Reads and returns the current consent state.
///
/// Only reads, no writes, so a `&self` ConsentManager is sufficient.
#[command]
pub async fn get_consent(state: tauri::State<'_, AppState>) -> Result<ConsentSnapshot, IpcError> {
    Ok(read_consent_snapshot(consent_mgr(&state)?.as_ref()))
}

/// Grants consent with the given permission set, records the audit log, and returns
/// the resulting snapshot.
///
/// `apply_set_consent` is a **synchronous** helper that runs `grant_consent`
/// (blocking `std::fs::write`) + `save_audit_entry` (blocking SQLite), so it is
/// moved onto `spawn_blocking` to avoid starving the async worker pool (F-RR-06
/// async-safety).
///
/// Immediately after the consent write, fire the VAD re-gate signal (F-MIC-2,
/// #4568): revoking/narrowing `microphone` makes a running VAD listener drop the
/// mic at once rather than waiting for the ≤2 s backstop tick. Use the **bare
/// `signal_vad_regate`** — `on_capture_pause_toggled` auto-rearms VAD on the unpause
/// edge, so this avoids a consent-to-surveillance escalation where the mic would
/// auto-start on a consent *grant*. Fire it unconditionally on any consent write
/// (if the gate is still open the receiver is a no-op — idempotent). The signal is
/// a non-blocking `notify_one` fired after the blocking guard is released.
#[command]
pub async fn set_consent(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    permissions: ConsentPermissions,
) -> Result<ConsentSnapshot, IpcError> {
    // Clone the Arc to move ownership into a 'static closure (a State borrow cannot
    // cross an await).
    let cm = consent_mgr(&state)?.clone();
    let storage = state.storage.clone();
    let snapshot =
        tokio::task::spawn_blocking(move || apply_set_consent(cm.as_ref(), &storage, permissions))
            .await
            .map_err(|join_err| {
                IpcError::new(
                    "internal.generic",
                    format!("set_consent task join failed: {join_err}"),
                )
            })??;
    // After persisting succeeds (after the blocking guard is released), fire the VAD
    // re-gate signal.
    crate::commands::audio::signal_vad_regate(&app);
    // #5056: re-evaluate the consent gate on the telemetry exporter. Granting
    // telemetry consent (with config already enabled) starts the exporter
    // immediately; any other grant is a no-op re-apply. Best-effort — never
    // fails the consent write.
    reapply_telemetry_gate(&app);
    Ok(snapshot)
}

/// Revokes consent, records the audit log, and returns the resulting snapshot.
///
/// `apply_withdraw_consent` is a **synchronous** helper that runs `revoke_consent`
/// (blocking write+rename, holding the write guard across the operation) +
/// `save_audit_entry` (blocking SQLite), so it is moved onto `spawn_blocking` to
/// avoid starving the async worker pool (F-RR-06 async-safety). The revoke's
/// single-guard atomicity design is preserved inside the helper (only the pool is
/// separated here).
///
/// # GDPR Art. 17 local erasure (#4801, Decision A: full erasure)
///
/// Immediately after the revoke persist succeeds, erase local data in 2 phases:
/// - Phase-1: atomic deletion of all SQLite tables. Returns Err on failure (R5).
/// - Phase-2: deletion of frame image files. On failure, write a retry marker +
///   report partial status (R2/R3).
///
/// Remote DeletionEvent propagation is delegated to sync_engine (R4: pending_deletion=true is kept).
///
/// Immediately after the revoke persists, fire the VAD re-gate signal (F-MIC-2,
/// #4568) so a running VAD listener drops the mic at once. As in `set_consent`, fire
/// the bare `signal_vad_regate` after the blocking guard is released.
#[command]
pub async fn withdraw_consent(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<ConsentSnapshot, IpcError> {
    let cm = consent_mgr(&state)?.clone();
    let storage = state.storage.clone();

    // Step 1: revoke persist + audit log (blocking I/O → spawn_blocking).
    let snapshot =
        tokio::task::spawn_blocking(move || apply_withdraw_consent(cm.as_ref(), &storage))
            .await
            .map_err(|join_err| {
                IpcError::new(
                    "internal.generic",
                    format!("withdraw_consent task join failed: {join_err}"),
                )
            })??;

    // After the revoke persist succeeds, fire the VAD re-gate signal (F-MIC-2).
    // Firing the signal has the effect of immediately blocking new captures
    // (is_permitted = false).
    crate::commands::audio::signal_vad_regate(&app);
    // #5056: re-evaluate the consent gate on the telemetry exporter. Revoking
    // consent zeroes `effective_permissions().telemetry`, so this drives the
    // OTLP exporter OFF immediately (fail-closed) rather than waiting for the
    // next config change. Best-effort — never fails the revoke.
    reapply_telemetry_gate(&app);

    // Step 2: GDPR Art. 17 local data erasure (R5: cannot return Ok on failure).
    let frame_storage = state.capture.frame_storage.clone();
    let storage = state.storage.clone();
    erase_all_local_data(storage, frame_storage).await?;

    Ok(snapshot)
}

/// #4928 round-3 (FIX B): RAII guard that holds the `erasing` signal for the
/// duration of the erase window.
///
/// On `set` it makes the shared `erasing` Arc `true`, and on `Drop` it sets it back
/// to `false`. This guarantees the signal is cleared on every exit path of erase
/// (Phase-1 Err, Phase-2 Err, normal completion, panic-unwind) — there is no risk
/// of forgetting to clear it manually only on the happy path. Since `grant_consent`
/// cannot touch `erasing`, no write resumes while this guard is alive even if a
/// re-grant comes in.
struct EraseWindowGuard {
    erasing: Arc<std::sync::atomic::AtomicBool>,
}

impl EraseWindowGuard {
    fn set(erasing: Arc<std::sync::atomic::AtomicBool>) -> Self {
        erasing.store(true, std::sync::atomic::Ordering::Release);
        Self { erasing }
    }
}

impl Drop for EraseWindowGuard {
    fn drop(&mut self) {
        // Close the erase window on every exit path (success/error/panic alike).
        self.erasing
            .store(false, std::sync::atomic::Ordering::Release);
    }
}

/// Performs GDPR Art. 17 local data erasure (#4801).
///
/// # Order (R6)
/// 1. Phase-1: atomic deletion of all SQLite user-data tables.
///    - On failure, return `IpcError` immediately (R5: do not report Ok with
///      residual SQLite data).
/// 2. Phase-2: deletion of frame image files (when frame_storage is Some).
///    - On failure, write a `pending_local_erase=1` marker via `set_meta_checked`
///      (R2/R3).
///    - Report the error to the caller regardless of whether the marker write
///      succeeded (R5).
///
/// # Remote propagation (R4)
/// Does not touch `pending_deletion=true` — sync_engine is the sole owner.
async fn erase_all_local_data(
    storage: Arc<SqliteStorage>,
    frame_storage: Option<Arc<dyn FrameStoragePort>>,
) -> Result<(), IpcError> {
    // #4928 round-3 (FIX B): block the grant_consent-during-erase TOCTOU.
    //
    // A re-grant (`grant_consent`) during erase can flip `deletion_flag` back to
    // `false`, but `erasing` is only set/cleared by erase and `grant_consent` cannot
    // touch it. Setting it for the entire erase span (Phase-1 + Phase-2) means that
    // even if a re-grant slips in between the Phase-1 commit and Phase-2, an
    // in-flight writer cannot write residual rows after the wipe because of the
    // `deletion_flag || erasing` skip predicate.
    //
    // `storage.erasing()` is the same `Arc` shared by the SQLite/frames/
    // ConsentManager trio at the composition root, so setting/clearing this one
    // handle is visible to both funnels. The RAII guard (`EraseWindowGuard`) clears
    // it on every exit path (success/Err/panic-unwind).
    let _erase_window = EraseWindowGuard::set(storage.erasing());

    // ── Phase-1: atomic SQLite deletion ──────────────────────────────────────
    // `delete_all_data` is blocking I/O, so isolate it on spawn_blocking.
    let storage_clone = storage.clone();
    tokio::task::spawn_blocking(move || storage_clone.delete_all_data())
        .await
        .map_err(|join_err| {
            IpcError::new(
                "internal.generic",
                format!("GDPR SQLite deletion task join failed: {join_err}"),
            )
        })?
        .map_err(|e| {
            // R5: do not return Ok on SQLite deletion failure — propagate Err immediately.
            tracing::error!(err = %e, "GDPR Art.17: full SQLite deletion failed — residual user data");
            IpcError::from(e)
        })?;

    tracing::info!("GDPR Art.17 Phase-1 complete: full SQLite deletion succeeded");

    // ── Phase-2: frame image file deletion ───────────────────────────────────
    let Some(fs) = frame_storage else {
        // When there is no frame storage (offline/test environment) — skip Phase-2.
        return Ok(());
    };

    match fs.delete_all_frames().await {
        Ok(count) => {
            tracing::info!(
                count,
                "GDPR Art.17 Phase-2 complete: frame file deletion succeeded"
            );
            // On Phase-2 success, clear the retry marker if present (recover from a
            // prior partial deletion).
            if let Err(e) = storage.delete_meta_checked(PENDING_LOCAL_ERASE_KEY) {
                // A marker-deletion failure is acceptable: it only means the retry
                // happens again on the next launch.
                tracing::warn!(err = %e, "GDPR: retry marker deletion failed (non-fatal)");
            }
            Ok(())
        }
        Err(e) => {
            // R3/R5: frame deletion failed → write the retry marker + report partial
            // status to the caller.
            tracing::error!(err = %e, "GDPR Art.17 Phase-2 failed: frame image deletion incomplete");
            if let Err(meta_err) = storage.set_meta_checked(PENDING_LOCAL_ERASE_KEY, "1") {
                // If the marker write also fails, log an extra warning (R2 cannot be
                // achieved — record the limitation).
                tracing::error!(
                    err = %meta_err,
                    "GDPR: retry marker write failed — automatic retry after restart is not possible"
                );
            }
            // Convert the frame-deletion error to IpcError and return it (R5).
            Err(IpcError::from(e))
        }
    }
}

/// Retries an incomplete local erasure on app launch (#4801, R2/R3 — restart
/// durability).
///
/// If the `pending_local_erase=1` marker is present in `app_meta`, re-run
/// `erase_all_local_data`. After both phases succeed, delete the marker via
/// `delete_meta_checked`. Even on failure it retries on the next launch, so it does
/// not abort app startup (best-effort retry).
///
/// # Call site
/// Called from the `build_and_spawn` function in `app_runtime_launch::mod.rs` right
/// after SQLite is ready, before the capture services are connected.
pub(crate) async fn retry_pending_local_erase(
    storage: Arc<SqliteStorage>,
    frame_storage: Option<Arc<dyn FrameStoragePort>>,
) {
    // If there is no marker, return immediately (normal startup path).
    if storage.get_meta(PENDING_LOCAL_ERASE_KEY).is_none() {
        return;
    }

    tracing::warn!("GDPR Art.17: detected incomplete local-erasure marker — starting retry");

    match erase_all_local_data(storage.clone(), frame_storage).await {
        Ok(()) => {
            tracing::info!("GDPR Art.17: retry succeeded — deleting retry marker");
            // Phase-2 succeeded, so the marker is deleted (already handled inside
            // erase_all_local_data).
        }
        Err(e) => {
            // The retry also failed — try again on the next launch (the marker is kept).
            tracing::error!(
                err = %e,
                "GDPR Art.17: retry failed — will retry on next launch"
            );
        }
    }
}

/// #4686: a **one-time** upgrade notice for a mic that quietly stopped after the
/// microphone split (#4568).
///
/// If `audio.enabled` (the user turned on audio capture) but the `microphone`
/// consent is absent (default OFF since the split) and the notice has not been shown
/// yet, returns `true` **exactly once** and writes the `app_meta` flag. The frontend
/// calls this on mount to show a one-time banner guiding the user to the privacy
/// page. Subsequent calls always return `false` (idempotent). Being an imperative
/// command, there is no startup emit-race (the frontend pulls it itself once ready).
///
/// It reads and writes `app_meta` (blocking SQLite), so it is moved onto
/// `spawn_blocking` to avoid starving the async worker pool (F-RR-06 async-safety).
#[command]
pub async fn take_microphone_upgrade_notice(
    state: tauri::State<'_, AppState>,
) -> Result<bool, IpcError> {
    let storage = state.storage.clone();
    let consent_manager = state.capture.consent_manager.clone();
    let audio_enabled = state.config.audio.enabled;

    tokio::task::spawn_blocking(move || {
        let already_shown = storage.get_meta(MIC_UPGRADE_NOTICE_FLAG).is_some();
        let microphone_granted = consent_manager
            .as_ref()
            .map(|cm| cm.effective_permissions().microphone)
            .unwrap_or(false);
        let show =
            should_show_microphone_upgrade_notice(audio_enabled, microphone_granted, already_shown);
        if show {
            // Record the flag only when showing, so it is surfaced exactly once.
            storage.set_meta(MIC_UPGRADE_NOTICE_FLAG, "true");
        }
        show
    })
    .await
    .map_err(|join_err| {
        IpcError::new(
            "internal.generic",
            format!("take_microphone_upgrade_notice task join failed: {join_err}"),
        )
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::consent::ConsentManager;

    /// Verifies the set_consent → get_consent round-trip + audit log recording.
    ///
    /// - Uses a bare `ConsentManager` + `SqliteStorage` directly, without `AppState`.
    /// - Calls the `apply_set_consent` / `read_consent_snapshot` helpers.
    /// - Asserts the resulting snapshot's status == Valid and screen_capture == true.
    /// - Asserts a `action_type == "consent_granted"` entry exists in `entries_by_command_id("system.consent")`.
    #[test]
    fn set_then_get_consent_round_trip_and_audit() {
        let dir = tempfile::tempdir().unwrap();
        let cm = std::sync::Arc::new(maekon_core::consent::ConsentManager::new(
            dir.path().join("consent.json"),
        ));
        let storage = std::sync::Arc::new(
            maekon_storage::sqlite::SqliteStorage::open(&dir.path().join("s.db"), 1, None).unwrap(),
        );

        // Grant consent + verify the snapshot
        let snap = apply_set_consent(
            cm.as_ref(),
            &storage,
            maekon_core::consent::ConsentPermissions {
                screen_capture: true,
                ..Default::default()
            },
        )
        .expect("apply_set_consent must not fail in a writable temp directory");
        assert_eq!(snap.status, maekon_core::consent::ConsentStatus::Valid);
        assert!(snap.permissions.screen_capture);

        // Re-check read_consent_snapshot (independent read)
        let got = read_consent_snapshot(cm.as_ref());
        assert!(got.permissions.screen_capture);

        // Verify the audit log: at least one consent_granted entry must exist.
        let audit = storage.entries_by_command_id("system.consent", 10);
        assert!(
            audit.iter().any(|e| e.action_type == "consent_granted"),
            "no consent_granted entry in audit_log: {audit:?}"
        );
    }

    /// Verifies the revoke audit records the revoked (prior) permission set (GDPR Art. 7§1 #4684).
    /// Regression: it used to record the all-false default, making "what was revoked" unreconstructable.
    #[test]
    fn withdraw_consent_audits_the_revoked_permission_set() {
        let dir = tempfile::tempdir().unwrap();
        let cm = std::sync::Arc::new(maekon_core::consent::ConsentManager::new(
            dir.path().join("consent.json"),
        ));
        let storage = std::sync::Arc::new(
            maekon_storage::sqlite::SqliteStorage::open(&dir.path().join("s.db"), 1, None).unwrap(),
        );
        apply_set_consent(
            cm.as_ref(),
            &storage,
            maekon_core::consent::ConsentPermissions {
                screen_capture: true,
                microphone: true,
                ..Default::default()
            },
        )
        .expect("grant");
        let snap = apply_withdraw_consent(cm.as_ref(), &storage).expect("withdraw");

        assert_eq!(snap.status, maekon_core::consent::ConsentStatus::NotGranted);
        assert!(!snap.permissions.screen_capture);
        assert!(!cm.effective_permissions().screen_capture);
        assert!(cm.has_pending_deletion());

        let audit = storage.entries_by_command_id("system.consent", 10);
        let revoke = audit
            .iter()
            .find(|e| e.action_type == "consent_revoked")
            .expect("a consent_revoked audit entry must exist");
        let details: serde_json::Value =
            serde_json::from_str(revoke.details.as_ref().expect("details")).expect("details json");
        // The revoked permission set (prior) must be recorded — not the all-false default.
        assert_eq!(
            details["permissions"]["microphone"],
            serde_json::json!(true),
            "the revoke audit must record the prior microphone=true (not all-false): {details}"
        );
        assert_eq!(
            details["permissions"]["screen_capture"],
            serde_json::json!(true)
        );
    }

    /// #5056: the consent→exporter gate the re-apply helper computes is OFF
    /// after a telemetry-consent revoke, even when config.telemetry.enabled=true.
    ///
    /// The IPC command `reapply_telemetry_gate` resolves managed state (Handle /
    /// ConfigRuntimeState / AppState), which is not constructible headless — but
    /// the load-bearing computation IS the pure
    /// `consent_gated_telemetry_config(&config, &consent_manager)` call. This
    /// test exercises exactly that pure gate through a real `ConsentManager`
    /// grant→revoke lifecycle, proving the helper drives the exporter OFF on
    /// revoke (fail-closed). The IPC wiring itself (`set_consent` /
    /// `withdraw_consent` calling `reapply_telemetry_gate`) is documented and
    /// covered by the manual call-site edits.
    #[test]
    fn reapply_gate_computes_off_after_telemetry_revoke() {
        use crate::telemetry::consent_gated_telemetry_config;
        use maekon_core::config::TelemetryConfig;

        let dir = tempfile::tempdir().unwrap();
        let cm = ConsentManager::new(dir.path().join("consent.json"));

        // User has telemetry config ON.
        let config = TelemetryConfig {
            enabled: true,
            ..Default::default()
        };

        // Grant telemetry consent → gate OPEN (config on + consent on).
        cm.grant_consent(
            ConsentPermissions {
                telemetry: true,
                ..Default::default()
            },
            RETENTION_DAYS,
        )
        .expect("grant");
        assert!(
            consent_gated_telemetry_config(&config, &cm).enabled,
            "config on + telemetry consent on → exporter ON"
        );

        // Revoke consent → gate CLOSED immediately even though config stays ON.
        cm.revoke_consent().expect("revoke");
        assert!(
            !consent_gated_telemetry_config(&config, &cm).enabled,
            "after revoke the gate computes OFF (fail-closed) — this is what \
             reapply_telemetry_gate pushes to Handle::apply on consent change"
        );
    }

    /// #4686: microphone upgrade notice decision logic (pure function, truth table).
    #[test]
    fn microphone_upgrade_notice_decision_truth_table() {
        // audio ON + mic not consented + not shown → show it.
        assert!(should_show_microphone_upgrade_notice(true, false, false));
        // already shown once → never show again.
        assert!(!should_show_microphone_upgrade_notice(true, false, true));
        // mic consent already present → nothing to explain.
        assert!(!should_show_microphone_upgrade_notice(true, true, false));
        // audio OFF → mic is not used, so this is irrelevant.
        assert!(!should_show_microphone_upgrade_notice(false, false, false));
    }

    // ─────────────────────────────────────────────────────────────────────────
    // #4801 GDPR Art. 17 tests
    // ─────────────────────────────────────────────────────────────────────────

    /// Manual Mock FrameStoragePort that tracks every method call.
    ///
    /// Controls the `delete_all_frames` call count and the result it returns.
    /// Pure manual implementation without mockall (ADR-001 §5).
    struct MockFrameStorage {
        /// `delete_all_frames` call count.
        delete_call_count: std::sync::atomic::AtomicU32,
        /// If true, `delete_all_frames` returns CoreError::Storage.
        should_fail: bool,
    }

    impl MockFrameStorage {
        /// Mock that returns success.
        fn success() -> Arc<Self> {
            Arc::new(Self {
                delete_call_count: std::sync::atomic::AtomicU32::new(0),
                should_fail: false,
            })
        }

        /// Mock that returns an error.
        fn failing() -> Arc<Self> {
            Arc::new(Self {
                delete_call_count: std::sync::atomic::AtomicU32::new(0),
                should_fail: true,
            })
        }

        fn delete_call_count(&self) -> u32 {
            self.delete_call_count
                .load(std::sync::atomic::Ordering::Relaxed)
        }
    }

    #[async_trait::async_trait]
    impl maekon_core::ports::frame_storage::FrameStoragePort for MockFrameStorage {
        async fn save_frame(
            &self,
            _ts: chrono::DateTime<chrono::Utc>,
            _data: &[u8],
        ) -> Result<std::path::PathBuf, maekon_core::error::CoreError> {
            unimplemented!("not used in tests")
        }

        async fn save_frames_batch(
            &self,
            _frames: Vec<(chrono::DateTime<chrono::Utc>, Vec<u8>)>,
        ) -> Vec<Result<std::path::PathBuf, maekon_core::error::CoreError>> {
            unimplemented!("not used in tests")
        }

        async fn load_frame(
            &self,
            _path: &std::path::Path,
        ) -> Result<Vec<u8>, maekon_core::error::CoreError> {
            unimplemented!("not used in tests")
        }

        async fn enforce_retention(&self) -> Result<usize, maekon_core::error::CoreError> {
            unimplemented!("not used in tests")
        }

        async fn enforce_storage_limit(&self) -> Result<usize, maekon_core::error::CoreError> {
            unimplemented!("not used in tests")
        }

        async fn delete_all_frames(&self) -> Result<usize, maekon_core::error::CoreError> {
            self.delete_call_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if self.should_fail {
                Err(maekon_core::error::CoreError::Storage {
                    code: maekon_core::error_codes::StorageCode::Failed,
                    message: "mock frame delete failure".into(),
                })
            } else {
                Ok(0)
            }
        }
    }

    /// Helper: opens a `SqliteStorage` in a temp directory and inserts some user data.
    fn open_storage_with_data() -> (maekon_storage::sqlite::SqliteStorage, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let storage =
            maekon_storage::sqlite::SqliteStorage::open(&dir.path().join("s.db"), 1, None).unwrap();
        // Insert 1 event — to verify it is empty after deletion.
        // #4928: connection_arc() returns Arc<GuardedConnection> — use the write_lock funnel.
        let conn = storage.connection_arc();
        conn.write_lock()
            .run::<_, usize, rusqlite::Error>(0, |c| {
                c.execute(
                    "INSERT INTO events (event_id, event_type, timestamp, data) \
                     VALUES ('e1', 'context', '2026-01-01T00:00:00Z', '{}')",
                    [],
                )
            })
            .unwrap();
        (storage, dir)
    }

    /// Helper that returns the row count of a SQLite table.
    fn count_rows(storage: &maekon_storage::sqlite::SqliteStorage, table: &str) -> i64 {
        // Read — read_lock funnel.
        let conn = storage.connection_arc();
        let read = conn.read_lock();
        read.conn()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap_or(0)
    }

    // ── Test 1: Phase-1 success + Phase-2 success ────────────────────────────

    /// After revoke, erase_all_local_data empties SQLite and calls delete_all_frames (#4801).
    #[tokio::test]
    async fn erase_all_local_data_sqlite_empty_and_frames_deleted() {
        let (storage, _dir) = open_storage_with_data();
        let storage = Arc::new(storage);
        let mock_fs = MockFrameStorage::success();

        assert!(
            count_rows(&storage, "events") > 0,
            "the event must have been inserted"
        );

        erase_all_local_data(
            storage.clone(),
            Some(mock_fs.clone() as Arc<dyn FrameStoragePort>),
        )
        .await
        .expect("erase_all_local_data must succeed");

        assert_eq!(
            count_rows(&storage, "events"),
            0,
            "the SQLite events table must be empty"
        );
        assert_eq!(
            mock_fs.delete_call_count(),
            1,
            "delete_all_frames must be called exactly once"
        );
        // After success, there must be no retry marker.
        assert!(
            storage.get_meta(PENDING_LOCAL_ERASE_KEY).is_none(),
            "there must be no pending_local_erase marker after success"
        );
    }

    // ── Test 2: Phase-2 failure → marker written + Err returned (R3/R5) ──────

    /// On Phase-2 (frame deletion) failure, the pending_local_erase marker is written and Err is returned (#4801 R3/R5).
    #[tokio::test]
    async fn erase_all_local_data_phase2_failure_sets_marker_and_returns_err() {
        let (storage, _dir) = open_storage_with_data();
        let storage = Arc::new(storage);
        let mock_fs = MockFrameStorage::failing();

        let result = erase_all_local_data(
            storage.clone(),
            Some(mock_fs.clone() as Arc<dyn FrameStoragePort>),
        )
        .await;

        // R5: on frame-deletion failure, Err must be returned (no concealment as Ok).
        let ipc_err = result.unwrap_err();
        assert!(
            ipc_err.code.contains("storage"),
            "a storage IpcError must be returned on Phase-2 failure (R5)"
        );

        // R3: the retry marker must be written.
        assert_eq!(
            storage.get_meta(PENDING_LOCAL_ERASE_KEY),
            Some("1".to_string()),
            "the pending_local_erase=1 marker must be written after Phase-2 failure (R3)"
        );

        // Phase-1 (SQLite) already completed, so the events table must be empty.
        assert_eq!(
            count_rows(&storage, "events"),
            0,
            "Phase-1 (SQLite deletion) succeeded, so the events table must be empty"
        );
    }

    // ── Test 3: when a marker exists, retry_pending_local_erase retries and clears the marker ──

    /// When a marker exists, retry_pending_local_erase retries the deletion and clears the marker on success (#4801 R2).
    #[tokio::test]
    async fn retry_pending_local_erase_reruns_and_clears_marker_on_success() {
        let (storage, _dir) = open_storage_with_data();
        let storage = Arc::new(storage);

        // Write the retry marker manually (simulating a Phase-2 failure on a prior launch).
        storage
            .set_meta_checked(PENDING_LOCAL_ERASE_KEY, "1")
            .unwrap();

        let mock_fs = MockFrameStorage::success();
        retry_pending_local_erase(
            storage.clone(),
            Some(mock_fs.clone() as Arc<dyn FrameStoragePort>),
        )
        .await;

        // After a successful retry, the marker must be deleted.
        assert!(
            storage.get_meta(PENDING_LOCAL_ERASE_KEY).is_none(),
            "the pending_local_erase marker must be deleted after a successful retry"
        );

        // delete_all_frames must be called during the retry.
        assert_eq!(
            mock_fs.delete_call_count(),
            1,
            "delete_all_frames must be called on the retry path"
        );
    }

    // ── Test 4: when there is no marker, retry_pending_local_erase is a no-op ──

    /// With no marker, retry_pending_local_erase does nothing (#4801 — normal startup path).
    #[tokio::test]
    async fn retry_pending_local_erase_noop_when_no_marker() {
        let (storage, _dir) = open_storage_with_data();
        let storage = Arc::new(storage);

        // Run retry without a marker.
        let mock_fs = MockFrameStorage::success();
        retry_pending_local_erase(
            storage.clone(),
            Some(mock_fs.clone() as Arc<dyn FrameStoragePort>),
        )
        .await;

        // delete_all_frames must not be called.
        assert_eq!(
            mock_fs.delete_call_count(),
            0,
            "delete_all_frames must not be called when there is no marker"
        );

        // The event data must remain intact.
        assert!(
            count_rows(&storage, "events") > 0,
            "data must not be deleted on a retry without a marker"
        );
    }

    // ── Test 5: when frame_storage is None, Phase-2 is skipped ───────────────

    /// When frame_storage is None, only Phase-1 runs and Ok is returned.
    #[tokio::test]
    async fn erase_all_local_data_no_frame_storage_erases_sqlite_only() {
        let (storage, _dir) = open_storage_with_data();
        let storage = Arc::new(storage);

        assert!(count_rows(&storage, "events") > 0);

        // frame_storage = None — skip Phase-2.
        erase_all_local_data(storage.clone(), None)
            .await
            .expect("Ok must be returned even when frame_storage is None");

        assert_eq!(
            count_rows(&storage, "events"),
            0,
            "SQLite must be deleted even when frame_storage=None"
        );
    }
}
