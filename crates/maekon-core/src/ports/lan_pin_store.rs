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

    /// Insert the pin (new rows default `trust_revoked = false`) or update an
    /// existing row's fingerprint and last-seen time. An update does NOT touch
    /// `trust_revoked`: once a pin is revoked it stays revoked across upserts —
    /// re-trusting a peer requires the explicit `clear_pin` recovery path, so a
    /// changed cert can never silently un-revoke a previously flagged peer.
    async fn upsert_pin(&self, device_id: &str, fingerprint: &str) -> Result<(), CoreError>;

    /// Mark the pin as revoked (TOFU violation). Subsequent `get_pin` returns
    /// `trust_revoked = true`, which the LAN transport rejects before any
    /// handshake. Idempotent; a no-op if the pin does not exist.
    async fn revoke_pin(&self, device_id: &str) -> Result<(), CoreError>;

    /// Remove the pin entirely (recovery path; next contact re-TOFUs and clears
    /// any prior `trust_revoked` state on the fresh insert).
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
            let mut pins = self.pins.lock().unwrap();
            // Mirror the production SQL `ON CONFLICT DO UPDATE`: a new row defaults
            // `trust_revoked = false`, an update keeps the existing flag (a revoked
            // pin stays revoked through an upsert).
            let revoked = pins.get(device_id).map(|(_, r)| *r).unwrap_or(false);
            pins.insert(device_id.to_string(), (fingerprint.to_string(), revoked));
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

    #[tokio::test]
    async fn upsert_does_not_un_revoke_an_existing_pin() {
        // Contract: an upsert must not silently clear a revoked flag — re-trusting
        // requires the explicit clear_pin recovery path. Mirrors the production SQL.
        let store = InMemoryLanPinStore::new();
        store.upsert_pin("dev-1", "fp-abc").await.unwrap();
        store.revoke_pin("dev-1").await.unwrap();
        // A changed cert presents a new fingerprint and upserts it; the pin stays revoked.
        store.upsert_pin("dev-1", "fp-changed").await.unwrap();
        assert_eq!(
            store.get_pin("dev-1").await.unwrap(),
            Some(("fp-changed".to_string(), true)),
            "upsert updates the fingerprint but must keep trust_revoked = true"
        );
        // Only clear_pin resets trust: the next contact re-TOFUs from scratch.
        store.clear_pin("dev-1").await.unwrap();
        store.upsert_pin("dev-1", "fp-fresh").await.unwrap();
        assert_eq!(
            store.get_pin("dev-1").await.unwrap(),
            Some(("fp-fresh".to_string(), false)),
            "after clear_pin, a fresh insert defaults to not-revoked"
        );
    }
}
