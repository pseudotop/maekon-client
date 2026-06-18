//! Explicit file-backed secret store.
//!
//! This backend is intended for headless, CI, or explicit fallback scenarios.
//! It is not the desktop-default backend.
//!
//! # On-disk format
//!
//! ## Encrypted (normal operation, key supplied)
//! ```text
//! [SECRET_FILE_MAGIC (7 bytes)] [AES-256-GCM payload]
//! ```
//! The AES-256-GCM payload is the output of `EncryptionKey::encrypt`, which
//! prepends a 12-byte random nonce before the ciphertext+tag.  The plaintext
//! is `serde_json::to_string_pretty(&FileSecretRegistry)`.
//!
//! ## Legacy plaintext (migration path)
//! Files that do NOT start with `SECRET_FILE_MAGIC` are treated as legacy
//! plaintext JSON.  They are loaded on first open and automatically rewritten
//! in the encrypted format on the next `save` (whenever a key is present).
//!
//! ## Degraded / no-key (not recommended)
//! If `new` is called with `None` for the encryption key, saves write
//! plaintext JSON and emit a `tracing::warn!`.  Encrypted files cannot be
//! opened without a key (fail-closed).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::encryption::EncryptionKey;
use crate::error::StorageError;
use async_trait::async_trait;
use maekon_core::error::CoreError;
use maekon_core::ports::secret_store::SecretStore;
use serde::{Deserialize, Serialize};

// Magic header that marks an encrypted secret registry file (version 1).
// 7 bytes chosen to be human-visible in a hex dump and unlikely to appear at
// the start of valid UTF-8 JSON (which always begins with '{').
const SECRET_FILE_MAGIC: &[u8] = b"MKSEC1\n";

#[derive(Debug, Default, Serialize, Deserialize)]
struct FileSecretRegistry {
    version: u32,
    namespaces: HashMap<String, HashMap<String, String>>,
}

impl FileSecretRegistry {
    fn new() -> Self {
        Self {
            version: 1,
            namespaces: HashMap::new(),
        }
    }

