use anyhow::Result;
use maekon_storage::encryption::EncryptionKey;
use maekon_storage::sqlite::SqliteStorage;
use std::path::Path;
use std::sync::Arc;
use tracing::info;

pub(crate) struct StorageRuntimeBundle {
    pub(crate) sqlite_storage: Arc<SqliteStorage>,
    /// Shared encryption key for frame file encryption and other at-rest crypto.
    pub(crate) encryption_key: Option<Arc<EncryptionKey>>,
}

pub(crate) struct StorageRuntimeBuilder<'a> {
    db_path: &'a Path,
    data_dir: &'a Path,
    retention_days: u32,
}

impl<'a> StorageRuntimeBuilder<'a> {
    pub(crate) fn new(db_path: &'a Path, data_dir: &'a Path, retention_days: u32) -> Self {
        Self {
            db_path,
            data_dir,
            retention_days,
        }
    }

    pub(crate) fn build(&self) -> Result<StorageRuntimeBundle> {
        // #6438 (F7): at-rest encryption is unconditional here — `load_or_create`
        // loads the existing key or generates+persists a new one, and only errors on a
        // genuine failure (corrupt/unreadable `.db_key`, I/O on save, RNG). A failure
        // therefore means encryption was intended but could not be provisioned; FAIL
        // CLOSED (abort startup) rather than silently opening the SQLite DB and all
        // screenshot files as plaintext. Mirrors the #6418 fail-closed reasoning one
        // layer up. There is no "run unencrypted" mode today; a genuine one would need
        // an explicit, audited opt-in, so any error here is a hard failure.
        let encryption_key = maekon_storage::encryption::EncryptionKey::load_or_create(
            self.data_dir,
        )
        .map_err(|error| {
            anyhow::anyhow!(
                "DB encryption key provisioning failed; refusing to start with at-rest \
                         encryption disabled (the database and screenshots would be stored in \
                         plaintext): {error}"
            )
        })?;
        info!(
            "DB encryption key ready ({})",
            self.data_dir.join(".db_key").display()
        );

        // #6864: acquire an advisory cross-process lock on the data dir BEFORE
        // opening the DB. The DBUS-based single-instance plugin degrades in
        // headless/DBUS-absent Linux sessions; this lock ensures a second instance
        // that bypassed single-instance aborts here rather than opening the same
        // SQLite file concurrently — the WAL-reset data-race precondition (#6830).
        // No-op on Windows (robust OS single-instance). `hold_for_process_lifetime`
        // keeps the lock held until the process exits (NOT stored in this bundle,
        // which is consumed during startup wiring and never reaches long-lived
        // AppState — a dropped guard would release the lock at end of startup,
        // defeating the guarantee; #6864 review).
        maekon_storage::process_lock::ProcessLock::try_acquire(&self.data_dir.join("maekon.lock"))
            .map_err(|error| {
                anyhow::anyhow!(
                    "data-directory lock acquisition failed; another maekon instance may be \
                 running (refusing to open the database concurrently): {error}"
                )
            })?
            .hold_for_process_lifetime();

        let sqlite_storage = Arc::new(SqliteStorage::open(
            self.db_path,
            self.retention_days,
            Some(&encryption_key),
        )?);
        info!(
            "SQLite initialized: {} (SQLCipher encrypted)",
            self.db_path.display()
        );

        Ok(StorageRuntimeBundle {
            sqlite_storage,
            encryption_key: Some(Arc::new(encryption_key)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #6438 (F7): if at-rest encryption-key provisioning fails, `build()` must FAIL
    /// CLOSED — return an error and create no database — rather than silently opening the
    /// SQLite DB (and screenshots) in plaintext.
    #[test]
    fn build_fails_closed_when_key_provisioning_fails() {
        // Unique scratch dir (no tempfile dev-dep in src-tauri).
        let base = std::env::temp_dir().join(format!("maekon-f7-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("create scratch dir");
        // Make `.db_key` unreadable as a key file (a directory) so `load_or_create`
        // errors, simulating a corrupt/inaccessible key.
        std::fs::create_dir(base.join(".db_key")).expect("create blocking .db_key dir");
        let db_path = base.join("test.db");

        let err_msg = match StorageRuntimeBuilder::new(&db_path, &base, 30).build() {
            Ok(_) => {
                let _ = std::fs::remove_dir_all(&base);
                panic!("build must fail closed on key-provisioning failure, not open plaintext");
            }
            Err(e) => e.to_string(),
        };
        let db_created = db_path.exists();
        let _ = std::fs::remove_dir_all(&base);

        assert!(
            err_msg.contains("encryption"),
            "expected a fail-closed encryption error, got: {err_msg}"
        );
        assert!(
            !db_created,
            "no (plaintext) database must be created when encryption provisioning fails"
        );
    }
}
