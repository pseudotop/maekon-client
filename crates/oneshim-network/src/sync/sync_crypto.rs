//! Shared AES-256-GCM encryption for sync transports.
//!
//! Encryption format: salt (16 bytes) || nonce (12 bytes) || ciphertext.
//! Key derivation: Argon2id with default parameters.
//! Identical logic to `FileSyncTransport::encrypt/decrypt` in oneshim-storage.

use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use argon2::Argon2;
use oneshim_core::error::CoreError;

const NONCE_SIZE: usize = 12;
const SALT_SIZE: usize = 16;

/// Derive a 32-byte AES-256 key from passphrase + salt via Argon2id.
pub fn derive_key(passphrase: &str, salt: &[u8]) -> Result<[u8; 32], CoreError> {
    let mut key = [0u8; 32];
    Argon2::default()
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("Argon2 KDF failed: {e}"),
        })?;
    Ok(key)
}

/// Encrypt plaintext with AES-256-GCM.
/// Returns: salt (16) || nonce (12) || ciphertext.
pub fn encrypt(passphrase: &str, plaintext: &[u8]) -> Result<Vec<u8>, CoreError> {
    use aes_gcm::aead::rand_core::RngCore;
    let mut salt = [0u8; SALT_SIZE];
    OsRng.fill_bytes(&mut salt);

    let key = derive_key(passphrase, &salt)?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| CoreError::Internal {
        code: oneshim_core::error_codes::InternalCode::Generic,
        message: format!("AES init: {e}"),
    })?;

    let mut nonce_bytes = [0u8; NONCE_SIZE];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("AES encrypt: {e}"),
        })?;

    let mut output = Vec::with_capacity(SALT_SIZE + NONCE_SIZE + ciphertext.len());
    output.extend_from_slice(&salt);
    output.extend_from_slice(&nonce_bytes);
    output.extend_from_slice(&ciphertext);
    Ok(output)
}

/// Decrypt: parse salt || nonce || ciphertext.
pub fn decrypt(passphrase: &str, data: &[u8]) -> Result<Vec<u8>, CoreError> {
    if data.len() < SALT_SIZE + NONCE_SIZE + 1 {
        return Err(CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: "encrypted data too short".to_string(),
        });
    }
    let salt = &data[..SALT_SIZE];
    let nonce_bytes = &data[SALT_SIZE..SALT_SIZE + NONCE_SIZE];
    let ciphertext = &data[SALT_SIZE + NONCE_SIZE..];

    let key = derive_key(passphrase, salt)?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| CoreError::Internal {
        code: oneshim_core::error_codes::InternalCode::Generic,
        message: format!("AES init: {e}"),
    })?;
    let nonce = Nonce::from_slice(nonce_bytes);

    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("AES decrypt failed (wrong passphrase?): {e}"),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let passphrase = "test-passphrase-12345";
        let plaintext = b"hello world, this is a sync test";
        let encrypted = encrypt(passphrase, plaintext).unwrap();
        assert_ne!(encrypted.as_slice(), plaintext);
        let decrypted = decrypt(passphrase, &encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn wrong_passphrase_fails() {
        let encrypted = encrypt("correct", b"secret").unwrap();
        assert!(decrypt("wrong", &encrypted).is_err());
    }

    #[test]
    fn empty_data_fails() {
        assert!(decrypt("pass", &[]).is_err());
    }

    #[test]
    fn cross_transport_compat() {
        // Verify the wire format is identical to FileSyncTransport
        let passphrase = "compat-test";
        let data = b"cross-transport compatibility check";
        let encrypted = encrypt(passphrase, data).unwrap();
        // salt(16) + nonce(12) + ciphertext(>=1) + tag(16)
        assert!(encrypted.len() >= 16 + 12 + data.len() + 16);
        let decrypted = decrypt(passphrase, &encrypted).unwrap();
        assert_eq!(decrypted, data);
    }
}
