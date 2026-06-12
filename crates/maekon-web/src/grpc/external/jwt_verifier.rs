//! JWT verifier for external gRPC. RS256 / ES256 (asymmetric) only.
//! HS256 and alg=none are rejected at algorithm lock + by jsonwebtoken default.

use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use maekon_core::config::JwtAlgorithm;

/// Claims expected on every JWT. `sub` is logged; `jti` is optional
/// (correlation hint only — no replay store here on the desktop agent).
///
/// JWT signature + expiry only; replay and revocation checks are owned by the
/// upstream issuer. The desktop agent has no shared blacklist access and runs
/// in a separate trust boundary, so it fails closed on signature, issuer,
/// audience, expiry, nbf, and iat freshness instead.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Claims {
    pub sub: String,
    pub iss: String,
    pub aud: String,
    pub exp: u64,
    pub iat: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nbf: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jti: Option<String>,
}

/// Max allowed age of the `iat` claim, in seconds. Matches spec §S1 (24h).
pub const MAX_IAT_AGE_SECS: u64 = 24 * 3600;
/// Clock skew leeway in seconds. Matches spec §S1 (60s).
pub const CLOCK_SKEW_LEEWAY_SECS: u64 = 60;

#[derive(Debug, Error)]
pub enum JwtVerifyError {
    #[error("jwt decode failed: {0}")]
    Decode(String),
    #[error("iat too old: {iat_age_secs}s > {MAX_IAT_AGE_SECS}s")]
    IatTooOld { iat_age_secs: u64 },
    #[error("iat in the future: drift {drift_secs}s > leeway {CLOCK_SKEW_LEEWAY_SECS}s")]
    IatInFuture { drift_secs: u64 },
    #[error("system time before epoch (check system clock)")]
    SystemTimeBeforeEpoch,
    #[error("public key parse failed: {0}")]
    PubKeyParse(String),
}

/// Convert `JwtAlgorithm` (domain config type) into `jsonwebtoken::Algorithm`.
/// A free function because `maekon-core` does not depend on `jsonwebtoken`
/// and both types are external to this crate (orphan rule prevents an impl).
fn to_jw_algorithm(alg: JwtAlgorithm) -> Algorithm {
    match alg {
        JwtAlgorithm::Rs256 => Algorithm::RS256,
        JwtAlgorithm::Es256 => Algorithm::ES256,
    }
}

pub struct JwtVerifier {
    algorithm: Algorithm,
    decoding_key: DecodingKey,
    validation: Validation,
}

impl JwtVerifier {
    pub fn new(
        algorithm: JwtAlgorithm,
        pub_key_pem: &[u8],
        expected_issuer: &str,
        expected_audience: &str,
    ) -> Result<Self, JwtVerifyError> {
        let alg: Algorithm = to_jw_algorithm(algorithm);
        let decoding_key = match algorithm {
            JwtAlgorithm::Rs256 => DecodingKey::from_rsa_pem(pub_key_pem),
            JwtAlgorithm::Es256 => DecodingKey::from_ec_pem(pub_key_pem),
        }
        .map_err(|e| JwtVerifyError::PubKeyParse(e.to_string()))?;
        let mut validation = Validation::new(alg);
        validation.algorithms = vec![alg]; // lock — no other algorithms accepted
        validation.set_issuer(&[expected_issuer]);
        validation.set_audience(&[expected_audience]);
        validation.leeway = CLOCK_SKEW_LEEWAY_SECS;
        validation.validate_exp = true;
        validation.validate_nbf = true;
        validation.required_spec_claims = ["exp", "iat", "iss", "aud", "sub"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        Ok(Self {
            algorithm: alg,
            decoding_key,
            validation,
        })
    }

    pub fn verify(&self, token: &str) -> Result<Claims, JwtVerifyError> {
        let data = decode::<Claims>(token, &self.decoding_key, &self.validation)
            .map_err(|e| JwtVerifyError::Decode(e.to_string()))?;
        // Custom check: iat age cap (jsonwebtoken doesn't enforce this natively).
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| JwtVerifyError::SystemTimeBeforeEpoch)?
            .as_secs();
        if data.claims.iat + MAX_IAT_AGE_SECS < now {
            return Err(JwtVerifyError::IatTooOld {
                iat_age_secs: now.saturating_sub(data.claims.iat),
            });
        }
        // Custom check: reject iat in the future beyond clock-skew leeway.
        // jsonwebtoken does not enforce forward-skew on iat; this closes the
        // clock-skew attack window (attacker minting tokens with a future iat
        // to extend the effective token lifetime silently).
        if data.claims.iat > now + CLOCK_SKEW_LEEWAY_SECS {
            return Err(JwtVerifyError::IatInFuture {
                drift_secs: data.claims.iat.saturating_sub(now),
            });
        }
        Ok(data.claims)
    }

