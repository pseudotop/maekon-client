//! LAN peer TOFU pin store -- CRUD for `lan_peer_pins` table.

use crate::error::StorageError;

use super::SqliteStorage;

impl SqliteStorage {
    /// Get the stored pin for a peer device.
    /// Returns `Some((fingerprint, trust_revoked))` if found, `None` otherwise.
    pub fn get_lan_pin(&self, device_id: &str) -> Result<Option<(String, bool)>, StorageError> {
        // Read — read_lock (independent of deletion_flag).
        let read = self.conn.read_lock();
        let conn = read.conn();
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
        // Write — write_lock (skipped when deletion_flag is set, lan_peer_pins ∈ ALL_TABLES).
        self.conn.write_lock().run((), |conn| {
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
        })
    }

    /// Revoke trust for a peer device (TOFU violation).
    pub fn revoke_lan_pin(&self, device_id: &str) -> Result<(), StorageError> {
        // Write — write_lock (skipped when deletion_flag is set).
        self.conn.write_lock().run((), |conn| {
            conn.execute(
                "UPDATE lan_peer_pins SET trust_revoked = 1 WHERE device_id = ?",
                [device_id],
            )
            .map_err(|e| StorageError::Internal(format!("revoke_lan_pin: {e}")))?;

            Ok(())
        })
    }

    /// Remove a peer's TOFU pin entirely (recovery path).
    pub fn clear_lan_pin(&self, device_id: &str) -> Result<(), StorageError> {
        // Write — write_lock (skipped when deletion_flag is set).
        self.conn.write_lock().run((), |conn| {
            conn.execute("DELETE FROM lan_peer_pins WHERE device_id = ?", [device_id])
                .map_err(|e| StorageError::Internal(format!("clear_lan_pin: {e}")))?;
            Ok(())
        })
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
    fn upsert_preserves_revoked_flag() {
        // The `ON CONFLICT DO UPDATE` clause only touches cert_fingerprint and
        // last_seen_at, never trust_revoked. A revoked peer presenting a changed
        // cert must stay revoked — re-trusting requires clear_lan_pin (DELETE).
        let storage = test_storage();
        storage.upsert_lan_pin("dev-1", "fp-old").unwrap();
        storage.revoke_lan_pin("dev-1").unwrap();
        storage.upsert_lan_pin("dev-1", "fp-new").unwrap();
        let (fp, revoked) = storage.get_lan_pin("dev-1").unwrap().unwrap();
        assert_eq!(fp, "fp-new", "fingerprint is updated by the upsert");
        assert!(revoked, "trust_revoked must survive the upsert");

        // clear_lan_pin (recovery) removes the row; the next insert defaults to 0.
        storage.clear_lan_pin("dev-1").unwrap();
        storage.upsert_lan_pin("dev-1", "fp-fresh").unwrap();
        let (_, revoked_after_clear) = storage.get_lan_pin("dev-1").unwrap().unwrap();
        assert!(
            !revoked_after_clear,
            "a fresh insert after clear defaults to not-revoked"
        );
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

    #[test]
    fn clear_pin_removes_row() {
        let storage = test_storage();
        storage.upsert_lan_pin("dev-1", "fp1").unwrap();
        storage.clear_lan_pin("dev-1").unwrap();
        assert!(storage.get_lan_pin("dev-1").unwrap().is_none());
    }
}
