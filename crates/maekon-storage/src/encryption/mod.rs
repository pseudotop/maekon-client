//! Local SQLite database encryption key management.
//!
//! # Key storage strategy
//! 1. Key file (`app_data_dir/.db_key`): 32-byte raw key
//! 2. File permissions: 0o600 (owner read/write only) on Unix, owner-only DACL on Windows.
//!    The file is created with restrictive permissions **atomically** — no world-readable
//!    window between creation and chmod (TOCTOU fix, issue #5991).
//!
//! # At-rest encryption (wired)
//! This module owns key generation / storage / loading. The 32-byte key it
//! produces is applied to the SQLite connection via SQLCipher (`PRAGMA key`) in
//! `sqlite::apply_sqlcipher_key` — with fail-closed key verification and legacy
//! plaintext detection — and is activated unconditionally in the production
//! composition root (`storage_runtime`). The database is therefore encrypted at
//! rest; this is no longer "planned" future work.

use crate::error::StorageError;
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

/// #8589 (ADR-030 §7 + Amendment I1): raw-plane per-account AEAD subkey
/// derivation (HKDF-SHA256 of this `EncryptionKey`) + crypto-shred semantics.
pub mod raw_plane;

/// #8040: minimal interface `EncryptionKey::load_or_create_sealed` needs from
/// an OS-keychain-backed secret vault — narrowed from
/// `crate::keychain::KeychainOps` so tests can drive the migration/fallback
/// decision tree with a manual in-memory fake instead of touching the real OS
/// keychain (mirrors `keychain.rs`'s `#[ignore]`-by-default policy for real
/// backend access — real-keychain calls can trigger interactive OS prompts /
/// require an unlocked login session, which `cargo test` must never depend
/// on). Manual mock implementations only — no mockall (ADR-001 §5).
pub trait MasterKeyVault: Send + Sync {
    fn store(&self, namespace: &str, key: &str, value: &str) -> Result<(), StorageError>;
    fn retrieve(&self, namespace: &str, key: &str) -> Result<Option<String>, StorageError>;
    fn delete(&self, namespace: &str, key: &str) -> Result<(), StorageError>;
}

impl MasterKeyVault for crate::keychain::KeychainOps {
    fn store(&self, namespace: &str, key: &str, value: &str) -> Result<(), StorageError> {
        self.store_sync(namespace, key, value)
    }

    fn retrieve(&self, namespace: &str, key: &str) -> Result<Option<String>, StorageError> {
        self.retrieve_sync(namespace, key)
    }

    fn delete(&self, namespace: &str, key: &str) -> Result<(), StorageError> {
        self.delete_sync(namespace, key)
    }
}

/// Fixed keychain namespace for the at-rest master key (#8040). Distinct from
/// the OAuth namespaces `keychain.rs`'s `KNOWN_OAUTH_KEYS` enumerates.
const MASTER_KEY_KEYCHAIN_NAMESPACE: &str = "master_key";

/// Derives a stable, data-dir-scoped keychain entry identifier for the
/// at-rest master key. Multiple maekon profiles/installs on the same OS user
/// account (e.g. a dev data dir alongside a production one) must each seal
/// their OWN key — a single global keychain entry would silently hand one
/// profile another profile's key. SHA-256 is used purely as a stable scoping
/// identifier here (not a security boundary): the path never leaves this
/// process.
fn master_key_keychain_entry(app_data_dir: &Path) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(app_data_dir.to_string_lossy().as_bytes());
    format!("db_key.{}", hex::encode(hasher.finalize()))
}

/// 32-byte AES-256 database encryption key.
///
/// `ZeroizeOnDrop` derive wipes the 32-byte key material when the last owner is
/// dropped, so the at-rest master key never lingers in freed heap memory
/// (finding #6242). `[u8; 32]` already implements `Zeroize`. `Clone` is retained
/// because the key is shared via `Arc` and produced by `from_bytes`/`generate`.
#[derive(Clone, zeroize::ZeroizeOnDrop)]
pub struct EncryptionKey([u8; 32]);

