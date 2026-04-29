//! FileSyncTransport -- encrypted changeset files in a shared folder.
//!
//! Each device writes its own changeset files. Other devices read them.
//! No file locking needed because each device owns its namespace via device_id prefix.

use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use argon2::Argon2;
use async_trait::async_trait;
use std::path::PathBuf;
use tracing::debug;

use crate::error::StorageError;
use oneshim_core::error::CoreError;
use oneshim_core::models::sync::{ChangeSet, PeerInfo};
use oneshim_core::ports::sync_transport::SyncTransport;
use oneshim_core::sync::Hlc;

const NONCE_SIZE: usize = 12; // AES-256-GCM nonce
const SALT_SIZE: usize = 16; // Argon2 salt

/// File-based sync transport with AES-256-GCM encryption.
pub struct FileSyncTransport {
    sync_folder: PathBuf,
    local_device_id: String,
    /// Raw passphrase (held in memory only while SyncEngine is alive).
    passphrase: String,
}

impl FileSyncTransport {
    pub fn new(
        sync_folder: PathBuf,
        local_device_id: String,
        passphrase: String,
    ) -> Result<Self, StorageError> {
        // Ensure the sync folder exists
        std::fs::create_dir_all(&sync_folder).map_err(|e| {
            StorageError::Internal(format!(
                "Failed to create sync folder {}: {e}",
                sync_folder.display()
            ))
        })?;

        Ok(Self {
            sync_folder,
            local_device_id,
            passphrase,
        })
    }

    /// Derive AES-256 key from passphrase + salt via Argon2id.
    fn derive_key(passphrase: &str, salt: &[u8]) -> Result<[u8; 32], StorageError> {
        let mut key = [0u8; 32];
        Argon2::default()
            .hash_password_into(passphrase.as_bytes(), salt, &mut key)
            .map_err(|e| StorageError::Internal(format!("Argon2 KDF failed: {e}")))?;
        Ok(key)
    }

    /// Encrypt plaintext with AES-256-GCM.
    /// Returns: salt (16) || nonce (12) || ciphertext
    fn encrypt(passphrase: &str, plaintext: &[u8]) -> Result<Vec<u8>, StorageError> {
        use aes_gcm::aead::rand_core::RngCore;
        let mut salt = [0u8; SALT_SIZE];
        OsRng.fill_bytes(&mut salt);

        let key = Self::derive_key(passphrase, &salt)?;
        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|e| StorageError::Internal(format!("AES init: {e}")))?;

        let mut nonce_bytes = [0u8; NONCE_SIZE];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = cipher
            .encrypt(nonce, plaintext)
            .map_err(|e| StorageError::Internal(format!("AES encrypt: {e}")))?;

        let mut output = Vec::with_capacity(SALT_SIZE + NONCE_SIZE + ciphertext.len());
        output.extend_from_slice(&salt);
        output.extend_from_slice(&nonce_bytes);
        output.extend_from_slice(&ciphertext);
        Ok(output)
    }

    /// Decrypt: parse salt || nonce || ciphertext
    fn decrypt(passphrase: &str, data: &[u8]) -> Result<Vec<u8>, StorageError> {
        if data.len() < SALT_SIZE + NONCE_SIZE + 1 {
            return Err(StorageError::Internal(
                "encrypted data too short".to_string(),
            ));
        }
        let salt = &data[..SALT_SIZE];
        let nonce_bytes = &data[SALT_SIZE..SALT_SIZE + NONCE_SIZE];
        let ciphertext = &data[SALT_SIZE + NONCE_SIZE..];

        let key = Self::derive_key(passphrase, salt)?;
        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|e| StorageError::Internal(format!("AES init: {e}")))?;
        let nonce = Nonce::from_slice(nonce_bytes);

        cipher.decrypt(nonce, ciphertext).map_err(|e| {
            StorageError::Internal(format!("AES decrypt failed (wrong passphrase?): {e}"))
        })
    }

    /// Build the filename for a changeset.
    fn changeset_filename(device_id: &str, hlc: &Hlc) -> String {
        format!(
            "changeset-{}-{}-{}.enc",
            device_id, hlc.wall_ms, hlc.counter
        )
    }

    /// Parse device_id and HLC from a changeset filename.
    fn parse_filename(name: &str) -> Option<(String, u64, u32)> {
        let name = name.strip_prefix("changeset-")?.strip_suffix(".enc")?;
        let parts: Vec<&str> = name.rsplitn(3, '-').collect();
        if parts.len() != 3 {
            return None;
        }
        let counter: u32 = parts[0].parse().ok()?;
        let wall_ms: u64 = parts[1].parse().ok()?;
        let device_id = parts[2].to_string();
        Some((device_id, wall_ms, counter))
    }
}