    /// Load the registry from `path`, transparently handling both the
    /// encrypted binary format (magic header present) and the legacy
    /// plaintext JSON format (no magic header, migration path).
    ///
    /// Returns a default empty registry when the file does not exist.
    /// Returns `Err` when:
    /// - the file starts with the magic header but no key is supplied (fail-closed)
    /// - decryption fails (wrong key or corrupt data)
    /// - JSON parsing fails
    fn load_or_default(path: &Path, key: Option<&EncryptionKey>) -> Result<Self, StorageError> {
        let raw = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::new());
            }
            Err(err) => return Err(err.into()),
        };

        if raw.starts_with(SECRET_FILE_MAGIC) {
            // Encrypted format: a key is required to proceed (fail-closed).
            let key = key.ok_or_else(|| {
                StorageError::SecretStore(
                    "secret registry is encrypted (MKSEC1 header present) but no \
                     encryption key was supplied — refusing to open without key"
                        .to_string(),
                )
            })?;
            let ciphertext = &raw[SECRET_FILE_MAGIC.len()..];
            let plaintext = key.decrypt(ciphertext).map_err(|e| {
                StorageError::SecretStore(format!(
                    "failed to decrypt secret registry: {e} — the encryption key \
                     (.db_key) may have changed or been replaced; restore your backup \
                     of .db_key to recover the stored secrets"
                ))
            })?;
            let json = std::str::from_utf8(&plaintext).map_err(|e| {
                StorageError::SecretStore(format!(
                    "decrypted secret registry is not valid UTF-8: {e}"
                ))
            })?;
            serde_json::from_str(json).map_err(|e| {
                StorageError::SecretStore(format!(
                    "decrypted secret registry JSON parse failed: {e}"
                ))
            })
        } else {
            // Legacy plaintext JSON — migration path.  No key required for reading;
            // the next save() with a key present will upgrade the file automatically.
            let json = std::str::from_utf8(&raw).map_err(|e| {
                StorageError::SecretStore(format!("legacy secret registry is not valid UTF-8: {e}"))
            })?;
            serde_json::from_str(json)
                .map_err(|e| StorageError::SecretStore(format!("file secret registry parse: {e}")))
        }
    }

    /// Persist the registry to `path` using the tmp-file + 0o600/DACL + atomic-rename
    /// hardening pattern.
    ///
    /// When `key` is `Some`, the JSON is AES-256-GCM encrypted and the file is
    /// prefixed with `SECRET_FILE_MAGIC`.
    ///
    /// When `key` is `None`, the JSON is written in plaintext and a warning is
    /// emitted — this is a degraded / no-key path and is not recommended for
    /// production use.
    fn save(&self, path: &Path, key: Option<&EncryptionKey>) -> Result<(), StorageError> {
        let json = serde_json::to_string_pretty(self).map_err(|e| {
            StorageError::SecretStore(format!("file secret registry serialization: {e}"))
        })?;

        // Produce the bytes to write: either encrypted (magic + ciphertext) or
        // plaintext with a degradation warning.
        let payload: Vec<u8> = if let Some(key) = key {
            let ciphertext = key.encrypt(json.as_bytes())?;
            let mut out = Vec::with_capacity(SECRET_FILE_MAGIC.len() + ciphertext.len());
            out.extend_from_slice(SECRET_FILE_MAGIC);
            out.extend(ciphertext);
            out
        } else {
            tracing::warn!(
                "FileSecretStore: writing secret registry as PLAINTEXT (no encryption \
                 key supplied) — this is a degraded/fallback mode; supply an \
                 EncryptionKey to enable at-rest encryption"
            );
            json.into_bytes()
        };

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp_path = path.with_extension("tmp");
        // Remove any orphaned tmp from a previously aborted save so the atomic
        // create below starts clean (no stale contents / inherited permissions).
        let _ = std::fs::remove_file(&temp_path);

        // Unix: create the tmp file with mode 0o600 ATOMICALLY (O_CREAT|O_EXCL +
        // mode in a single open) so there is never a window where the secrets tmp
        // is world-readable — mirrors EncryptionKey::save_to_file. Previously the
        // file was created with the umask default and chmod'd afterwards, leaving a
        // brief world-readable window. On any write error the tmp is cleaned up.
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temp_path)
                .map_err(|e| {
                    StorageError::SecretStore(format!("file secret store tmp create: {e}"))
                })?;
            f.write_all(&payload).map_err(|e| {
                let _ = std::fs::remove_file(&temp_path);
                StorageError::SecretStore(format!("file secret store tmp write: {e}"))
            })?;
        }

        // Windows: write the payload, then apply an owner-only DACL so the secrets
        // file is not readable via inherited parent-directory ACLs (reuses the
        // shared helper in `encryption.rs`). The payload is ciphertext when a key
        // is present. DACL failure is non-fatal (warn and continue).
        #[cfg(windows)]
        {
            if let Err(e) = std::fs::write(&temp_path, &payload) {
                let _ = std::fs::remove_file(&temp_path);
                return Err(StorageError::SecretStore(format!(
                    "file secret store tmp write: {e}"
                )));
            }
            if let Err(e) = crate::encryption::set_owner_only_dacl(&temp_path) {
                tracing::warn!("file secret store: failed to set owner-only DACL: {e}");
            }
        }

        // Exotic targets without unix/windows permission models: plain write.
        #[cfg(not(any(unix, windows)))]
        {
            std::fs::write(&temp_path, &payload).map_err(|e| {
                let _ = std::fs::remove_file(&temp_path);
                StorageError::SecretStore(format!("file secret store tmp write: {e}"))
            })?;
        }

        std::fs::rename(&temp_path, path)?;
        Ok(())
    }

    fn store(&mut self, namespace: &str, key: &str, value: &str) {
        self.namespaces
            .entry(namespace.to_string())
            .or_default()
            .insert(key.to_string(), value.to_string());
    }

    fn retrieve(&self, namespace: &str, key: &str) -> Option<String> {
        self.namespaces
            .get(namespace)
            .and_then(|keys| keys.get(key))
            .cloned()
    }

    fn delete(&mut self, namespace: &str, key: &str) {
        if let Some(keys) = self.namespaces.get_mut(namespace) {
            keys.remove(key);
            if keys.is_empty() {
                self.namespaces.remove(namespace);
            }
        }
    }

    fn delete_namespace(&mut self, namespace: &str) {
        self.namespaces.remove(namespace);
    }
}

