//! TLS ServerConfig builder + cert file loader.
//! The hot-reload mechanism lives in cert_resolver.rs (swap inner Arc).
//! This module provides the PEM → CertifiedKey path and a file-watcher that
//! swaps the resolver on atomic rename (cert rotation without restart).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration as StdDuration;

use notify::RecursiveMode;
use notify_debouncer_mini::{new_debouncer, DebounceEventResult};
use rustls::crypto::aws_lc_rs;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::sign::CertifiedKey;
use thiserror::Error;
use tokio::sync::{mpsc, watch};
use tracing::{error, info, warn};

use crate::grpc::external::cert_resolver::HotReloadCertResolver;

#[derive(Debug, Error)]
pub enum TlsLoadError {
    #[error("read {path:?}: {source}")]
    Read {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parse cert PEM: {0}")]
    ParseCert(String),
    #[error("parse key PEM: {0}")]
    ParseKey(String),
    #[error("empty cert chain in {0:?}")]
    EmptyChain(std::path::PathBuf),
    #[error("no private key in {0:?}")]
    NoKey(std::path::PathBuf),
}

pub fn load_certified_key(
    cert_path: &Path,
    key_path: &Path,
) -> Result<Arc<CertifiedKey>, TlsLoadError> {
    let cert_pem = fs::read(cert_path).map_err(|e| TlsLoadError::Read {
        path: cert_path.to_owned(),
        source: e,
    })?;
    let key_pem = fs::read(key_path).map_err(|e| TlsLoadError::Read {
        path: key_path.to_owned(),
        source: e,
    })?;

    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(&cert_pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| TlsLoadError::ParseCert(e.to_string()))?;
    if certs.is_empty() {
        return Err(TlsLoadError::EmptyChain(cert_path.to_owned()));
    }

    let private_key: PrivateKeyDer<'static> = match PrivateKeyDer::from_pem_slice(&key_pem) {
        Ok(k) => k,
        Err(rustls::pki_types::pem::Error::NoItemsFound) => {
            return Err(TlsLoadError::NoKey(key_path.to_owned()));
        }
        Err(e) => return Err(TlsLoadError::ParseKey(e.to_string())),
    };

    let signing_key = aws_lc_rs::sign::any_supported_type(&private_key)
        .map_err(|e| TlsLoadError::ParseKey(format!("no supported signer: {e:?}")))?;

    Ok(Arc::new(CertifiedKey::new(certs, signing_key)))
}

/// Spawn a background task that watches cert/key paths and swaps the resolver on change.
///
/// Uses a 500 ms debounce window so rapid succession of file events (e.g. write + rename)
/// produces only one reload attempt. Watches **parent directories** rather than individual
/// file paths because many tools (including `openssl`, `certbot`) use atomic rename which
/// the OS reports as a create+delete event at the directory level, not a modify on the
/// original file path.
///
/// The task exits when `shutdown` is signalled (`true`) or when the notify channel closes.
pub async fn spawn_cert_watcher(
    cert_path: PathBuf,
    key_path: PathBuf,
    resolver: Arc<HotReloadCertResolver>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), TlsLoadError> {
    // Canonicalize paths so comparison against watcher-reported events works on macOS
    // where /tmp is a symlink to /private/tmp and notify returns the resolved path.
    let canonical_cert = cert_path
        .canonicalize()
        .unwrap_or_else(|_| cert_path.clone());
    let canonical_key = key_path.canonicalize().unwrap_or_else(|_| key_path.clone());

    let (tx, mut rx) = mpsc::unbounded_channel::<DebounceEventResult>();

    let mut debouncer = new_debouncer(StdDuration::from_millis(500), move |res| {
        let _ = tx.send(res);
    })
    .map_err(|e| TlsLoadError::ParseCert(format!("watcher init: {e}")))?;

    // Watch parent dirs — file renames are seen as create/delete of the specific path
    // at directory level rather than a Modify on the original path.
    let cert_parent = canonical_cert
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_owned();
    let key_parent = canonical_key
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_owned();

    debouncer
        .watcher()
        .watch(&cert_parent, RecursiveMode::NonRecursive)
        .map_err(|e| TlsLoadError::ParseCert(format!("watch cert dir: {e}")))?;
    if key_parent != cert_parent {
        debouncer
            .watcher()
            .watch(&key_parent, RecursiveMode::NonRecursive)
            .map_err(|e| TlsLoadError::ParseCert(format!("watch key dir: {e}")))?;
    }

    tokio::spawn(async move {
        let _keep_alive = debouncer; // keep watcher alive until task ends
        loop {
            tokio::select! {
                maybe_event = rx.recv() => {
                    match maybe_event {
                        Some(evt_res) => {
                            match evt_res {
                                Ok(events) => {
                                    let affected = events
                                        .iter()
                                        .any(|e| e.path == canonical_cert || e.path == canonical_key);
                                    if !affected {
                                        continue;
                                    }
                                    match load_certified_key(&cert_path, &key_path) {
                                        Ok(key) => {
                                            resolver.swap(key);
                                            info!("external_grpc: TLS cert hot-reloaded successfully");
                                        }
                                        Err(e) => {
                                            warn!(
                                                err = %e,
                                                "external_grpc: cert reload failed, keeping previous cert"
                                            );
                                        }
                                    }
                                }
                                Err(e) => {
                                    error!(err = ?e, "external_grpc: file watcher error");
                                }
                            }
                        }
                        None => {
                            // Notify channel closed — stop watching.
                            break;
                        }
                    }
                }
                changed = shutdown.changed() => {
                    match changed {
                        // Channel closed (all senders dropped) — exit to avoid busy-spin.
                        Err(_) => break,
                        // Explicit shutdown signal.
                        Ok(()) if *shutdown.borrow() => break,
                        // Value changed but not to `true`; keep watching.
                        Ok(()) => {}
                    }
                }
            }
        }
    });

    Ok(())
}

