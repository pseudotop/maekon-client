//! Local SQLite database encryption key management.
//!
//! # Key storage strategy
//! 1. Key file (`app_data_dir/.db_key`): 32-byte raw key
//! 2. File permissions: 0o600 (owner read/write only) on Unix, owner-only DACL on Windows.
//!    The file is created with restrictive permissions **atomically** — no world-readable
//!    window between creation and chmod (TOCTOU fix, issue #5991).
//!
//! # Future work
//! Integration with SQLCipher or an at-rest encryption layer is planned.
//! Currently only the key generation / storage / load infrastructure is provided.

use crate::error::StorageError;
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

/// 32-byte AES-256 database encryption key.
///
/// `ZeroizeOnDrop` derive wipes the 32-byte key material when the last owner is
/// dropped, so the at-rest master key never lingers in freed heap memory
/// (finding #6242). `[u8; 32]` already implements `Zeroize`. `Clone` is retained
/// because the key is shared via `Arc` and produced by `from_bytes`/`generate`.
#[derive(Clone, zeroize::ZeroizeOnDrop)]
pub struct EncryptionKey([u8; 32]);

impl EncryptionKey {
    /// Load from the key file, or generate a new key.
    ///
    /// - If the file exists: load it (validating the 32-byte length).
    /// - If the file is absent: generate, then persist to file (Unix: mode 0o600).
    pub fn load_or_create(app_data_dir: &Path) -> Result<Self, StorageError> {
        let key_path = app_data_dir.join(".db_key");

        if key_path.exists() {
            return Self::load_from_file(&key_path);
        }

        let key = Self::generate()?;
        key.save_to_file(&key_path)?;
        tracing::info!("New DB encryption key generated: {:?}", key_path);
        Ok(key)
    }

    /// Build a key from raw bytes.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// SQLite pragma key format (hex string).
    ///
    /// Returns the hex inside `Zeroizing` so this plaintext copy of the master
    /// key is wiped from the heap when the returned value is dropped (#6242).
    pub fn as_hex(&self) -> Zeroizing<String> {
        Zeroizing::new(self.0.iter().map(|b| format!("{b:02x}")).collect())
    }

    /// Borrow the raw bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Encrypt data with AES-256-GCM.
    /// Output format: nonce(12 bytes) || ciphertext(+16 bytes auth tag)
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, StorageError> {
        use aes_gcm::aead::Aead;
        use aes_gcm::{Aes256Gcm, KeyInit, Nonce};

        let cipher = Aes256Gcm::new_from_slice(&self.0)
            .map_err(|e| StorageError::Encryption(format!("cipher init: {e}")))?;

        let mut nonce_bytes = [0u8; 12];
        getrandom::fill(&mut nonce_bytes)
            .map_err(|e| StorageError::Encryption(format!("nonce generation: {e}")))?;
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = cipher
            .encrypt(nonce, plaintext)
            .map_err(|e| StorageError::Encryption(format!("encrypt: {e}")))?;

        let mut result = Vec::with_capacity(12 + ciphertext.len());
        result.extend_from_slice(&nonce_bytes);
        result.extend(ciphertext);
        Ok(result)
    }

    /// Decrypt data produced by `encrypt()` with AES-256-GCM.
    ///
    /// The plaintext is returned inside `Zeroizing` so the decrypted secret
    /// material (e.g. the secret-registry plaintext) is wiped from the heap when
    /// the returned buffer is dropped, rather than lingering in freed memory
    /// (#6242). `Zeroizing<Vec<u8>>` derefs to `&[u8]`/`Vec<u8>`, so callers that
    /// only read the bytes need no changes.
    pub fn decrypt(&self, data: &[u8]) -> Result<Zeroizing<Vec<u8>>, StorageError> {
        if data.len() < 12 {
            return Err(StorageError::Encryption(
                "ciphertext too short (< 12 bytes)".into(),
            ));
        }

        use aes_gcm::aead::Aead;
        use aes_gcm::{Aes256Gcm, KeyInit, Nonce};

        let (nonce_bytes, ciphertext) = data.split_at(12);
        let cipher = Aes256Gcm::new_from_slice(&self.0)
            .map_err(|e| StorageError::Encryption(format!("cipher init: {e}")))?;
        let nonce = Nonce::from_slice(nonce_bytes);

        cipher
            .decrypt(nonce, ciphertext)
            .map(Zeroizing::new)
            .map_err(|e| StorageError::Encryption(format!("decrypt: {e}")))
    }

