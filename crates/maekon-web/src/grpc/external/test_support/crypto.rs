//! Real TLS and JWT credentials for the external gRPC test stack.

use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Once;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use tempfile::TempDir;

use super::FixtureError;

static RUSTLS_INIT: Once = Once::new();

/// Install the process crypto provider once, preserving an existing provider.
pub fn install_rustls_crypto_provider() {
    RUSTLS_INIT.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

struct ServerCert {
    _dir: TempDir,
    cert_path: PathBuf,
    key_path: PathBuf,
}

// Commit only a fully written certificate pair. A failed attempt leaves None,
// so a caller can retry after a transient storage failure has been repaired.
static CERT_CACHE: Mutex<Option<ServerCert>> = Mutex::new(None);

fn cached_cert_pair(
    cache: &Mutex<Option<ServerCert>>,
    generate: impl FnOnce() -> Result<ServerCert, FixtureError>,
) -> Result<(PathBuf, PathBuf), FixtureError> {
    let mut cached = cache.lock();
    if let Some(cert) = cached.as_ref() {
        return Ok((cert.cert_path.clone(), cert.key_path.clone()));
    }
    let cert = generate()?;
    let paths = (cert.cert_path.clone(), cert.key_path.clone());
    *cached = Some(cert);
    drop(cached);
    Ok(paths)
}

fn create_server_cert(dir: TempDir) -> Result<ServerCert, FixtureError> {
    use rcgen::{CertificateParams, KeyPair, SanType};
    let key = KeyPair::generate()?;
    let mut params = CertificateParams::new(vec!["localhost".to_string()])?;
    params
        .subject_alt_names
        .push(SanType::IpAddress(IpAddr::from([127, 0, 0, 1])));
    let cert = params.self_signed(&key)?;
    let cert_path = dir.path().join("cert.pem");
    let key_path = dir.path().join("key.pem");
    std::fs::write(&cert_path, cert.pem())
        .map_err(|e| FixtureError::io("writing the server certificate", e))?;
    std::fs::write(&key_path, key.serialize_pem())
        .map_err(|e| FixtureError::io("writing the server key", e))?;
    Ok(ServerCert {
        _dir: dir,
        cert_path,
        key_path,
    })
}

/// Return the cached certificate/key paths, or the actual generation/I/O error.
/// The SANs are localhost and 127.0.0.1; the cache owns their temporary directory.
pub fn test_cert_pair() -> Result<(PathBuf, PathBuf), FixtureError> {
    cached_cert_pair(&CERT_CACHE, || {
        let dir = TempDir::new()
            .map_err(|e| FixtureError::io("creating the server certificate directory", e))?;
        create_server_cert(dir)
    })
}

/// JWT test key pair, with its on-disk public key kept alive by the directory.
pub struct TestJwt {
    pub pub_pem_path: PathBuf,
    pub enc_key: jsonwebtoken::EncodingKey,
    pub _dir: TempDir,
}

/// Generate a real ES256 key pair and persist its public key.
pub fn test_jwt_keypair() -> Result<TestJwt, FixtureError> {
    use rcgen::{KeyPair, PKCS_ECDSA_P256_SHA256};
    let dir = TempDir::new().map_err(|e| FixtureError::io("creating the JWT directory", e))?;
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
    let pub_pem_path = dir.path().join("jwt_pub.pem");
    std::fs::write(&pub_pem_path, key.public_key_pem())
        .map_err(|e| FixtureError::io("writing the JWT public key", e))?;
    let enc_key = jsonwebtoken::EncodingKey::from_ec_pem(key.serialize_pem().as_bytes())?;
    Ok(TestJwt {
        pub_pem_path,
        enc_key,
        _dir: dir,
    })
}

/// Mint a real ES256 token. Negative offsets deliberately produce expired tokens.
/// Clock, timestamp range, and signing failures are returned to the caller.
pub fn test_mint_jwt(
    enc: &jsonwebtoken::EncodingKey,
    sub: &str,
    iss: &str,
    aud: &str,
    exp_offset_secs: i64,
) -> Result<String, FixtureError> {
    mint_jwt_at(enc, sub, iss, aud, exp_offset_secs, SystemTime::now())
}

fn mint_jwt_at(
    enc: &jsonwebtoken::EncodingKey,
    sub: &str,
    iss: &str,
    aud: &str,
    exp_offset_secs: i64,
    clock: SystemTime,
) -> Result<String, FixtureError> {
    use jsonwebtoken::{encode, Algorithm, Header};
    let now = i64::try_from(clock.duration_since(UNIX_EPOCH)?.as_secs())
        .map_err(|_| FixtureError::TimestampRange)?;
    let exp = now
        .checked_add(exp_offset_secs)
        .ok_or(FixtureError::TimestampRange)?;
    let claims = serde_json::json!({ "sub": sub, "iss": iss, "aud": aud, "exp": exp, "iat": now });
    Ok(encode(&Header::new(Algorithm::ES256), &claims, enc)?)
}

/// CA and client certificate, including the real DER used by verifier tests.
pub struct TestCaAndClient {
    pub ca_pem_path: PathBuf,
    pub client_cert_pem_path: PathBuf,
    pub client_key_pem_path: PathBuf,
    pub client_cert_der: Vec<u8>,
    pub _dir: TempDir,
}

/// Generate a CA and a signed client certificate. Preserve the existing whole-day
/// validity convention; checked arithmetic rejects unsupported lifetimes.
pub fn test_ca_and_client_cert(lifetime_hours: i64) -> Result<TestCaAndClient, FixtureError> {
    use chrono::{Datelike, Duration as ChronoDuration, Utc};
    use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};

    let now = Utc::now();
    let duration =
        ChronoDuration::try_hours(lifetime_hours).ok_or(FixtureError::CertificateLifetime)?;
    let expiry = now
        .checked_add_signed(duration)
        .ok_or(FixtureError::CertificateLifetime)?;
    // rcgen's infallible date constructor delegates to time's supported years.
    if !(-9999..=9999).contains(&now.year()) || !(-9999..=9999).contains(&expiry.year()) {
        return Err(FixtureError::CertificateLifetime);
    }
    let dir = TempDir::new()
        .map_err(|e| FixtureError::io("creating the client certificate directory", e))?;
    let ca_key = KeyPair::generate()?;
    let make_ca_params = || -> Result<CertificateParams, rcgen::Error> {
        let mut params = CertificateParams::new(vec!["test-ca".to_string()])?;
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "test-ca");
        Ok(params)
    };
    let ca_cert = make_ca_params()?.self_signed(&ca_key)?;
    let ca_pem_path = dir.path().join("ca.pem");
    std::fs::write(&ca_pem_path, ca_cert.pem())
        .map_err(|e| FixtureError::io("writing the client CA certificate", e))?;
    let issuer = Issuer::new(make_ca_params()?, ca_key);
    let client_key = KeyPair::generate()?;
    let mut params = CertificateParams::new(vec!["test-client".to_string()])?;
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "test-client");
    params.not_before = rcgen::date_time_ymd(now.year(), now.month() as u8, now.day() as u8);
    params.not_after =
        rcgen::date_time_ymd(expiry.year(), expiry.month() as u8, expiry.day() as u8);
    let client_cert = params.signed_by(&client_key, &issuer)?;
    let client_cert_pem_path = dir.path().join("client_cert.pem");
    let client_key_pem_path = dir.path().join("client_key.pem");
    std::fs::write(&client_cert_pem_path, client_cert.pem())
        .map_err(|e| FixtureError::io("writing the client certificate", e))?;
    std::fs::write(&client_key_pem_path, client_key.serialize_pem())
        .map_err(|e| FixtureError::io("writing the client key", e))?;
    Ok(TestCaAndClient {
        ca_pem_path,
        client_cert_pem_path,
        client_key_pem_path,
        client_cert_der: client_cert.der().to_vec(),
        _dir: dir,
    })
}

