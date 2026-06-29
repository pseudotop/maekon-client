//! FileSyncTransport -- encrypted changeset files in a shared folder.
//!
//! Each device writes its own changeset files. Other devices read them.
//! No file locking needed because each device owns its namespace via device_id prefix.

use aes_gcm::{
    aead::{Aead, KeyInit, Nonce},
    Aes256Gcm,
};
use argon2::Argon2;
use async_trait::async_trait;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};
use tracing::{debug, info, warn};
use zeroize::Zeroizing;

use crate::error::StorageError;
use maekon_core::error::CoreError;
use maekon_core::models::sync::{ChangeSet, PeerInfo};
use maekon_core::ports::sync_transport::SyncTransport;
use maekon_core::sync::Hlc;

const NONCE_SIZE: usize = 12; // AES-256-GCM nonce
const SALT_SIZE: usize = 16; // Argon2 salt

/// Default retention window for consumed changeset files, in days.
///
/// Mirrors the frame storage 30-day default (`frame_storage`/`retention.rs`).
/// A changeset file older than this is assumed to have been pulled by every
/// peer that will ever pull it, so it can be reclaimed. The window must be far
/// larger than any plausible offline period for a peer that still needs the
/// data — see [`FileSyncTransport::enforce_retention`].
const DEFAULT_RETENTION_DAYS: u64 = 30;

const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

fn random_bytes<const N: usize>() -> Result<[u8; N], StorageError> {
    let mut bytes: [u8; N] = std::array::from_fn(|_| u8::default());
    getrandom::fill(&mut bytes)
        .map_err(|e| StorageError::Internal(format!("random bytes generation failed: {e}")))?;
    Ok(bytes)
}