/// Spawn a background task that checks TLS cert expiry at startup and every 24h
/// thereafter. Logs a `warn!` when expiry is within 7 days and updates the
/// `tls_cert_expiry_seconds` metric gauge on every tick.
///
/// Tokio's `interval()` fires the first tick immediately — this is intentional:
/// the metric is populated on boot rather than waiting 24h for the first sample.
///
/// The task exits when `shutdown` is signalled (`true`) or the shutdown channel
/// is dropped (all senders released).
pub fn spawn_expiry_monitor(
    resolver: Arc<HotReloadCertResolver>,
    metrics: Arc<crate::grpc::external::metrics::ExternalMetrics>,
    mut shutdown: watch::Receiver<bool>,
) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(StdDuration::from_secs(24 * 3600));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Some(days) = resolver.days_until_expiry() {
                        metrics
                            .tls_cert_expiry_seconds
                            .store(days * 24 * 3600, std::sync::atomic::Ordering::Relaxed);
                        if days < 7 {
                            tracing::warn!(days, "external_grpc: TLS cert expiry within 7 days");
                        }
                    }
                }
                changed = shutdown.changed() => {
                    match changed {
                        // Channel closed — exit to avoid busy-spin.
                        Err(_) => break,
                        Ok(()) if *shutdown.borrow() => break,
                        Ok(()) => {}
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_cert_pair(dir: &TempDir) -> (std::path::PathBuf, std::path::PathBuf) {
        // rcgen 0.14 API
        use rcgen::{CertificateParams, KeyPair};
        let key_pair = KeyPair::generate().unwrap();
        let params = CertificateParams::new(vec!["localhost".into()]).unwrap();
        let cert = params.self_signed(&key_pair).unwrap();
        let cert_path = dir.path().join("cert.pem");
        let key_path = dir.path().join("key.pem");
        fs::write(&cert_path, cert.pem()).unwrap();
        fs::write(&key_path, key_pair.serialize_pem()).unwrap();
        (cert_path, key_path)
    }

    #[test]
    fn load_certified_key_from_pem() {
        let dir = TempDir::new().unwrap();
        let (cert_path, key_path) = write_cert_pair(&dir);
        let key = load_certified_key(&cert_path, &key_path).expect("load");
        assert_eq!(key.cert.len(), 1, "one leaf cert expected");
    }

    #[test]
    fn load_fails_on_missing_cert() {
        let result = load_certified_key(
            std::path::Path::new("/does/not/exist.pem"),
            std::path::Path::new("/does/not/exist.key"),
        );
        assert!(result.is_err());
    }

    /// Verifies that an atomic rename (write-to-tmp + rename-over) triggers the cert
    /// watcher to swap the resolver. Gated `#[ignore]` because inotify/FSEvents watchers
    /// are occasionally slow on loaded CI runners; run with `-- --ignored` locally.
    #[ignore]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn watcher_swaps_resolver_on_rename() {
        use crate::grpc::external::cert_resolver::HotReloadCertResolver;
        use std::time::Duration;

        let dir = TempDir::new().unwrap();
        let (cert_path, key_path) = write_cert_pair(&dir);
        let initial = load_certified_key(&cert_path, &key_path).unwrap();
        let resolver = Arc::new(HotReloadCertResolver::new(initial.clone()));

        let watcher_resolver = resolver.clone();
        let cert_path_c = cert_path.clone();
        let key_path_c = key_path.clone();
        let (_shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let handle = tokio::spawn(async move {
            spawn_cert_watcher(cert_path_c, key_path_c, watcher_resolver, shutdown_rx).await
        });

        tokio::time::sleep(Duration::from_millis(100)).await; // let watcher arm

        // Atomic rename: write to tmp, then rename over the old path.
        use rcgen::{CertificateParams, KeyPair};
        let new_kp = KeyPair::generate().unwrap();
        let new_cert = CertificateParams::new(vec!["localhost".into()])
            .unwrap()
            .self_signed(&new_kp)
            .unwrap();
        let tmp_cert = dir.path().join("cert.new");
        let tmp_key = dir.path().join("key.new");
        fs::write(&tmp_cert, new_cert.pem()).unwrap();
        fs::write(&tmp_key, new_kp.serialize_pem()).unwrap();
        fs::rename(&tmp_cert, &cert_path).unwrap();
        fs::rename(&tmp_key, &key_path).unwrap();

        // Wait up to 2s for watcher + debounce + reload cycle.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            let cur = resolver.current();
            if !Arc::ptr_eq(&cur, &initial) {
                handle.abort();
                return; // success
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        handle.abort();
        panic!("cert watcher did not swap resolver within 2s");
    }
}