/// Read the actual cached server certificate, propagating missing/unreadable files.
pub fn server_cert_pem() -> Result<Vec<u8>, FixtureError> {
    let (cert_path, _) = test_cert_pair()?;
    std::fs::read(cert_path).map_err(|e| FixtureError::io("reading the server certificate", e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn provider_initializes_in_a_fresh_process_and_preserves_existing_provider() {
        const CHILD_MODE: &str = "MAEKON_TEST_CRYPTO_PROVIDER_CHILD";
        if let Ok(mode) = std::env::var(CHILD_MODE) {
            assert!(rustls::crypto::CryptoProvider::get_default().is_none());
            if mode == "preserve" {
                rustls::crypto::aws_lc_rs::default_provider()
                    .install_default()
                    .unwrap();
            } else {
                assert_eq!(mode, "install");
                install_rustls_crypto_provider();
            }
            let installed = rustls::crypto::CryptoProvider::get_default()
                .unwrap()
                .clone();
            install_rustls_crypto_provider();
            assert!(Arc::ptr_eq(
                &installed,
                rustls::crypto::CryptoProvider::get_default().unwrap()
            ));
            return;
        }
        // Other TLS tests install global state. A child process proves this
        // helper works without depending on test order or another initializer.
        for mode in ["install", "preserve"] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    concat!(
                        module_path!(),
                        "::provider_initializes_in_a_fresh_process_and_preserves_existing_provider"
                    ),
                    "--nocapture",
                ])
                .env(CHILD_MODE, mode)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "provider child failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[test]
    fn real_jwt_signature_and_expired_offset_are_preserved() {
        let keys = test_jwt_keypair().unwrap();
        let key =
            jsonwebtoken::DecodingKey::from_ec_pem(&std::fs::read(&keys.pub_pem_path).unwrap())
                .unwrap();
        let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::ES256);
        validation.set_audience(&["aud"]);
        validation.set_issuer(&["iss"]);
        let valid = test_mint_jwt(&keys.enc_key, "sub", "iss", "aud", 60).unwrap();
        let decoded = jsonwebtoken::decode::<serde_json::Value>(&valid, &key, &validation).unwrap();
        assert_eq!(decoded.claims["sub"], "sub");
        let expired = test_mint_jwt(&keys.enc_key, "sub", "iss", "aud", -3600).unwrap();
        let error =
            jsonwebtoken::decode::<serde_json::Value>(&expired, &key, &validation).unwrap_err();
        assert_eq!(
            *error.kind(),
            jsonwebtoken::errors::ErrorKind::ExpiredSignature
        );
    }

    #[test]
    fn timestamp_boundaries_return_errors_without_overflow() {
        let keys = test_jwt_keypair().unwrap();
        let epoch_plus_one = UNIX_EPOCH + Duration::from_secs(1);
        assert!(matches!(
            mint_jwt_at(&keys.enc_key, "sub", "iss", "aud", i64::MAX, epoch_plus_one),
            Err(FixtureError::TimestampRange)
        ));
        let maximum = mint_jwt_at(
            &keys.enc_key,
            "sub",
            "iss",
            "aud",
            i64::MAX - 1,
            epoch_plus_one,
        )
        .expect("largest representable expiry must be signed");
        let public_key =
            jsonwebtoken::DecodingKey::from_ec_pem(&std::fs::read(&keys.pub_pem_path).unwrap())
                .unwrap();
        let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::ES256);
        validation.set_audience(&["aud"]);
        validation.set_issuer(&["iss"]);
        let decoded = jsonwebtoken::decode::<serde_json::Value>(&maximum, &public_key, &validation)
            .expect("maximum expiry token must retain a valid signature");
        assert_eq!(decoded.claims["exp"], i64::MAX);
        assert_eq!(decoded.claims["iat"], 1);
        assert!(matches!(
            mint_jwt_at(
                &keys.enc_key,
                "sub",
                "iss",
                "aud",
                60,
                UNIX_EPOCH - Duration::from_secs(1)
            ),
            Err(FixtureError::Clock(_))
        ));
    }

    #[test]
    fn invalid_signing_key_is_an_error_without_sensitive_diagnostics() {
        let key = jsonwebtoken::EncodingKey::from_secret(b"invalid-private-key-fixture");
        let error = test_mint_jwt(&key, "private-subject", "iss", "aud", 60).unwrap_err();
        assert!(matches!(error, FixtureError::Jwt(_)));
        assert_eq!(error.to_string(), "fixture JWT signing failed");
    }

    #[test]
    fn certificate_lifetime_extremes_are_errors_and_normal_certificates_parse() {
        // The finite offsets fit chrono but exceed rcgen's supported years;
        // the extrema exercise the earlier checked-duration boundary.
        for lifetime in [i64::MIN, i64::MAX, 100_000_000, -200_000_000] {
            assert!(matches!(
                test_ca_and_client_cert(lifetime),
                Err(FixtureError::CertificateLifetime)
            ));
        }
        let cert = test_ca_and_client_cert(24).unwrap();
        let (remaining, parsed) =
            x509_parser::parse_x509_certificate(&cert.client_cert_der).unwrap();
        assert!(remaining.is_empty());
        assert_eq!(
            parsed
                .subject()
                .iter_common_name()
                .next()
                .unwrap()
                .as_str()
                .unwrap(),
            "test-client"
        );
        assert_eq!(
            parsed
                .issuer()
                .iter_common_name()
                .next()
                .unwrap()
                .as_str()
                .unwrap(),
            "test-ca"
        );
        assert_eq!(
            parsed.validity().not_after.timestamp() - parsed.validity().not_before.timestamp(),
            24 * 3600
        );
        assert!(!std::fs::read(cert.client_key_pem_path).unwrap().is_empty());
    }

    #[test]
    fn partial_certificate_write_is_not_cached_and_storage_recovery_succeeds() {
        let cache = Mutex::new(None);
        let dir = TempDir::new().unwrap();
        let failed_path = dir.path().to_path_buf();
        std::fs::create_dir(dir.path().join("key.pem")).unwrap();
        let error = cached_cert_pair(&cache, || create_server_cert(dir)).unwrap_err();
        assert!(matches!(
            error,
            FixtureError::Io {
                operation: "writing the server key",
                ..
            }
        ));
        assert!(cache.lock().is_none());
        assert!(
            !failed_path.exists(),
            "partial credentials must be removed with the failed directory"
        );
        let (cert, key) =
            cached_cert_pair(&cache, || create_server_cert(TempDir::new().unwrap())).unwrap();
        install_rustls_crypto_provider();
        let loaded = crate::grpc::external::tls_config::load_certified_key(&cert, &key)
            .expect("recovered certificate pair must load");
        assert_eq!(loaded.cert.len(), 1);
        loaded
            .keys_match()
            .expect("recovered key must match its certificate");
    }

    #[test]
    fn concurrent_initialization_publishes_one_complete_pair() {
        let cache = Arc::new(Mutex::new(None));
        let calls = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(std::sync::Barrier::new(8));
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let (cache, calls, barrier) = (cache.clone(), calls.clone(), barrier.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    cached_cert_pair(&cache, || {
                        calls.fetch_add(1, Ordering::SeqCst);
                        create_server_cert(TempDir::new().unwrap())
                    })
                    .unwrap()
                })
            })
            .collect();
        let paths: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(paths.iter().all(|pair| pair == &paths[0]));
        install_rustls_crypto_provider();
        let loaded =
            crate::grpc::external::tls_config::load_certified_key(&paths[0].0, &paths[0].1)
                .expect("concurrent initialization must publish a loadable pair");
        assert_eq!(loaded.cert.len(), 1);
        loaded
            .keys_match()
            .expect("published key must match its certificate");
    }
}
