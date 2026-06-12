//! LAN peer TOFU pin store port.
//!
//! Abstracts the `lan_peer_pins` TOFU store so `maekon-network` reaches it
//! through `maekon-core` (ADR-015 pattern) rather than depending on
//! `maekon-storage` directly. Keyed by peer `device_id`; the value is the
//! pinned TLS leaf-cert SHA-256 fingerprint (hex) plus a `trust_revoked` flag.

use async_trait::async_trait;

use crate::error::CoreError;

#[async_trait]
pub trait LanPinStorePort: Send + Sync {
    /// Returns `Some((fingerprint_hex, trust_revoked))` if a pin exists.
    async fn get_pin(&self, device_id: &str) -> Result<Option<(String, bool)>, CoreError>;

    /// Insert or update the pin (resets `trust_revoked` semantics per the SQL).
    async fn upsert_pin(&self, device_id: &str, fingerprint: &str) -> Result<(), CoreError>;

    /// Mark the pin as revoked (TOFU violation).
    async fn revoke_pin(&self, device_id: &str) -> Result<(), CoreError>;

    /// Remove the pin entirely (recovery path; next contact re-TOFUs).
    async fn clear_pin(&self, device_id: &str) -> Result<(), CoreError>;
}

#[cfg(test)]
pub(crate) mod test_double {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// In-memory `LanPinStorePort` for unit tests.
    #[derive(Default)]
    pub struct InMemoryLanPinStore {
        pins: Mutex<HashMap<String, (String, bool)>>,
    }

    impl InMemoryLanPinStore {
        pub fn new() -> Self {
            Self::default()
        }
    }

    #[async_trait]
    impl LanPinStorePort for InMemoryLanPinStore {
        async fn get_pin(&self, device_id: &str) -> Result<Option<(String, bool)>, CoreError> {
            Ok(self.pins.lock().unwrap().get(device_id).cloned())
        }
        async fn upsert_pin(&self, device_id: &str, fingerprint: &str) -> Result<(), CoreError> {
            self.pins
                .lock()
                .unwrap()
                .insert(device_id.to_string(), (fingerprint.to_string(), false));
            Ok(())
        }
        async fn revoke_pin(&self, device_id: &str) -> Result<(), CoreError> {
            if let Some(entry) = self.pins.lock().unwrap().get_mut(device_id) {
                entry.1 = true;
            }
            Ok(())
        }
        async fn clear_pin(&self, device_id: &str) -> Result<(), CoreError> {
            self.pins.lock().unwrap().remove(device_id);
            Ok(())
        }
    }

    #[tokio::test]
    async fn upsert_get_revoke_clear_roundtrip() {
        let store = InMemoryLanPinStore::new();
        assert_eq!(store.get_pin("dev-1").await.unwrap(), None);
        store.upsert_pin("dev-1", "fp-abc").await.unwrap();
        assert_eq!(
            store.get_pin("dev-1").await.unwrap(),
            Some(("fp-abc".to_string(), false))
        );
        store.revoke_pin("dev-1").await.unwrap();
        assert_eq!(
            store.get_pin("dev-1").await.unwrap(),
            Some(("fp-abc".to_string(), true))
        );
        store.clear_pin("dev-1").await.unwrap();
        assert_eq!(store.get_pin("dev-1").await.unwrap(), None);
    }
}
