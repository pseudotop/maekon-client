use anyhow::Result;
use maekon_storage::encryption::EncryptionKey;
#[cfg(test)]
use maekon_storage::encryption::MasterKeyVault;
use maekon_storage::keychain::KeychainOps;
use maekon_storage::sqlite::SqliteStorage;
use std::path::Path;
use std::sync::Arc;
use tracing::info;

pub(crate) struct StorageRuntimeBundle {
    pub(crate) sqlite_storage: Arc<SqliteStorage>,
    /// Shared encryption key for frame file encryption and other at-rest crypto.
    pub(crate) encryption_key: Option<Arc<EncryptionKey>>,
}

/// Registry-cache file name for the master-key keychain wrapper (#8040).
/// Deliberately distinct from `provider_secret_backend.rs`'s
/// `maekon-keychain-registry.json` (OAuth/BYOK secrets, different `config_dir`
/// entirely) — this is only an enumeration cache; the actual secret lives in
/// the OS keychain, namespaced/scoped per `data_dir` by
/// `master_key_keychain_entry` (`maekon-storage::encryption`).
const MASTER_KEY_KEYCHAIN_REGISTRY_FILE: &str = "maekon-master-key-keychain-registry.json";

/// #8040: resolve the shared at-rest master key for `data_dir`, preferring
/// the OS keychain over the plaintext `.db_key` file colocated with the
/// ciphertext it protects. Falls back to the plaintext-file scheme when the
/// keychain itself is unavailable (headless Linux/CI without a keyring
/// backend).
///
/// This is the ONE production entry point for resolving `data_dir`'s master
/// key — `StorageRuntimeBuilder::build` (SQLCipher + frame encryption) and
/// `ServerBootstrapContext::build` (`server` feature — integration state
/// store, #7073) both call it for the SAME `data_dir_path`/`.db_key`, so they
/// MUST observe the identical resolution (keychain vs. file, migrated vs.
/// not) — two independent `load_or_create`-style calls racing a migration
/// would risk one caller regenerating a fresh, DIFFERENT key after the other
/// deletes the legacy file (silent data loss for whichever store used the
/// stale key). `EncryptionKey::load_or_create_sealed` is idempotent/
/// self-consistent across repeated calls within one launch (each call fully
/// resolves current keychain+file state before returning), so both callers
/// converge on the same key regardless of call order.
pub(crate) fn resolve_shared_master_key(
    data_dir: &Path,
) -> std::result::Result<EncryptionKey, maekon_storage::error::StorageError> {
    let registry_path = data_dir.join(MASTER_KEY_KEYCHAIN_REGISTRY_FILE);
    match KeychainOps::new(registry_path) {
        Ok(keychain) => EncryptionKey::load_or_create_sealed(data_dir, &keychain),
        Err(e) => {
            tracing::warn!(
                "#8040: keychain registry init failed ({e}); using the plaintext key-file \
                 scheme (expected on headless Linux/CI without a keyring backend)"
            );
            EncryptionKey::load_or_create(data_dir)
        }
    }
}

pub(crate) struct StorageRuntimeBuilder<'a> {
    db_path: &'a Path,
    data_dir: &'a Path,
    retention_days: u32,
    /// Test-only seam (#8040): inject a fake `MasterKeyVault` so unit tests
    /// can drive the keychain migration/fallback decision tree
    /// deterministically without touching the real OS keychain (mirrors
    /// `maekon_storage::keychain`'s `#[ignore]`-by-default policy for real
    /// backend access). The exhaustive fake-vault coverage of
    /// `load_or_create_sealed` itself lives in
    /// `maekon_storage::encryption::sealed_key_tests`; tests here only need
    /// to prove `build()` wires the result through correctly.
    #[cfg(test)]
    master_key_vault: Option<Arc<dyn MasterKeyVault>>,
}

