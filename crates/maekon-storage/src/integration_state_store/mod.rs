mod inner;
mod port_impls;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::encryption::EncryptionKey;
use crate::error::StorageError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use maekon_core::models::integration::{
    IntegrationAckCursor, IntegrationInsightAuditRecord, IntegrationSessionState,
    QueuedIntegrationEgressMessage, StoredProactivePrompt,
};

pub use port_impls::{
    FileIntegrationAuditStore, FileIntegrationCheckpointStore, FileIntegrationInboxStore,
    FileIntegrationOutboxStore, FileIntegrationSessionStore,
};

use inner::FileIntegrationStateInner;

const MAX_AUDIT_RECORDS: usize = 512;

// #7073: magic header that marks an AES-256-GCM-encrypted integration state
// registry file (version 1). Mirrors `FileSecretRegistry`'s `MKSEC1\n` header:
// 7 bytes, human-visible in a hex dump, and never the first bytes of valid JSON
// (which begins with '{'). Files without this header are treated as legacy
// plaintext JSON and upgraded to the encrypted format on the next save.
const INTEGRATION_STATE_MAGIC: &[u8] = b"MKINT1\n";

#[derive(Debug, Default, Serialize, Deserialize)]
struct FileIntegrationStateRegistry {
    version: u32,
    session: Option<IntegrationSessionState>,
    outbox: Vec<QueuedIntegrationEgressMessage>,
    outbox_ack_cursor: Option<IntegrationAckCursor>,
    inbox: BTreeMap<String, StoredProactivePrompt>,
    inbox_ack_cursor: Option<IntegrationAckCursor>,
    producer_checkpoints: BTreeMap<String, String>,
    audit_records: Vec<IntegrationInsightAuditRecord>,
}

impl FileIntegrationStateRegistry {
    fn new() -> Self {
        Self {
            version: 1,
            session: None,
            outbox: Vec::new(),
            outbox_ack_cursor: None,
            inbox: BTreeMap::new(),
            inbox_ack_cursor: None,
            producer_checkpoints: BTreeMap::new(),
            audit_records: Vec::new(),
        }
    }

    /// Load the registry from `path`, transparently handling both the encrypted
    /// binary format (magic header present) and the legacy plaintext JSON format
    /// (no magic header — migration path, upgraded on the next save).
    ///
    /// Returns a default empty registry when the file does not exist.
    /// Returns `Err` when:
    /// - the file starts with the magic header but no key is supplied (fail-closed)
    /// - decryption fails (wrong key or corrupt data)
    /// - JSON parsing fails
    fn load_or_default(path: &Path, key: Option<&EncryptionKey>) -> Result<Self, StorageError> {
        let raw = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Self::new()),
            Err(err) => return Err(err.into()),
        };

        if raw.starts_with(INTEGRATION_STATE_MAGIC) {
            // Encrypted format: a key is required to proceed (fail-closed).
            let key = key.ok_or_else(|| {
                StorageError::Internal(
                    "integration state registry is encrypted (MKINT1 header present) but no \
                     encryption key was supplied — refusing to open without key"
                        .to_string(),
                )
            })?;
            let ciphertext = &raw[INTEGRATION_STATE_MAGIC.len()..];
            let plaintext = key.decrypt(ciphertext).map_err(|e| {
                StorageError::Internal(format!(
                    "failed to decrypt integration state registry: {e} — the encryption key \
                     (.db_key) may have changed or been replaced"
                ))
            })?;
            let json = std::str::from_utf8(&plaintext).map_err(|e| {
                StorageError::Internal(format!(
                    "decrypted integration state registry is not valid UTF-8: {e}"
                ))
            })?;
            serde_json::from_str(json).map_err(|e| {
                StorageError::Internal(format!("integration state registry parse: {e}"))
            })
        } else {
            // Legacy plaintext JSON — migration path. No key required to read; the
            // next save() with a key present upgrades the file to the encrypted
            // format automatically.
            let json = std::str::from_utf8(&raw).map_err(|e| {
                StorageError::Internal(format!(
                    "legacy integration state registry is not valid UTF-8: {e}"
                ))
            })?;
            serde_json::from_str(json).map_err(|e| {
                StorageError::Internal(format!("integration state registry parse: {e}"))
            })
        }
    }

    /// Persist the registry to `path` using the tmp-file + 0o600/DACL + atomic-rename
    /// hardening pattern (mirrors `FileSecretRegistry::save`).
    ///
    /// The registry holds PII/insight state at rest (pending proactive-prompt
    /// bodies derived from the monitored user context, plus insight/audit
    /// records), so it is encrypted at rest:
    /// - When `key` is `Some`, the JSON is AES-256-GCM encrypted and the file is
    ///   prefixed with `INTEGRATION_STATE_MAGIC` (#7073 — this is no longer the
    ///   lone unencrypted-at-rest store in the crate).
    /// - When `key` is `None`, the JSON is written in plaintext and a warning is
    ///   emitted — a degraded / no-key path that should not be reached in
    ///   production (the composition root always supplies the shared key).
    ///
    /// The persisted file is additionally created owner-only: mode 0o600 on Unix
    /// (atomic `create_new` + `mode`, no world-readable window) and an owner-only
    /// DACL on Windows before the rename.
    fn save(&self, path: &Path, key: Option<&EncryptionKey>) -> Result<(), StorageError> {
        let json = serde_json::to_string_pretty(self).map_err(|err| {
            StorageError::Internal(format!("integration state registry serialization: {err}"))
        })?;

        // Produce the bytes to write: either encrypted (magic + ciphertext) or
        // plaintext with a degradation warning (mirrors FileSecretRegistry::save).
        let payload: Vec<u8> = if let Some(key) = key {
            let ciphertext = key.encrypt(json.as_bytes())?;
            let mut out = Vec::with_capacity(INTEGRATION_STATE_MAGIC.len() + ciphertext.len());
            out.extend_from_slice(INTEGRATION_STATE_MAGIC);
            out.extend(ciphertext);
            out
        } else {
            tracing::warn!(
                "integration state store: writing registry as PLAINTEXT (no encryption \
                 key supplied) — this is a degraded/fallback mode; supply an EncryptionKey \
                 to enable at-rest encryption"
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
        // mode in a single open) so the integration state is never world-readable
        // — mirrors FileSecretRegistry::save. On any write error the tmp is
        // cleaned up.
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
                    StorageError::Internal(format!("integration state store tmp create: {e}"))
                })?;
            f.write_all(&payload).map_err(|e| {
                let _ = std::fs::remove_file(&temp_path);
                StorageError::Internal(format!("integration state store tmp write: {e}"))
            })?;
        }

        // Windows: write the payload, then apply an owner-only DACL so the file
        // is not readable via inherited parent-directory ACLs (reuses the shared
        // helper in `encryption.rs`). The payload is ciphertext when a key is
        // present. DACL failure is non-fatal (warn and continue).
        #[cfg(windows)]
        {
            if let Err(e) = std::fs::write(&temp_path, &payload) {
                let _ = std::fs::remove_file(&temp_path);
                return Err(StorageError::Internal(format!(
                    "integration state store tmp write: {e}"
                )));
            }
            if let Err(e) = crate::encryption::set_owner_only_dacl(&temp_path) {
                tracing::warn!("integration state store: failed to set owner-only DACL: {e}");
            }
        }

        // Exotic targets without unix/windows permission models: plain write.
        #[cfg(not(any(unix, windows)))]
        {
            std::fs::write(&temp_path, &payload).map_err(|e| {
                let _ = std::fs::remove_file(&temp_path);
                StorageError::Internal(format!("integration state store tmp write: {e}"))
            })?;
        }

        std::fs::rename(&temp_path, path)?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct IntegrationStateStorePolicy {
    pub max_stored_prompts: usize,
    /// Hard cap on queued outbound egress messages held at the store layer.
    /// The egress coordinator applies its own caps, but the store can be fed
    /// directly (e.g. prompt-receipt writes), so this is the last-resort bound
    /// that keeps the persisted outbox from growing unbounded over an 8h+ run.
    /// Pruning is drop-oldest (FIFO), mirroring the audit-record cap.
    pub max_outbox_messages: usize,
    pub redact_completed_prompt_bodies: bool,
}

