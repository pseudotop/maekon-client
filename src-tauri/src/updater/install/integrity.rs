use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

use super::{UpdateError, Updater};

impl Updater {
    pub(in crate::updater) async fn fetch_signature(
        &self,
        download_url: &reqwest::Url,
    ) -> Result<Vec<u8>, UpdateError> {
        let sig_url = reqwest::Url::parse(&format!("{}.sig", download_url))
            .map_err(|e| UpdateError::Integrity(format!("Failed to parse signature URL: {}", e)))?;

        self.validate_download_url(sig_url.as_str())?;

        let response = self.http_client.get(sig_url.clone()).send().await?;
        if !response.status().is_success() {
            return Err(UpdateError::Integrity(format!(
                "Failed to download signature file: HTTP {} ({})",
                response.status(),
                sig_url
            )));
        }

        let body = response.bytes().await?;
        let body = String::from_utf8(body.to_vec()).map_err(|e| {
            UpdateError::Integrity(format!("Invalid signature file encoding: {}", e))
        })?;

        let sig_b64 = body
            .split_whitespace()
            .next()
            .ok_or_else(|| UpdateError::Integrity("Signature file is empty".to_string()))?;

        BASE64.decode(sig_b64).map_err(|e| {
            UpdateError::Integrity(format!("Failed to decode signature base64: {}", e))
        })
    }

    /// Verify the Ed25519 signature of `payload` against any trusted key.
    ///
    /// D9 multi-key trust:
    /// 1. Walk the built-in `TRUSTED_PUBLIC_KEYS` array first (primary
    ///    trust source — rotation story lives here).
    /// 2. If no built-in key validates, fall back to `config.signature_public_key`
    ///    IF it is non-empty AND different from every built-in key (a
    ///    genuine user override, e.g., dev self-signing).
    ///
    /// Returns `Integrity` error when no trusted key validates.
    pub(in crate::updater) fn verify_signature(
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
    pub(in crate::updater) fn verify_signature_with_keys(
        trusted: &[&str],
        configured: Option<&str>,
        payload: &[u8],
        signature_bytes: &[u8],
    ) -> Result<(), UpdateError> {
        // Normalize signature bytes once (same across all key attempts).
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
    /// verification; any parse/validation failure is an Err but callers treat
    /// it as "next key please" — only the absence of any successful key is
    /// surfaced as an integrity error (by the caller).
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

    pub(in crate::updater) async fn fetch_expected_sha256(
        &self,
        download_url: &reqwest::Url,
    ) -> Result<String, UpdateError> {
        let checksum_url = reqwest::Url::parse(&format!("{}.sha256", download_url))
            .map_err(|e| UpdateError::Integrity(format!("Failed to parse checksum URL: {}", e)))?;

        self.validate_download_url(checksum_url.as_str())?;

        let response = self.http_client.get(checksum_url.clone()).send().await?;
        if !response.status().is_success() {
            return Err(UpdateError::Integrity(format!(
                "Failed to download checksum file: HTTP {} ({})",
                response.status(),
                checksum_url
            )));
        }

        let body = response.bytes().await?;
        let body = String::from_utf8(body.to_vec()).map_err(|e| {
            UpdateError::Integrity(format!("Invalid checksum file encoding: {}", e))
        })?;

        Self::parse_sha256_manifest(&body)
    }

    pub(in crate::updater) fn parse_sha256_manifest(content: &str) -> Result<String, UpdateError> {
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

    pub(in crate::updater) fn sha256_hex(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        hasher.finalize().iter().fold(String::new(), |mut acc, b| {
            use std::fmt::Write as _;
            let _ = write!(acc, "{b:02x}");
            acc
        })
    }

    pub(in crate::updater) fn validate_download_url(
        &self,
        url: &str,
    ) -> Result<reqwest::Url, UpdateError> {
        let parsed = reqwest::Url::parse(url)
            .map_err(|e| UpdateError::Download(format!("Failed to parse download URL: {}", e)))?;

        let Some(host) = parsed.host_str() else {
            return Err(UpdateError::Download(
                "Download URL host is missing".to_string(),
            ));
        };

        if parsed.scheme() != "https" {
            #[cfg(test)]
            if parsed.scheme() == "http" && matches!(host, "localhost" | "127.0.0.1") {
                // Local test server is allowed for deterministic updater tests.
            } else {
                return Err(UpdateError::Download(format!(
                    "Only HTTPS download URLs are allowed: {}",
                    parsed
                )));
            }

            #[cfg(not(test))]
            return Err(UpdateError::Download(format!(
                "Only HTTPS download URLs are allowed: {}",
                parsed
            )));
        }

        if !Self::is_allowed_download_host(host) {
            return Err(UpdateError::Download(format!(
                "Disallowed download host: {}",
                host
            )));
        }

        Ok(parsed)
    }

    pub(in crate::updater) fn is_allowed_download_host(host: &str) -> bool {
        let allowlisted = Self::ALLOWED_DOWNLOAD_HOSTS.iter().any(|allowed_host| {
            host == *allowed_host || host.ends_with(&format!(".{}", allowed_host))
        });
        if allowlisted {
            return true;
        }

        #[cfg(test)]
        {
            matches!(host, "localhost" | "127.0.0.1")
        }

        #[cfg(not(test))]
        {
            false
        }
    }
}
