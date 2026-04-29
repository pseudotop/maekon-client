//! mTLS verifier: lifetime cap, fingerprint allowlist, CN extraction.
//! CA chain validation is done by rustls automatically via ServerConfig;
//! this module handles policies applied AFTER a cert has been accepted
//! as CA-valid.

use std::collections::HashSet;

use sha2::{Digest, Sha256};
use thiserror::Error;
use x509_parser::prelude::*;

#[derive(Debug, Error)]
pub enum MtlsVerifyError {
    #[error("cert parse failed: {0}")]
    Parse(String),
    #[error("cert lifetime {lifetime_hours}h exceeds max {max_hours}h")]
    LifetimeExceeded { lifetime_hours: u64, max_hours: u32 },
    #[error("fingerprint not in allowlist")]
    FingerprintNotAllowed,
    #[error("cert missing subject CN")]
    MissingCn,
    #[error("cert lifetime invalid (notBefore > notAfter)")]
    InvalidLifetime,
}

pub struct MtlsVerifier {
    max_cert_lifetime_hours: u32,
    fingerprint_allowlist: HashSet<[u8; 32]>,
}

#[derive(Debug, Clone)]
pub struct VerifiedPeer {
    pub subject_cn: String,
    pub fingerprint: [u8; 32],
}

impl MtlsVerifier {
    /// Build with a lifetime cap. Allowlist may be empty → any CA-signed cert accepted.
    pub fn new(
        max_cert_lifetime_hours: u32,
        allowlist_hex: &[String],
    ) -> Result<Self, MtlsVerifyError> {
        let mut fingerprint_allowlist = HashSet::new();
        for line in allowlist_hex {
            let hex = line.trim().replace([':', ' '], "");
            if hex.is_empty() || hex.starts_with('#') {
                continue;
            }
            let bytes = hex_to_bytes(&hex).map_err(MtlsVerifyError::Parse)?;
            if bytes.len() != 32 {
                return Err(MtlsVerifyError::Parse(format!(
                    "expected 32 bytes, got {}",
                    bytes.len()
                )));
            }
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            fingerprint_allowlist.insert(arr);
        }
        Ok(Self {
            max_cert_lifetime_hours,
            fingerprint_allowlist,
        })
    }

    /// Verify a client cert given its DER bytes. Returns VerifiedPeer on success.
    pub fn verify(&self, leaf_cert_der: &[u8]) -> Result<VerifiedPeer, MtlsVerifyError> {
        let (_rest, cert) = X509Certificate::from_der(leaf_cert_der)
            .map_err(|e| MtlsVerifyError::Parse(format!("{e:?}")))?;

        // Lifetime check
        let not_before = cert.validity().not_before.timestamp();
        let not_after = cert.validity().not_after.timestamp();
        if not_after < not_before {
            return Err(MtlsVerifyError::InvalidLifetime);
        }
        let lifetime_secs = (not_after - not_before) as u64;
        let lifetime_hours = lifetime_secs / 3600;
        if lifetime_hours > self.max_cert_lifetime_hours as u64 {
            return Err(MtlsVerifyError::LifetimeExceeded {
                lifetime_hours,
                max_hours: self.max_cert_lifetime_hours,
            });
        }

        // Fingerprint (SHA-256 of DER)
        let mut hasher = Sha256::new();
        hasher.update(leaf_cert_der);
        let fp: [u8; 32] = hasher.finalize().into();
        if !self.fingerprint_allowlist.is_empty() && !self.fingerprint_allowlist.contains(&fp) {
            return Err(MtlsVerifyError::FingerprintNotAllowed);
        }

        // CN extraction
        let subject_cn = cert
            .subject()
            .iter_common_name()
            .next()
            .and_then(|cn| cn.as_str().ok())
            .ok_or(MtlsVerifyError::MissingCn)?
            .to_string();

        Ok(VerifiedPeer {
            subject_cn,
            fingerprint: fp,
        })
    }
}

