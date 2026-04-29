use crate::integration::transport::{IntegrationRequestProof, IntegrationRequestProofFactory};
use async_trait::async_trait;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use chrono::Utc;
use ed25519_dalek::{Signer, SigningKey};
use oneshim_core::error::CoreError;
use oneshim_core::models::integration::IntegrationAuthContext;
use oneshim_core::ports::secret_store::{
    SecretStore, INTEGRATION_AUTH_SECRET_NAMESPACE, INTEGRATION_DPOP_SIGNING_KEY_SECRET_KEY,
};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct NoopIntegrationRequestProofFactory;

#[async_trait]
impl IntegrationRequestProofFactory for NoopIntegrationRequestProofFactory {
    async fn build_proof(
        &self,
        _auth: &IntegrationAuthContext,
        _method: &str,
        _url: &str,
    ) -> Result<Option<IntegrationRequestProof>, CoreError> {
        Ok(None)
    }
}

pub struct StaticIntegrationRequestProofFactory {
    proof: IntegrationRequestProof,
}

impl StaticIntegrationRequestProofFactory {
    pub fn new(header_name: impl Into<String>, header_value: impl Into<String>) -> Self {
        Self {
            proof: IntegrationRequestProof {
                header_name: header_name.into(),
                header_value: header_value.into(),
            },
        }
    }
}

#[async_trait]
impl IntegrationRequestProofFactory for StaticIntegrationRequestProofFactory {
    async fn build_proof(
        &self,
        _auth: &IntegrationAuthContext,
        _method: &str,
        _url: &str,
    ) -> Result<Option<IntegrationRequestProof>, CoreError> {
        Ok(Some(self.proof.clone()))
    }
}

pub struct Ed25519DpopProofFactory {
    secret_store: Option<Arc<dyn SecretStore>>,
    signing_key: Arc<Mutex<Option<SigningKey>>>,
}

impl Ed25519DpopProofFactory {
    pub fn new(secret_store: Option<Arc<dyn SecretStore>>) -> Self {
        Self {
            secret_store,
            signing_key: Arc::new(Mutex::new(None)),
        }
    }

    async fn get_signing_key(&self) -> Result<SigningKey, CoreError> {
        let mut guard = self.signing_key.lock().await;
        if let Some(key) = guard.as_ref() {
            return Ok(key.clone());
        }

        let key = if let Some(secret_store) = self.secret_store.as_ref() {
            if let Some(serialized) = secret_store
                .retrieve(
                    INTEGRATION_AUTH_SECRET_NAMESPACE,
                    INTEGRATION_DPOP_SIGNING_KEY_SECRET_KEY,
                )
                .await?
            {
                let bytes = URL_SAFE_NO_PAD
                    .decode(serialized.as_bytes())
                    .map_err(|error| CoreError::SecretStoreError {
                        code: oneshim_core::error_codes::SecretCode::Failed,
                        message: format!("failed to decode integration DPoP signing key: {error}"),
                    })?;
                let secret_bytes: [u8; 32] =
                    bytes.try_into().map_err(|_| CoreError::SecretStoreError {
                        code: oneshim_core::error_codes::SecretCode::Failed,
                        message: "integration DPoP signing key must be 32 bytes".to_string(),
                    })?;
                SigningKey::from_bytes(&secret_bytes)
            } else {
                let secret_bytes: [u8; 32] = rand::random();
                secret_store
                    .store(
                        INTEGRATION_AUTH_SECRET_NAMESPACE,
                        INTEGRATION_DPOP_SIGNING_KEY_SECRET_KEY,
                        &URL_SAFE_NO_PAD.encode(secret_bytes),
                    )
                    .await?;
                SigningKey::from_bytes(&secret_bytes)
            }
        } else {
            let secret_bytes: [u8; 32] = rand::random();
            SigningKey::from_bytes(&secret_bytes)
        };

        *guard = Some(key.clone());
        Ok(key)
    }
}

#[async_trait]
impl IntegrationRequestProofFactory for Ed25519DpopProofFactory {
    async fn build_proof(
        &self,
        auth: &IntegrationAuthContext,
        method: &str,
        url: &str,
    ) -> Result<Option<IntegrationRequestProof>, CoreError> {
        let signing_key = self.get_signing_key().await?;
        let verifying_key = signing_key.verifying_key();
        let jwk = serde_json::json!({
            "kty": "OKP",
            "crv": "Ed25519",
            "x": URL_SAFE_NO_PAD.encode(verifying_key.to_bytes()),
        });
        let header = serde_json::json!({
            "typ": "dpop+jwt",
            "alg": "EdDSA",
            "jwk": jwk,
        });

        let mut claims = serde_json::Map::from_iter([
            (
                "htu".to_string(),
                serde_json::Value::String(url.to_string()),
            ),
            (
                "htm".to_string(),
                serde_json::Value::String(method.to_ascii_uppercase()),
            ),
            (
                "iat".to_string(),
                serde_json::Value::Number(serde_json::Number::from(Utc::now().timestamp())),
            ),
            (
                "jti".to_string(),
                serde_json::Value::String(uuid::Uuid::new_v4().to_string()),
            ),
        ]);

        if !auth.access_token.trim().is_empty() {
            let digest = Sha256::digest(auth.access_token.as_bytes());
            claims.insert(
                "ath".to_string(),
                serde_json::Value::String(URL_SAFE_NO_PAD.encode(digest)),
            );
        }

        let header_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).map_err(|error| {
            CoreError::Serialization(serde_json::Error::io(std::io::Error::other(format!(
                "failed to serialize DPoP JWT header: {error}"
            ))))
        })?);
        let payload_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).map_err(|error| {
            CoreError::Serialization(serde_json::Error::io(std::io::Error::other(format!(
                "failed to serialize DPoP JWT payload: {error}"
            ))))
        })?);
        let signing_input = format!("{header_b64}.{payload_b64}");
        let signature = signing_key.sign(signing_input.as_bytes()).to_bytes();
        let proof = format!(
            "{signing_input}.{}",
            URL_SAFE_NO_PAD.encode(signature.as_slice())
        );

        Ok(Some(IntegrationRequestProof {
            header_name: "dpop".to_string(),
            header_value: proof,
        }))
    }
}