impl<'a> StorageRuntimeBuilder<'a> {
    pub(crate) fn new(db_path: &'a Path, data_dir: &'a Path, retention_days: u32) -> Self {
        Self {
            db_path,
            data_dir,
            retention_days,
            #[cfg(test)]
            master_key_vault: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_master_key_vault(mut self, vault: Arc<dyn MasterKeyVault>) -> Self {
        self.master_key_vault = Some(vault);
        self
    }

    /// Resolves the master key via `resolve_shared_master_key` — except in
    /// tests, where an injected fake vault (`with_master_key_vault`) takes
    /// precedence so `cargo test` never touches the real OS keychain.
    fn load_master_key(
        &self,
    ) -> std::result::Result<EncryptionKey, maekon_storage::error::StorageError> {
        #[cfg(test)]
        if let Some(ref vault) = self.master_key_vault {
            return EncryptionKey::load_or_create_sealed(self.data_dir, vault.as_ref());
        }

        resolve_shared_master_key(self.data_dir)
    }

    pub(crate) fn build(&self) -> Result<StorageRuntimeBundle> {
        // #6438 (F7): at-rest encryption is unconditional here — `load_master_key`
        // loads the existing key (keychain-sealed or, on a headless/keychain-
        // unavailable box, plaintext-file) or generates+persists a new one, and only
        // errors on a genuine failure (corrupt/unreadable key material, I/O on save,
        // RNG). A failure therefore means encryption was intended but could not be
        // provisioned; FAIL CLOSED (abort startup) rather than silently opening the
        // SQLite DB and all screenshot files as plaintext. Mirrors the #6418
        // fail-closed reasoning one layer up. There is no "run unencrypted" mode
        // today; a genuine one would need an explicit, audited opt-in, so any error
        // here is a hard failure.
        let encryption_key = self.load_master_key().map_err(|error| {
            anyhow::anyhow!(
                "DB encryption key provisioning failed; refusing to start with at-rest \
                         encryption disabled (the database and screenshots would be stored in \
                         plaintext): {error}"
            )
        })?;
        info!("DB encryption key ready (keychain-sealed or plaintext fallback)");

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
    use maekon_storage::error::StorageError;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Manual in-memory fake `MasterKeyVault` (ADR-001 §5: no mockall) — never
    /// touches the real OS keychain. Mirrors
    /// `maekon_storage::encryption::sealed_key_tests::FakeVault`; kept as a
    /// separate minimal copy here since it is a distinct crate (`maekon-app`
    /// vs `maekon-storage`) and this module only needs an "empty vault" double
    /// to prove `build()` wires `load_or_create_sealed`'s result through.
    #[derive(Default)]
    struct FakeVault {
        entries: Mutex<HashMap<(String, String), String>>,
    }

    impl MasterKeyVault for FakeVault {
        fn store(&self, namespace: &str, key: &str, value: &str) -> Result<(), StorageError> {
            self.entries
                .lock()
                .unwrap()
                .insert((namespace.to_string(), key.to_string()), value.to_string());
            Ok(())
        }

        fn retrieve(&self, namespace: &str, key: &str) -> Result<Option<String>, StorageError> {
            Ok(self
                .entries
                .lock()
                .unwrap()
                .get(&(namespace.to_string(), key.to_string()))
                .cloned())
        }

        fn delete(&self, namespace: &str, key: &str) -> Result<(), StorageError> {
            self.entries
                .lock()
                .unwrap()
                .remove(&(namespace.to_string(), key.to_string()));
            Ok(())
        }
    }

    /// #6438 (F7) + #8040: if at-rest encryption-key provisioning fails, `build()`
    /// must FAIL CLOSED — return an error and create no database — rather than
    /// silently opening the SQLite DB (and screenshots) in plaintext. A fake
    /// (empty) `MasterKeyVault` is injected so this test never touches the real
    /// OS keychain (see `with_master_key_vault`'s doc comment) — the
    /// keychain-migration/fallback decision tree itself is covered exhaustively
    /// in `maekon_storage::encryption::sealed_key_tests`.
    #[test]
    fn build_fails_closed_when_key_provisioning_fails() {
        // Unique scratch dir (no tempfile dev-dep in src-tauri).
        let base = std::env::temp_dir().join(format!("maekon-f7-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("create scratch dir");
        // Make `.db_key` unreadable as a key file (a directory) so
        // `load_or_create_sealed`'s file-load errors, simulating a
        // corrupt/inaccessible legacy key during migration (the fake vault
        // reports "no keychain entry yet", so the migration branch is taken).
        std::fs::create_dir(base.join(".db_key")).expect("create blocking .db_key dir");
        let db_path = base.join("test.db");

        let err_msg = match StorageRuntimeBuilder::new(&db_path, &base, 30)
            .with_master_key_vault(Arc::new(FakeVault::default()))
            .build()
        {
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

    /// #8040 regression: `build()` must wire a successful keychain-sealed
    /// migration through end-to-end — the resulting `SqliteStorage` must open
    /// with the SAME key the fake vault now holds, and no plaintext `.db_key`
    /// file must remain.
    #[test]
    fn build_migrates_legacy_key_to_keychain_and_opens_with_it() {
        let base = std::env::temp_dir().join(format!("maekon-8040-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("create scratch dir");
        let legacy = EncryptionKey::from_bytes([4u8; 32]);
        // `EncryptionKey::save_to_file` is crate-private to `maekon-storage`;
        // write the raw 32-byte legacy key format directly (the same on-disk
        // shape `load_or_create`/`load_or_create_sealed` read).
        std::fs::write(base.join(".db_key"), legacy.as_bytes()).unwrap();
        let db_path = base.join("test.db");

        let bundle = StorageRuntimeBuilder::new(&db_path, &base, 30)
            .with_master_key_vault(Arc::new(FakeVault::default()))
            .build()
            .expect("build must succeed and migrate the legacy key into the fake vault");

        assert!(
            !base.join(".db_key").exists(),
            "the legacy plaintext key file must be deleted after a verified migration"
        );
        assert_eq!(
            bundle
                .encryption_key
                .expect("encryption_key must be Some")
                .as_hex()
                .as_str(),
            legacy.as_hex().as_str(),
            "the migrated key used to open SQLite must be byte-identical to the legacy key"
        );

        let _ = std::fs::remove_dir_all(&base);
    }
}