struct FileSecretInner {
    registry_path: PathBuf,
    /// Optional AES-256-GCM encryption key.  When `Some`, saves write the
    /// encrypted binary format with `SECRET_FILE_MAGIC`.  When `None`, saves
    /// write plaintext (degraded mode) and a warning is emitted.
    encryption_key: Option<Arc<EncryptionKey>>,
    registry: parking_lot::Mutex<FileSecretRegistry>,
}

impl FileSecretInner {
    fn new(
        registry_path: PathBuf,
        encryption_key: Option<Arc<EncryptionKey>>,
    ) -> Result<Self, StorageError> {
        if let Some(parent) = registry_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let registry =
            FileSecretRegistry::load_or_default(&registry_path, encryption_key.as_deref())?;
        Ok(Self {
            registry_path,
            encryption_key,
            registry: parking_lot::Mutex::new(registry),
        })
    }

    fn store_sync(&self, namespace: &str, key: &str, value: &str) -> Result<(), StorageError> {
        let mut registry = self.registry.lock();
        registry.store(namespace, key, value);
        registry.save(&self.registry_path, self.encryption_key.as_deref())
    }

    fn retrieve_sync(&self, namespace: &str, key: &str) -> Option<String> {
        self.registry.lock().retrieve(namespace, key)
    }

    fn delete_sync(&self, namespace: &str, key: &str) -> Result<(), StorageError> {
        let mut registry = self.registry.lock();
        registry.delete(namespace, key);
        registry.save(&self.registry_path, self.encryption_key.as_deref())
    }

    fn delete_namespace_sync(&self, namespace: &str) -> Result<(), StorageError> {
        let mut registry = self.registry.lock();
        registry.delete_namespace(namespace);
        registry.save(&self.registry_path, self.encryption_key.as_deref())
    }
}

#[derive(Clone)]
pub struct FileSecretStore {
    inner: Arc<FileSecretInner>,
}

impl FileSecretStore {
    /// Create (or open) a file-backed secret store at `registry_path`.
    ///
    /// `encryption_key` controls at-rest encryption:
    /// - `Some(key)` — the registry is AES-256-GCM encrypted on every save.
    ///   Legacy plaintext files (written without a key) are transparently
    ///   upgraded to the encrypted format on the first write.
    /// - `None` — the registry is written as plaintext JSON (degraded mode).
    ///   Encrypted files **cannot** be opened in this mode; `new` returns
    ///   `Err` if the file already has the `MKSEC1` magic header (fail-closed).
    ///
    /// The canonical call site should obtain a key via
    /// `EncryptionKey::load_or_create(config_dir)` and wrap it in
    /// `Some(Arc::new(...))`.
    pub fn new(
        registry_path: PathBuf,
        encryption_key: Option<Arc<EncryptionKey>>,
    ) -> Result<Self, StorageError> {
        Ok(Self {
            inner: Arc::new(FileSecretInner::new(registry_path, encryption_key)?),
        })
    }
}

