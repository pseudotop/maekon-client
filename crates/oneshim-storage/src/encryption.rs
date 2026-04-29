//! 로컬 SQLite 데이터베이스 암호화 키 관리
//!
//! # 키 저장 전략
//! 1. 키 파일 (`app_data_dir/.db_key`): 32바이트 원시 키
//! 2. 파일 권한: Unix에서 0o600 (소유자만 읽기/쓰기)
//!
//! # 향후 작업
//! SQLCipher 또는 at-rest 암호화 레이어와 연동 예정.
//! 현재는 키 생성/저장/로드 인프라만 제공.

use crate::error::StorageError;
use std::path::{Path, PathBuf};

/// 32바이트 AES-256 데이터베이스 암호화 키
#[derive(Clone)]
pub struct EncryptionKey([u8; 32]);

impl EncryptionKey {
    /// 키 파일에서 로드하거나 신규 생성
    ///
    /// - 파일 존재 시: 로드 (32바이트 검증)
    /// - 파일 없을 시: 생성 후 파일에 저장 (Unix: 0o600 권한)
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

    /// 원시 바이트에서 키 생성
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// SQLite pragma key 형식 (hex 문자열)
    pub fn as_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// 원시 바이트 참조
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// AES-256-GCM으로 데이터 암호화.
    /// 반환 형식: nonce(12 bytes) || ciphertext(+16 bytes auth tag)
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

    /// AES-256-GCM으로 encrypt()가 생성한 데이터 복호화.
    pub fn decrypt(&self, data: &[u8]) -> Result<Vec<u8>, StorageError> {
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
        std::fs::write(path, self.0).map_err(|e| {
            StorageError::Internal(format!("Key file write failed ({path:?}): {e}"))
        })?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(
                |e| StorageError::Internal(format!("Key file permission set failed: {e}")),
            )?;
        }

        #[cfg(windows)]
        {
            if let Err(e) = set_owner_only_dacl(path) {
                tracing::warn!("Failed to set owner-only DACL on key file: {e}");
            }
        }

        Ok(())
    }
}

/// Set an owner-only DACL on a file (Windows equivalent of Unix chmod 0o600).
///
/// Creates an ACL with a single ACE granting the current user GENERIC_ALL,
/// and applies it as a protected DACL (no inheritance from parent).
#[cfg(windows)]
fn set_owner_only_dacl(path: &std::path::Path) -> Result<(), StorageError> {
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

// 키가 로그에 출력되지 않도록 Debug 구현 안전하게
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
        assert_eq!(key1.as_hex(), key2.as_hex());
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
        let key = EncryptionKey::from_bytes([0xAB; 32]);
        let debug_str = format!("{key:?}");
        assert!(!debug_str.contains("AB"));
        assert!(debug_str.contains("redacted"));
    }

    #[test]
    fn encrypt_decrypt_round_trip() {
        let key = EncryptionKey::from_bytes([0x42; 32]);
        let plaintext = b"Hello, ONESHIM frame data!";

        let encrypted = key.encrypt(plaintext).unwrap();
        // encrypted = 12-byte nonce + ciphertext + 16-byte auth tag
        assert!(encrypted.len() > plaintext.len());
        assert_ne!(&encrypted[12..], plaintext);

        let decrypted = key.decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn encrypt_produces_different_ciphertexts() {
        let key = EncryptionKey::from_bytes([0x42; 32]);
        let plaintext = b"same input";

        let enc1 = key.encrypt(plaintext).unwrap();
        let enc2 = key.encrypt(plaintext).unwrap();
        // Different random nonces produce different ciphertexts
        assert_ne!(enc1, enc2);

        // Both decrypt to the same plaintext
        assert_eq!(key.decrypt(&enc1).unwrap(), plaintext);
        assert_eq!(key.decrypt(&enc2).unwrap(), plaintext);
    }

    #[test]
    fn decrypt_with_wrong_key_fails() {
        let key1 = EncryptionKey::from_bytes([0x42; 32]);
        let key2 = EncryptionKey::from_bytes([0x43; 32]);
        let plaintext = b"secret data";

        let encrypted = key1.encrypt(plaintext).unwrap();
        let result = key2.decrypt(&encrypted);
        assert!(result.is_err());
    }

    #[test]
    fn decrypt_too_short_data_fails() {
        let key = EncryptionKey::from_bytes([0x42; 32]);
        let result = key.decrypt(&[0u8; 5]);
        assert!(result.is_err());
    }

    #[test]
    fn decrypt_corrupted_data_fails() {
        let key = EncryptionKey::from_bytes([0x42; 32]);
        let mut encrypted = key.encrypt(b"test data").unwrap();
        // Corrupt a byte in the ciphertext region
        if encrypted.len() > 15 {
            encrypted[15] ^= 0xFF;
        }
        let result = key.decrypt(&encrypted);
        assert!(result.is_err());
    }

    #[test]
    fn encrypt_empty_data() {
        let key = EncryptionKey::from_bytes([0x42; 32]);
        let encrypted = key.encrypt(b"").unwrap();
        // 12 nonce + 16 auth tag = 28 bytes minimum
        assert_eq!(encrypted.len(), 28);
        let decrypted = key.decrypt(&encrypted).unwrap();
        assert!(decrypted.is_empty());
    }

    #[test]
    fn encrypt_large_data() {
        let key = EncryptionKey::from_bytes([0x42; 32]);
        let plaintext = vec![0xAB_u8; 1024 * 1024]; // 1 MB

        let encrypted = key.encrypt(&plaintext).unwrap();
        let decrypted = key.decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }
}