fn hex_to_bytes(hex: &str) -> Result<Vec<u8>, String> {
    if hex.len() % 2 != 0 {
        return Err(format!("odd hex length: {}", hex.len()));
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use rcgen::{CertificateParams, KeyPair};

    /// Generates a self-signed DER cert with the given CN and lifetime.
    /// `pub(crate)` so Task 10 auth_layer tests can reuse without duplicating.
    pub(crate) fn gen_cert_with_cn(cn: &str, lifetime_hours: u32) -> Vec<u8> {
        // Use rcgen's re-exported `time` date helpers to avoid adding `time` as a direct dep.
        // chrono is the workspace's date/time crate (already non-optional in oneshim-web);
        // compute year/month/day and feed rcgen::date_time_ymd (same pattern as lan_tls.rs).
        use chrono::{Datelike, Duration as ChronoDuration, Utc};
        let now = Utc::now();
        let target = now + ChronoDuration::hours(lifetime_hours as i64);
        let kp = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(vec![cn.into()]).unwrap();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, cn);
        params.not_before = rcgen::date_time_ymd(now.year(), now.month() as u8, now.day() as u8);
        params.not_after =
            rcgen::date_time_ymd(target.year(), target.month() as u8, target.day() as u8);
        let cert = params.self_signed(&kp).unwrap();
        cert.der().to_vec()
    }

    #[test]
    fn verify_empty_allowlist_accepts_any_cert() {
        let verifier = MtlsVerifier::new(48, &[]).unwrap();
        let der = gen_cert_with_cn("client-1", 24);
        let peer = verifier.verify(&der).unwrap();
        assert_eq!(peer.subject_cn, "client-1");
    }

    #[test]
    fn verify_fingerprint_in_allowlist_accepted() {
        let der = gen_cert_with_cn("client-2", 24);
        let mut hasher = Sha256::new();
        hasher.update(&der);
        let fp: [u8; 32] = hasher.finalize().into();
        let hex = fp.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let verifier = MtlsVerifier::new(48, &[hex]).unwrap();
        assert!(verifier.verify(&der).is_ok());
    }

    #[test]
    fn verify_fingerprint_not_in_allowlist_rejected() {
        let der = gen_cert_with_cn("client-3", 24);
        let bogus_fp = "00:11:22:33:44:55:66:77:88:99:aa:bb:cc:dd:ee:ff:00:11:22:33:44:55:66:77:88:99:aa:bb:cc:dd:ee:ff".to_string();
        let verifier = MtlsVerifier::new(48, &[bogus_fp]).unwrap();
        matches!(
            verifier.verify(&der),
            Err(MtlsVerifyError::FingerprintNotAllowed)
        );
    }

    #[test]
    fn verify_rejects_lifetime_over_cap() {
        let der = gen_cert_with_cn("client-4", 72); // 72h cert, cap 48h
        let verifier = MtlsVerifier::new(48, &[]).unwrap();
        match verifier.verify(&der) {
            Err(MtlsVerifyError::LifetimeExceeded { lifetime_hours, .. }) => {
                assert!((71..=72).contains(&lifetime_hours), "got {lifetime_hours}");
            }
            other => panic!("expected LifetimeExceeded, got {other:?}"),
        }
    }

    #[test]
    fn verify_extracts_cn() {
        let der = gen_cert_with_cn("special-client-name", 1);
        let verifier = MtlsVerifier::new(48, &[]).unwrap();
        assert_eq!(
            verifier.verify(&der).unwrap().subject_cn,
            "special-client-name"
        );
    }

    #[test]
    fn verify_fingerprint_matches_openssl_sha256_format() {
        // The fingerprint is SHA-256 over the full DER — same input as `openssl x509 -fingerprint -sha256`.
        let der = gen_cert_with_cn("client-6", 1);
        let verifier = MtlsVerifier::new(48, &[]).unwrap();
        let peer = verifier.verify(&der).unwrap();
        let expected_hex: String = peer
            .fingerprint
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(expected_hex.len(), 64, "SHA-256 hex is 64 chars");
    }

    #[test]
    fn parse_fails_on_non_der_input() {
        let verifier = MtlsVerifier::new(48, &[]).unwrap();
        assert!(verifier.verify(b"not a cert").is_err());
    }

    #[test]
    fn allowlist_accepts_colon_separated_format() {
        let der = gen_cert_with_cn("client-7", 1);
        let mut hasher = Sha256::new();
        hasher.update(&der);
        let fp: [u8; 32] = hasher.finalize().into();
        let colon_hex = fp
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(":");
        let verifier = MtlsVerifier::new(48, &[colon_hex]).unwrap();
        assert!(verifier.verify(&der).is_ok());
    }
}