    fn generate() -> Result<Self, StorageError> {
        let mut key = [0u8; 32];
        getrandom::fill(&mut key).map_err(|e| {
            StorageError::Encryption(format!("OS random number generation failed: {e}"))
        })?;
        Ok(Self(key))
    }

    fn load_from_file(path: &PathBuf) -> Result<Self, StorageError> {
        let bytes = std::fs::read(path)
            .map_err(|e| StorageError::Internal(format!("Key file read failed ({path:?}): {e}")))?;

        if bytes.len() != 32 {
            return Err(StorageError::Internal(format!(
                "Key file size error: expected 32 bytes, got {} bytes",
                bytes.len()
            )));
        }

        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        Ok(Self(key))
    }

    fn save_to_file(&self, path: &PathBuf) -> Result<(), StorageError> {
        // Create the file with restrictive permissions BEFORE any bytes are written so
        // there is never a world-readable window (TOCTOU fix, issue #5991).
        //
        // If the file already exists (unexpected on the new-key path, but possible in
        // concurrent or retry scenarios) we remove it and recreate rather than
        // opening/truncating an existing file whose permissions may be too open.
        use std::io::Write as _;

        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            // O_CREAT | O_EXCL | mode 0o600 in one syscall — atomically restrictive.
            let file_result = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path);

            let mut file = match file_result {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    // Remove stale file and retry with the same atomic open.
                    std::fs::remove_file(path).map_err(|re| {
                        StorageError::Internal(format!(
                            "Key file removal for overwrite failed ({path:?}): {re}"
                        ))
                    })?;
                    std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(path)
                        .map_err(|re| {
                            StorageError::Internal(format!(
                                "Key file create failed after removal ({path:?}): {re}"
                            ))
                        })?
                }
                Err(e) => {
                    return Err(StorageError::Internal(format!(
                        "Key file create failed ({path:?}): {e}"
                    )))
                }
            };

            file.write_all(&self.0).map_err(|e| {
                StorageError::Internal(format!("Key file write failed ({path:?}): {e}"))
            })?;
        }

        #[cfg(windows)]
        {
            // On Windows we must apply the owner-only DACL before writing any bytes.
            // We write to a temporary path first so we can set ACLs on it while it
            // is still empty, then rename atomically.  If any step fails we treat it
            // as a hard error — falling back to an unprotected write is not acceptable.
            let tmp_path = path.with_extension("tmp_key");

            // Remove any leftover temp file from a previous aborted attempt.
            let _ = std::fs::remove_file(&tmp_path);

            // Create the empty temp file, apply owner-only DACL, then write. The
            // temp file inherits the parent directory ACL for the brief window
            // before set_owner_only_dacl runs, but it is EMPTY during that window —
            // no key bytes exist until after the DACL is applied, so no secret is
            // ever exposed under the inherited ACL.
            std::fs::File::create(&tmp_path).map_err(|e| {
                StorageError::Internal(format!("Key temp file create failed ({tmp_path:?}): {e}"))
            })?;

            // Hard error if DACL cannot be set — do not leave the file unprotected.
            set_owner_only_dacl(&tmp_path).map_err(|e| {
                let _ = std::fs::remove_file(&tmp_path);
                StorageError::Internal(format!(
                    "Key file owner-only DACL failed ({tmp_path:?}): {e}"
                ))
            })?;

            // Write bytes into the now-protected temp file.
            {
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .open(&tmp_path)
                    .map_err(|e| {
                        let _ = std::fs::remove_file(&tmp_path);
                        StorageError::Internal(format!(
                            "Key temp file open for write failed ({tmp_path:?}): {e}"
                        ))
                    })?;
                file.write_all(&self.0).map_err(|e| {
                    let _ = std::fs::remove_file(&tmp_path);
                    StorageError::Internal(format!("Key file write failed ({tmp_path:?}): {e}"))
                })?;
            }

            // Remove any existing destination then rename into place.
            let _ = std::fs::remove_file(path);
            std::fs::rename(&tmp_path, path).map_err(|e| {
                let _ = std::fs::remove_file(&tmp_path);
                StorageError::Internal(format!(
                    "Key file rename failed ({tmp_path:?} -> {path:?}): {e}"
                ))
            })?;
        }

        #[cfg(not(any(unix, windows)))]
        {
            // Fallback for exotic platforms: best-effort write (no atomicity guarantee).
            std::fs::write(path, self.0).map_err(|e| {
                StorageError::Internal(format!("Key file write failed ({path:?}): {e}"))
            })?;
        }

        Ok(())
    }
}

