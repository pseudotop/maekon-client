use crate::capture_services::SharedCaptureServices;
use anyhow::Result;
use maekon_core::ports::consent_manager::ConsentManagerPort;
use maekon_storage::encryption::EncryptionKey;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub(super) struct CaptureLaunchWiring {
    pub(super) capture_paused: Arc<AtomicBool>,
    pub(super) indicator_visible: Arc<AtomicBool>,
    pub(super) detection_active: Arc<AtomicBool>,
    /// #8039: NOT `Option` — `build_capture_wiring` only returns `Ok` once this
    /// is genuinely built (with encryption wired via `with_encryption`,
    /// `capture_services.rs:38`). There is no "capture without shared
    /// services" state a caller can construct: the type itself makes an
    /// unencrypted `FrameFileStorage` unreachable downstream.
    pub(super) shared_capture_services: Arc<SharedCaptureServices>,
    pub(super) consent_manager: Arc<dyn ConsentManagerPort>,
}

/// #8039: previously, a `SharedCaptureServices::build` failure (e.g. the
/// `frames/` directory owner-only creation failing on permission-denied / AV
/// lock / disk pressure — `frame_storage/io.rs`) was caught, logged via
/// `tracing::warn!`, and downgraded to `shared_capture_services = None`. The
/// agent runtime then silently built an UNENCRYPTED
/// `FrameFileStorage::new()` fallback (`agent_runtime_support.rs`) and kept
/// running — plaintext screenshots, no fail signal, and GDPR Art. 17 erase
/// Phase-2 silently skipped (no shared frame_storage handle to erase from).
/// This mirrors `storage_runtime.rs`'s DB-key fail-closed reasoning: there is
/// no "run capture unencrypted" mode, so any error here is a hard failure —
/// the caller (`app_runtime_launch::mod.rs::build_and_spawn`, itself
/// `Result`-returning) propagates it via `?` and aborts startup instead of
/// continuing fail-open.
pub(super) fn build_capture_wiring(
    runtime_handle: &tokio::runtime::Handle,
    data_dir_path: &Path,
    config: &maekon_core::config::AppConfig,
    encryption_key: Option<Arc<EncryptionKey>>,
    cua_safe_mode: bool,
) -> Result<CaptureLaunchWiring> {
    let (capture_paused_initial, indicator_visible_initial) =
        super::cua_safe_mode::initial_capture_flags(config.indicator.show_border, cua_safe_mode);
    let capture_paused = Arc::new(AtomicBool::new(capture_paused_initial));
    let indicator_visible = Arc::new(AtomicBool::new(indicator_visible_initial));
    let detection_active = Arc::new(AtomicBool::new(false));

    let shared_capture_services = Arc::new(
        runtime_handle
            .block_on(SharedCaptureServices::build(
                data_dir_path,
                config,
                encryption_key,
            ))
            .map_err(|error| {
                anyhow::anyhow!(
                    "capture services init failed; refusing to start with frame encryption \
                     unavailable (screenshots would be stored in plaintext): {error}"
                )
            })?,
    );
    let consent_manager = shared_capture_services.consent_manager.clone();

    Ok(CaptureLaunchWiring {
        capture_paused,
        indicator_visible,
        detection_active,
        shared_capture_services,
        consent_manager,
    })
}

/// Finalizes the erasure wiring right after storage is ready (before the
/// scheduler / agent / web adapters start).
///
/// (1) #4928: installs the ConsentManager's shared `deletion_flag` into the LIVE
///     `SqliteStorage`. `set_deletion_flag(&self)` swaps the ArcSwap cell, so it
///     applies immediately to the already-Arc-shared storage (and to every
///     adapter that shares `connection_arc()`). The same Arc is installed into
///     frame storage by `capture_services::build`, so the trio (consent ↔ SQLite
///     ↔ frames) shares one flag. This happens before any write adapter starts.
/// (2) #4801 GDPR Art. 17: retries any local deletion marker that did not
///     complete in a previous launch.
pub(super) fn install_erasure_wiring(
    handle: &tokio::runtime::Handle,
    sqlite_storage: &Arc<maekon_storage::sqlite::SqliteStorage>,
    consent_manager: &Arc<dyn ConsentManagerPort>,
    shared_capture_services: &Arc<SharedCaptureServices>,
    config_manager: &maekon_core::config_manager::ConfigManager,
) {
    sqlite_storage.set_deletion_flag(consent_manager.deletion_flag());
    // #4928 round-3 (FIX B): install the erase-window blocking signal `erasing`
    // via the same Arc. The same Arc is installed into frame storage by
    // `capture_services::build`, so the trio (consent ↔ SQLite ↔ frames) shares
    // one `erasing` flag (ptr-eq).
    sqlite_storage.set_erasing(consent_manager.erasing());
    // #8039: `shared_capture_services` is unconditionally present now, so
    // Phase-2 GDPR erase always has a frame_storage handle to retry against —
    // it can no longer silently skip because the composition root degraded to
    // `None` on a capture-services build failure.
    let retry_frame_storage = Some(shared_capture_services.frame_storage.clone());
    // ADR-033 Phase-3 participates in the crash-recovery retry: a crash
    // between phases re-runs vault erasure here on the next launch.
    let retry_vault_writer = Some(crate::vault_wiring::build_vault_writer(
        sqlite_storage.clone(),
        Some(consent_manager.clone()),
        config_manager.clone(),
    ));
    handle.block_on(crate::commands::consent::retry_pending_local_erase(
        sqlite_storage.clone(),
        retry_frame_storage,
        retry_vault_writer,
    ));
}

