use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use oneshim_core::config::AppConfig;
use std::path::Path;

pub fn run_preflight(config: &AppConfig, offline_mode: bool) -> Result<()> {
    if !config.integrity.enabled {
        return Ok(());
    }

    if !offline_mode {
        config
            .update
            .validate_integrity_policy()
            .map_err(|e| anyhow!("Update integrity policy validation failed: {}", e))?;
    }

    if config.integrity.require_signed_policy_bundle {
        verify_signed_policy_bundle(config)?;
    }

    Ok(())
}

fn verify_signed_policy_bundle(config: &AppConfig) -> Result<()> {
    let policy_path = match config
        .integrity
        .policy_file_path
        .as_deref()
        .filter(|p| !p.trim().is_empty())
    {
        Some(p) => p,
        None => {
            tracing::warn!(
                "integrity.require_signed_policy_bundle is enabled but policy_file_path is not configured; \
                 skipping policy bundle verification"
            );
            return Ok(());
        }
    };
    let signature_path = match config
        .integrity
        .policy_signature_path
        .as_deref()
        .filter(|p| !p.trim().is_empty())
    {
        Some(p) => p,
        None => {
            tracing::warn!(
                "integrity.require_signed_policy_bundle is enabled but policy_signature_path is not configured; \
                 skipping policy bundle verification"
            );
            return Ok(());
        }
    };

    let policy_bytes = std::fs::read(Path::new(policy_path))
        .map_err(|e| anyhow!("Failed to read policy file ({}): {}", policy_path, e))?;
    let signature_text = std::fs::read_to_string(Path::new(signature_path)).map_err(|e| {
        anyhow!(
            "Failed to read policy signature file ({}): {}",
            signature_path,
            e
        )
    })?;

    let signature_b64 = signature_text
        .split_whitespace()
        .next()
        .ok_or_else(|| anyhow!("Policy signature file is empty"))?;
    let signature_bytes = BASE64
        .decode(signature_b64)
        .map_err(|e| anyhow!("Failed to decode policy signature base64: {}", e))?;
    let signature_array: [u8; 64] = signature_bytes
        .try_into()
        .map_err(|_| anyhow!("Policy signature length is invalid (expected 64 bytes)"))?;

    let key_b64 = config
        .integrity
        .policy_public_key
        .as_deref()
        .and_then(|k| k.split_whitespace().next())
        .filter(|k| !k.trim().is_empty())
        .unwrap_or(config.update.signature_public_key.as_str());

    let key_bytes = BASE64
        .decode(key_b64)
        .map_err(|e| anyhow!("Failed to decode policy public key base64: {}", e))?;
    let key_array: [u8; 32] = key_bytes
        .try_into()
        .map_err(|_| anyhow!("Policy public key length is invalid (expected 32 bytes)"))?;

    let verifying_key = VerifyingKey::from_bytes(&key_array)
        .map_err(|e| anyhow!("Failed to parse policy public key: {}", e))?;
    let signature = Signature::from_bytes(&signature_array);

    verifying_key
        .verify(&policy_bytes, &signature)
        .map_err(|e| anyhow!("Policy signature verification failed: {}", e))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use oneshim_core::config::AppConfig;
    use tempfile::tempdir;

    fn config_for_test() -> AppConfig {
        let mut config = AppConfig::default_config();
        config.update.enabled = true;
        config.update.require_signature_verification = true;
        config.update.signature_public_key = BASE64.encode([3u8; 32]);
        config
    }

    #[test]
    fn preflight_skips_when_policy_paths_not_configured() {
        let mut config = config_for_test();
        config.integrity.require_signed_policy_bundle = true;
        // policy_file_path and policy_signature_path are None by default
        // should gracefully skip, not error
        let result = run_preflight(&config, false);
        assert!(result.is_ok());
    }

    #[test]
    fn preflight_accepts_valid_signed_policy_bundle() {
        let dir = tempdir().expect("tempdir");
        let policy_path = dir.path().join("policy.json");
        let signature_path = dir.path().join("policy.json.sig");
        let payload = br#"{"policy":"strict"}"#;

        std::fs::write(&policy_path, payload).expect("write policy");

        let signing_key = SigningKey::from_bytes(&[9u8; 32]);
        let verifying_key = signing_key.verifying_key();
        let signature = signing_key.sign(payload).to_bytes();
        std::fs::write(&signature_path, format!("{}\n", BASE64.encode(signature)))
            .expect("write sig");

        let mut config = config_for_test();
        config.integrity.require_signed_policy_bundle = true;
        config.integrity.policy_file_path = Some(policy_path.to_string_lossy().to_string());
        config.integrity.policy_signature_path = Some(signature_path.to_string_lossy().to_string());
        config.integrity.policy_public_key = Some(BASE64.encode(verifying_key.as_bytes()));

        let result = run_preflight(&config, false);
        assert!(result.is_ok());
    }

    #[test]
    fn preflight_rejects_tampered_policy_bundle() {
        let dir = tempdir().expect("tempdir");
        let policy_path = dir.path().join("policy.json");
        let signature_path = dir.path().join("policy.json.sig");

        let signed_payload = br#"{"policy":"strict"}"#;
        let tampered_payload = br#"{"policy":"relaxed"}"#;
        std::fs::write(&policy_path, tampered_payload).expect("write policy");

        let signing_key = SigningKey::from_bytes(&[9u8; 32]);
        let verifying_key = signing_key.verifying_key();
        let signature = signing_key.sign(signed_payload).to_bytes();
        std::fs::write(&signature_path, format!("{}\n", BASE64.encode(signature)))
            .expect("write sig");

        let mut config = config_for_test();
        config.integrity.require_signed_policy_bundle = true;
        config.integrity.policy_file_path = Some(policy_path.to_string_lossy().to_string());
        config.integrity.policy_signature_path = Some(signature_path.to_string_lossy().to_string());
        config.integrity.policy_public_key = Some(BASE64.encode(verifying_key.as_bytes()));

        let result = run_preflight(&config, false);
        assert!(result.is_err());
    }

    #[test]
    fn preflight_rejects_invalid_signature_file_format() {
        let dir = tempdir().expect("tempdir");
        let policy_path = dir.path().join("policy.json");
        let signature_path = dir.path().join("policy.json.sig");
        std::fs::write(&policy_path, br#"{"policy":"strict"}"#).expect("write policy");
        std::fs::write(&signature_path, "not-base64\n").expect("write sig");

        let mut config = config_for_test();
        config.integrity.require_signed_policy_bundle = true;
        config.integrity.policy_file_path = Some(policy_path.to_string_lossy().to_string());
        config.integrity.policy_signature_path = Some(signature_path.to_string_lossy().to_string());

        let result = run_preflight(&config, false);
        assert!(result.is_err());
    }
}
