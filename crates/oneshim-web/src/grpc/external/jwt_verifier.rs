//! JWT verifier for external gRPC. RS256 / ES256 (asymmetric) only.
//! HS256 and alg=none are rejected at algorithm lock + by jsonwebtoken default.

use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use oneshim_core::config::JwtAlgorithm;

/// Claims expected on every JWT. `sub` is logged; `jti` is optional
/// (correlation hint only — no replay store).
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
/// A free function because `oneshim-core` does not depend on `jsonwebtoken`
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
    use jsonwebtoken::{encode, EncodingKey, Header};

    /// Returns (priv_pem_pkcs8, pub_pem_spki) for an RSA-2048 test key pair.
    ///
    /// Uses a pre-generated test key instead of runtime generation because the
    /// `ring` backend (default rcgen feature) does not support RSA key generation.
    /// This key is TEST-ONLY and MUST NOT be used in production.
    /// `pub(crate)` so Task 10 auth_layer tests can reuse without duplicating.
    pub(crate) fn rsa_keypair_pem() -> (Vec<u8>, Vec<u8>) {
        let priv_pem = b"-----BEGIN PRIVATE KEY-----
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQDBZMG5h9Kz66YM
IxS1W6e2BXA/CH4tzMEcnasPGXD/dHHIvSIFgWM7Gf+617FE8UA8V9BHzifRGOWW
V+Ikw2hX2gDa47sjWaHVc7so5MlrNSXkBx6lAo3GOs1qbbNWdXt0I7orCxXShSG3
ymElMb9jjjTPxVOr0B2d/jv9BPU+Y4rhw2HDfY/P7ZL7NmOK8dMeH9xLCRdX+/oH
I/2A3G0EOoDaRpoJE15J28d7HekZVdlektwSy2Ox/lT4V9/8kAhrAQhtr59dFf41
1ENN4gyrTVpglGFNIOqGBdFxaLKjjwUiCQHWTMuGvmYlKO2HJIqwkyfOrn3penHY
0ff1K8szAgMBAAECggEASWJ24nSIPy6x1RQwxPrRIpBgvgJ2guGZ+8ZWhUXFq6Hf
lWkzcjxdT613bUpwoXUcR2mZBs5TLJSSdiDGFuYxf3ihb24P8oOOFeWzBPr/9Vb3
GFadScc0zh49GWAkN7Af1vvBppivwLE1EL1SbJ86fUgWgSrjK6SuwGebEtFhUDki
9dAHsox0UPJGOrmqlolMC/CepRk8k3FGquD02Hg2S8uNQh15OH7xHiE2ERYMR6Wt
Ht0HrLrzsGDxm1j1xKNsUYG34JA2dE5mVez3OeZWRn9+P3eAwiTD4e6e7zediynq
jQvx8+iqTgl4J6qhKXCFUBcd4IB3tfZ3RVjjpG5qZQKBgQD0v2WTI48whgW0+RuK
J9M0dt1KB//ihPoe9N2ex5ufuA/ESsmQc6HgsjT/FnAhLaAfOq/pS55w+gwQX2uH
nByW/Xj1aqz2gpY00ghDW6QyFduNvD0uqyzO06qqp+aqDe26zUeRyGT4roG0o6qU
8Td/j7yAvTkRCAxwvWj0xYH5DwKBgQDKSPBO5OZ+UkjDo8fawmX1E0+TvfGz+HbK
fbav6ME/5FOIHtJeCDitECgTQ8MZ+9IXAif8VAy3zT5bd8vEqO4ZKVJ3+/FJvEL4
+J8UaEIagHeqHaTuhRf6ViQcLkGZizZ4jqz5E3k19wYofUUfZVNNjNR4eHQHPKTy
agJvwAcjnQKBgB2k+SajTfqwoQxUh/Np83kNVKxc36+OL8WEHzvWLZFg9/fsnxFy
EA9pRmYHT7mVDyn5L8lwMVa50rBA/oNEc2oOdZI0Q5LwKkVnkzylYvP2FcvLGxYG
Ab1jge59u8CpQzw3FQ4hWamNaYR5tnWn6fL3c/ub78eSU/9r0cSkD6QdAoGAPH/+
J4p8iZFwo9rLPllgByGEbmqj7LDGTp+00P3rNoHCnfah8m/BC7nGUqS0qIPRfQIv
FV/KAfsHyHGW5zWjKLFcMfiPXP9KhI5PfdoE00pS//UnzBLQbhXvbOJEyniBjSMX
BtPVL9e25ss4rkAu3wXc0j8sbLGtn7cnDWdAe10CgYEAmug51NKEhlLy3VbEzxQ2
hTSqxYlFMnnUW3/nzisK1ftY1ugjhbSnCo1KJVDPQ9FeUP+2GQrTLMoV65N9RvN8
rc39jJXz8eCZDpHnTFC4/VFAGyRrPRPgK7+9N3wi9r/ElHvdFGXDJqWeci/7HKwi
2+PrIkT3I7Oqk+2rPeT9/8g=
-----END PRIVATE KEY-----";
        let pub_pem = b"-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAwWTBuYfSs+umDCMUtVun
tgVwPwh+LczBHJ2rDxlw/3RxyL0iBYFjOxn/utexRPFAPFfQR84n0RjlllfiJMNo
V9oA2uO7I1mh1XO7KOTJazUl5AcepQKNxjrNam2zVnV7dCO6KwsV0oUht8phJTG/
Y440z8VTq9Adnf47/QT1PmOK4cNhw32Pz+2S+zZjivHTHh/cSwkXV/v6ByP9gNxt
BDqA2kaaCRNeSdvHex3pGVXZXpLcEstjsf5U+Fff/JAIawEIba+fXRX+NdRDTeIM
q01aYJRhTSDqhgXRcWiyo48FIgkB1kzLhr5mJSjthySKsJMnzq596Xpx2NH39SvL
MwIDAQAB
-----END PUBLIC KEY-----";
        (priv_pem.to_vec(), pub_pem.to_vec())
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
        assert!(verifier.verify(&token).is_ok());
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
            verifier.verify(&token).is_err(),
            "HS256 must be rejected under RS256 config"
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
        assert!(verifier.verify(&token).is_err());
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
        assert!(verifier.verify(&token).is_err());
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
        assert!(verifier.verify(&token).is_err());
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
        let (priv_pem, pub_pem) = rsa_keypair_pem();
        let _enc = EncodingKey::from_rsa_pem(&priv_pem).unwrap();
        // Construct an `alg: none` token manually: header.claims.<empty signature>
        let header = r#"{"alg":"none","typ":"JWT"}"#;
        let header_b64 = base64_url(header.as_bytes());
        let claims_json = serde_json::to_string(&base_claims(now(), now() + 3600)).unwrap();
        let claims_b64 = base64_url(claims_json.as_bytes());
        let forged = format!("{header_b64}.{claims_b64}.");
        let verifier =
            JwtVerifier::new(JwtAlgorithm::Rs256, &pub_pem, "central-auth", "agent-1").unwrap();
        assert!(
            verifier.verify(&forged).is_err(),
            "alg:none must be rejected"
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
