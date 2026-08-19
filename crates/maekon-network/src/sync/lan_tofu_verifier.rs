//! TOFU certificate-fingerprint verifier (LAN sync, rustls `ServerCertVerifier`).
//!
//! Pin I/O is handled by the async code *around* the handshake. This verifier
//! performs only a synchronous, pure comparison over the data handed to it at
//! construction time. The computed fingerprint is stored in `captured`, so the
//! caller can `upsert_pin` after a successful first-contact handshake. The
//! verifier itself performs no I/O and no async/block_on.

use std::sync::Arc;

use parking_lot::Mutex;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, Error as RustlsError, SignatureScheme};
use sha2::{Digest, Sha256};

/// SHA-256 hex of the DER bytes (the fingerprint stored as the pin).
// Consumed via the verifier by Task 5 (per-peer reqwest client). Allow dead_code until then.
pub(super) fn fingerprint_hex(der: &[u8]) -> String {
    hex::encode(Sha256::digest(der))
}

/// TOFU rejection cause. Distinguishes the security-significant case
/// (presented cert differs from an *already-stored* pin = possible MITM /
/// peer key change) from benign or already-handled cases, so the async caller
/// can decide whether to revoke the stored pin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TofuReject {
    /// Pin already revoked (caller short-circuits before the handshake; the
    /// verifier never normally sees this since `build_peer_client` pre-checks).
    AlreadyRevoked,
    /// Presented fingerprint does not match an existing, non-revoked pin.
    /// This is the case that SHOULD revoke the stored pin.
    PinMismatch,
    /// First contact, but the mDNS-advertised fingerprint disagreed with the
    /// presented cert. No stored pin exists to revoke (could be an mDNS race).
    AdvertisedMismatch,
}

impl TofuReject {
    /// Human-readable reason surfaced in the rustls error string.
    pub(super) fn reason(self) -> &'static str {
        match self {
            TofuReject::AlreadyRevoked => "peer pin is revoked",
            TofuReject::PinMismatch => {
                "presented fingerprint does not match the pinned one (possible MITM)"
            }
            TofuReject::AdvertisedMismatch => {
                "mDNS-advertised fingerprint does not match the presented cert"
            }
        }
    }
}

/// Pure TOFU decision. `Ok(true)` = first contact (the caller must upsert the
/// pin); `Ok(false)` = matched the existing pin; `Err(TofuReject)` = rejected
/// (with the cause).
pub(super) fn tofu_decision(
    presented_fp: &str,
    advertised_fp: Option<&str>,
    stored_pin: Option<(String, bool)>,
) -> Result<bool, TofuReject> {
    match stored_pin {
        Some((_, true)) => Err(TofuReject::AlreadyRevoked),
        Some((pinned, false)) => {
            if pinned == presented_fp {
                Ok(false)
            } else {
                Err(TofuReject::PinMismatch)
            }
        }
        None => match advertised_fp {
            Some(adv) if adv != presented_fp => Err(TofuReject::AdvertisedMismatch),
            _ => Ok(true),
        },
    }
}

/// TOFU fingerprint-pin verifier for LAN sync.
///
/// The caller (Task 5) reads the pin (async) and injects `stored_pin` +
/// `advertised_fp` at construction time, then reads `captured` after a
/// successful handshake to upsert the pin on first contact. On rejection — in
/// particular a mismatch against an existing pin (`PinMismatch`) — it sets the
/// `pin_mismatch` cell to signal that the async caller may revoke the pin after
/// the handshake fails.
#[derive(Debug)]
pub(super) struct TofuVerifier {
    pub advertised_fp: Option<String>,
    pub stored_pin: Option<(String, bool)>,
    /// Receives the computed fingerprint on a first-contact accept (Ok(true)).
    pub captured: Arc<Mutex<Option<String>>>,
    /// Set to `true` when rejected due to a mismatch against an existing
    /// (non-revoked) pin. The verifier cannot perform async I/O, so after the
    /// handshake fails the caller reads this signal to `revoke_pin`.
    pub pin_mismatch: Arc<Mutex<bool>>,
    /// The default rustls provider to delegate signature verification to.
    pub provider: Arc<rustls::crypto::CryptoProvider>,
}

impl ServerCertVerifier for TofuVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, RustlsError> {
        let fp = fingerprint_hex(end_entity.as_ref());
        match tofu_decision(&fp, self.advertised_fp.as_deref(), self.stored_pin.clone()) {
            Ok(first_contact) => {
                if first_contact {
                    *self.captured.lock() = Some(fp);
                }
                Ok(ServerCertVerified::assertion())
            }
            Err(reject) => {
                // Signal a stored-pin mismatch so the async caller can revoke the
                // pin (the verifier itself cannot perform async storage I/O).
                if reject == TofuReject::PinMismatch {
                    *self.pin_mismatch.lock() = true;
                }
                Err(RustlsError::General(format!(
                    "LAN TOFU rejected: {}",
                    reject.reason()
                )))
            }
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // tofu_decision(presented_fp, advertised_fp, stored_pin) -> Result<bool, TofuReject>
    // Ok(true)  = first-contact pin (caller should upsert)
    // Ok(false) = matched an existing pin (no write)
    // Err(_)    = reject (cause carried by TofuReject)

    #[test]
    fn revoked_pin_is_rejected() {
        let err = tofu_decision("fp", Some("fp"), Some(("fp".into(), true))).unwrap_err();
        assert_eq!(err, TofuReject::AlreadyRevoked);
        assert_eq!(err.reason(), "peer pin is revoked");
    }
    #[test]
    fn matching_pin_passes_no_write() {
        assert_eq!(
            tofu_decision("fp", Some("fp"), Some(("fp".into(), false))),
            Ok(false)
        );
    }
    #[test]
    fn pin_mismatch_is_rejected() {
        // The security-significant branch: presented cert differs from an
        // existing non-revoked pin → PinMismatch (the cause the caller revokes on).
        let err =
            tofu_decision("fp-new", Some("fp-new"), Some(("fp-old".into(), false))).unwrap_err();
        assert_eq!(err, TofuReject::PinMismatch);
        assert_eq!(
            err.reason(),
            "presented fingerprint does not match the pinned one (possible MITM)"
        );
    }
    #[test]
    fn first_contact_with_matching_mdns_pins() {
        assert_eq!(tofu_decision("fp", Some("fp"), None), Ok(true));
    }
    #[test]
    fn first_contact_with_mismatched_mdns_is_rejected() {
        // No stored pin exists, so this is NOT a PinMismatch (nothing to revoke).
        let err = tofu_decision("fp", Some("other"), None).unwrap_err();
        assert_eq!(err, TofuReject::AdvertisedMismatch);
        assert_eq!(
            err.reason(),
            "mDNS-advertised fingerprint does not match the presented cert"
        );
    }
    #[test]
    fn first_contact_without_mdns_is_pure_tofu() {
        assert_eq!(tofu_decision("fp", None, None), Ok(true));
    }
    #[test]
    fn fingerprint_is_sha256_hex_of_der() {
        let der = b"\x01\x02\x03";
        assert_eq!(
            fingerprint_hex(der),
            "039058c6f2c0cb492c533b0a4d14ef77cc0f78abccced5287d84a1a2011cfb81"
        );
    }
}