impl Default for IntegrationStateStorePolicy {
    fn default() -> Self {
        Self {
            max_stored_prompts: 256,
            max_outbox_messages: 1024,
            redact_completed_prompt_bodies: true,
        }
    }
}

#[derive(Clone)]
pub struct FileIntegrationStateStore {
    inner: Arc<FileIntegrationStateInner>,
}

impl FileIntegrationStateStore {
    /// Create (or open) a file-backed integration state store at `registry_path`.
    ///
    /// `encryption_key` controls at-rest encryption (#7073):
    /// - `Some(key)` — the registry is AES-256-GCM encrypted on every save, and a
    ///   legacy plaintext file is transparently upgraded on the first write.
    /// - `None` — the registry is written as plaintext JSON (degraded mode). The
    ///   production composition root always supplies the shared key.
    pub fn new(
        registry_path: PathBuf,
        encryption_key: Option<Arc<EncryptionKey>>,
    ) -> Result<Self, StorageError> {
        Self::with_policy(
            registry_path,
            IntegrationStateStorePolicy::default(),
            encryption_key,
        )
    }

    pub fn with_policy(
        registry_path: PathBuf,
        policy: IntegrationStateStorePolicy,
        encryption_key: Option<Arc<EncryptionKey>>,
    ) -> Result<Self, StorageError> {
        Ok(Self {
            inner: Arc::new(FileIntegrationStateInner::new(
                registry_path,
                policy,
                encryption_key,
            )?),
        })
    }

    pub fn session_store(&self) -> FileIntegrationSessionStore {
        FileIntegrationSessionStore {
            inner: self.inner.clone(),
        }
    }

    pub fn outbox_store(&self) -> FileIntegrationOutboxStore {
        FileIntegrationOutboxStore {
            inner: self.inner.clone(),
        }
    }

    pub fn inbox_store(&self) -> FileIntegrationInboxStore {
        FileIntegrationInboxStore {
            inner: self.inner.clone(),
        }
    }

    pub fn audit_store(&self) -> FileIntegrationAuditStore {
        FileIntegrationAuditStore {
            inner: self.inner.clone(),
        }
    }

    pub fn checkpoint_store(&self) -> FileIntegrationCheckpointStore {
        FileIntegrationCheckpointStore {
            inner: self.inner.clone(),
        }
    }
}