impl EncryptionKey {
    /// Load from the key file, or generate a new key.
    ///
    /// - If the file exists: load it (validating the 32-byte length).
    /// - If the file is absent: generate, then persist to file (Unix: mode 0o600).
    pub fn load_or_create(app_data_dir: &Path) -> Result<Self, StorageError> {
        let key_path = app_data_dir.join(".db_key");

        if key_path.exists() {
            return Self::load_from_file(&key_path);
        }

        let key = Self::generate()?;
        key.save_to_file(&key_path)?;
        tracing::info!("New DB encryption key generated: {:?}", key_path);
        Ok(key)
    }

    /// #8040: load the at-rest master key, preferring the OS keychain (macOS
    /// Keychain / Windows Credential Manager / Linux kernel keyring) over the
    /// plaintext `.db_key` file `load_or_create` writes unconditionally —
    /// same directory as the SQLCipher database and encrypted frame files it
    /// protects.
    ///
    /// Precedence:
    /// 1. Keychain already holds the key → use it (and remove any stray
    ///    plaintext `.db_key` left over from an interrupted migration).
    /// 2. No keychain entry, but a legacy plaintext `.db_key` exists →
    ///    migrate: seal the file's key into the keychain and read it straight
    ///    back to confirm the round trip (`seal`). Only on a VERIFIED round
    ///    trip is the plaintext file deleted. ANY failure (keychain write
    ///    error, readback mismatch, or the keychain being unreachable at all)
    ///    keeps the file in place — existing data must stay decryptable —
    ///    and this launch continues on the file-sourced key; migration
    ///    retries on the next launch.
    /// 3. No keychain entry, no file (fresh install) → generate a new key and
    ///    seal it directly in the keychain. If the keychain itself is
    ///    unavailable (expected on headless Linux/CI without a keyring
    ///    backend), fall back to the pre-#8040 plaintext-file scheme, logged
    ///    explicitly.
    pub fn load_or_create_sealed(
        app_data_dir: &Path,
        keychain: &dyn MasterKeyVault,
    ) -> Result<Self, StorageError> {
        let key_path = app_data_dir.join(".db_key");
        let entry = master_key_keychain_entry(app_data_dir);

        match keychain.retrieve(MASTER_KEY_KEYCHAIN_NAMESPACE, &entry) {
            Ok(Some(hex)) => {
                let key = Self::from_hex_string(&hex)?;
                if key_path.exists() {
                    match std::fs::remove_file(&key_path) {
                        Ok(()) => tracing::info!(
                            "#8040: removed redundant plaintext {key_path:?} — the OS \
                             keychain already holds the master key"
                        ),
                        Err(e) => tracing::warn!(
                            "#8040: keychain holds the master key but the redundant \
                             plaintext {key_path:?} could not be removed: {e}"
                        ),
                    }
                }
                Ok(key)
            }
            Ok(None) if key_path.exists() => {
                // Migration path: legacy plaintext key, no keychain entry yet.
                let file_key = Self::load_from_file(&key_path)?;
                match file_key.seal(keychain, &entry) {
                    Ok(()) => match std::fs::remove_file(&key_path) {
                        Ok(()) => tracing::info!(
                            "#8040: master key migrated from plaintext file to the OS \
                             keychain; {key_path:?} removed"
                        ),
                        Err(e) => tracing::warn!(
                            "#8040: master key sealed in the OS keychain, but the legacy \
                             plaintext {key_path:?} could not be deleted: {e}. It will be \
                             ignored from now on (the keychain copy is authoritative)."
                        ),
                    },
                    Err(e) => tracing::warn!(
                        "#8040: master key keychain migration failed ({e}); continuing on \
                         the existing plaintext {key_path:?} (no data loss — migration \
                         retries on the next launch)"
                    ),
                }
                Ok(file_key)
            }
            Ok(None) => {
                // Fresh install: no keychain entry, no file.
                let key = Self::generate()?;
                match key.seal(keychain, &entry) {
                    Ok(()) => {
                        tracing::info!(
                            "#8040: new master key generated and sealed in the OS keychain \
                             (no plaintext key file written)"
                        );
                        Ok(key)
                    }
                    Err(e) => {
                        tracing::warn!(
                            "#8040: OS keychain unavailable ({e}); falling back to the \
                             plaintext key-file scheme (expected on headless Linux/CI \
                             without a keyring backend)"
                        );
                        key.save_to_file(&key_path)?;
                        tracing::info!("New DB encryption key generated: {:?}", key_path);
                        Ok(key)
                    }
                }
            }
            Err(e) => {
                // Keychain unreachable right now (locked / no backend / etc.) — fall
                // back to the plaintext-file scheme wholesale rather than fail
                // closed: master-key availability across reboots must not depend
                // on an OS feature that may legitimately be absent (headless
                // Linux/CI).
                tracing::warn!(
                    "#8040: OS keychain unavailable ({e}); using the plaintext key-file \
                     scheme (expected on headless Linux/CI without a keyring backend)"
                );
                Self::load_or_create(app_data_dir)
            }
        }
    }

