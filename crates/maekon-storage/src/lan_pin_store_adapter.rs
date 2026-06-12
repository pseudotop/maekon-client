//! `LanPinStorePort` adapter backed by `SqliteStorage`.

use std::sync::Arc;

use async_trait::async_trait;
use maekon_core::error::CoreError;
use maekon_core::ports::lan_pin_store::LanPinStorePort;

use crate::sqlite::SqliteStorage;

pub struct LanPinStoreAdapter {
    storage: Arc<SqliteStorage>,
}

impl LanPinStoreAdapter {
    pub fn new(storage: Arc<SqliteStorage>) -> Self {
        Self { storage }
    }
}

#[async_trait]
impl LanPinStorePort for LanPinStoreAdapter {
    async fn get_pin(&self, device_id: &str) -> Result<Option<(String, bool)>, CoreError> {
        let storage = self.storage.clone();
        let device_id = device_id.to_string();
        tokio::task::spawn_blocking(move || storage.get_lan_pin(&device_id))
            .await
            .map_err(|e| CoreError::Storage {
                code: maekon_core::error_codes::StorageCode::Failed,
                message: format!("spawn_blocking join: {e}"),
            })?
            .map_err(CoreError::from)
    }

    async fn upsert_pin(&self, device_id: &str, fingerprint: &str) -> Result<(), CoreError> {
        let storage = self.storage.clone();
        let device_id = device_id.to_string();
        let fingerprint = fingerprint.to_string();
        tokio::task::spawn_blocking(move || storage.upsert_lan_pin(&device_id, &fingerprint))
            .await
            .map_err(|e| CoreError::Storage {
                code: maekon_core::error_codes::StorageCode::Failed,
                message: format!("spawn_blocking join: {e}"),
            })?
            .map_err(CoreError::from)
    }

    async fn revoke_pin(&self, device_id: &str) -> Result<(), CoreError> {
        let storage = self.storage.clone();
        let device_id = device_id.to_string();
        tokio::task::spawn_blocking(move || storage.revoke_lan_pin(&device_id))
            .await
            .map_err(|e| CoreError::Storage {
                code: maekon_core::error_codes::StorageCode::Failed,
                message: format!("spawn_blocking join: {e}"),
            })?
            .map_err(CoreError::from)
    }

    async fn clear_pin(&self, device_id: &str) -> Result<(), CoreError> {
        let storage = self.storage.clone();
        let device_id = device_id.to_string();
        tokio::task::spawn_blocking(move || storage.clear_lan_pin(&device_id))
            .await
            .map_err(|e| CoreError::Storage {
                code: maekon_core::error_codes::StorageCode::Failed,
                message: format!("spawn_blocking join: {e}"),
            })?
            .map_err(CoreError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter() -> LanPinStoreAdapter {
        LanPinStoreAdapter::new(Arc::new(
            SqliteStorage::open_in_memory(30).expect("in-memory sqlite"),
        ))
    }

    #[tokio::test]
    async fn adapter_roundtrips_pin() {
        let a = adapter();
        assert_eq!(a.get_pin("d").await.unwrap(), None);
        a.upsert_pin("d", "fp").await.unwrap();
        assert_eq!(
            a.get_pin("d").await.unwrap(),
            Some(("fp".to_string(), false))
        );
        a.clear_pin("d").await.unwrap();
        assert_eq!(a.get_pin("d").await.unwrap(), None);
    }
}
