mod inner;
mod port_impls;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::StorageError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use oneshim_core::models::integration::{
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

    fn save(&self, path: &Path) -> Result<(), StorageError> {
        let serialized = serde_json::to_string_pretty(self).map_err(|err| {
            StorageError::Internal(format!("integration state registry serialization: {err}"))
        })?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp_path = path.with_extension("tmp");
        std::fs::write(&temp_path, serialized)?;
        std::fs::rename(&temp_path, path)?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct IntegrationStateStorePolicy {
    pub max_stored_prompts: usize,
    pub redact_completed_prompt_bodies: bool,
}

impl Default for IntegrationStateStorePolicy {
    fn default() -> Self {
        Self {
            max_stored_prompts: 256,
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
