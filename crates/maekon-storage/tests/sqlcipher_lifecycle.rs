//! #11996: SQLCipher must remain usable after explicit shutdown/reinitialization.
//!
//! Keep this as the only test in its integration-test executable: SQLite's
//! process-global shutdown requires every connection and statement to be closed.

use maekon_storage::encryption::EncryptionKey;
use maekon_storage::error::StorageError;
use maekon_storage::sqlite::SqliteStorage;
use rusqlite::ffi::{sqlite3_initialize, sqlite3_shutdown, SQLITE_OK};
use tempfile::TempDir;

#[test]
fn encryption_and_rejection_survive_shutdown_and_reinitialization() {
    let directory = TempDir::new().expect("temporary database directory");
    let encrypted_path = directory.path().join("encrypted.db");
    let plaintext_path = directory.path().join("plaintext.db");
    let key = EncryptionKey::from_bytes([0x42; 32]);
    let wrong_key = EncryptionKey::from_bytes([0x24; 32]);
    {
        let plaintext = SqliteStorage::open(&plaintext_path, 30, None).expect("plaintext control");
        plaintext
            .set_meta_checked("control", "plaintext")
            .expect("write plaintext control");
    }

    for cycle in 0..3 {
        let value = format!("cycle-{cycle}");
        {
            let encrypted =
                SqliteStorage::open(&encrypted_path, 30, Some(&key)).expect("encrypted open");
            encrypted
                .set_meta_checked("lifecycle", &value)
                .expect("write encrypted value");
        }

        // SAFETY: This executable has one synchronous test. Every connection
        // created above has been dropped, and no database worker is running.
        assert_eq!(unsafe { sqlite3_shutdown() }, SQLITE_OK);
        // SAFETY: The preceding shutdown completed with no active connections.
        assert_eq!(unsafe { sqlite3_initialize() }, SQLITE_OK);

        let wrong_key_error = SqliteStorage::open(&encrypted_path, 30, Some(&wrong_key))
            .err()
            .expect("reinitialization must reject a wrong encryption key");
        assert!(
            matches!(wrong_key_error, StorageError::Internal(ref message)
            if message.contains("wrong or rotated key"))
        );
        let plaintext_error = SqliteStorage::open(&plaintext_path, 30, Some(&key))
            .err()
            .expect("reinitialization must reject plaintext as encrypted");
        assert!(
            matches!(plaintext_error, StorageError::Internal(ref message)
            if message.contains("is a plaintext SQLite database"))
        );
        {
            let encrypted =
                SqliteStorage::open(&encrypted_path, 30, Some(&key)).expect("same-key reopen");
            assert_eq!(encrypted.get_meta("lifecycle"), Some(value));
            let plaintext =
                SqliteStorage::open(&plaintext_path, 30, None).expect("plaintext stays readable");
            assert_eq!(plaintext.get_meta("control"), Some("plaintext".to_string()));
        }

        let bytes = std::fs::read(&encrypted_path).expect("read closed encrypted file");
        assert!(bytes.len() >= 16);
        assert_ne!(&bytes[..16], b"SQLite format 3\0");
    }
}
