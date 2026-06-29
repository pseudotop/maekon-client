//! Self-signed TLS certificate generation + TOFU pin store logic.
//!
//! Uses `rcgen` for cert generation and SHA-256 fingerprints for TOFU.
//! Certs are persisted as PEM files in the config directory.
//! Requires the `lan-sync` feature flag.

use std::path::Path;

use chrono::Datelike;
use sha2::Digest;
use tracing::{debug, info};

use maekon_core::error::CoreError;

/// Generate a self-signed TLS certificate for the given device ID.
///
/// Returns (cert_pem, key_pem) as byte vectors.
pub fn generate_self_signed_cert(device_id: &str) -> Result<(Vec<u8>, Vec<u8>), CoreError> {
    let subject_alt_name = format!("maekon-sync-{device_id}");
    let mut params =
        rcgen::CertificateParams::new(vec![subject_alt_name]).map_err(|e| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("cert params: {e}"),
        })?;
    params.distinguished_name = rcgen::DistinguishedName::new();
    params.distinguished_name.push(
        rcgen::DnType::CommonName,
        format!("MAEKON Sync {device_id}"),
    );
    // Valid for 10 years from now
    let now = chrono::Utc::now();
    let expiry_year = now.year() + 10;
    params.not_after = rcgen::date_time_ymd(expiry_year, now.month() as u8, now.day() as u8);

    let key_pair = rcgen::KeyPair::generate().map_err(|e| CoreError::Internal {
        code: maekon_core::error_codes::InternalCode::Generic,
        message: format!("key generation: {e}"),
    })?;
    let cert = params
        .self_signed(&key_pair)
        .map_err(|e| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
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
        code: maekon_core::error_codes::InternalCode::Generic,
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
            code: maekon_core::error_codes::InternalCode::Generic,
            message: "no certificate data found in PEM".to_string(),
        });
    }

    base64::engine::general_purpose::STANDARD
        .decode(&base64_content)
        .map_err(|e| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
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
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("read cert: {e}"),
        })?;
        let key_pem = std::fs::read(&key_path).map_err(|e| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("read key: {e}"),
        })?;
        let fingerprint = compute_cert_fingerprint(&cert_pem)?;
        info!("loaded existing TLS cert (fingerprint: {fingerprint})");
        return Ok((cert_pem, key_pem, fingerprint));
    }

    let (cert_pem, key_pem) = generate_self_signed_cert(device_id)?;
    let fingerprint = compute_cert_fingerprint(&cert_pem)?;

    std::fs::create_dir_all(config_dir).map_err(|e| CoreError::Internal {
        code: maekon_core::error_codes::InternalCode::Generic,
        message: format!("create config dir: {e}"),
    })?;
    std::fs::write(&cert_path, &cert_pem).map_err(|e| CoreError::Internal {
        code: maekon_core::error_codes::InternalCode::Generic,
        message: format!("write cert: {e}"),
    })?;
    // #6937: self-heal a partial-state key. The load guard above is
    // `cert.exists() && key.exists()` (AND), so this generate branch runs whenever
    // EITHER file is absent — including key-present/cert-absent (e.g. an operator
    // deleting only sync_cert.pem to force TOFU rotation, or a partial backup
    // restore). We are regenerating BOTH cert and key here, so any surviving key is
    // stale and inconsistent with the freshly-written cert; remove it before the
    // atomic create_new write. Without this, #6927's create_new(true) returned
    // AlreadyExists and permanently disabled LAN sync (the pre-#6927 fs::write
    // overwrite self-healed this; create_new lost that property).
    if key_path.exists() {
        std::fs::remove_file(&key_path).map_err(|e| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("remove stale key before regenerate: {e}"),
        })?;
    }
    // #6927: write the private key ATOMICALLY owner-only. The previous
    // `fs::write` then post-write `chmod 0o600` left a window where the key was
    // world-readable under the default umask (typically 0o644 on Linux). The
    // sibling secret stores (storage encryption.rs / file_secret_store.rs /
    // keychain.rs) all use O_CREAT|O_EXCL|mode(0o600) in one syscall — mirror it.
    // The stale key (if any) was removed just above, so create_new(true) creates a
    // fresh 0o600 key; a remaining AlreadyExists now genuinely means a concurrent
    // writer raced us, which is fail-closed (do not overwrite a concurrent key).
    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&key_path)
            .map_err(|e| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!("create key (atomic 0o600): {e}"),
            })?;
        file.write_all(&key_pem).map_err(|e| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("write key: {e}"),
        })?;
    }
    // On non-Unix (Windows) there is no umask/mode; the key lives under the
    // per-user config dir. Preserve the prior plain write (no behavior change /
    // no regression — the old code's chmod was already #[cfg(unix)]-only).
    #[cfg(not(unix))]
    {
        std::fs::write(&key_path, &key_pem).map_err(|e| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("write key: {e}"),
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

    /// #6927: the generated private key must be owner-only (0o600) on Unix — the
    /// atomic create_new+mode write leaves no world-readable window. Pre-fix the
    /// key was written under the default umask (often 0o644) before a post-write
    /// chmod, exposing a race window.
    #[test]
    #[cfg(unix)]
    fn generated_key_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        load_or_generate_cert(dir.path(), "dev-perm").unwrap();
        let key_path = dir.path().join("sync_key.pem");
        let mode = std::fs::metadata(&key_path).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "private key must be 0o600 owner-only, got {:o}",
            mode & 0o777
        );
    }

    /// #6937 regression guard: with a key-present/cert-absent partial state (e.g.
    /// operator deleted only the cert to force TOFU rotation), load_or_generate_cert
    /// must self-heal — regenerate the pair successfully, NOT return Err. Pre-fix the
    /// #6927 create_new(true) returned AlreadyExists on the surviving key and
    /// permanently disabled LAN sync.
    #[test]
    fn load_or_generate_self_heals_key_present_cert_absent() {
        let dir = tempfile::tempdir().unwrap();
        // First generate a full pair.
        let (_c1, key1, _fp1) = load_or_generate_cert(dir.path(), "dev-heal").unwrap();
        // Simulate partial state: delete the cert, leave the key.
        std::fs::remove_file(dir.path().join("sync_cert.pem")).unwrap();
        assert!(
            dir.path().join("sync_key.pem").exists(),
            "precondition: key survives"
        );

        // Must NOT Err — regenerate a fresh, consistent pair.
        let (_c2, key2, _fp2) = load_or_generate_cert(dir.path(), "dev-heal")
            .expect("partial-state (key-present/cert-absent) must self-heal, not deadlock");
        assert!(dir.path().join("sync_cert.pem").exists());
        assert!(dir.path().join("sync_key.pem").exists());
        // The stale key was replaced (new keypair), so the new key differs.
        assert_ne!(key1, key2, "regenerated key must replace the stale one");

        // And the regenerated key still has owner-only perms on Unix.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join("sync_key.pem"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "regenerated key must stay 0o600");
        }
    }
}