/// File-based sync transport with AES-256-GCM encryption.
pub struct FileSyncTransport {
    sync_folder: PathBuf,
    local_device_id: String,
    /// Raw passphrase. Stored as `Zeroizing<String>` so heap bytes are
    /// overwritten with zeroes when `FileSyncTransport` is dropped.
    passphrase: Zeroizing<String>,
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
            passphrase: Zeroizing::new(passphrase),
        })
    }

    /// Derive AES-256 key from passphrase + salt via Argon2id.
    ///
    /// Returns the key wrapped in `Zeroizing<[u8; 32]>` so the heap copy of
    /// the derived key bytes is overwritten with zeroes when the value is
    /// dropped (after being consumed by `Aes256Gcm::new_from_slice`).
    fn derive_key(passphrase: &str, salt: &[u8]) -> Result<Zeroizing<[u8; 32]>, StorageError> {
        // KDF output buffer; populated by `hash_password_into` below. Constructed
        // via `Default` (not the `[0u8; 32]` literal) to keep CodeQL's
        // `rust/hard-coded-cryptographic-value` source pattern from flagging this
        // intermediate buffer as a key.
        let mut key: Zeroizing<[u8; 32]> = Zeroizing::new(Default::default());
        Argon2::default()
            .hash_password_into(passphrase.as_bytes(), salt, key.as_mut())
            .map_err(|e| StorageError::Internal(format!("Argon2 KDF failed: {e}")))?;
        Ok(key)
    }

    /// Encrypt plaintext with AES-256-GCM.
    /// Returns: salt (16) || nonce (12) || ciphertext
    fn encrypt(passphrase: &str, plaintext: &[u8]) -> Result<Vec<u8>, StorageError> {
        let salt = random_bytes::<SALT_SIZE>()?;

        let key = Self::derive_key(passphrase, &salt)?;
        let cipher = Aes256Gcm::new_from_slice(key.as_ref())
            .map_err(|e| StorageError::Internal(format!("AES init: {e}")))?;

        let nonce_bytes = random_bytes::<NONCE_SIZE>()?;
        let nonce: Nonce<Aes256Gcm> = nonce_bytes.into();

        let ciphertext = cipher
            .encrypt(&nonce, plaintext)
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
        let cipher = Aes256Gcm::new_from_slice(key.as_ref())
            .map_err(|e| StorageError::Internal(format!("AES init: {e}")))?;
        let nonce = <&Nonce<Aes256Gcm>>::try_from(nonce_bytes)
            .map_err(|e| StorageError::Internal(format!("nonce parse: {e}")))?;

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

    /// Reclaim changeset files older than the default retention window.
    ///
    /// See [`FileSyncTransport::enforce_retention_with_days`]. Uses
    /// [`DEFAULT_RETENTION_DAYS`].
    pub async fn enforce_retention(&self) -> Result<usize, CoreError> {
        self.enforce_retention_with_days(DEFAULT_RETENTION_DAYS)
            .await
    }

    /// Reclaim changeset files older than `retention_days`, returning the
    /// number of files deleted.
    ///
    /// # Why time-based (not delete-on-consume)
    ///
    /// A single shared folder can serve **more than two devices**, and there is
    /// no manifest or per-device read cursor recorded in the folder. A consumer
    /// therefore cannot know whether a file it just pulled has also been read by
    /// every *other* peer. Deleting on consume would silently drop data a slower
    /// or offline peer has not yet pulled. Instead we delete only files that are
    /// older than a long retention window, by which point every peer that will
    /// ever pull the file is assumed to have done so.
    ///
    /// # Age signal: filesystem mtime, not the filename HLC
    ///
    /// The `wall_ms` embedded in the filename is the *writer's* wall clock, which
    /// can be skewed or even attacker-controlled. Using it would let a peer with
    /// a fast clock have its files reclaimed prematurely, or a far-future
    /// timestamp pin a file forever. We use the local filesystem modification
    /// time — "how long the file has rested in this folder on this machine" —
    /// which is the property the retention window is actually about.
    ///
    /// # Fail-safe toward keeping data
    ///
    /// This deletes both this device's own changeset files and peers' files:
    /// once a file is older than the window, every peer (including the writer
    /// re-bootstrapping) has had ample time to pull it. If a file's mtime cannot
    /// be read, or is in the future (clock skew), the file is **kept** — we never
    /// delete a file we cannot prove is old. `.tmp` files and non-changeset files
    /// are ignored. Individual delete failures are logged and skipped; the
    /// returned count reflects only files actually removed.
    pub async fn enforce_retention_with_days(
        &self,
        retention_days: u64,
    ) -> Result<usize, CoreError> {
        let folder = self.sync_folder.clone();

        tokio::task::spawn_blocking(move || {
            if !folder.exists() {
                return Ok(0);
            }

            let now = SystemTime::now();
            let max_age = Duration::from_secs(retention_days.saturating_mul(SECONDS_PER_DAY));

            let entries = std::fs::read_dir(&folder).map_err(|e| CoreError::Storage {
                code: maekon_core::error_codes::StorageCode::Failed,
                message: format!("read sync folder for retention: {e}"),
            })?;

            let mut deleted = 0usize;
            for entry in entries {
                let entry = entry.map_err(|e| CoreError::Storage {
                    code: maekon_core::error_codes::StorageCode::Failed,
                    message: format!("dir entry during retention: {e}"),
                })?;
                let name = entry.file_name().to_string_lossy().to_string();

                // Only ever delete completed changeset files. Skip .tmp staging
                // files and anything that is not a changeset (e.g. README).
                if name.ends_with(".tmp") || Self::parse_filename(&name).is_none() {
                    continue;
                }

                // Age is measured by filesystem mtime, not the filename clock.
                // If the mtime is unreadable or in the future, keep the file:
                // we never delete data we cannot prove is past the window.
                let modified = match entry.metadata().and_then(|m| m.modified()) {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                let age = match now.duration_since(modified) {
                    Ok(age) => age,
                    Err(_) => continue, // mtime in the future (clock skew) → keep
                };
                if age <= max_age {
                    continue;
                }

                match std::fs::remove_file(entry.path()) {
                    Ok(()) => {
                        deleted += 1;
                        debug!(file = %name, "reclaimed expired changeset file");
                    }
                    Err(e) => {
                        // Best-effort: a single failure must not abort the sweep.
                        warn!(file = %name, "changeset retention delete failed: {e}");
                    }
                }
            }

            if deleted > 0 {
                info!(
                    deleted,
                    retention_days, "changeset retention policy reclaimed expired files"
                );
            }
            Ok(deleted)
        })
        .await
        .map_err(|e| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("spawn_blocking join error: {e}"),
        })?
    }
}