    /// Writes this key's hex encoding into the keychain and reads it straight
    /// back to confirm a successful round trip (#8040) BEFORE the caller is
    /// allowed to treat the keychain copy as authoritative — and, on the
    /// migration path, before the legacy plaintext file is deleted. On a
    /// write success but readback mismatch/failure, the just-written entry is
    /// deleted (best-effort) so no partially-verified state is left behind.
    fn seal(&self, keychain: &dyn MasterKeyVault, entry: &str) -> Result<(), StorageError> {
        let hex = self.as_hex();
        keychain.store(MASTER_KEY_KEYCHAIN_NAMESPACE, entry, hex.as_str())?;
        match keychain.retrieve(MASTER_KEY_KEYCHAIN_NAMESPACE, entry) {
            Ok(Some(verify)) if verify.as_str() == hex.as_str() => Ok(()),
            Ok(_) => {
                let _ = keychain.delete(MASTER_KEY_KEYCHAIN_NAMESPACE, entry);
                Err(StorageError::SecretStore(
                    "keychain readback did not match the key just sealed".into(),
                ))
            }
            Err(e) => {
                let _ = keychain.delete(MASTER_KEY_KEYCHAIN_NAMESPACE, entry);
                Err(e)
            }
        }
    }

    /// Parses a hex-encoded 32-byte key (the wire format `seal`/`as_hex` use
    /// to round-trip through the keychain's string-only storage).
    fn from_hex_string(hex_str: &str) -> Result<Self, StorageError> {
        let bytes = hex::decode(hex_str)
            .map_err(|e| StorageError::Encryption(format!("keychain key hex decode: {e}")))?;
        if bytes.len() != 32 {
            return Err(StorageError::Encryption(format!(
                "keychain key size error: expected 32 bytes, got {} bytes",
                bytes.len()
            )));
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        Ok(Self(key))
    }

    /// Build a key from raw bytes.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// SQLite pragma key format (hex string).
    ///
    /// Returns the hex inside `Zeroizing` so this plaintext copy of the master
    /// key is wiped from the heap when the returned value is dropped (#6242).
    pub fn as_hex(&self) -> Zeroizing<String> {
        Zeroizing::new(self.0.iter().map(|b| format!("{b:02x}")).collect())
    }

    /// Borrow the raw bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Encrypt data with AES-256-GCM.
    /// Output format: nonce(12 bytes) || ciphertext(+16 bytes auth tag)
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, StorageError> {
        use aes_gcm::aead::{Aead, Nonce};
        use aes_gcm::{Aes256Gcm, KeyInit};

        let cipher = Aes256Gcm::new_from_slice(&self.0)
            .map_err(|e| StorageError::Encryption(format!("cipher init: {e}")))?;

        let mut nonce_bytes = [0u8; 12];
        getrandom::fill(&mut nonce_bytes)
            .map_err(|e| StorageError::Encryption(format!("nonce generation: {e}")))?;
        let nonce: Nonce<Aes256Gcm> = nonce_bytes.into();

        let ciphertext = cipher
            .encrypt(&nonce, plaintext)
            .map_err(|e| StorageError::Encryption(format!("encrypt: {e}")))?;

        let mut result = Vec::with_capacity(12 + ciphertext.len());
        result.extend_from_slice(&nonce_bytes);
        result.extend(ciphertext);
        Ok(result)
    }

    /// Decrypt data produced by `encrypt()` with AES-256-GCM.
    ///
    /// The plaintext is returned inside `Zeroizing` so the decrypted secret
    /// material (e.g. the secret-registry plaintext) is wiped from the heap when
    /// the returned buffer is dropped, rather than lingering in freed memory
    /// (#6242). `Zeroizing<Vec<u8>>` derefs to `&[u8]`/`Vec<u8>`, so callers that
    /// only read the bytes need no changes.
    pub fn decrypt(&self, data: &[u8]) -> Result<Zeroizing<Vec<u8>>, StorageError> {
        if data.len() < 12 {
            return Err(StorageError::Encryption(
                "ciphertext too short (< 12 bytes)".into(),
            ));
        }

        use aes_gcm::aead::{Aead, Nonce};
        use aes_gcm::{Aes256Gcm, KeyInit};

        let (nonce_bytes, ciphertext) = data.split_at(12);
        let cipher = Aes256Gcm::new_from_slice(&self.0)
            .map_err(|e| StorageError::Encryption(format!("cipher init: {e}")))?;
        let nonce = <&Nonce<Aes256Gcm>>::try_from(nonce_bytes)
            .map_err(|e| StorageError::Encryption(format!("nonce parse: {e}")))?;

        cipher
            .decrypt(nonce, ciphertext)
            .map(Zeroizing::new)
            .map_err(|e| StorageError::Encryption(format!("decrypt: {e}")))
    }

    fn generate() -> Result<Self, StorageError> {
        let mut key = [0u8; 32];
        getrandom::fill(&mut key).map_err(|e| {
            StorageError::Encryption(format!("OS random number generation failed: {e}"))
        })?;
        Ok(Self(key))
    }

    fn load_from_file(path: &PathBuf) -> Result<Self, StorageError> {
        let bytes = std::fs::read(path)
            .map_err(|e| StorageError::Internal(format!("Key file read failed ({path:?}): {e}")))?;

        if bytes.len() != 32 {
            return Err(StorageError::Internal(format!(
                "Key file size error: expected 32 bytes, got {} bytes",
                bytes.len()
            )));
        }

        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        Ok(Self(key))
    }

    fn save_to_file(&self, path: &PathBuf) -> Result<(), StorageError> {
        // Create the file with restrictive permissions BEFORE any bytes are written so
        // there is never a world-readable window (TOCTOU fix, issue #5991).
        //
        // If the file already exists (unexpected on the new-key path, but possible in
        // concurrent or retry scenarios) we remove it and recreate rather than
        // opening/truncating an existing file whose permissions may be too open.
        use std::io::Write as _;

        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            // O_CREAT | O_EXCL | mode 0o600 in one syscall — atomically restrictive.
            let file_result = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path);

            let mut file = match file_result {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    // Remove stale file and retry with the same atomic open.
                    std::fs::remove_file(path).map_err(|re| {
                        StorageError::Internal(format!(
                            "Key file removal for overwrite failed ({path:?}): {re}"
                        ))
                    })?;
                    std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(path)
                        .map_err(|re| {
                            StorageError::Internal(format!(
                                "Key file create failed after removal ({path:?}): {re}"
                            ))
                        })?
                }
                Err(e) => {
                    return Err(StorageError::Internal(format!(
                        "Key file create failed ({path:?}): {e}"
                    )))
                }
            };

