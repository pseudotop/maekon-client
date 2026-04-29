//! Trusted Ed25519 verification keys for update artifacts.
//!
//! The updater validates downloaded `.sig` files against every entry in this
//! array; the first matching key succeeds. Day-1 key rotation support is
//! provided by keeping both old and new keys resident during the overlap
//! window — see `docs/guides/updater-key-rotation.md` for the runbook.
//!
//! **Add new keys to the TOP of this array** when rotating. Remove deprecated
//! keys only as part of a compromise response (immediate removal) — normal
//! scheduled rotation retains old keys across 1-2 release cycles.
//!
//! Public keys listed here are base64-encoded, 32-byte Ed25519 `VerifyingKey`
//! bytes, matching the output of `nacl.signing.SigningKey(seed).verify_key.encode()`
//! or equivalent.

/// Static array of trusted Ed25519 verification keys (base64, 32 bytes each).
pub(crate) const TRUSTED_PUBLIC_KEYS: &[&str] = &[
    // v1 — introduced 2026-04-18 (Phase 4 Updater Hardening).
    // Production key since v0.4.x; identical to the default at
    // `crates/oneshim-core/src/config/sections/storage.rs:354`.
    "GIdf7Wg4kvvvoT7jR0xwKLKna8hUR1kvowONbHbPz1E=",
];

#[cfg(test)]
mod tests {
    use super::TRUSTED_PUBLIC_KEYS;

    #[test]
    fn trusted_keys_array_is_non_empty() {
        assert!(
            !TRUSTED_PUBLIC_KEYS.is_empty(),
            "At least one trusted verification key must be configured"
        );
    }

    #[test]
    fn trusted_keys_decode_to_32_bytes() {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        for (idx, key_b64) in TRUSTED_PUBLIC_KEYS.iter().enumerate() {
            let decoded = STANDARD
                .decode(key_b64)
                .unwrap_or_else(|e| panic!("TRUSTED_PUBLIC_KEYS[{idx}] is not valid base64: {e}"));
            assert_eq!(
                decoded.len(),
                32,
                "TRUSTED_PUBLIC_KEYS[{idx}] must decode to exactly 32 bytes, got {}",
                decoded.len()
            );
        }
    }
}
