mod inner;
mod port_impls;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::Arc;

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

    fn load_or_default(path: &Path) -> Result<Self, StorageError> {
        match std::fs::read_to_string(path) {
            Ok(contents) => serde_json::from_str(&contents).map_err(|err| {
                StorageError::Internal(format!("integration state registry parse: {err}"))
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::new()),
            Err(err) => Err(err.into()),
        }
    }

    /// Persist the registry to `path` using the tmp-file + 0o600/DACL + atomic-rename
    /// hardening pattern (mirrors `FileSecretRegistry::save`).
    ///
    /// The registry holds PII/insight state at rest, so the persisted file is
    /// created owner-only: mode 0o600 on Unix (atomic `create_new` + `mode`, no
    /// world-readable window) and an owner-only DACL on Windows before the
    /// rename. Contents are still plaintext JSON; at-rest encryption is future
    /// work (would mirror `FileSecretRegistry`'s AES-256-GCM path).
    fn save(&self, path: &Path) -> Result<(), StorageError> {
        let serialized = serde_json::to_string_pretty(self).map_err(|err| {
            StorageError::Internal(format!("integration state registry serialization: {err}"))
        })?;
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
            f.write_all(serialized.as_bytes()).map_err(|e| {
                let _ = std::fs::remove_file(&temp_path);
                StorageError::Internal(format!("integration state store tmp write: {e}"))
            })?;
        }

        // Windows: write the payload, then apply an owner-only DACL so the file
        // is not readable via inherited parent-directory ACLs (reuses the shared
        // helper in `encryption.rs`). DACL failure is non-fatal (warn and continue).
        #[cfg(windows)]
        {
            if let Err(e) = std::fs::write(&temp_path, serialized.as_bytes()) {
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
            std::fs::write(&temp_path, serialized.as_bytes()).map_err(|e| {
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
    pub fn new(registry_path: PathBuf) -> Result<Self, StorageError> {
        Self::with_policy(registry_path, IntegrationStateStorePolicy::default())
    }

    pub fn with_policy(
        registry_path: PathBuf,
        policy: IntegrationStateStorePolicy,
    ) -> Result<Self, StorageError> {
        Ok(Self {
            inner: Arc::new(FileIntegrationStateInner::new(registry_path, policy)?),
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