#[async_trait]
impl SyncTransport for FileSyncTransport {
    async fn push(&self, changes: &ChangeSet) -> Result<(), CoreError> {
        let folder = self.sync_folder.clone();
        let device_id = self.local_device_id.clone();
        let passphrase = self.passphrase.clone();
        let changes = changes.clone();

        tokio::task::spawn_blocking(move || {
            let json = serde_json::to_vec(&changes).map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("serialize changeset: {e}"),
            })?;
            let encrypted = Self::encrypt(&passphrase, &json)?;

            let filename = Self::changeset_filename(&device_id, &changes.watermark);
            let final_path = folder.join(&filename);
            let tmp_path = folder.join(format!("{filename}.tmp"));

            // Atomic write: write to .tmp, fsync, rename
            std::fs::write(&tmp_path, &encrypted).map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("write tmp file: {e}"),
            })?;

            // fsync the file
            let file = std::fs::File::open(&tmp_path).map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("open tmp for fsync: {e}"),
            })?;
            file.sync_all().map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("fsync: {e}"),
            })?;

            std::fs::rename(&tmp_path, &final_path).map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("rename tmp to final: {e}"),
            })?;

            debug!(filename = %filename, bytes = encrypted.len(), "changeset pushed to file");
            Ok(())
        })
        .await
        .map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("spawn_blocking join error: {e}"),
        })?
    }

    async fn pull(&self, since: &Hlc) -> Result<Option<ChangeSet>, CoreError> {
        let folder = self.sync_folder.clone();
        let local_device_id = self.local_device_id.clone();
        let passphrase = self.passphrase.clone();
        let since = since.clone();

        tokio::task::spawn_blocking(move || {
            let entries = std::fs::read_dir(&folder).map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("read sync folder: {e}"),
            })?;

            let mut best: Option<(Hlc, PathBuf)> = None;

            for entry in entries {
                let entry = entry.map_err(|e| CoreError::Internal {
                    code: oneshim_core::error_codes::InternalCode::Generic,
                    message: format!("dir entry: {e}"),
                })?;
                let name = entry.file_name().to_string_lossy().to_string();

                // Skip .tmp files and own files
                if name.ends_with(".tmp") {
                    continue;
                }

                if let Some((device_id, wall_ms, counter)) = Self::parse_filename(&name) {
                    // Skip own changesets
                    if device_id == local_device_id {
                        continue;
                    }

                    let file_hlc = Hlc {
                        wall_ms,
                        counter,
                        device_id: device_id.clone(),
                    };

                    // Only consider files newer than watermark
                    if !file_hlc.is_after(&since) {
                        continue;
                    }

                    // Pick the oldest unprocessed file (lowest HLC after since)
                    match &best {
                        None => best = Some((file_hlc, entry.path())),
                        Some((current_best, _)) if file_hlc < *current_best => {
                            best = Some((file_hlc, entry.path()));
                        }
                        _ => {}
                    }
                }
            }

            match best {
                None => Ok(None),
                Some((_, path)) => {
                    let data = std::fs::read(&path).map_err(|e| CoreError::Internal {
                        code: oneshim_core::error_codes::InternalCode::Generic,
                        message: format!("read changeset file: {e}"),
                    })?;
                    let plaintext = Self::decrypt(&passphrase, &data)?;
                    let cs: ChangeSet =
                        serde_json::from_slice(&plaintext).map_err(|e| CoreError::Internal {
                            code: oneshim_core::error_codes::InternalCode::Generic,
                            message: format!("deserialize changeset: {e}"),
                        })?;
                    debug!(
                        file = %path.display(),
                        rows = cs.row_count(),
                        "changeset pulled from file"
                    );
                    Ok(Some(cs))
                }
            }
        })
        .await
        .map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("spawn_blocking join error: {e}"),
        })?
    }

    async fn discover_peers(&self) -> Result<Vec<PeerInfo>, CoreError> {
        let folder = self.sync_folder.clone();
        let local_device_id = self.local_device_id.clone();

        tokio::task::spawn_blocking(move || {
            let entries = std::fs::read_dir(&folder).map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("read sync folder: {e}"),
            })?;

            let mut peers: std::collections::HashMap<String, (u64, u32)> =
                std::collections::HashMap::new();

            for entry in entries {
                let entry = entry.map_err(|e| CoreError::Internal {
                    code: oneshim_core::error_codes::InternalCode::Generic,
                    message: format!("dir entry: {e}"),
                })?;
                let name = entry.file_name().to_string_lossy().to_string();

                if let Some((device_id, wall_ms, counter)) = Self::parse_filename(&name) {
                    if device_id == local_device_id {
                        continue;
                    }
                    let existing = peers.entry(device_id).or_insert((0, 0));
                    if wall_ms > existing.0 || (wall_ms == existing.0 && counter > existing.1) {
                        *existing = (wall_ms, counter);
                    }
                }
            }

            Ok(peers
                .into_iter()
                .map(|(device_id, (wall_ms, counter))| PeerInfo {
                    device_id: device_id.clone(),
                    device_name: device_id, // Name not available from filenames alone
                    last_sync_at: chrono::Utc::now().to_rfc3339(),
                    watermark: Hlc {
                        wall_ms,
                        counter,
                        device_id: String::new(),
                    },
                })
                .collect())
        })
        .await
        .map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("spawn_blocking join error: {e}"),
        })?
    }

    async fn forget_peer(&self, device_id: &str) -> Result<(), CoreError> {
        let folder = self.sync_folder.clone();
        let device_id = device_id.to_string();

        tokio::task::spawn_blocking(move || {
            let entries = std::fs::read_dir(&folder).map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("read sync folder: {e}"),
            })?;

            let mut removed = 0u32;
            for entry in entries {
                let entry = entry.map_err(|e| CoreError::Internal {
                    code: oneshim_core::error_codes::InternalCode::Generic,
                    message: format!("dir entry: {e}"),
                })?;
                let name = entry.file_name().to_string_lossy().to_string();
                if let Some((file_device_id, _, _)) = Self::parse_filename(&name) {
                    if file_device_id == device_id {
                        std::fs::remove_file(entry.path()).map_err(|e| CoreError::Internal {
                            code: oneshim_core::error_codes::InternalCode::Generic,
                            message: format!("remove changeset file: {e}"),
                        })?;
                        removed += 1;
                    }
                }
            }

            debug!(device_id = %device_id, removed, "file peer forgotten");
            Ok(())
        })
        .await
        .map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("spawn_blocking join error: {e}"),
        })?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oneshim_core::models::sync::ChangeSetKind;

    fn test_passphrase() -> String {
        "test-passphrase-12345".to_string()
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let passphrase = test_passphrase();
        let plaintext = b"hello world, this is a sync test";

        let encrypted = FileSyncTransport::encrypt(&passphrase, plaintext).unwrap();
        assert_ne!(encrypted.as_slice(), plaintext);
        assert!(encrypted.len() > SALT_SIZE + NONCE_SIZE);

        let decrypted = FileSyncTransport::decrypt(&passphrase, &encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn wrong_passphrase_fails_decrypt() {
        let plaintext = b"secret data";
        let encrypted = FileSyncTransport::encrypt("correct-pass", plaintext).unwrap();

        let result = FileSyncTransport::decrypt("wrong-pass", &encrypted);
        assert!(result.is_err());
    }

    #[test]
    fn filename_parsing() {
        let parsed = FileSyncTransport::parse_filename("changeset-dev-abc-100-5.enc");
        assert_eq!(parsed, Some(("dev-abc".to_string(), 100, 5)));

        let parsed2 = FileSyncTransport::parse_filename("changeset-mydev-1710859200000-42.enc");
        assert_eq!(parsed2, Some(("mydev".to_string(), 1710859200000, 42)));

        // Invalid names
        assert!(FileSyncTransport::parse_filename("not-a-changeset.enc").is_none());
        assert!(FileSyncTransport::parse_filename("changeset-.enc").is_none());
    }

    #[test]
    fn filename_generation() {
        let hlc = Hlc {
            wall_ms: 1710859200000,
            counter: 42,
            device_id: "dev-a".to_string(),
        };
        let name = FileSyncTransport::changeset_filename("dev-a", &hlc);
        assert_eq!(name, "changeset-dev-a-1710859200000-42.enc");
    }

    #[tokio::test]
    async fn push_creates_enc_file() {
        let dir = tempfile::tempdir().unwrap();
        let transport = FileSyncTransport::new(
            dir.path().to_path_buf(),
            "local-dev".to_string(),
            test_passphrase(),
        )
        .unwrap();

        let cs = ChangeSet {
            kind: ChangeSetKind::Data,
            origin_device_id: "local-dev".to_string(),
            origin_device_name: "Test".to_string(),
            watermark: Hlc {
                wall_ms: 100,
                counter: 1,
                device_id: "local-dev".to_string(),
            },
            segments: vec![serde_json::json!({"id": "seg-1"})],
            ..Default::default()
        };

        transport.push(&cs).await.unwrap();

        // Verify .enc file exists and .tmp does not
        let files: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(files.len(), 1);
        let name = files[0].file_name().to_string_lossy().to_string();
        assert!(name.ends_with(".enc"));
        assert!(!name.ends_with(".tmp"));
    }

    #[tokio::test]
    async fn pull_returns_none_on_empty_folder() {
        let dir = tempfile::tempdir().unwrap();
        let transport = FileSyncTransport::new(
            dir.path().to_path_buf(),
            "local-dev".to_string(),
            test_passphrase(),
        )
        .unwrap();

        let result = transport.pull(&Hlc::default()).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn push_then_pull_roundtrip() {
        let dir = tempfile::tempdir().unwrap();

        // Device A pushes
        let transport_a = FileSyncTransport::new(
            dir.path().to_path_buf(),
            "dev-a".to_string(),
            test_passphrase(),
        )
        .unwrap();

        let cs = ChangeSet {
            kind: ChangeSetKind::Data,
            origin_device_id: "dev-a".to_string(),
            origin_device_name: "Device A".to_string(),
            watermark: Hlc {
                wall_ms: 200,
                counter: 1,
                device_id: "dev-a".to_string(),
            },
            segments: vec![serde_json::json!({"id": "seg-from-a"})],
            ..Default::default()
        };
        transport_a.push(&cs).await.unwrap();

        // Device B pulls
        let transport_b = FileSyncTransport::new(
            dir.path().to_path_buf(),
            "dev-b".to_string(),
            test_passphrase(),
        )
        .unwrap();

        let pulled = transport_b.pull(&Hlc::default()).await.unwrap();
        assert!(pulled.is_some());
        let pulled_cs = pulled.unwrap();
        assert_eq!(pulled_cs.origin_device_id, "dev-a");
        assert_eq!(pulled_cs.segments.len(), 1);
        assert_eq!(pulled_cs.segments[0]["id"], "seg-from-a");
    }

    #[tokio::test]
    async fn discover_peers_finds_remote_devices() {
        let dir = tempfile::tempdir().unwrap();

        // Device A pushes two files
        let transport_a = FileSyncTransport::new(
            dir.path().to_path_buf(),
            "dev-a".to_string(),
            test_passphrase(),
        )
        .unwrap();

        let cs1 = ChangeSet {
            watermark: Hlc {
                wall_ms: 100,
                counter: 0,
                device_id: "dev-a".to_string(),
            },
            origin_device_id: "dev-a".to_string(),
            ..Default::default()
        };
        transport_a.push(&cs1).await.unwrap();

        let cs2 = ChangeSet {
            watermark: Hlc {
                wall_ms: 200,
                counter: 0,
                device_id: "dev-a".to_string(),
            },
            origin_device_id: "dev-a".to_string(),
            ..Default::default()
        };
        transport_a.push(&cs2).await.unwrap();

        // Device B discovers peers
        let transport_b = FileSyncTransport::new(
            dir.path().to_path_buf(),
            "dev-b".to_string(),
            test_passphrase(),
        )
        .unwrap();

        let peers = transport_b.discover_peers().await.unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].device_id, "dev-a");
        assert_eq!(peers[0].watermark.wall_ms, 200);
    }

    #[tokio::test]
    async fn forget_peer_removes_matching_changeset_files() {
        let dir = tempfile::tempdir().unwrap();
        let dev_a = "dev-a";
        let dev_b = "dev-b";
        for i in 0..3u64 {
            std::fs::write(
                dir.path().join(format!("changeset-{dev_a}-{i}-{i}.enc")),
                b"ciphertext",
            )
            .unwrap();
            std::fs::write(
                dir.path().join(format!("changeset-{dev_b}-{i}-{i}.enc")),
                b"ciphertext",
            )
            .unwrap();
        }

        let transport = FileSyncTransport::new(
            dir.path().to_path_buf(),
            "local-device".to_string(),
            test_passphrase(),
        )
        .unwrap();

        transport.forget_peer(dev_a).await.unwrap();

        let remaining: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect();
        assert!(
            remaining.iter().all(|n| !n.contains(&format!("-{dev_a}-"))),
            "dev-a files should be gone, remaining={remaining:?}"
        );
        assert_eq!(
            remaining
                .iter()
                .filter(|n| n.contains(&format!("-{dev_b}-")))
                .count(),
            3,
            "dev-b files must survive, remaining={remaining:?}"
        );
    }

    #[tokio::test]
    async fn forget_peer_leaves_unrelated_files_alone() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("README.txt"), b"notes").unwrap();

        let transport = FileSyncTransport::new(
            dir.path().to_path_buf(),
            "local".to_string(),
            test_passphrase(),
        )
        .unwrap();

        transport.forget_peer("unknown-dev").await.unwrap();
        assert!(dir.path().join("README.txt").exists());
    }

    #[tokio::test]
    async fn forget_peer_ok_on_empty_folder() {
        let dir = tempfile::tempdir().unwrap();
        let transport = FileSyncTransport::new(
            dir.path().to_path_buf(),
            "local".to_string(),
            test_passphrase(),
        )
        .unwrap();
        transport.forget_peer("nobody").await.unwrap();
    }
}
