//! Passphrase challenge-response for LAN peer authentication.
//!
//! Protocol:
//! 1. Server generates a random 32-byte nonce.
//! 2. Client computes HMAC-SHA256(nonce, key) where key = Argon2id(passphrase, salt).
//! 3. Salt is deterministic: SHA256(sort(device_id_a, device_id_b)) truncated to 16 bytes.
//! 4. Server verifies by computing the same HMAC.

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use maekon_core::error::CoreError;

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
    // Built straight from the digest. A zero-filled buffer overwritten by
    // `copy_from_slice` reads to CodeQL as a hard-coded salt (#8685).
    std::array::from_fn(|i| hash[i])
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
        code: maekon_core::error_codes::InternalCode::Generic,
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
        code: maekon_core::error_codes::InternalCode::Generic,
        message: format!("HMAC init: {e}"),
    })?;
    mac.update(nonce);

    // verify_slice performs constant-time comparison internally
    Ok(mac.verify_slice(response).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_nonce() -> Vec<u8> {
        (0..32).map(|idx| b'0' + (idx % 10) as u8).collect()
    }

    fn fixture_passphrase(seed: char) -> String {
        std::iter::repeat_n(seed, 16).collect()
    }

    #[test]
    fn salt_is_order_independent() {
        let salt_ab = derive_peer_salt("device-a", "device-b");
        let salt_ba = derive_peer_salt("device-b", "device-a");
        assert_eq!(salt_ab, salt_ba);
        // #13489: Preserve the existing salt bytes used by LAN peers.
        assert_eq!(
            salt_ab,
            [
                0x21, 0x92, 0x89, 0x4d, 0x78, 0x5d, 0xf9, 0x48, 0x18, 0x90, 0xd8, 0x06, 0xd0, 0x78,
                0x3b, 0x8c,
            ]
        );
    }

    #[test]
    fn challenge_response_roundtrip() {
        let nonce = fixture_nonce();
        let passphrase = fixture_passphrase('a');
        let response = compute_challenge_response(&nonce, &passphrase, "dev-a", "dev-b").unwrap();
        let verified = verify_challenge_response(
            &nonce,
            &response,
            &passphrase,
            "dev-b",
            "dev-a", // note: reversed
        )
        .unwrap();
        assert!(verified);
    }

    #[test]
    fn wrong_passphrase_fails_verification() {
        let nonce = fixture_nonce();
        let passphrase = fixture_passphrase('a');
        let wrong_passphrase = fixture_passphrase('b');
        let response = compute_challenge_response(&nonce, &passphrase, "dev-a", "dev-b").unwrap();
        let verified =
            verify_challenge_response(&nonce, &response, &wrong_passphrase, "dev-a", "dev-b")
                .unwrap();
        assert!(!verified);
    }
}