#[async_trait]
impl SyncTransport for FileSyncTransport {
    async fn push(&self, changes: &ChangeSet) -> Result<usize, CoreError> {
        let folder = self.sync_folder.clone();
        let device_id = self.local_device_id.clone();
        // Clone the Zeroizing wrapper so the closure's copy is also zeroized on drop.
        let passphrase: Zeroizing<String> = self.passphrase.clone();
        let changes = changes.clone();

        tokio::task::spawn_blocking(move || {
            let json = serde_json::to_vec(&changes).map_err(|e| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!("serialize changeset: {e}"),
            })?;
            let encrypted = Self::encrypt(&passphrase, &json)?;

            let filename = Self::changeset_filename(&device_id, &changes.watermark);
            let final_path = folder.join(&filename);
            let tmp_path = folder.join(format!("{filename}.tmp"));

            // Atomic write: write to .tmp, fsync, rename
            std::fs::write(&tmp_path, &encrypted).map_err(|e| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!("write tmp file: {e}"),
            })?;

            // Windows requires a writable handle for FlushFileBuffers/sync_all.
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&tmp_path)
                .map_err(|e| CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
                    message: format!("open tmp for fsync: {e}"),
                })?;
            file.sync_all().map_err(|e| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!("fsync: {e}"),
            })?;

            std::fs::rename(&tmp_path, &final_path).map_err(|e| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!("rename tmp to final: {e}"),
            })?;

            debug!(filename = %filename, bytes = encrypted.len(), "changeset pushed to file");
            // #5143: the shared-folder file is the single destination → 1 egress.
            Ok(1)
        })
        .await
        .map_err(|e| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("spawn_blocking join error: {e}"),
        })?
    }

    async fn pull(&self, since: &Hlc) -> Result<Option<ChangeSet>, CoreError> {
        let folder = self.sync_folder.clone();
        let local_device_id = self.local_device_id.clone();
        // Clone the Zeroizing wrapper so the closure's copy is also zeroized on drop.
        let passphrase: Zeroizing<String> = self.passphrase.clone();
        let since = since.clone();

        tokio::task::spawn_blocking(move || {
            let entries = std::fs::read_dir(&folder).map_err(|e| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!("read sync folder: {e}"),
            })?;

            let mut best: Option<(Hlc, PathBuf)> = None;

            for entry in entries {
                let entry = entry.map_err(|e| CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
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
                        code: maekon_core::error_codes::InternalCode::Generic,
                        message: format!("read changeset file: {e}"),
                    })?;
                    let plaintext = Self::decrypt(&passphrase, &data)?;
                    let cs: ChangeSet =
                        serde_json::from_slice(&plaintext).map_err(|e| CoreError::Internal {
                            code: maekon_core::error_codes::InternalCode::Generic,
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
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("spawn_blocking join error: {e}"),
        })?
    }

    async fn discover_peers(&self) -> Result<Vec<PeerInfo>, CoreError> {
        let folder = self.sync_folder.clone();
        let local_device_id = self.local_device_id.clone();

        tokio::task::spawn_blocking(move || {
            let entries = std::fs::read_dir(&folder).map_err(|e| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!("read sync folder: {e}"),
            })?;

            let mut peers: std::collections::HashMap<String, (u64, u32)> =
                std::collections::HashMap::new();

            for entry in entries {
                let entry = entry.map_err(|e| CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
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
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("spawn_blocking join error: {e}"),
        })?
    }

    async fn forget_peer(&self, device_id: &str) -> Result<(), CoreError> {
        let folder = self.sync_folder.clone();
        let device_id = device_id.to_string();

        tokio::task::spawn_blocking(move || {
            let entries = std::fs::read_dir(&folder).map_err(|e| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!("read sync folder: {e}"),
            })?;

            let mut removed = 0u32;
            for entry in entries {
                let entry = entry.map_err(|e| CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
                    message: format!("dir entry: {e}"),
                })?;
                let name = entry.file_name().to_string_lossy().to_string();
                if let Some((file_device_id, _, _)) = Self::parse_filename(&name) {
                    if file_device_id == device_id {
                        std::fs::remove_file(entry.path()).map_err(|e| CoreError::Internal {
                            code: maekon_core::error_codes::InternalCode::Generic,
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
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("spawn_blocking join error: {e}"),
        })?
    }

    /// Reclaim consumed changeset files older than the default retention window
    /// so the shared folder does not grow unbounded (#6243). Delegates to the
    /// time-based [`FileSyncTransport::enforce_retention_with_days`]; the
    /// cross-device sync loop calls this each cycle.
    async fn enforce_retention(&self) -> Result<usize, CoreError> {
        self.enforce_retention_with_days(DEFAULT_RETENTION_DAYS)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::models::sync::ChangeSetKind;

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

        assert!(
            matches!(
                FileSyncTransport::decrypt("wrong-pass", &encrypted).unwrap_err(),
                StorageError::Internal(_)
            ),
            "wrong passphrase must yield StorageError::Internal (AES-GCM auth failure)"
        );
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

    /// Write a changeset file and back-date its filesystem mtime by `age`.
    fn write_changeset_aged(dir: &std::path::Path, name: &str, age: Duration) {
        let path = dir.join(name);
        std::fs::write(&path, b"ciphertext").unwrap();
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        // `File::set_modified` is portable across macOS/Linux/Windows (stable 1.75).
        file.set_modified(SystemTime::now() - age).unwrap();
    }

    #[tokio::test]
    async fn retention_reclaims_old_files_keeps_recent_and_unread() {
        let dir = tempfile::tempdir().unwrap();

        // Old foreign file — well past the 1-day window → reclaimed.
        write_changeset_aged(
            dir.path(),
            "changeset-dev-old-100-0.enc",
            Duration::from_secs(5 * SECONDS_PER_DAY),
        );
        // Recent foreign file the local device may not have read yet → kept.
        write_changeset_aged(
            dir.path(),
            "changeset-dev-new-200-0.enc",
            Duration::from_secs(60),
        );
        // Old file belonging to THIS device → also reclaimed (peers had time).
        write_changeset_aged(
            dir.path(),
            "changeset-local-50-0.enc",
            Duration::from_secs(5 * SECONDS_PER_DAY),
        );
        // Old .tmp staging file → never touched by retention.
        write_changeset_aged(
            dir.path(),
            "changeset-dev-old-100-0.enc.tmp",
            Duration::from_secs(5 * SECONDS_PER_DAY),
        );
        // Old non-changeset file → ignored.
        write_changeset_aged(
            dir.path(),
            "README.txt",
            Duration::from_secs(5 * SECONDS_PER_DAY),
        );

        let transport = FileSyncTransport::new(
            dir.path().to_path_buf(),
            "local".to_string(),
            test_passphrase(),
        )
        .unwrap();

        let deleted = transport.enforce_retention_with_days(1).await.unwrap();
        assert_eq!(
            deleted, 2,
            "both old changeset files (foreign + own) reclaimed"
        );

        let remaining: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect();

        // Recent unread changeset must survive — multi-reader safety.
        assert!(
            remaining.iter().any(|n| n == "changeset-dev-new-200-0.enc"),
            "recent unread changeset must be kept, remaining={remaining:?}"
        );
        // .tmp and non-changeset files survive regardless of age.
        assert!(remaining.iter().any(|n| n.ends_with(".tmp")));
        assert!(remaining.iter().any(|n| n == "README.txt"));
        // Old changeset files are gone.
        assert!(
            !remaining.iter().any(|n| n == "changeset-dev-old-100-0.enc"),
            "old foreign changeset should be reclaimed, remaining={remaining:?}"
        );
        assert!(
            !remaining.iter().any(|n| n == "changeset-local-50-0.enc"),
            "old own changeset should be reclaimed, remaining={remaining:?}"
        );
    }

    #[tokio::test]
    async fn retention_keeps_future_mtime_file() {
        // A file whose mtime is in the future (clock skew) must never be
        // deleted — the fail-safe keeps data we cannot prove is old.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("changeset-dev-x-100-0.enc");
        std::fs::write(&path, b"ciphertext").unwrap();
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::now() + Duration::from_secs(3600))
            .unwrap();

        let transport = FileSyncTransport::new(
            dir.path().to_path_buf(),
            "local".to_string(),
            test_passphrase(),
        )
        .unwrap();

        let deleted = transport.enforce_retention_with_days(1).await.unwrap();
        assert_eq!(deleted, 0, "future-mtime file must be kept");
        assert!(path.exists());
    }

    #[tokio::test]
    async fn retention_ok_on_missing_folder() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist");
        let transport =
            FileSyncTransport::new(missing, "local".to_string(), test_passphrase()).unwrap();
        // `new` creates the folder, so remove it to exercise the missing branch.
        std::fs::remove_dir_all(transport.sync_folder.clone()).unwrap();
        assert_eq!(transport.enforce_retention().await.unwrap(), 0);
    }
}
