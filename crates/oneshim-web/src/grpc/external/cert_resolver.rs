//! ResolvesServerCert impl holding an Arc<ArcSwap<Arc<CertifiedKey>>>.
//! Swap is atomic — live connections keep the cert they started with;
//! new connections get the latest. No ServerConfig rebuild needed.

use std::sync::Arc;

use arc_swap::ArcSwap;
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use x509_parser::prelude::*;

#[derive(Debug)]
pub struct HotReloadCertResolver {
    current: ArcSwap<CertifiedKey>,
}

impl HotReloadCertResolver {
    pub fn new(initial: Arc<CertifiedKey>) -> Self {
        Self {
            current: ArcSwap::new(initial),
        }
    }

    pub fn swap(&self, next: Arc<CertifiedKey>) {
        self.current.store(next);
    }

    pub fn current(&self) -> Arc<CertifiedKey> {
        self.current.load_full()
    }
}

impl ResolvesServerCert for HotReloadCertResolver {
    fn resolve(&self, _client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.current.load_full())
    }
}

impl HotReloadCertResolver {
    /// Returns days until the current leaf cert's `notAfter` field, or `0` if already expired.
    /// Returns `None` if the cert chain is empty or the DER cannot be parsed.
    pub fn days_until_expiry(&self) -> Option<i64> {
        let certified = self.current();
        let leaf = certified.cert.first()?;
        let (_, cert) = X509Certificate::from_der(leaf.as_ref()).ok()?;
        let not_after = cert.validity().not_after.timestamp();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs() as i64;
        Some((not_after - now).max(0) / (24 * 3600))
    }
}

#[cfg(all(test, feature = "external-grpc-tools"))]
mod tests {
    use super::*;
    use rustls::sign::CertifiedKey;
    use std::sync::Arc;

    fn fixture_certified_key() -> Arc<CertifiedKey> {
        // rcgen 0.14 API (matches pattern in crates/oneshim-network/src/sync/lan_tls.rs)
        use rcgen::{CertificateParams, KeyPair};
        let key_pair = KeyPair::generate().expect("keypair");
        let params = CertificateParams::new(vec!["localhost".into()]).expect("params");
        let cert = params.self_signed(&key_pair).expect("self-signed");

        let cert_der = rustls::pki_types::CertificateDer::from(cert.der().to_vec());
        let key_pkcs8_der = key_pair.serialize_der();
        let key_der = rustls::pki_types::PrivateKeyDer::try_from(key_pkcs8_der).expect("key");
        let signing_key =
            rustls::crypto::aws_lc_rs::sign::any_supported_type(&key_der).expect("signer");
        Arc::new(CertifiedKey::new(vec![cert_der], signing_key))
    }

    #[test]
    fn resolver_returns_current_key() {
        let key = fixture_certified_key();
        let resolver = HotReloadCertResolver::new(key.clone());
        let resolved = resolver.current();
        assert!(Arc::ptr_eq(&resolved, &key));
    }

    #[test]
    fn resolver_swap_changes_current() {
        let key1 = fixture_certified_key();
        let resolver = HotReloadCertResolver::new(key1.clone());
        let key2 = fixture_certified_key();
        resolver.swap(key2.clone());
        let resolved = resolver.current();
        assert!(Arc::ptr_eq(&resolved, &key2));
        assert!(!Arc::ptr_eq(&resolved, &key1));
    }

    #[test]
    fn days_until_expiry_returns_value() {
        // rcgen 0.14 CertificateParams::default() sets not_after = year 4096 (~2070 years out).
        // We check that the returned value is positive (not expired) and plausible.
        let key = fixture_certified_key();
        let resolver = HotReloadCertResolver::new(key);
        let days = resolver.days_until_expiry().unwrap();
        // year 4096 is ~755_000–756_000 days from 2026.
        assert!(
            days > 700_000 && days < 760_000,
            "expected ~2070 years from rcgen default, got {days} days"
        );
    }
}
