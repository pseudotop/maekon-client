//! LAN peer TOFU pin store -- CRUD for `lan_peer_pins` table.

use crate::error::StorageError;

use super::SqliteStorage;

impl SqliteStorage {
    /// Get the stored pin for a peer device.
    /// Returns `Some((fingerprint, trust_revoked))` if found, `None` otherwise.
    pub fn get_lan_pin(&self, device_id: &str) -> Result<Option<(String, bool)>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("SQLite lock poisoned: {e}")))?;
        let mut stmt = conn
            .prepare(
                "SELECT cert_fingerprint, trust_revoked FROM lan_peer_pins WHERE device_id = ?",
            )
            .map_err(|e| StorageError::Internal(format!("prepare get_lan_pin: {e}")))?;

        let result = stmt
            .query_row([device_id], |row| {
                let fingerprint: String = row.get(0)?;
                let revoked: bool = row.get(1)?;
                Ok((fingerprint, revoked))
            })
            .optional()
            .map_err(|e| StorageError::Internal(format!("get_lan_pin: {e}")))?;

        Ok(result)
    }

    /// Insert or update a peer's TOFU pin.
    /// On conflict (existing device_id), updates the fingerprint and last_seen_at.
    pub fn upsert_lan_pin(
        &self,
        device_id: &str,
        cert_fingerprint: &str,
    ) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("SQLite lock poisoned: {e}")))?;
        conn.execute(
            "INSERT INTO lan_peer_pins (device_id, cert_fingerprint)
             VALUES (?, ?)
             ON CONFLICT(device_id) DO UPDATE SET
                cert_fingerprint = excluded.cert_fingerprint,
                last_seen_at = datetime('now')",
            rusqlite::params![device_id, cert_fingerprint],
        )
        .map_err(|e| StorageError::Internal(format!("upsert_lan_pin: {e}")))?;

        Ok(())
    }

    /// Revoke trust for a peer device (TOFU violation).
    pub fn revoke_lan_pin(&self, device_id: &str) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("SQLite lock poisoned: {e}")))?;
        conn.execute(
            "UPDATE lan_peer_pins SET trust_revoked = 1 WHERE device_id = ?",
            [device_id],
        )
        .map_err(|e| StorageError::Internal(format!("revoke_lan_pin: {e}")))?;

        Ok(())
    }
}

// Bring in the `optional()` extension for query_row.
use rusqlite::OptionalExtension;

#[cfg(test)]
mod tests {
    use super::*;

    fn test_storage() -> SqliteStorage {
        SqliteStorage::open_in_memory(30).unwrap()
    }

    #[test]
    fn pin_not_found_returns_none() {
        let storage = test_storage();
        let result = storage.get_lan_pin("nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn upsert_and_get_pin() {
        let storage = test_storage();
        storage.upsert_lan_pin("dev-1", "abc123").unwrap();
        let result = storage.get_lan_pin("dev-1").unwrap();
        assert!(result.is_some());
        let (fp, revoked) = result.unwrap();
        assert_eq!(fp, "abc123");
        assert!(!revoked);
    }

    #[test]
    fn upsert_updates_fingerprint() {
        let storage = test_storage();
        storage.upsert_lan_pin("dev-1", "old-fp").unwrap();
        storage.upsert_lan_pin("dev-1", "new-fp").unwrap();
        let (fp, _) = storage.get_lan_pin("dev-1").unwrap().unwrap();
        assert_eq!(fp, "new-fp");
    }

    #[test]
    fn revoke_pin() {
        let storage = test_storage();
        storage.upsert_lan_pin("dev-1", "fp1").unwrap();
        storage.revoke_lan_pin("dev-1").unwrap();
        let (_, revoked) = storage.get_lan_pin("dev-1").unwrap().unwrap();
        assert!(revoked);
    }

    #[test]
    fn fingerprint_mismatch_detectable() {
        let storage = test_storage();
        storage.upsert_lan_pin("dev-1", "original-fp").unwrap();

        // Simulate a peer presenting a different fingerprint
        let (stored_fp, _) = storage.get_lan_pin("dev-1").unwrap().unwrap();
        let new_fp = "different-fp";
        assert_ne!(stored_fp, new_fp); // TOFU violation detected
    }
}