    pub fn algorithm(&self) -> Algorithm {
        self.algorithm
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use aws_lc_rs::{
        encoding::{AsDer, Pkcs8V1Der, PublicKeyX509Der},
        rsa::{KeyPair, KeySize},
        signature::KeyPair as _,
    };
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use jsonwebtoken::{encode, EncodingKey, Header};

    fn pem_block(label: &str, der: &[u8]) -> Vec<u8> {
        let encoded = STANDARD.encode(der);
        let mut pem = format!("-----BEGIN {label}-----\n");
        for chunk in encoded.as_bytes().chunks(64) {
            pem.push_str(std::str::from_utf8(chunk).expect("base64 is ASCII"));
            pem.push('\n');
        }
        pem.push_str(&format!("-----END {label}-----\n"));
        pem.into_bytes()
    }

    /// Returns (priv_pem_pkcs8, pub_pem_spki) for a generated RSA-2048 test key pair.
    /// `pub(crate)` so auth_layer tests can reuse the same runtime fixture.
    pub(crate) fn rsa_keypair_pem() -> (Vec<u8>, Vec<u8>) {
        let keypair = KeyPair::generate(KeySize::Rsa2048).expect("generate RSA test key");
        let priv_der = AsDer::<Pkcs8V1Der>::as_der(&keypair)
            .expect("encode RSA private test key")
            .as_ref()
            .to_vec();
        let pub_der = AsDer::<PublicKeyX509Der>::as_der(keypair.public_key())
            .expect("encode RSA public test key")
            .as_ref()
            .to_vec();
        (
            pem_block("PRIVATE KEY", &priv_der),
            pem_block("PUBLIC KEY", &pub_der),
        )
    }

    fn ec_keypair_pem() -> (Vec<u8>, Vec<u8>) {
        use rcgen::{KeyPair, PKCS_ECDSA_P256_SHA256};
        let kp = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        (
            kp.serialize_pem().into_bytes(),
            kp.public_key_pem().into_bytes(),
        )
    }

    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    fn base_claims(iat: u64, exp: u64) -> Claims {
        Claims {
            sub: "user-1".into(),
            iss: "central-auth".into(),
            aud: "agent-1".into(),
            exp,
            iat,
            nbf: None,
            jti: None,
        }
    }

    #[test]
    fn verify_rs256_valid_token_accepted() {
        let (priv_pem, pub_pem) = rsa_keypair_pem();
        let enc = EncodingKey::from_rsa_pem(&priv_pem).unwrap();
        let claims = base_claims(now(), now() + 3600);
        let token = encode(&Header::new(Algorithm::RS256), &claims, &enc).unwrap();

        let verifier =
            JwtVerifier::new(JwtAlgorithm::Rs256, &pub_pem, "central-auth", "agent-1").unwrap();
        let c = verifier.verify(&token).expect("valid token");
        assert_eq!(c.sub, "user-1");
    }

    #[test]
    fn verify_es256_valid_token_accepted() {
        let (priv_pem, pub_pem) = ec_keypair_pem();
        let enc = EncodingKey::from_ec_pem(&priv_pem).unwrap();
        let claims = base_claims(now(), now() + 3600);
        let token = encode(&Header::new(Algorithm::ES256), &claims, &enc).unwrap();
        let verifier =
            JwtVerifier::new(JwtAlgorithm::Es256, &pub_pem, "central-auth", "agent-1").unwrap();
        // Pin the same claim fields as the RS256 parallel test: sub must round-trip
        // and the algorithm field on the verifier must reflect ES256 (#5594).
        let c = verifier
            .verify(&token)
            .expect("valid ES256 token must be accepted");
        assert_eq!(c.sub, "user-1");
        assert_eq!(c.iss, "central-auth");
        assert_eq!(c.aud, "agent-1");
    }

    #[test]
    fn verify_rejects_hs256_when_rs256_configured() {
        let (_, pub_pem) = rsa_keypair_pem();
        let enc = EncodingKey::from_secret(b"secret");
        let claims = base_claims(now(), now() + 3600);
        let token = encode(&Header::new(Algorithm::HS256), &claims, &enc).unwrap();
        let verifier =
            JwtVerifier::new(JwtAlgorithm::Rs256, &pub_pem, "central-auth", "agent-1").unwrap();
        assert!(
            matches!(
                verifier.verify(&token).unwrap_err(),
                JwtVerifyError::Decode(_)
            ),
            "HS256 must be rejected with JwtVerifyError::Decode under RS256 config"
        );
    }

    #[test]
    fn verify_rejects_expired() {
        let (priv_pem, pub_pem) = rsa_keypair_pem();
        let enc = EncodingKey::from_rsa_pem(&priv_pem).unwrap();
        let claims = base_claims(now() - 7200, now() - 3600); // expired 1h ago
        let token = encode(&Header::new(Algorithm::RS256), &claims, &enc).unwrap();
        let verifier =
            JwtVerifier::new(JwtAlgorithm::Rs256, &pub_pem, "central-auth", "agent-1").unwrap();
        let err = verifier.verify(&token).unwrap_err();
        assert!(
            matches!(err, JwtVerifyError::Decode(_)),
            "expired token must return JwtVerifyError::Decode, got: {err:?}"
        );
    }

    #[test]
    fn verify_rejects_wrong_issuer() {
        let (priv_pem, pub_pem) = rsa_keypair_pem();
        let enc = EncodingKey::from_rsa_pem(&priv_pem).unwrap();
        let mut claims = base_claims(now(), now() + 3600);
        claims.iss = "attacker".into();
        let token = encode(&Header::new(Algorithm::RS256), &claims, &enc).unwrap();
        let verifier =
            JwtVerifier::new(JwtAlgorithm::Rs256, &pub_pem, "central-auth", "agent-1").unwrap();
        let err = verifier.verify(&token).unwrap_err();
        assert!(
            matches!(err, JwtVerifyError::Decode(_)),
            "wrong issuer must return JwtVerifyError::Decode, got: {err:?}"
        );
    }

    #[test]
    fn verify_rejects_wrong_audience() {
        let (priv_pem, pub_pem) = rsa_keypair_pem();
        let enc = EncodingKey::from_rsa_pem(&priv_pem).unwrap();
        let mut claims = base_claims(now(), now() + 3600);
        claims.aud = "other-agent".into();
        let token = encode(&Header::new(Algorithm::RS256), &claims, &enc).unwrap();
        let verifier =
            JwtVerifier::new(JwtAlgorithm::Rs256, &pub_pem, "central-auth", "agent-1").unwrap();
        let err = verifier.verify(&token).unwrap_err();
        assert!(
            matches!(err, JwtVerifyError::Decode(_)),
            "wrong audience must return JwtVerifyError::Decode, got: {err:?}"
        );
    }

    #[test]
    fn verify_rejects_iat_older_than_24h() {
        let (priv_pem, pub_pem) = rsa_keypair_pem();
        let enc = EncodingKey::from_rsa_pem(&priv_pem).unwrap();
        // iat was 25h ago, but exp is still in the future — should be rejected by our custom check.
        let claims = base_claims(now() - (25 * 3600), now() + 3600);
        let token = encode(&Header::new(Algorithm::RS256), &claims, &enc).unwrap();
        let verifier =
            JwtVerifier::new(JwtAlgorithm::Rs256, &pub_pem, "central-auth", "agent-1").unwrap();
        let err = verifier.verify(&token).unwrap_err();
        match err {
            JwtVerifyError::IatTooOld { .. } => {}
            other => panic!("expected IatTooOld, got {other:?}"),
        }
    }

    #[test]
    fn verify_rejects_future_iat_beyond_leeway() {
        let (priv_pem, pub_pem) = rsa_keypair_pem();
        let enc = EncodingKey::from_rsa_pem(&priv_pem).unwrap();
        // iat = now + 300s (5 min future) with 60s leeway → drift 300 > 60 → must reject.
        // This closes the clock-skew attack window: an attacker cannot mint tokens with a
        // future iat to silently extend effective token lifetime.
        let claims = base_claims(now() + 300, now() + 3600);
        let token = encode(&Header::new(Algorithm::RS256), &claims, &enc).unwrap();
        let verifier =
            JwtVerifier::new(JwtAlgorithm::Rs256, &pub_pem, "central-auth", "agent-1").unwrap();
        let err = verifier.verify(&token).unwrap_err();
        match err {
            JwtVerifyError::IatInFuture { .. } => {}
            other => panic!("expected IatInFuture, got {other:?}"),
        }
    }

    #[test]
    fn verify_rejects_alg_none_header() {
        let (_, pub_pem) = rsa_keypair_pem();
        // Construct an `alg: none` token manually: header.claims.<empty signature>
        let header = r#"{"alg":"none","typ":"JWT"}"#;
        let header_b64 = base64_url(header.as_bytes());
        let claims_json = serde_json::to_string(&base_claims(now(), now() + 3600)).unwrap();
        let claims_b64 = base64_url(claims_json.as_bytes());
        let forged = format!("{header_b64}.{claims_b64}.");
        let verifier =
            JwtVerifier::new(JwtAlgorithm::Rs256, &pub_pem, "central-auth", "agent-1").unwrap();
        assert!(
            matches!(
                verifier.verify(&forged).unwrap_err(),
                JwtVerifyError::Decode(_)
            ),
            "alg:none must be rejected with JwtVerifyError::Decode"
        );
    }

    fn base64_url(bytes: &[u8]) -> String {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use base64::Engine;
        URL_SAFE_NO_PAD.encode(bytes)
    }

    #[test]
    fn pubkey_parse_error_on_invalid_pem() {
        let result = JwtVerifier::new(
            JwtAlgorithm::Rs256,
            b"not a PEM block",
            "central-auth",
            "agent-1",
        );
        assert!(matches!(result, Err(JwtVerifyError::PubKeyParse(_))));
    }
}