#[async_trait]
impl SecretStore for FileSecretStore {
    async fn store(&self, namespace: &str, key: &str, value: &str) -> Result<(), CoreError> {
        let inner = self.inner.clone();
        let namespace = namespace.to_string();
        let key = key.to_string();
        let value = value.to_string();
        tokio::task::spawn_blocking(move || inner.store_sync(&namespace, &key, &value))
            .await
            .map_err(|e| CoreError::SecretStoreError {
                code: maekon_core::error_codes::SecretCode::Failed,
                message: format!("spawn_blocking: {e}"),
            })?
            .map_err(CoreError::from)
    }

    async fn retrieve(&self, namespace: &str, key: &str) -> Result<Option<String>, CoreError> {
        let inner = self.inner.clone();
        let namespace = namespace.to_string();
        let key = key.to_string();
        tokio::task::spawn_blocking(move || Ok(inner.retrieve_sync(&namespace, &key)))
            .await
            .map_err(|e| CoreError::SecretStoreError {
                code: maekon_core::error_codes::SecretCode::Failed,
                message: format!("spawn_blocking: {e}"),
            })?
    }

    async fn delete(&self, namespace: &str, key: &str) -> Result<(), CoreError> {
        let inner = self.inner.clone();
        let namespace = namespace.to_string();
        let key = key.to_string();
        tokio::task::spawn_blocking(move || inner.delete_sync(&namespace, &key))
            .await
            .map_err(|e| CoreError::SecretStoreError {
                code: maekon_core::error_codes::SecretCode::Failed,
                message: format!("spawn_blocking: {e}"),
            })?
            .map_err(CoreError::from)
    }

    async fn delete_namespace(&self, namespace: &str) -> Result<(), CoreError> {
        let inner = self.inner.clone();
        let namespace = namespace.to_string();
        tokio::task::spawn_blocking(move || inner.delete_namespace_sync(&namespace))
            .await
            .map_err(|e| CoreError::SecretStoreError {
                code: maekon_core::error_codes::SecretCode::Failed,
                message: format!("spawn_blocking: {e}"),
            })?
            .map_err(CoreError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::ports::secret_store::SecretStore;

    fn test_key() -> Arc<EncryptionKey> {
        Arc::new(EncryptionKey::from_bytes([0x42; 32]))
    }

    // ── existing tests (updated to pass encryption key) ────────────────────────

    #[tokio::test]
    async fn file_secret_store_roundtrips_values() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store =
            FileSecretStore::new(temp_dir.path().join("secrets.json"), Some(test_key())).unwrap();

        store
            .store("provider/openai/default", "api_key", "sk-test")
            .await
            .unwrap();

        let value = store
            .retrieve("provider/openai/default", "api_key")
            .await
            .unwrap();
        assert_eq!(value.as_deref(), Some("sk-test"));
    }

    #[tokio::test]
    async fn file_secret_store_persists_to_disk() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("secrets.json");

        FileSecretStore::new(path.clone(), Some(test_key()))
            .unwrap()
            .store("provider/openai/default", "api_key", "sk-test")
            .await
            .unwrap();

        let reloaded = FileSecretStore::new(path, Some(test_key())).unwrap();
        let value = reloaded
            .retrieve("provider/openai/default", "api_key")
            .await
            .unwrap();
        assert_eq!(value.as_deref(), Some("sk-test"));
    }

    #[tokio::test]
    async fn file_secret_store_delete_namespace_removes_only_target_namespace() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store =
            FileSecretStore::new(temp_dir.path().join("secrets.json"), Some(test_key())).unwrap();

        store
            .store("provider/openai/default", "api_key", "a")
            .await
            .unwrap();
        store
            .store("provider/anthropic/default", "api_key", "b")
            .await
            .unwrap();

        store
            .delete_namespace("provider/openai/default")
            .await
            .unwrap();

