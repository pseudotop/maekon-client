//! Self-signed TLS certificate generation + TOFU pin store logic.
//!
//! Uses `rcgen` for cert generation and SHA-256 fingerprints for TOFU.
//! Certs are persisted as PEM files in the config directory.
//! Requires the `lan-sync` feature flag.

use std::path::Path;

use chrono::Datelike;
use sha2::Digest;
use tracing::{debug, info};

use oneshim_core::error::CoreError;

/// Generate a self-signed TLS certificate for the given device ID.
///
/// Returns (cert_pem, key_pem) as byte vectors.
pub fn generate_self_signed_cert(device_id: &str) -> Result<(Vec<u8>, Vec<u8>), CoreError> {
    let subject_alt_name = format!("oneshim-sync-{device_id}");
    let mut params =
        rcgen::CertificateParams::new(vec![subject_alt_name]).map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("cert params: {e}"),
        })?;
    params.distinguished_name = rcgen::DistinguishedName::new();
    params.distinguished_name.push(
        rcgen::DnType::CommonName,
        format!("ONESHIM Sync {device_id}"),
    );
    // Valid for 10 years from now
    let now = chrono::Utc::now();
    let expiry_year = now.year() + 10;
    params.not_after = rcgen::date_time_ymd(expiry_year, now.month() as u8, now.day() as u8);

    let key_pair = rcgen::KeyPair::generate().map_err(|e| CoreError::Internal {
        code: oneshim_core::error_codes::InternalCode::Generic,
        message: format!("key generation: {e}"),
    })?;
    let cert = params
        .self_signed(&key_pair)
        .map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("self-sign: {e}"),
        })?;

    let cert_pem = cert.pem().into_bytes();
    let key_pem = key_pair.serialize_pem().into_bytes();

    debug!(device_id, "generated self-signed TLS certificate");
    Ok((cert_pem, key_pem))
}

/// Compute the SHA-256 fingerprint of a PEM-encoded certificate.
///
/// Returns the hex-encoded fingerprint.
pub fn compute_cert_fingerprint(cert_pem: &[u8]) -> Result<String, CoreError> {
    // Parse PEM to get DER bytes
    let pem_str = std::str::from_utf8(cert_pem).map_err(|e| CoreError::Internal {
        code: oneshim_core::error_codes::InternalCode::Generic,
        message: format!("invalid PEM encoding: {e}"),
    })?;

    // Extract DER from PEM manually
    let der_bytes = extract_der_from_pem(pem_str)?;
    let hash = sha2::Sha256::digest(&der_bytes);
    Ok(hex::encode(hash))
}

/// Extract DER bytes from a PEM string.
fn extract_der_from_pem(pem_str: &str) -> Result<Vec<u8>, CoreError> {
    use base64::Engine;
    let mut base64_content = String::new();
    let mut in_cert = false;

    for line in pem_str.lines() {
        if line.contains("BEGIN CERTIFICATE") {
            in_cert = true;
            continue;
        }
        if line.contains("END CERTIFICATE") {
            break;
        }
        if in_cert {
            base64_content.push_str(line.trim());
        }
    }

    if base64_content.is_empty() {
        return Err(CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: "no certificate data found in PEM".to_string(),
        });
    }

    base64::engine::general_purpose::STANDARD
        .decode(&base64_content)
        .map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("base64 decode: {e}"),
        })
}

/// Load an existing cert/key from disk, or generate + save a new pair.
///
/// Returns (cert_pem, key_pem, fingerprint_hex).
pub fn load_or_generate_cert(
    config_dir: &Path,
    device_id: &str,
) -> Result<(Vec<u8>, Vec<u8>, String), CoreError> {
    let cert_path = config_dir.join("sync_cert.pem");
    let key_path = config_dir.join("sync_key.pem");

    if cert_path.exists() && key_path.exists() {
        let cert_pem = std::fs::read(&cert_path).map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("read cert: {e}"),
        })?;
        let key_pem = std::fs::read(&key_path).map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("read key: {e}"),
        })?;
        let fingerprint = compute_cert_fingerprint(&cert_pem)?;
        info!("loaded existing TLS cert (fingerprint: {fingerprint})");
        return Ok((cert_pem, key_pem, fingerprint));
    }

    let (cert_pem, key_pem) = generate_self_signed_cert(device_id)?;
    let fingerprint = compute_cert_fingerprint(&cert_pem)?;

    std::fs::create_dir_all(config_dir).map_err(|e| CoreError::Internal {
        code: oneshim_core::error_codes::InternalCode::Generic,
        message: format!("create config dir: {e}"),
    })?;
    std::fs::write(&cert_path, &cert_pem).map_err(|e| CoreError::Internal {
        code: oneshim_core::error_codes::InternalCode::Generic,
        message: format!("write cert: {e}"),
    })?;
    std::fs::write(&key_path, &key_pem).map_err(|e| CoreError::Internal {
        code: oneshim_core::error_codes::InternalCode::Generic,
        message: format!("write key: {e}"),
    })?;

    // Restrict private key file permissions to owner-only on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(&key_path, perms).map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("set key permissions: {e}"),
        })?;
    }

    info!("generated new TLS cert (fingerprint: {fingerprint})");
    Ok((cert_pem, key_pem, fingerprint))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_cert_produces_valid_pem() {
        let (cert_pem, key_pem) = generate_self_signed_cert("test-dev").unwrap();
        let cert_str = String::from_utf8(cert_pem.clone()).unwrap();
        let key_str = String::from_utf8(key_pem).unwrap();
        assert!(cert_str.contains("BEGIN CERTIFICATE"));
        assert!(key_str.contains("BEGIN PRIVATE KEY"));
    }

    #[test]
    fn fingerprint_is_consistent() {
        let (cert_pem, _) = generate_self_signed_cert("test-fp").unwrap();
        let fp1 = compute_cert_fingerprint(&cert_pem).unwrap();
        let fp2 = compute_cert_fingerprint(&cert_pem).unwrap();
        assert_eq!(fp1, fp2);
        // SHA-256 hex is 64 chars
        assert_eq!(fp1.len(), 64);
    }

    #[test]
    fn load_or_generate_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let (cert1, key1, fp1) = load_or_generate_cert(dir.path(), "dev-1").unwrap();
        let (cert2, key2, fp2) = load_or_generate_cert(dir.path(), "dev-1").unwrap();
        assert_eq!(cert1, cert2);
        assert_eq!(key1, key2);
        assert_eq!(fp1, fp2);
    }
}
