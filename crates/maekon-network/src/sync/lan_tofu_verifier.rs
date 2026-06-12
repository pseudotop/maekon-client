//! TOFU 인증서 핑거프린트 검증기 (LAN sync, rustls `ServerCertVerifier`).
//!
//! 핀 I/O 는 핸드셰이크 *주변* 의 async 코드에서 처리합니다. 이 검증기는 생성
//! 시점에 전달받은 데이터에 대한 동기·순수 비교만 수행합니다. 계산된 핑거프린트는
//! `captured` 에 저장되므로, 호출자는 first-contact 핸드셰이크 성공 후 `upsert_pin`
//! 할 수 있습니다. 검증기 내부에서는 어떠한 I/O 도 async/block_on 도 수행하지 않습니다.

use std::sync::Arc;

use parking_lot::Mutex;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, Error as RustlsError, SignatureScheme};
use sha2::{Digest, Sha256};

/// DER 바이트의 SHA-256 hex (핀으로 저장되는 핑거프린트).
// Task 5 (per-peer reqwest client) 가 검증기를 통해 소비. 그 전까지 dead_code 허용.
#[allow(dead_code)]
pub(super) fn fingerprint_hex(der: &[u8]) -> String {
    hex::encode(Sha256::digest(der))
}

/// 순수 TOFU 결정. `Ok(true)` = 최초 접촉(호출자가 핀 upsert 해야 함);
/// `Ok(false)` = 기존 핀과 일치; `Err` = 거부.
// Task 5 (per-peer reqwest client) 가 검증기를 통해 소비. 그 전까지 dead_code 허용.
#[allow(dead_code)]
pub(super) fn tofu_decision(
    presented_fp: &str,
    advertised_fp: Option<&str>,
    stored_pin: Option<(String, bool)>,
) -> Result<bool, &'static str> {
    match stored_pin {
        Some((_, true)) => Err("peer pin is revoked"),
        Some((pinned, false)) => {
            if pinned == presented_fp {
                Ok(false)
            } else {
                Err("presented fingerprint does not match the pinned one (possible MITM)")
            }
        }
        None => match advertised_fp {
            Some(adv) if adv != presented_fp => {
                Err("mDNS-advertised fingerprint does not match the presented cert")
            }
            _ => Ok(true),
        },
    }
}

/// LAN sync 용 TOFU 핑거프린트 핀 검증기.
///
/// 호출자(Task 5)가 핀(async)을 읽어 `stored_pin` + `advertised_fp` 를 생성 시점에
/// 주입하고, 핸드셰이크 성공 후 `captured` 를 읽어 최초 접촉 시 핀을 upsert 합니다.
// Task 5 (per-peer reqwest client) 가 구성·소비. 그 전까지 dead_code 허용.
#[allow(dead_code)]
#[derive(Debug)]
pub(super) struct TofuVerifier {
    pub advertised_fp: Option<String>,
    pub stored_pin: Option<(String, bool)>,
    /// 최초 접촉 accept(Ok(true)) 시 계산된 핑거프린트를 수신.
    pub captured: Arc<Mutex<Option<String>>>,
    /// 서명 검증을 위임할 rustls 기본 provider.
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
            Err(reason) => Err(RustlsError::General(format!("LAN TOFU rejected: {reason}"))),
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

    // tofu_decision(presented_fp, advertised_fp, stored_pin) -> Result<bool, &'static str>
    // Ok(true)  = first-contact pin (caller should upsert)
    // Ok(false) = matched an existing pin (no write)
    // Err(_)    = reject

    #[test]
    fn revoked_pin_is_rejected() {
        assert_eq!(
            tofu_decision("fp", Some("fp"), Some(("fp".into(), true))).unwrap_err(),
            "peer pin is revoked"
        );
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
        assert_eq!(
            tofu_decision("fp-new", Some("fp-new"), Some(("fp-old".into(), false))).unwrap_err(),
            "presented fingerprint does not match the pinned one (possible MITM)"
        );
    }
    #[test]
    fn first_contact_with_matching_mdns_pins() {
        assert_eq!(tofu_decision("fp", Some("fp"), None), Ok(true));
    }
    #[test]
    fn first_contact_with_mismatched_mdns_is_rejected() {
        assert_eq!(
            tofu_decision("fp", Some("other"), None).unwrap_err(),
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
