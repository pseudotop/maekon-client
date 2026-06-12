//! Ed25519 signature verification, SHA-256 checksum, manifest parsing.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

use super::super::{UpdateError, Updater};

impl Updater {
    /// Verify the Ed25519 signature of `payload` against any trusted key.
    ///
    /// D9 multi-key trust:
    /// 1. Walk the built-in `TRUSTED_PUBLIC_KEYS` array first.
    /// 2. Fall back to `config.signature_public_key` if non-empty and distinct
    ///    from every built-in key.
    ///
    /// Returns `Integrity` error when no trusted key validates.
    pub(crate) fn verify_signature(
        &self,
        payload: &[u8],
        signature_bytes: &[u8],
    ) -> Result<(), UpdateError> {
        let configured = self
            .config
            .signature_public_key
            .split_whitespace()
            .next()
            .filter(|k| !k.trim().is_empty());
        Self::verify_signature_with_keys(
            super::super::trusted_keys::TRUSTED_PUBLIC_KEYS,
            configured,
            payload,
            signature_bytes,
        )
    }

    /// Inner verification helper with an explicit trusted-key list + optional
    /// configured-key override. Extracted so tests can supply an arbitrary
    /// trusted list without mutating the production `const` array.
    pub(crate) fn verify_signature_with_keys(
        trusted: &[&str],
        configured: Option<&str>,
        payload: &[u8],
        signature_bytes: &[u8],
    ) -> Result<(), UpdateError> {
        let signature_array: [u8; 64] = signature_bytes.try_into().map_err(|_| {
            UpdateError::Integrity(format!(
                "Invalid signature length: {} bytes (expected 64)",
                signature_bytes.len()
            ))
        })?;
        let signature = Signature::from_bytes(&signature_array);

        // (1) Try every built-in trusted key.
        for (idx, key_b64) in trusted.iter().enumerate() {
            if Self::try_verify_with_key_b64(key_b64, payload, &signature).is_ok() {
                if idx > 0 {
                    tracing::info!(
                        "signature validated by trusted key #{idx} (rotation in progress)"
                    );
                }
                return Ok(());
            }
        }

        // (2) Fall back to the user-configured key if present AND
        //     genuinely distinct from any built-in key.
        if let Some(configured_key) = configured {
            let already_tried = trusted.contains(&configured_key);
            if !already_tried
                && Self::try_verify_with_key_b64(configured_key, payload, &signature).is_ok()
            {
                tracing::warn!("signature validated via user-configured key (override)");
                return Ok(());
            }
        }

        Err(UpdateError::Integrity(
            "no trusted key validated the signature".into(),
        ))
    }

    /// Try a single base64-encoded 32-byte public key. Returns Ok on successful
    /// verification; any parse/validation failure is treated as "next key please"
    /// by the caller.
    fn try_verify_with_key_b64(
        key_b64: &str,
        payload: &[u8],
        signature: &Signature,
    ) -> Result<(), UpdateError> {
        let key_bytes = BASE64.decode(key_b64).map_err(|e| {
            UpdateError::Integrity(format!("Failed to decode public key base64: {}", e))
        })?;
        let key_len = key_bytes.len();
        let key_array: [u8; 32] = key_bytes.try_into().map_err(|_| {
            UpdateError::Integrity(format!(
                "Invalid public key length: {} bytes (expected 32)",
                key_len
            ))
        })?;
        let public_key = VerifyingKey::from_bytes(&key_array)
            .map_err(|e| UpdateError::Integrity(format!("Failed to parse public key: {}", e)))?;
        public_key
            .verify(payload, signature)
            .map_err(|e| UpdateError::Integrity(format!("Signature verification failed: {}", e)))
    }

    pub(crate) fn parse_sha256_manifest(content: &str) -> Result<String, UpdateError> {
        let hash = content
            .split_whitespace()
            .next()
            .ok_or_else(|| UpdateError::Integrity("Checksum file is empty".to_string()))?
            .to_ascii_lowercase();

        let is_hex = hash.len() == 64 && hash.chars().all(|ch| ch.is_ascii_hexdigit());
        if !is_hex {
            return Err(UpdateError::Integrity(format!(
                "Invalid SHA-256 format: {}",
                hash
            )));
        }

        Ok(hash)
    }

    pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        hasher.finalize().iter().fold(String::new(), |mut acc, b| {
            use std::fmt::Write as _;
            let _ = write!(acc, "{b:02x}");
            acc
        })
    }
}
