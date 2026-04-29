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
    let cert_reader = &mut std::io::BufReader::new(cert_pem);
    let certs: Vec<_> = rustls_pemfile::certs(cert_reader)
        .filter_map(|r| r.ok())
        .collect();

    if certs.is_empty() {
        warn!("no valid certificates in PEM data -- TLS disabled");
        return None;
    }

    // Parse private key from PEM
    let key_reader = &mut std::io::BufReader::new(key_pem);
    let key = rustls_pemfile::private_key(key_reader).ok().flatten();

    let key = match key {
        Some(k) => k,
        None => {
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