            file.write_all(&self.0).map_err(|e| {
                StorageError::Internal(format!("Key file write failed ({path:?}): {e}"))
            })?;
        }

        #[cfg(windows)]
        {
            // On Windows we must apply the owner-only DACL before writing any bytes.
            // We write to a temporary path first so we can set ACLs on it while it
            // is still empty, then rename atomically.  If any step fails we treat it
            // as a hard error — falling back to an unprotected write is not acceptable.
            let tmp_path = path.with_extension("tmp_key");

            // Remove any leftover temp file from a previous aborted attempt.
            let _ = std::fs::remove_file(&tmp_path);

            // Create the empty temp file, apply owner-only DACL, then write. The
            // temp file inherits the parent directory ACL for the brief window
            // before set_owner_only_dacl runs, but it is EMPTY during that window —
            // no key bytes exist until after the DACL is applied, so no secret is
            // ever exposed under the inherited ACL.
            std::fs::File::create(&tmp_path).map_err(|e| {
                StorageError::Internal(format!("Key temp file create failed ({tmp_path:?}): {e}"))
            })?;

            // Hard error if DACL cannot be set — do not leave the file unprotected.
            set_owner_only_dacl(&tmp_path).map_err(|e| {
                let _ = std::fs::remove_file(&tmp_path);
                StorageError::Internal(format!(
                    "Key file owner-only DACL failed ({tmp_path:?}): {e}"
                ))
            })?;

            // Write bytes into the now-protected temp file.
            {
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .open(&tmp_path)
                    .map_err(|e| {
                        let _ = std::fs::remove_file(&tmp_path);
                        StorageError::Internal(format!(
                            "Key temp file open for write failed ({tmp_path:?}): {e}"
                        ))
                    })?;
                file.write_all(&self.0).map_err(|e| {
                    let _ = std::fs::remove_file(&tmp_path);
                    StorageError::Internal(format!("Key file write failed ({tmp_path:?}): {e}"))
                })?;
            }

            // Remove any existing destination then rename into place.
            let _ = std::fs::remove_file(path);
            std::fs::rename(&tmp_path, path).map_err(|e| {
                let _ = std::fs::remove_file(&tmp_path);
                StorageError::Internal(format!(
                    "Key file rename failed ({tmp_path:?} -> {path:?}): {e}"
                ))
            })?;
        }

        #[cfg(not(any(unix, windows)))]
        {
            // Fallback for exotic platforms: best-effort write (no atomicity guarantee).
            std::fs::write(path, self.0).map_err(|e| {
                StorageError::Internal(format!("Key file write failed ({path:?}): {e}"))
            })?;
        }

        Ok(())
    }
}

