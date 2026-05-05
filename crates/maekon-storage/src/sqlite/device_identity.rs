use crate::error::StorageError;
use tracing::info;

use super::SqliteStorage;

impl SqliteStorage {
    /// Ensure a device identity row exists in the `device_identity` table.
    ///
    /// On first call (empty table), generates a UUID v4 device_id and inserts
    /// it with the given device_name. On subsequent calls, returns the existing
    /// identity. The table enforces `id = 1` (singleton row).
    ///
    /// Returns `(device_id, device_name)`.
    pub fn ensure_device_identity(
        &self,
        device_name: &str,
    ) -> Result<(String, String), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("SQLite lock poisoned: {e}")))?;

        // Try to read existing identity first.
        let existing: Option<(String, String)> = conn
            .query_row(
                "SELECT device_id, device_name FROM device_identity WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();

        if let Some(identity) = existing {
            return Ok(identity);
        }

        // First launch -- generate a new UUID v4 device_id.
        let device_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO device_identity (id, device_id, device_name) VALUES (1, ?1, ?2)",
            rusqlite::params![device_id, device_name],
        )
        .map_err(|e| StorageError::Internal(format!("Failed to insert device identity: {e}")))?;

        info!(
            device_id = %device_id,
            device_name = %device_name,
            "device identity generated (first launch)"
        );

        Ok((device_id, device_name.to_string()))
    }

    /// Reset the device identity by deleting the existing row and generating
    /// a new one. This allows users to disassociate from their sync history.
    ///
    /// Returns the new `(device_id, device_name)`.
    pub fn reset_device_identity(
        &self,
        device_name: &str,
    ) -> Result<(String, String), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("SQLite lock poisoned: {e}")))?;

        conn.execute("DELETE FROM device_identity WHERE id = 1", [])
            .map_err(|e| {
                StorageError::Internal(format!("Failed to delete device identity: {e}"))
            })?;

        drop(conn); // Release lock before calling ensure_device_identity

        self.ensure_device_identity(device_name)
    }
}
