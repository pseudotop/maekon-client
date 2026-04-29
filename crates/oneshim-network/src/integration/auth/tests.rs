use super::*;
use crate::integration::transport::IntegrationRequestProofFactory;
use async_trait::async_trait;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use chrono::Utc;
use mockito::Matcher;
use oneshim_core::error::CoreError;
use oneshim_core::models::integration::{
    IntegrationAuthContext, IntegrationAuthScheme, IntegrationAuthStatusKind,
    IntegrationCapabilityScope,
};
use oneshim_core::ports::integration::IntegrationAuthPort;
use oneshim_core::ports::secret_store::{
    SecretStore, INTEGRATION_ACCESS_TOKEN_SECRET_KEY, INTEGRATION_AUTH_SECRET_NAMESPACE,
    INTEGRATION_EXPIRES_AT_SECRET_KEY, INTEGRATION_REFRESH_TOKEN_SECRET_KEY,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

#[derive(Default)]
struct InMemorySecretStore {
    values: std::sync::Mutex<HashMap<String, String>>,
}

#[async_trait]
impl SecretStore for InMemorySecretStore {
    async fn store(&self, namespace: &str, key: &str, value: &str) -> Result<(), CoreError> {
        self.values
            .lock()
            .unwrap()
            .insert(format!("{namespace}:{key}"), value.to_string());
        Ok(())
    }

    async fn retrieve(&self, namespace: &str, key: &str) -> Result<Option<String>, CoreError> {
        Ok(self
            .values
            .lock()
            .unwrap()
            .get(&format!("{namespace}:{key}"))
            .cloned())
    }

    async fn delete(&self, namespace: &str, key: &str) -> Result<(), CoreError> {
        self.values
            .lock()
            .unwrap()
            .remove(&format!("{namespace}:{key}"));
        Ok(())
    }

    async fn delete_namespace(&self, namespace: &str) -> Result<(), CoreError> {
        self.values
            .lock()
            .unwrap()
            .retain(|entry, _| !entry.starts_with(&format!("{namespace}:")));
        Ok(())
    }
}

#[tokio::test]
async fn static_auth_port_preserves_auth_context() {
    let port = StaticIntegrationAuthPort::new(IntegrationAuthContext {
        access_token: "token-1".to_string(),
        scheme: IntegrationAuthScheme::BearerToken,
        expires_at: None,
        resource_indicator: None,
    });

    let context = port
        .resolve_session_auth(
            &[IntegrationCapabilityScope::InsightWrite],
            Some("https://integration.example.com"),
        )
        .await
        .unwrap();

    assert_eq!(context.access_token, "token-1");
    assert_eq!(context.scheme, IntegrationAuthScheme::BearerToken);
    assert_eq!(
        context.resource_indicator.as_deref(),
        Some("https://integration.example.com")
    );
    assert_eq!(
        port.current_auth_status().await.unwrap().status,
        IntegrationAuthStatusKind::Ready
    );
}

#[tokio::test]
async fn env_auth_port_reads_token_from_env() {
    let env_name = "ONESHIM_TEST_INTEGRATION_TOKEN";
    unsafe {
        std::env::set_var(env_name, "token-from-env");
    }

    let port = EnvIntegrationAuthPort::new(
        env_name,
        IntegrationAuthScheme::DpopBearer,
        None,
        Some("https://integration.example.com".to_string()),
    );

    let context = port
        .resolve_session_auth(&[IntegrationCapabilityScope::SessionManage], None)
        .await
        .unwrap();

    assert_eq!(context.access_token, "token-from-env");
    assert_eq!(context.scheme, IntegrationAuthScheme::DpopBearer);
    assert!(port.current_auth_status().await.unwrap().authenticated);

    unsafe {
        std::env::remove_var(env_name);
    }
}

#[tokio::test]
async fn noop_proof_factory_returns_none() {
    let factory = NoopIntegrationRequestProofFactory;
    let proof = factory
        .build_proof(
            &IntegrationAuthContext {
                access_token: "token".to_string(),
                scheme: IntegrationAuthScheme::BearerToken,
                expires_at: None,
                resource_indicator: None,
            },
            "POST",
            "https://integration.example.com/bootstrap",
        )
        .await
        .unwrap();

    assert!(proof.is_none());
}

#[tokio::test]
async fn static_proof_factory_returns_configured_header() {
    let factory = StaticIntegrationRequestProofFactory::new("dpop", "proof-value");
    let proof = factory
        .build_proof(
            &IntegrationAuthContext {
                access_token: "token".to_string(),
                scheme: IntegrationAuthScheme::DpopBearer,
                expires_at: None,
                resource_indicator: None,
            },
            "POST",
            "https://integration.example.com/bootstrap",
        )
        .await
        .unwrap()
        .unwrap();

    assert_eq!(proof.header_name, "dpop");
    assert_eq!(proof.header_value, "proof-value");
}

#[tokio::test]
async fn ed25519_dpop_factory_builds_real_jwt_proof() {
    let factory = Ed25519DpopProofFactory::new(None);
    let proof = factory
        .build_proof(
            &IntegrationAuthContext {
                access_token: "access-token".to_string(),
                scheme: IntegrationAuthScheme::DpopBearer,
                expires_at: None,
                resource_indicator: None,
            },
            "POST",
            "https://integration.example.com/bootstrap",
        )
        .await
        .unwrap()
        .unwrap();

    assert_eq!(proof.header_name, "dpop");
    let parts: Vec<&str> = proof.header_value.split('.').collect();
    assert_eq!(parts.len(), 3);

    let header: serde_json::Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0].as_bytes()).unwrap()).unwrap();
    let payload: serde_json::Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1].as_bytes()).unwrap()).unwrap();
    assert_eq!(header["typ"], "dpop+jwt");
    assert_eq!(header["alg"], "EdDSA");
    assert_eq!(payload["htu"], "https://integration.example.com/bootstrap");
    assert_eq!(payload["htm"], "POST");
    assert!(payload.get("ath").is_some());
}