/// Set an owner-only DACL on a file (Windows equivalent of Unix chmod 0o600).
///
/// Creates an ACL with a single ACE granting the current user GENERIC_ALL,
/// and applies it as a protected DACL (no inheritance from parent).
#[cfg(windows)]
pub(crate) fn set_owner_only_dacl(path: &std::path::Path) -> Result<(), StorageError> {
    // windows-sys 0.61: `OpenProcessToken` moved from `Win32::Security` to
    // `Win32::System::Threading`, `GENERIC_ALL` moved to `Win32::Foundation`,
    // and `HANDLE` is now `*mut c_void` instead of `isize` — token_handle
    // must be initialised with `std::ptr::null_mut()`.
    use windows_sys::Win32::Foundation::{LocalFree, GENERIC_ALL, HANDLE};
    use windows_sys::Win32::Security::Authorization::{SetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        AddAccessAllowedAce, GetTokenInformation, InitializeAcl, TokenUser, ACL as WIN_ACL,
        ACL_REVISION, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, TOKEN_QUERY,
        TOKEN_USER,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let wide_path: Vec<u16> = path
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        // 1. Get the current user's SID
        let mut token_handle: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token_handle) == 0 {
            return Err(StorageError::Internal("OpenProcessToken failed".into()));
        }

        // Query token user size
        let mut needed: u32 = 0;
        GetTokenInformation(
            token_handle,
            TokenUser,
            std::ptr::null_mut(),
            0,
            &mut needed,
        );
        if needed == 0 || needed > 4096 {
            windows_sys::Win32::Foundation::CloseHandle(token_handle);
            return Err(StorageError::Internal(format!(
                "unexpected token info size: {needed} bytes"
            )));
        }
        let mut user_buf = vec![0u8; needed as usize];
        if GetTokenInformation(
            token_handle,
            TokenUser,
            user_buf.as_mut_ptr().cast(),
            needed,
            &mut needed,
        ) == 0
        {
            windows_sys::Win32::Foundation::CloseHandle(token_handle);
            return Err(StorageError::Internal("GetTokenInformation failed".into()));
        }
        windows_sys::Win32::Foundation::CloseHandle(token_handle);

        let token_user = &*(user_buf.as_ptr() as *const TOKEN_USER);
        let user_sid = token_user.User.Sid;

        // 2. Build an ACL with a single owner-only ACE
        let sid_len = windows_sys::Win32::Security::GetLengthSid(user_sid);
        // SidStart field in ACCESS_ALLOWED_ACE is already counted once in the
        // struct size, so subtract sizeof(u32) to avoid double-counting.
        if (sid_len as usize) < std::mem::size_of::<u32>() {
            return Err(StorageError::Internal(format!(
                "SID length too small: {sid_len} bytes"
            )));
        }
        let acl_size = std::mem::size_of::<WIN_ACL>() as u32
            + std::mem::size_of::<windows_sys::Win32::Security::ACCESS_ALLOWED_ACE>() as u32
            + sid_len
            - std::mem::size_of::<u32>() as u32;
        let mut acl_buf = vec![0u8; acl_size as usize];
        let acl_ptr = acl_buf.as_mut_ptr() as *mut WIN_ACL;

        if InitializeAcl(acl_ptr, acl_size, ACL_REVISION) == 0 {
            return Err(StorageError::Internal("InitializeAcl failed".into()));
        }

        if AddAccessAllowedAce(acl_ptr, ACL_REVISION, GENERIC_ALL, user_sid) == 0 {
            return Err(StorageError::Internal("AddAccessAllowedAce failed".into()));
        }

        // 3. Apply as protected DACL (blocks inheritance from parent)
        let result = SetNamedSecurityInfoW(
            wide_path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            acl_ptr,
            std::ptr::null_mut(),
        );

        // acl_buf is stack-allocated, no LocalFree needed
        let _ = LocalFree; // suppress unused import warning

        if result != 0 {
            return Err(StorageError::Internal(format!(
                "SetNamedSecurityInfoW failed with error {result}"
            )));
        }

        tracing::debug!("Key file DACL set to owner-only: {:?}", path);
        Ok(())
    }
}