/// #8044: the ONE capture-history re-auth gate, built from config.
///
/// The same `Arc` is shared between the web `require_capture_reauth` middleware
/// and the Tauri biometric/PIN command (registered as `ReauthRuntimeState` in
/// `setup.rs`) — two gates would let one surface be satisfied while the other
/// stays locked. Default-on for privacy; `is_satisfied()` is a pass-through
/// when disabled.
///
/// Lives here rather than in the composition root because it is capture-history
/// wiring, and `mod.rs` is held to the ADR-013 composition-root budget (#9738).
pub(super) fn build_capture_reauth_gate(
    config: &maekon_core::config::AppConfig,
) -> Arc<maekon_core::reauth::CaptureReauthGate> {
    Arc::new(maekon_core::reauth::CaptureReauthGate::new(
        config.privacy.reauth.enabled,
        config.privacy.reauth.effective_idle_timeout(),
    ))
}

#[cfg(test)]
mod tests {
    use super::{build_capture_wiring, CaptureLaunchWiring};
    use crate::capture_services::SharedCaptureServices;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn capture_launch_wiring_exposes_shared_flags() {
        let dir = tempfile::tempdir().unwrap();
        let config = maekon_core::config::AppConfig::default_config();
        let shared_capture_services = Arc::new(
            SharedCaptureServices::build(dir.path(), &config, None)
                .await
                .expect("SharedCaptureServices::build must succeed against a writable temp dir"),
        );

        let wiring = CaptureLaunchWiring {
            capture_paused: Arc::new(AtomicBool::new(true)),
            indicator_visible: Arc::new(AtomicBool::new(false)),
            detection_active: Arc::new(AtomicBool::new(false)),
            shared_capture_services,
            consent_manager: Arc::new(maekon_core::consent::ConsentManager::new(
                dir.path().join("consent.json"),
            )),
        };

        assert!(wiring.capture_paused.load(Ordering::SeqCst));
        assert!(!wiring.indicator_visible.load(Ordering::SeqCst));
        assert!(!wiring.detection_active.load(Ordering::SeqCst));
    }

    /// #8039 regression: previously, a `SharedCaptureServices::build` failure
    /// (e.g. the `frames/` directory being blocked) was caught, logged with
    /// `tracing::warn!`, and downgraded to `shared_capture_services = None`.
    /// `agent_runtime_support.rs` then silently built an UNENCRYPTED
    /// `FrameFileStorage::new()` fallback instead — plaintext screenshots,
    /// no fail signal. This test proves the fix: the exact same failure mode
    /// now propagates as `Err` from `build_capture_wiring`, so the caller's
    /// `?` (`app_runtime_launch::mod.rs::build_and_spawn`, itself
    /// `Result`-returning) aborts startup instead of continuing fail-open.
    #[test]
    fn build_capture_wiring_fails_closed_when_frame_storage_init_fails() {
        // A plain `#[test]` thread has no active tokio runtime, so a
        // freestanding `Runtime` + `Handle::block_on` (the same pattern
        // `build_capture_wiring` itself uses) is safe here — mirrors
        // `gui_ticket_secret.rs`'s test pattern for sync fns with an internal
        // `block_on`.
        let rt = tokio::runtime::Runtime::new().unwrap();
        let dir = tempfile::tempdir().unwrap();
        // Block frame storage's owner-only directory creation by pre-creating
        // "frames" as a FILE, not a directory — mirrors the #8039 issue's
        // cited failure mode (frame_storage/io.rs's `create_dir_owner_only`
        // permission-denied / AV-lock / disk-pressure scenarios).
        std::fs::write(dir.path().join("frames"), b"not a directory").unwrap();
        let config = maekon_core::config::AppConfig::default_config();

        let result = build_capture_wiring(rt.handle(), dir.path(), &config, None, false);

        // #5631: a message assertion on the extracted error instead of a
        // value-blind `is_err()` hedge — pins the fail-closed wrapper's actual
        // text (the `anyhow::anyhow!(...)` wrapper this function constructs
        // above), not merely "some Err arrived", so a future refactor that
        // accidentally swaps in a different failure mode still trips this
        // test. `.err().expect(..)` (not `.unwrap_err()`) because
        // `CaptureLaunchWiring` does not derive `Debug`.
        let err = result.err().expect(
            "build_capture_wiring must fail closed (return Err) when \
             SharedCaptureServices::build fails",
        );
        assert!(
            err.to_string().contains("capture services init failed"),
            "build_capture_wiring must fail closed with its fail-closed wrapper message when \
             SharedCaptureServices::build fails — it must never fall back to a \
             missing/unencrypted capture subsystem; got: {err}"
        );
    }
}
