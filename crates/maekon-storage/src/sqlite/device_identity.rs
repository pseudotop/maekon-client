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
        // device_identity ∈ ALL_TABLES and this path performs a new INSERT, so it uses
        // write_lock (called at app startup; never overlaps the erase path). If skipped
        // when deletion_flag is set an empty identity would be returned, but this path is
        // never invoked during erase.
        self.conn
            .write_lock()
            .run((String::new(), String::new()), |conn| {
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
                // device_id is sent to the server in IntegrationBootstrapRequest; format
                // is kept as UUID v4 to avoid a server-side wire-contract regression
                // (ADR-022 exemption: server-wire IDs with unverified format contract).
                let device_id = uuid::Uuid::new_v4().to_string();
                conn.execute(
                    "INSERT INTO device_identity (id, device_id, device_name) VALUES (1, ?1, ?2)",
                    rusqlite::params![device_id, device_name],
                )
                .map_err(|e| {
                    StorageError::Internal(format!("Failed to insert device identity: {e}"))
                })?;

                info!(
                    device_id = %device_id,
                    device_name = %device_name,
                    "device identity generated (first launch)"
                );

                Ok((device_id, device_name.to_string()))
            })
    }

    /// Reset the device identity by deleting the existing row and generating
    /// a new one. This allows users to disassociate from their sync history.
    ///
    /// Returns the new `(device_id, device_name)`.
    pub fn reset_device_identity(
        &self,
        device_name: &str,
    ) -> Result<(String, String), StorageError> {
        // Write — write_lock (skipped when deletion_flag is set). Once the lock is released
        // after the delete, ensure_device_identity generates a fresh identity.
        self.conn
            .write_lock()
            .run::<_, (), StorageError>((), |conn| {
                conn.execute("DELETE FROM device_identity WHERE id = 1", [])
                    .map_err(|e| {
                        StorageError::Internal(format!("Failed to delete device identity: {e}"))
                    })?;
                Ok(())
            })?;

        self.ensure_device_identity(device_name)
    }
}