        assert_eq!(
            store
                .retrieve("provider/openai/default", "api_key")
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            store
                .retrieve("provider/anthropic/default", "api_key")
                .await
                .unwrap()
                .as_deref(),
            Some("b")
        );
    }

    /// Net-5: on Unix the persisted secrets file must be owner-only (0o600).
    /// (The Windows owner-only DACL path is applied via the shared
    /// `encryption::set_owner_only_dacl` helper and exercised on Windows.)
    #[cfg(unix)]
    #[tokio::test]
    async fn file_secret_store_is_owner_only_on_unix() {
        use std::os::unix::fs::PermissionsExt;
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("secrets.json");
        let store = FileSecretStore::new(path.clone(), Some(test_key())).unwrap();
        store
            .store("provider/openai/default", "api_key", "sk-test")
            .await
            .unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        // Only the owner rw bits may be set; group/other must be clear.
        assert_eq!(
            mode & 0o777,
            0o600,
            "secrets file must be 0o600, got {:o}",
            mode & 0o777
        );
    }

    // ── new security tests ──────────────────────────────────────────────────────

    /// The raw on-disk bytes must start with the magic header and must not
    /// contain any cleartext representation of the stored secret or key name.
    #[tokio::test]
    async fn on_disk_blob_is_encrypted_not_plaintext() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("secrets.json");
        let store = FileSecretStore::new(path.clone(), Some(test_key())).unwrap();

        store
            .store(
                "provider/openai/default",
                "api_key",
                "sk-supersecretvalue123",
            )
            .await
            .unwrap();

        let raw = std::fs::read(&path).expect("secrets file must exist after store");

        // File must begin with the version-tagged binary magic header.
        assert!(
            raw.starts_with(SECRET_FILE_MAGIC),
            "on-disk file must start with SECRET_FILE_MAGIC (MKSEC1\\n), got first 16 bytes: {:?}",
            &raw[..raw.len().min(16)]
        );

        // The secret value must NOT appear in cleartext anywhere in the file.
        assert!(
            !raw.windows(b"sk-supersecretvalue123".len())
                .any(|w| w == b"sk-supersecretvalue123"),
            "cleartext secret value must not appear in the encrypted file"
        );

        // The key name must NOT appear in cleartext anywhere in the file.
        assert!(
            !raw.windows(b"api_key".len()).any(|w| w == b"api_key"),
            "cleartext key name 'api_key' must not appear in the encrypted file"
        );
    }

    /// A legacy plaintext file (written without a magic header) must be
    /// transparently loaded, and the next save must upgrade the file to the
    /// encrypted format.  Both the old value and a newly stored value must be
    /// retrievable after the upgrade.
    #[tokio::test]
    async fn legacy_plaintext_file_is_migrated() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("secrets.json");
        let key = test_key();

        // Write a legacy plaintext registry directly, bypassing FileSecretStore.
        let legacy_registry = FileSecretRegistry {
            version: 1,
            namespaces: {
                let mut ns = HashMap::new();
                let mut kv = HashMap::new();
                kv.insert("api_key".to_string(), "sk-legacy-value".to_string());
                ns.insert("provider/openai/default".to_string(), kv);
                ns
            },
        };
        let legacy_json = serde_json::to_string_pretty(&legacy_registry).unwrap();
        std::fs::write(&path, &legacy_json).expect("write legacy plaintext registry");

        // Verify the file does NOT yet have the magic header (it's plaintext).
        let before_raw = std::fs::read(&path).unwrap();
        assert!(
            !before_raw.starts_with(SECRET_FILE_MAGIC),
            "pre-condition: legacy file must not have the magic header"
        );

        // Open with a key — the legacy plaintext file should load successfully.
        let store = FileSecretStore::new(path.clone(), Some(Arc::clone(&key))).unwrap();

        // Verify the legacy value is accessible.
        let legacy_val = store
            .retrieve("provider/openai/default", "api_key")
            .await
            .unwrap();
        assert_eq!(
            legacy_val.as_deref(),
            Some("sk-legacy-value"),
            "legacy value must be retrievable after transparent plaintext load"
        );

        // Store a new value — this triggers an encrypted save (auto-upgrade).
        store
            .store("provider/anthropic/default", "api_key", "sk-new-value")
            .await
            .unwrap();

        // The file must now start with the magic header (upgraded).
        let after_raw = std::fs::read(&path).unwrap();
        assert!(
            after_raw.starts_with(SECRET_FILE_MAGIC),
            "file must be upgraded to encrypted format after first write with key"
        );

        // A fresh store instance (same key) must surface both old and new values.
        let store2 = FileSecretStore::new(path.clone(), Some(Arc::clone(&key))).unwrap();

        let legacy_still = store2
            .retrieve("provider/openai/default", "api_key")
            .await
            .unwrap();
        assert_eq!(
            legacy_still.as_deref(),
            Some("sk-legacy-value"),
            "legacy value must survive the encrypted upgrade"
        );

        let new_val = store2
            .retrieve("provider/anthropic/default", "api_key")
            .await
            .unwrap();
        assert_eq!(
            new_val.as_deref(),
            Some("sk-new-value"),
            "new value stored after upgrade must be retrievable"
        );
    }

    /// Opening an encrypted file (MKSEC1 header) without supplying a key must
    /// fail with an error rather than silently returning an empty registry.
    #[tokio::test]
    async fn encrypted_file_without_key_fails_closed() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("secrets.json");

        // Create and populate an encrypted store.
        {
            let store = FileSecretStore::new(path.clone(), Some(test_key())).unwrap();
            store
                .store("provider/openai/default", "api_key", "sk-sensitive")
                .await
                .unwrap();
        }

        // Verify the file is indeed encrypted.
        let raw = std::fs::read(&path).unwrap();
        assert!(
            raw.starts_with(SECRET_FILE_MAGIC),
            "pre-condition: file must be encrypted"
        );

        // Opening without a key must return Err (fail-closed), not Ok with empty store.
        // Bind the error and assert the correct variant + that it names the
        // MKSEC1 header so the diagnostic is unambiguous.
        // `let-else` (not unwrap_err) because the Ok type FileSecretStore is
        // intentionally not `Debug` (it must never format its secret contents).
        let Err(err) = FileSecretStore::new(path.clone(), None) else {
            panic!("opening an encrypted file without a key must fail-closed, not return Ok");
        };
        assert!(
            matches!(err, StorageError::SecretStore(_)),
            "expected StorageError::SecretStore for no-key encrypted open, got: {err}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("MKSEC1"),
            "error message must mention the MKSEC1 magic header; got: {msg}"
        );
    }

    /// Opening an encrypted file with the wrong key must fail (AES-GCM auth
    /// tag mismatch), confirming that wrong-key access is rejected.
    #[tokio::test]
    async fn wrong_key_fails_closed() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("secrets.json");

        let key_a = Arc::new(EncryptionKey::from_bytes([0xAA; 32]));
        let key_b = Arc::new(EncryptionKey::from_bytes([0xBB; 32]));

        // Write with key A.
        {
            let store = FileSecretStore::new(path.clone(), Some(Arc::clone(&key_a))).unwrap();
            store
                .store("provider/openai/default", "api_key", "sk-secret-a")
                .await
                .unwrap();
        }

        // Opening with key B (different key) must return Err (AES-GCM auth-tag
        // mismatch).  Bind and assert the variant + that the message mentions
        // decryption failure so the root cause is explicit.
        // `let-else` (not unwrap_err) because FileSecretStore is intentionally not `Debug`.
        let Err(err) = FileSecretStore::new(path.clone(), Some(Arc::clone(&key_b))) else {
            panic!("opening an encrypted file with the wrong key must fail-closed, not return Ok");
        };
        assert!(
            matches!(err, StorageError::SecretStore(_)),
            "expected StorageError::SecretStore for wrong-key decrypt failure, got: {err}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("decrypt"),
            "error message must mention decryption failure; got: {msg}"
        );
    }
}
