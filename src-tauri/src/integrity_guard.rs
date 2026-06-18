use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use maekon_core::config::AppConfig;
use std::path::Path;
use tracing::warn;

/// Boot-time integrity preflight. Returns `Err` for conditions that MUST block
/// startup; the caller (`BootstrapPreflightCoordinator::run`) propagates that
/// error so the integration wiring matches this unit-level fail-closed contract
/// (#6257 — the previous caller swallowed the `Err` into a warning, rendering
/// the gate fail-open at runtime).
pub fn run_preflight(config: &AppConfig, offline_mode: bool) -> Result<()> {
    if !config.integrity.enabled {
        return Ok(());
    }

    // Updater signature-verification downgrade gate (fatal). This passes on the
    // default config (verification on) and the policy is satisfied unless
    // `update.enabled` is paired with `require_signature_verification=false`,
    // i.e. an on-disk downgrade of the updater trust chain. Skipped in offline
    // mode (no updates are fetched, so there is nothing to verify).
    if !offline_mode {
        config
            .update
            .validate_integrity_policy()
            .map_err(|e| anyhow!("Update integrity policy validation failed: {}", e))?;
    }

    // Signed-policy-bundle gate. When `require_signed_policy_bundle` is set, the
    // enforcement depends on whether a bundle is actually PROVISIONED:
    //   - both paths configured  -> verify; missing/unreadable/tampered/forged
    //                                bundle is a HARD start-blocking error.
    //   - exactly one path set    -> half-provisioned misconfiguration -> fatal.
    //   - neither path set        -> NOT provisioned -> warn + skip (do NOT
    //                                block boot). The shipped default sets
    //                                `require_signed_policy_bundle=true` without
    //                                bundling a policy file, so a hard failure
    //                                here would brick every default install.
    // This split is what makes #6257 safe to make fatal: a genuinely tampered
    // provisioned bundle now blocks startup, while the unprovisioned default
    // still boots (with a warning).
    if config.integrity.require_signed_policy_bundle {
        match resolve_policy_bundle_paths(config)? {
            Some((policy_path, signature_path)) => {
                verify_signed_policy_bundle(config, policy_path, signature_path)?;
            }
            None => {
                warn!(
                    "integrity.require_signed_policy_bundle is enabled but no policy bundle is \
                     provisioned (policy_file_path / policy_signature_path unset); skipping bundle \
                     verification. Provision a signed policy bundle to enforce this gate."
                );
            }
        }
    }

    Ok(())
}

/// Resolve the configured policy/signature paths into an enforcement decision.
///
/// `Ok(None)` = not provisioned (skip); `Ok(Some((policy, sig)))` = both
/// configured (verify); `Err` = exactly one configured (half-provisioned
/// misconfiguration — fail closed rather than silently skipping). Mirrors the
/// Landlock precedent in maekon-automation's Linux sandbox: a partial isolation
/// request refuses to exec rather than downgrading to an unverified path.
fn resolve_policy_bundle_paths(config: &AppConfig) -> Result<Option<(&str, &str)>> {
    let policy_path = config
        .integrity
        .policy_file_path
        .as_deref()
        .filter(|p| !p.trim().is_empty());
    let signature_path = config
        .integrity
        .policy_signature_path
        .as_deref()
        .filter(|p| !p.trim().is_empty());

    match (policy_path, signature_path) {
        (None, None) => Ok(None),
        (Some(policy), Some(signature)) => Ok(Some((policy, signature))),
        (Some(_), None) => Err(anyhow!(
            "integrity.require_signed_policy_bundle is enabled and policy_file_path is set, but \
             policy_signature_path is not configured — refusing to start with a half-provisioned \
             policy bundle"
        )),
        (None, Some(_)) => Err(anyhow!(
            "integrity.require_signed_policy_bundle is enabled and policy_signature_path is set, but \
             policy_file_path is not configured — refusing to start with a half-provisioned \
             policy bundle"
        )),
    }
}

fn verify_signed_policy_bundle(
    config: &AppConfig,
    policy_path: &str,
    signature_path: &str,
) -> Result<()> {
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
    use maekon_core::config::AppConfig;
    use tempfile::tempdir;

    fn config_for_test() -> AppConfig {
        let mut config = AppConfig::default_config();
        config.update.enabled = true;
        config.update.require_signature_verification = true;
        config.update.signature_public_key = BASE64.encode([3u8; 32]);
        config
    }

    #[test]
    fn preflight_skips_when_policy_bundle_not_provisioned() {
        let mut config = config_for_test();
        config.integrity.require_signed_policy_bundle = true;
        // #6257: policy_file_path and policy_signature_path are None (the shipped
        // default). require_signed_policy_bundle=true is the fail-closed BASELINE,
        // but with NO bundle provisioned the preflight must NOT block boot — a
        // hard failure here would brick every default install. It warns and skips.
        run_preflight(&config, false).expect(
            "an unprovisioned policy bundle (both paths unset) must not block startup — \
             default installs ship require_signed_policy_bundle=true without a bundle",
        );
    }

    #[test]
    fn preflight_fails_closed_when_bundle_half_provisioned() {
        let dir = tempdir().expect("tempdir");
        let policy_path = dir.path().join("policy.json");
        std::fs::write(&policy_path, br#"{"policy":"strict"}"#).expect("write policy");

        let mut config = config_for_test();
        config.integrity.require_signed_policy_bundle = true;
        config.integrity.policy_file_path = Some(policy_path.to_string_lossy().to_string());
        // #6257: policy_signature_path remains unset. A HALF-provisioned bundle
        // (exactly one path set) is a misconfiguration and must fail-closed —
        // distinct from the unprovisioned default, which skips.
        let err = run_preflight(&config, false)
            .expect_err("preflight must fail-closed when the bundle is half-provisioned");
        assert!(
            err.to_string().contains("half-provisioned")
                && err
                    .to_string()
                    .contains("policy_signature_path is not configured"),
            "half-provisioned bundle (missing signature) must be reported; got: {err}"
        );
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

        run_preflight(&config, false)
            .expect("valid signed policy bundle must pass preflight without error");
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

        let err = run_preflight(&config, false).unwrap_err();
        assert!(
            err.to_string()
                .contains("Policy signature verification failed"),
            "tampered bundle must fail signature verification; got: {err}"
        );
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

        let err = run_preflight(&config, false).unwrap_err();
        assert!(
            err.to_string()
                .contains("Failed to decode policy signature base64"),
            "non-base64 signature file must fail base64 decode; got: {err}"
        );
    }
}
