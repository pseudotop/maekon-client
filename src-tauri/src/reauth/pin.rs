//! #8044: capture-history re-authentication PIN fallback — Argon2id hash
//! storage/verification.
//!
//! The baseline re-authentication method on platforms without biometrics
//! (Linux) or when hardware is unavailable. The raw PIN is never stored —
//! only the Argon2id PHC string (an offline-brute-force-resistant verifier)
//! is kept in SQLite `app_meta`. The `app_meta` key follows the same threat
//! model as the rest of local data, and a PHC is a one-way verifier, not a
//! plaintext secret.

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;

use crate::ipc_error::IpcError;

/// The `app_meta` key holding the PIN verifier (Argon2id PHC string).
pub(crate) const REAUTH_PIN_HASH_KEY: &str = "reauth_pin_hash";

/// Minimum PIN length (character count). Anything under 4 digits is
/// effectively no protection, so it is rejected.
pub(crate) const MIN_PIN_LEN: usize = 4;
/// Maximum PIN length. Guards against excessive length / DoS.
pub(crate) const MAX_PIN_LEN: usize = 64;

/// Validates the PIN's shape.
///
/// 4–64 characters, and not made up of whitespace alone. Character class
/// (digits/alphanumeric) is not restricted (letting the user choose a
/// stronger PIN). The raw value is not trimmed — intentional whitespace is
/// preserved.
pub(crate) fn validate_pin(pin: &str) -> Result<(), IpcError> {
    let len = pin.chars().count();
    if pin.trim().is_empty() {
        return Err(IpcError::new(
            "validation.invalid_arguments",
            "PIN must not be empty",
        ));
    }
    if len < MIN_PIN_LEN {
        return Err(IpcError::new(
            "validation.invalid_arguments",
            format!("PIN must be at least {MIN_PIN_LEN} characters"),
        ));
    }
    if len > MAX_PIN_LEN {
        return Err(IpcError::new(
            "validation.invalid_arguments",
            format!("PIN must be at most {MAX_PIN_LEN} characters"),
        ));
    }
    Ok(())
}

/// Hashes a PIN into an Argon2id PHC string (CPU-intensive — the caller runs
/// this in `spawn_blocking`).
///
/// The salt is 16 OS-CSPRNG bytes, base64-encoded. Every enrollment gets a
/// fresh salt.
pub(crate) fn hash_pin(pin: &str) -> Result<String, IpcError> {
    validate_pin(pin)?;
    let salt_bytes: [u8; 16] = rand::random();
    let salt = SaltString::encode_b64(&salt_bytes).map_err(|error| {
        IpcError::new(
            "internal.generic",
            format!("PIN salt encode failed: {error}"),
        )
    })?;
    let hash = Argon2::default()
        .hash_password(pin.as_bytes(), &salt)
        .map_err(|error| IpcError::new("internal.generic", format!("PIN hash failed: {error}")))?
        .to_string();
    Ok(hash)
}

/// Verifies a PIN against the stored PHC string (CPU-intensive — the caller
/// runs this in `spawn_blocking`).
///
/// **Fail-closed**: returns `false` on every non-success path (PHC parse
/// failure, verification error, etc.). Argon2's own verification is
/// constant-time, avoiding a timing oracle.
#[must_use]
pub(crate) fn verify_pin(pin: &str, stored_phc: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(stored_phc) else {
        return false;
    };
    Argon2::default()
        .verify_password(pin.as_bytes(), &parsed)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_rejects_too_short_and_empty() {
        validate_pin("123").unwrap_err();
        validate_pin("").unwrap_err();
        validate_pin("   ").unwrap_err();
    }

    #[test]
    fn validate_rejects_too_long() {
        let long = "1".repeat(MAX_PIN_LEN + 1);
        validate_pin(&long).unwrap_err();
    }

    #[test]
    fn validate_accepts_reasonable_pin() {
        validate_pin("1234").unwrap();
        validate_pin("correct horse battery").unwrap();
    }

    #[test]
    fn hash_then_verify_round_trip() {
        let phc = hash_pin("2468").expect("hash");
        assert!(verify_pin("2468", &phc), "correct PIN must verify");
    }

    #[test]
    fn wrong_pin_does_not_verify() {
        let phc = hash_pin("2468").expect("hash");
        assert!(!verify_pin("0000", &phc), "wrong PIN must be rejected");
    }

    #[test]
    fn verify_is_fail_closed_on_garbage_hash() {
        // A corrupted / non-PHC string fails verification (fail-closed).
        assert!(!verify_pin("2468", "not-a-valid-phc-string"));
        assert!(!verify_pin("2468", ""));
    }

    #[test]
    fn distinct_salts_produce_distinct_hashes() {
        let a = hash_pin("1234").expect("hash a");
        let b = hash_pin("1234").expect("hash b");
        assert_ne!(
            a, b,
            "each enrollment must produce a distinct PHC via a fresh salt"
        );
        // Both must still verify against the original PIN.
        assert!(verify_pin("1234", &a));
        assert!(verify_pin("1234", &b));
    }
}
