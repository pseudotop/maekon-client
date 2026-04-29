//! Passphrase challenge-response for LAN peer authentication.
//!
//! Protocol:
//! 1. Server generates a random 32-byte nonce.
//! 2. Client computes HMAC-SHA256(nonce, key) where key = Argon2id(passphrase, salt).
//! 3. Salt is deterministic: SHA256(sort(device_id_a, device_id_b)) truncated to 16 bytes.
//! 4. Server verifies by computing the same HMAC.

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use oneshim_core::error::CoreError;

use super::sync_crypto;

/// Derive a deterministic salt from two device IDs.
/// Sort lexicographically, concatenate, SHA-256, truncate to 16 bytes.
pub fn derive_peer_salt(device_id_a: &str, device_id_b: &str) -> [u8; 16] {
    use sha2::Digest;
    let (first, second) = if device_id_a <= device_id_b {
        (device_id_a, device_id_b)
    } else {
        (device_id_b, device_id_a)
    };
    let combined = format!("{first}{second}");
    let hash = sha2::Sha256::digest(combined.as_bytes());
    let mut salt = [0u8; 16];
    salt.copy_from_slice(&hash[..16]);
    salt
}

/// Compute the HMAC-SHA256 response for a challenge nonce.
pub fn compute_challenge_response(
    nonce: &[u8],
    passphrase: &str,
    local_device_id: &str,
    peer_device_id: &str,
) -> Result<Vec<u8>, CoreError> {
    let salt = derive_peer_salt(local_device_id, peer_device_id);
    let key = sync_crypto::derive_key(passphrase, &salt)?;

    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(&key).map_err(|e| CoreError::Internal {
        code: oneshim_core::error_codes::InternalCode::Generic,
        message: format!("HMAC init: {e}"),
    })?;
    mac.update(nonce);
    Ok(mac.finalize().into_bytes().to_vec())
}

/// Verify a challenge response using constant-time comparison.
///
/// Uses `hmac::Mac::verify_slice` internally, which is constant-time
/// to prevent timing side-channel attacks.
pub fn verify_challenge_response(
    nonce: &[u8],
    response: &[u8],
    passphrase: &str,
    local_device_id: &str,
    peer_device_id: &str,
) -> Result<bool, CoreError> {
    let salt = derive_peer_salt(local_device_id, peer_device_id);
    let key = sync_crypto::derive_key(passphrase, &salt)?;

    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(&key).map_err(|e| CoreError::Internal {
        code: oneshim_core::error_codes::InternalCode::Generic,
        message: format!("HMAC init: {e}"),
    })?;
    mac.update(nonce);

    // verify_slice performs constant-time comparison internally
    Ok(mac.verify_slice(response).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn salt_is_order_independent() {
        let salt_ab = derive_peer_salt("device-a", "device-b");
        let salt_ba = derive_peer_salt("device-b", "device-a");
        assert_eq!(salt_ab, salt_ba);
    }

    #[test]
    fn challenge_response_roundtrip() {
        let nonce = b"12345678901234567890123456789012";
        let passphrase = "shared-secret";
        let response = compute_challenge_response(nonce, passphrase, "dev-a", "dev-b").unwrap();
        let verified = verify_challenge_response(
            nonce, &response, passphrase, "dev-b", "dev-a", // note: reversed
        )
        .unwrap();
        assert!(verified);
    }

    #[test]
    fn wrong_passphrase_fails_verification() {
        let nonce = b"12345678901234567890123456789012";
        let response = compute_challenge_response(nonce, "correct-pass", "dev-a", "dev-b").unwrap();
        let verified =
            verify_challenge_response(nonce, &response, "wrong-pass", "dev-a", "dev-b").unwrap();
        assert!(!verified);
    }
}
