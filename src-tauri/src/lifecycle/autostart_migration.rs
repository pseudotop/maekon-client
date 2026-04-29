//! One-time migration check at app startup.
//!
//! If an existing systemd service file matches a known PR-B1-era template,
//! overwrite with the new PR-B2 Type=notify template (DEFERRED reload — file
//! takes effect on next user login; we do NOT call `daemon-reload` on the
//! currently-running service).
//!
//! If file content is unrecognized (user customized): log warn + skip.

#[cfg(target_os = "linux")]
pub fn run_startup_migration() {
    use super::migration_hashes::matches_known_template;
    use crate::autostart::linux::{generate_service_file, service_path};
    use oneshim_core::error_codes::AutostartCode;

    let path = match service_path() {
        Ok(p) => p,
        Err(e) => {
            tracing::debug!(
                err.code = AutostartCode::ServiceMigrationSkipped.as_str(),
                "Migration check skipped — service path unresolved: {e}"
            );
            return;
        }
    };

    if !path.exists() {
        // Autostart never enabled — no migration needed
        return;
    }

    let existing = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                tracing::debug!(
                    err.code = AutostartCode::ServiceMigrationSkipped.as_str(),
                    "Migration check skipped — service file vanished after exists() check (benign race): {e}"
                );
            } else {
                tracing::warn!(
                    err.code = AutostartCode::ServiceMigrationSkipped.as_str(),
                    "Migration check skipped — failed to read service file: {e}"
                );
            }
            return;
        }
    };

    let binary_path = match std::env::current_exe() {
        Ok(p) => p.to_string_lossy().to_string(),
        Err(e) => {
            tracing::debug!(
                err.code = AutostartCode::ServiceMigrationSkipped.as_str(),
                "Migration check skipped — current_exe() failed: {e}"
            );
            return;
        }
    };

    match matches_known_template(&existing, &binary_path) {
        Some(label) => {
            // Safe to overwrite. Write new file. DO NOT daemon-reload.
            let new_content = generate_service_file(&binary_path);
            if let Err(e) = std::fs::write(&path, new_content) {
                tracing::warn!(
                    err.code = AutostartCode::ServiceMigrationFailed.as_str(),
                    "Migration write failed: {e}"
                );
                return;
            }
            tracing::info!(
                err.code = AutostartCode::ServiceMigrated.as_str(),
                from = %label,
                "Migrated systemd unit file from {label} to Type=notify; takes effect next login"
            );
        }
        None => {
            tracing::warn!(
                err.code = AutostartCode::ServiceMigrationSkipped.as_str(),
                path = %path.display(),
                "Skipping autostart unit migration — file appears customized OR ExecStart path differs from current binary location (symlink?). Manual update required (see docs/guides/autostart.ko.md)"
            );
        }
    }
}

#[cfg(not(target_os = "linux"))]
#[allow(dead_code)] // called only inside #[cfg(target_os = "linux")] block in setup.rs; non-Linux stub
pub fn run_startup_migration() {
    // No-op on non-Linux
}
