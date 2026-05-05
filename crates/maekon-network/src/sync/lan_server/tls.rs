use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use tracing::{debug, warn};

pub(super) async fn try_build_tls_config(
    cert_pem: &[u8],
    key_pem: &[u8],
) -> Option<axum_server::tls_rustls::RustlsConfig> {
    if cert_pem.is_empty() || key_pem.is_empty() {
        debug!("empty cert/key -- TLS disabled, using plain HTTP");
        return None;
    }

    // Parse certificate chain from PEM
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(cert_pem)
        .filter_map(|r| r.ok())
        .collect();

    if certs.is_empty() {
        warn!("no valid certificates in PEM data -- TLS disabled");
        return None;
    }

    // Parse private key from PEM
    let key = match PrivateKeyDer::from_pem_slice(key_pem) {
        Ok(k) => k,
        Err(_) => {
            warn!("no valid private key in PEM data -- TLS disabled");
            return None;
        }
    };

    // Build axum-server RustlsConfig (async constructor)
    let config = axum_server::tls_rustls::RustlsConfig::from_der(
        certs.into_iter().map(|c| c.to_vec()).collect(),
        key.secret_der().to_vec(),
    )
    .await;

    match config {
        Ok(c) => {
            debug!("TLS configuration built successfully");
            Some(c)
        }
        Err(e) => {
            warn!("failed to build TLS config: {e} -- TLS disabled");
            None
        }
    }
}