#[tokio::test]
async fn oidc_device_flow_starts_and_polls_into_ready_auth() {
    let mut server = mockito::Server::new_async().await;
    let device_endpoint = format!("{}/oauth/device/code", server.url());
    let token_endpoint = format!("{}/oauth/token", server.url());

    let _device_mock = server
        .mock("POST", "/oauth/device/code")
        .match_body(Matcher::AllOf(vec![
            Matcher::UrlEncoded("client_id".into(), "desktop-client".into()),
            Matcher::UrlEncoded("scope".into(), "openid insight:write".into()),
            Matcher::UrlEncoded("resource".into(), "https://integration.example.com".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            serde_json::json!({
                "device_code": "device-code-1",
                "user_code": "ABCD-EFGH",
                "verification_uri": "https://id.example.com/activate",
                "verification_uri_complete": "https://id.example.com/activate?user_code=ABCD-EFGH",
                "expires_in": 900,
                "interval": 5
            })
            .to_string(),
        )
        .create_async()
        .await;

    let _token_mock = server
        .mock("POST", "/oauth/token")
        .match_body(Matcher::AllOf(vec![
            Matcher::UrlEncoded(
                "grant_type".into(),
                "urn:ietf:params:oauth:grant-type:device_code".into(),
            ),
            Matcher::UrlEncoded("client_id".into(), "desktop-client".into()),
            Matcher::UrlEncoded("device_code".into(), "device-code-1".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            serde_json::json!({
                "access_token": "access-token-1",
                "refresh_token": "refresh-token-1",
                "token_type": "Bearer",
                "expires_in": 3600
            })
            .to_string(),
        )
        .create_async()
        .await;

    let secret_store = Arc::new(InMemorySecretStore::default()) as Arc<dyn SecretStore>;
    let port = OidcDeviceFlowIntegrationAuthPort::new(
        OidcDeviceFlowAuthConfig {
            client_id: "desktop-client".to_string(),
            device_authorization_url: device_endpoint,
            token_url: token_endpoint,
            default_scopes: vec!["openid".to_string()],
            resource_indicator: Some("https://integration.example.com".to_string()),
            scheme: IntegrationAuthScheme::BearerToken,
            request_timeout: Duration::from_secs(5),
        },
        Arc::new(NoopIntegrationRequestProofFactory),
        Some(secret_store.clone()),
    )
    .unwrap();

    let flow = port
        .start_device_authorization(
            &[IntegrationCapabilityScope::InsightWrite],
            Some("https://integration.example.com"),
        )
        .await
        .unwrap();
    assert_eq!(flow.user_code, "ABCD-EFGH");

    let status = port.poll_device_authorization(&flow.flow_id).await.unwrap();
    assert_eq!(status.status, IntegrationAuthStatusKind::Ready);
    assert!(status.authenticated);

    let auth = port
        .resolve_session_auth(&[IntegrationCapabilityScope::InsightWrite], None)
        .await
        .unwrap();
    assert_eq!(auth.access_token, "access-token-1");
    assert_eq!(
        secret_store
            .retrieve(
                INTEGRATION_AUTH_SECRET_NAMESPACE,
                INTEGRATION_ACCESS_TOKEN_SECRET_KEY
            )
            .await
            .unwrap()
            .as_deref(),
        Some("access-token-1")
    );
}

#[tokio::test]
async fn start_device_authorization_reuses_matching_pending_flow() {
    let mut server = mockito::Server::new_async().await;
    let device_endpoint = format!("{}/oauth/device/code", server.url());
    let token_endpoint = format!("{}/oauth/token", server.url());

    let device_mock = server
        .mock("POST", "/oauth/device/code")
        .match_body(Matcher::AllOf(vec![
            Matcher::UrlEncoded("client_id".into(), "desktop-client".into()),
            Matcher::UrlEncoded("scope".into(), "openid prompt:read".into()),
            Matcher::UrlEncoded("resource".into(), "https://integration.example.com".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            serde_json::json!({
                "device_code": "device-code-2",
                "user_code": "IJKL-MNOP",
                "verification_uri": "https://id.example.com/activate",
                "verification_uri_complete": "https://id.example.com/activate?user_code=IJKL-MNOP",
                "expires_in": 900,
                "interval": 5
            })
            .to_string(),
        )
        .expect(1)
        .create_async()
        .await;

    let port = OidcDeviceFlowIntegrationAuthPort::new(
        OidcDeviceFlowAuthConfig {
            client_id: "desktop-client".to_string(),
            device_authorization_url: device_endpoint,
            token_url: token_endpoint,
            default_scopes: vec!["openid".to_string()],
            resource_indicator: Some("https://integration.example.com".to_string()),
            scheme: IntegrationAuthScheme::BearerToken,
            request_timeout: Duration::from_secs(5),
        },
        Arc::new(NoopIntegrationRequestProofFactory),
        None,
    )
    .unwrap();

    let first = port
        .start_device_authorization(
            &[IntegrationCapabilityScope::PromptRead],
            Some("https://integration.example.com"),
        )
        .await
        .unwrap();
    let second = port
        .start_device_authorization(
            &[IntegrationCapabilityScope::PromptRead],
            Some("https://integration.example.com"),
        )
        .await
        .unwrap();

    assert_eq!(first.flow_id, second.flow_id);
    assert_eq!(first.user_code, second.user_code);
    device_mock.assert_async().await;
}

#[tokio::test]
async fn current_auth_status_refreshes_expired_material_when_refresh_token_exists() {
    let mut server = mockito::Server::new_async().await;
    let token_endpoint = format!("{}/oauth/token", server.url());

    let refresh_mock = server
        .mock("POST", "/oauth/token")
        .match_body(Matcher::AllOf(vec![
            Matcher::UrlEncoded("grant_type".into(), "refresh_token".into()),
            Matcher::UrlEncoded("client_id".into(), "desktop-client".into()),
            Matcher::UrlEncoded("refresh_token".into(), "refresh-token-2".into()),
            Matcher::UrlEncoded("resource".into(), "https://integration.example.com".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            serde_json::json!({
                "access_token": "access-token-2",
                "refresh_token": "refresh-token-2b",
                "token_type": "Bearer",
                "expires_in": 3600
            })
            .to_string(),
        )
        .expect(1)
        .create_async()
        .await;

    let secret_store = Arc::new(InMemorySecretStore::default()) as Arc<dyn SecretStore>;
    secret_store
        .store(
            INTEGRATION_AUTH_SECRET_NAMESPACE,
            INTEGRATION_ACCESS_TOKEN_SECRET_KEY,
            "expired-access-token",
        )
        .await
        .unwrap();
    secret_store
        .store(
            INTEGRATION_AUTH_SECRET_NAMESPACE,
            INTEGRATION_REFRESH_TOKEN_SECRET_KEY,
            "refresh-token-2",
        )
        .await
        .unwrap();
    secret_store
        .store(
            INTEGRATION_AUTH_SECRET_NAMESPACE,
            INTEGRATION_EXPIRES_AT_SECRET_KEY,
            &(Utc::now() - chrono::Duration::minutes(5)).to_rfc3339(),
        )
        .await
        .unwrap();

    let port = OidcDeviceFlowIntegrationAuthPort::new(
        OidcDeviceFlowAuthConfig {
            client_id: "desktop-client".to_string(),
            device_authorization_url: format!("{}/oauth/device/code", server.url()),
            token_url: token_endpoint,
            default_scopes: vec!["openid".to_string()],
            resource_indicator: Some("https://integration.example.com".to_string()),
            scheme: IntegrationAuthScheme::BearerToken,
            request_timeout: Duration::from_secs(5),
        },
        Arc::new(NoopIntegrationRequestProofFactory),
        Some(secret_store.clone()),
    )
    .unwrap();

    let status = port.current_auth_status().await.unwrap();
    assert_eq!(status.status, IntegrationAuthStatusKind::Ready);
    assert!(status.authenticated);
    assert_eq!(
        secret_store
            .retrieve(
                INTEGRATION_AUTH_SECRET_NAMESPACE,
                INTEGRATION_ACCESS_TOKEN_SECRET_KEY
            )
            .await
            .unwrap()
            .as_deref(),
        Some("access-token-2")
    );
    assert_eq!(
        secret_store
            .retrieve(
                INTEGRATION_AUTH_SECRET_NAMESPACE,
                INTEGRATION_REFRESH_TOKEN_SECRET_KEY
            )
            .await
            .unwrap()
            .as_deref(),
        Some("refresh-token-2b")
    );
    refresh_mock.assert_async().await;
}

#[tokio::test]
async fn cancel_device_authorization_requires_existing_flow() {
    let port = OidcDeviceFlowIntegrationAuthPort::new(
        OidcDeviceFlowAuthConfig {
            client_id: "desktop-client".to_string(),
            device_authorization_url: "https://id.example.com/oauth/device/code".to_string(),
            token_url: "https://id.example.com/oauth/token".to_string(),
            default_scopes: vec!["openid".to_string()],
            resource_indicator: Some("https://integration.example.com".to_string()),
            scheme: IntegrationAuthScheme::BearerToken,
            request_timeout: Duration::from_secs(5),
        },
        Arc::new(NoopIntegrationRequestProofFactory),
        None,
    )
    .unwrap();

    let error = port
        .cancel_device_authorization("missing-flow")
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        CoreError::NotFound {
            code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
            ..
        }
    ));
}

#[tokio::test]
async fn reset_auth_state_clears_pending_flow_and_stored_material() {
    let mut server = mockito::Server::new_async().await;
    let device_endpoint = format!("{}/oauth/device/code", server.url());
    let token_endpoint = format!("{}/oauth/token", server.url());

    let _device_mock = server
        .mock("POST", "/oauth/device/code")
        .match_body(Matcher::AllOf(vec![
            Matcher::UrlEncoded("client_id".into(), "desktop-client".into()),
            Matcher::UrlEncoded("scope".into(), "openid prompt:read".into()),
            Matcher::UrlEncoded("resource".into(), "https://integration.example.com".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            serde_json::json!({
                "device_code": "device-code-3",
                "user_code": "QRST-UVWX",
                "verification_uri": "https://id.example.com/activate",
                "verification_uri_complete": "https://id.example.com/activate?user_code=QRST-UVWX",
                "expires_in": 900,
                "interval": 5
            })
            .to_string(),
        )
        .expect(1)
        .create_async()
        .await;

    let secret_store = Arc::new(InMemorySecretStore::default()) as Arc<dyn SecretStore>;
    secret_store
        .store(
            INTEGRATION_AUTH_SECRET_NAMESPACE,
            INTEGRATION_ACCESS_TOKEN_SECRET_KEY,
            "stale-access-token",
        )
        .await
        .unwrap();
    secret_store
        .store(
            INTEGRATION_AUTH_SECRET_NAMESPACE,
            INTEGRATION_REFRESH_TOKEN_SECRET_KEY,
            "stale-refresh-token",
        )
        .await
        .unwrap();

    let port = OidcDeviceFlowIntegrationAuthPort::new(
        OidcDeviceFlowAuthConfig {
            client_id: "desktop-client".to_string(),
            device_authorization_url: device_endpoint,
            token_url: token_endpoint,
            default_scopes: vec!["openid".to_string()],
            resource_indicator: Some("https://integration.example.com".to_string()),
            scheme: IntegrationAuthScheme::BearerToken,
            request_timeout: Duration::from_secs(5),
        },
        Arc::new(NoopIntegrationRequestProofFactory),
        Some(secret_store.clone()),
    )
    .unwrap();

    let flow = port
        .start_device_authorization(
            &[IntegrationCapabilityScope::PromptRead],
            Some("https://integration.example.com"),
        )
        .await
        .unwrap();

    assert!(!flow.flow_id.is_empty());

    port.reset_auth_state().await.unwrap();

    let status_after_reset = port.current_auth_status().await.unwrap();
    assert_eq!(
        status_after_reset.status,
        IntegrationAuthStatusKind::Unauthenticated
    );
    assert!(status_after_reset.pending_flow.is_none());
    assert_eq!(
        secret_store
            .retrieve(
                INTEGRATION_AUTH_SECRET_NAMESPACE,
                INTEGRATION_ACCESS_TOKEN_SECRET_KEY
            )
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        secret_store
            .retrieve(
                INTEGRATION_AUTH_SECRET_NAMESPACE,
                INTEGRATION_REFRESH_TOKEN_SECRET_KEY
            )
            .await
            .unwrap(),
        None
    );
}
