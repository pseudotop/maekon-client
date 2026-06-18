use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use tracing::debug;

use maekon_core::error::CoreError;
use maekon_core::error_codes::ConfigCode;

/// Build a TLS configuration from PEM cert/key material.
///
/// The return value distinguishes "TLS intentionally disabled" from
/// "TLS requested but the supplied material is invalid":
///
/// - `Ok(None)` — both `cert_pem` and `key_pem` are empty. TLS is genuinely
///   disabled (e.g., transport-level unit tests); the caller may bind plain HTTP.
/// - `Ok(Some(_))` — valid cert/key; the caller binds with TLS.
/// - `Err(_)` — non-empty cert/key were supplied but failed to parse or build.
///   The caller MUST fail closed and must NOT downgrade to cleartext HTTP, which
///   would expose device metadata and defeat the TOFU certificate pinning.
pub(super) async fn try_build_tls_config(
    cert_pem: &[u8],
    key_pem: &[u8],
) -> Result<Option<axum_server::tls_rustls::RustlsConfig>, CoreError> {
    if cert_pem.is_empty() && key_pem.is_empty() {
        debug!("empty cert/key -- TLS disabled, using plain HTTP");
        return Ok(None);
    }

    // Once any material is supplied, TLS is requested: every failure below is
    // fatal (fail closed) rather than a silent cleartext downgrade.
    if cert_pem.is_empty() || key_pem.is_empty() {
        return Err(CoreError::Config {
            code: ConfigCode::Invalid,
            message: "LAN sync TLS requested but only one of cert/key supplied".to_string(),
        });
    }

    // Parse certificate chain from PEM
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(cert_pem)
        .filter_map(|r| r.ok())
        .collect();

    if certs.is_empty() {
        return Err(CoreError::Config {
            code: ConfigCode::Invalid,
            message: "LAN sync TLS cert PEM contains no valid certificates".to_string(),
        });
    }

    // Parse private key from PEM
    let key = PrivateKeyDer::from_pem_slice(key_pem).map_err(|e| CoreError::Config {
        code: ConfigCode::Invalid,
        message: format!("LAN sync TLS key PEM is invalid: {e}"),
    })?;

    // Build axum-server RustlsConfig (async constructor)
    let config = axum_server::tls_rustls::RustlsConfig::from_der(
        certs.into_iter().map(|c| c.to_vec()).collect(),
        key.secret_der().to_vec(),
    )
    .await
    .map_err(|e| CoreError::Config {
        code: ConfigCode::Invalid,
        message: format!("LAN sync TLS config build failed: {e}"),
    })?;

    debug!("TLS configuration built successfully");
    Ok(Some(config))
}