/// Set an owner-only DACL on a file or directory (Windows equivalent of Unix
/// `chmod 0o600`).
///
/// Thin delegation to the single canonical primitive in
/// `maekon_core::secure_file::set_owner_only_dacl` (#7101). The owner-only DACL
/// logic used to be duplicated here as a `pub(crate)` copy; it now lives once in
/// `maekon-core` so the two implementations cannot diverge. The `CoreError`
/// returned by the primitive is mapped into `StorageError::Core` so every
/// in-crate caller (`save_to_file`, `keychain`, `file_secret_store`,
/// `temp_file_projection`, `integration_state_store`, `frame_storage::io`) keeps
/// its existing `StorageError` contract unchanged.
#[cfg(windows)]
pub(crate) fn set_owner_only_dacl(path: &std::path::Path) -> Result<(), StorageError> {
    maekon_core::secure_file::set_owner_only_dacl(path).map_err(StorageError::Core)
}

// Safe Debug implementation so the key is never printed to logs.
impl std::fmt::Debug for EncryptionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EncryptionKey([redacted])")
    }
}

// #8040 (ADR-013 LOC gate, crates/maekon-lint/adr013_loc_baseline.json): tests
// live in `tests.rs` (mirrors `crates/maekon-analysis/src/adaptive_search`'s
// mod.rs+tests.rs split) so this production file stays under the 900-line
// unbaselined-giant threshold.
#[cfg(test)]
#[path = "tests.rs"]
mod tests;