// Safe Debug implementation so the key is never printed to logs.
impl std::fmt::Debug for EncryptionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EncryptionKey([redacted])")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn fixture_key(fill: u8) -> EncryptionKey {
        EncryptionKey::from_bytes(std::array::from_fn(|_| fill))
    }

    /// Regression for #6242: the master key must wipe its bytes on drop. This is
    /// a compile-time guarantee — if the `ZeroizeOnDrop` derive is ever removed
    /// from `EncryptionKey`, this bound fails to compile.
    #[test]
    fn master_key_is_zeroize_on_drop() {
        fn assert_zeroize_on_drop<T: zeroize::ZeroizeOnDrop>() {}
        assert_zeroize_on_drop::<EncryptionKey>();
    }

    /// Regression for #6242: transient plaintext / hex copies of secret material
    /// are returned inside `Zeroizing` so they wipe on drop. The explicit type
    /// annotations pin the wrapper — dropping it back to a bare `String`/`Vec<u8>`
    /// breaks this test.
    #[test]
    fn secret_outputs_are_zeroizing_wrapped() {
        let key = fixture_key(0x42);

        let hex: zeroize::Zeroizing<String> = key.as_hex();
        assert_eq!(hex.len(), 64);

        let encrypted = key.encrypt(b"top secret").unwrap();
        let plaintext: zeroize::Zeroizing<Vec<u8>> = key.decrypt(&encrypted).unwrap();
        assert_eq!(&plaintext[..], b"top secret");
    }

    #[test]
    fn generates_32_byte_key() {
        let dir = TempDir::new().unwrap();
        let key = EncryptionKey::load_or_create(dir.path()).unwrap();
        assert_eq!(key.as_bytes().len(), 32);
    }

    #[test]
    fn hex_is_64_chars() {
        let dir = TempDir::new().unwrap();
        let key = EncryptionKey::load_or_create(dir.path()).unwrap();
        assert_eq!(key.as_hex().len(), 64);
        assert!(key.as_hex().chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn load_returns_same_key_as_generated() {
        let dir = TempDir::new().unwrap();
        let key1 = EncryptionKey::load_or_create(dir.path()).unwrap();
        let key2 = EncryptionKey::load_or_create(dir.path()).unwrap();
        // `as_hex` returns `Zeroizing<String>`; compare the inner strings.
        assert_eq!(key1.as_hex().as_str(), key2.as_hex().as_str());
    }

    #[test]
    fn key_file_created_with_correct_size() {
        let dir = TempDir::new().unwrap();
        EncryptionKey::load_or_create(dir.path()).unwrap();
        let content = fs::read(dir.path().join(".db_key")).unwrap();
        assert_eq!(content.len(), 32);
    }

    #[test]
    fn debug_does_not_leak_key_bytes() {
        let key = fixture_key(0xAB);
        let debug_str = format!("{key:?}");
        assert!(!debug_str.contains("AB"));
        assert!(debug_str.contains("redacted"));
    }

    #[test]
    fn encrypt_decrypt_round_trip() {
        let key = fixture_key(0x42);
        let plaintext = b"Hello, MAEKON frame data!";

        let encrypted = key.encrypt(plaintext).unwrap();
        // encrypted = 12-byte nonce + ciphertext + 16-byte auth tag
        assert!(encrypted.len() > plaintext.len());
        assert_ne!(&encrypted[12..], plaintext);

        let decrypted = key.decrypt(&encrypted).unwrap();
        // `decrypted` is `Zeroizing<Vec<u8>>`; compare as slices.
        assert_eq!(&decrypted[..], &plaintext[..]);
    }

    #[test]
    fn encrypt_produces_different_ciphertexts() {
        let key = fixture_key(0x42);
        let plaintext = b"same input";

        let enc1 = key.encrypt(plaintext).unwrap();
        let enc2 = key.encrypt(plaintext).unwrap();
        // Different random nonces produce different ciphertexts
        assert_ne!(enc1, enc2);

        // Both decrypt to the same plaintext (decrypt yields Zeroizing<Vec<u8>>).
        assert_eq!(&key.decrypt(&enc1).unwrap()[..], &plaintext[..]);
        assert_eq!(&key.decrypt(&enc2).unwrap()[..], &plaintext[..]);
    }

    #[test]
    fn decrypt_with_wrong_key_fails() {
        let key1 = fixture_key(0x42);
        let key2 = fixture_key(0x43);
        let plaintext = b"secret data";

        let encrypted = key1.encrypt(plaintext).unwrap();
        assert!(
            matches!(
                key2.decrypt(&encrypted).unwrap_err(),
                StorageError::Encryption(_)
            ),
            "wrong key must yield StorageError::Encryption (AES-GCM auth failure)"
        );
    }

    #[test]
    fn decrypt_too_short_data_fails() {
        let key = fixture_key(0x42);
        assert!(
            matches!(
                key.decrypt(&[0u8; 5]).unwrap_err(),
                StorageError::Encryption(_)
            ),
            "ciphertext shorter than nonce must yield StorageError::Encryption"
        );
    }

    #[test]
    fn decrypt_corrupted_data_fails() {
        let key = fixture_key(0x42);
        let mut encrypted = key.encrypt(b"test data").unwrap();
        // Corrupt a byte in the ciphertext region
        if encrypted.len() > 15 {
            encrypted[15] ^= 0xFF;
        }
        assert!(
            matches!(
                key.decrypt(&encrypted).unwrap_err(),
                StorageError::Encryption(_)
            ),
            "corrupted ciphertext must yield StorageError::Encryption (auth tag mismatch)"
        );
    }

    #[test]
    fn encrypt_empty_data() {
        let key = fixture_key(0x42);
        let encrypted = key.encrypt(b"").unwrap();
        // 12 nonce + 16 auth tag = 28 bytes minimum
        assert_eq!(encrypted.len(), 28);
        let decrypted = key.decrypt(&encrypted).unwrap();
        assert!(decrypted.is_empty());
    }

    #[test]
    fn encrypt_large_data() {
        let key = fixture_key(0x42);
        let plaintext = vec![0xAB_u8; 1024 * 1024]; // 1 MB

        let encrypted = key.encrypt(&plaintext).unwrap();
        let decrypted = key.decrypt(&encrypted).unwrap();
        // `decrypted` is `Zeroizing<Vec<u8>>`; compare as slices.
        assert_eq!(&decrypted[..], &plaintext[..]);
    }

    /// Verify that the key file is created with mode 0o600 (owner read/write only),
    /// with no world-readable window at any point (TOCTOU fix, issue #5991).
    #[cfg(unix)]
    #[test]
    fn key_file_created_with_mode_0o600() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = TempDir::new().unwrap();
        EncryptionKey::load_or_create(dir.path()).unwrap();

        let key_path = dir.path().join(".db_key");
        let metadata = fs::metadata(&key_path).unwrap();
        // Mask to the permission bits only (drop file type bits).
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "key file must be created with mode 0o600, got 0o{mode:o}"
        );
    }

    /// Verify that `save_to_file` recovers when the target file already exists
    /// (the `AlreadyExists` branch of the atomic `create_new` path) and the
    /// recreated file still has mode 0o600 — exercised by calling `save_to_file`
    /// directly twice on the same path (`load_or_create` cannot reach this branch
    /// because it short-circuits on `key_path.exists()`).
    #[cfg(unix)]
    #[test]
    fn key_file_save_over_existing_keeps_mode_0o600() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".db_key");

        // First write creates the file via create_new(true).
        EncryptionKey::from_bytes([1u8; 32])
            .save_to_file(&path)
            .unwrap();

        // Second write finds the file present -> hits the AlreadyExists branch
        // (remove + recreate). It must still land mode 0o600.
        EncryptionKey::from_bytes([2u8; 32])
            .save_to_file(&path)
            .unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "key file must keep mode 0o600 after the AlreadyExists recreate branch, got 0o{mode:o}"
        );
    }
}
