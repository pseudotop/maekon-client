//! OAuth client implementation — generic runtime for provider-managed credentials.
//!
//! Coordinates PKCE, loopback callback server, token exchange, and secure
//! storage via the `SecretStore` port.

pub mod callback_server;
pub mod pkce;
pub mod provider_config;
pub mod refresh_coordinator;
pub mod token_exchange;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::{oneshot, Mutex};
use tracing::{debug, info, warn};

use oneshim_core::error::CoreError;
use oneshim_core::ports::oauth::{
    OAuthConnectionStatus, OAuthErrorKind, OAuthFlowHandle, OAuthFlowStatus, OAuthPort,
    RefreshResult,
};
use oneshim_core::ports::secret_store::SecretStore;

use self::provider_config::OAuthProviderConfig;

/// Active OAuth flow state.
struct ActiveFlow {
    provider_id: String,
    #[allow(dead_code)] // Retained for potential token re-exchange
    pkce_verifier: String,
    cancel_tx: Option<oneshot::Sender<()>>,
    status: OAuthFlowStatus,
}

/// SecretStore key names.
const KEY_ACCESS_TOKEN: &str = "access_token";
const KEY_REFRESH_TOKEN: &str = "refresh_token";
const KEY_EXPIRES_AT: &str = "expires_at";
const KEY_SCOPES: &str = "scopes";

/// OAuth client implementing `OAuthPort`.
pub struct OAuthClient {
    http: reqwest::Client,
    secret_store: Arc<dyn SecretStore>,
    providers: HashMap<String, OAuthProviderConfig>,
    active_flows: Arc<Mutex<HashMap<String, ActiveFlow>>>,
    refresh_in_progress: AtomicBool,
}

impl OAuthClient {
    /// Create a new OAuthClient with provider configurations.
    pub fn new(secret_store: Arc<dyn SecretStore>, providers: Vec<OAuthProviderConfig>) -> Self {
        let provider_map: HashMap<String, OAuthProviderConfig> = providers
            .into_iter()
            .map(|p| (p.provider_id.clone(), p))
            .collect();

        Self {
            http: reqwest::Client::new(),
            secret_store,
            providers: provider_map,
            active_flows: Arc::new(Mutex::new(HashMap::new())),
            refresh_in_progress: AtomicBool::new(false),
        }
    }

    fn get_provider(&self, provider_id: &str) -> Result<&OAuthProviderConfig, CoreError> {
        self.providers
            .get(provider_id)
            .ok_or_else(|| CoreError::OAuthError {
                code: oneshim_core::error_codes::OAuthCode::Failed,
                provider: provider_id.into(),
                message: "unknown OAuth provider".into(),
            })
    }

    /// Check if a stored access token is still valid (not expired).
    async fn is_token_valid(&self, provider_id: &str) -> bool {
        if let Ok(Some(expires_str)) = self
            .secret_store
            .retrieve(provider_id, KEY_EXPIRES_AT)
            .await
        {
            if let Ok(expires_at) = chrono::DateTime::parse_from_rfc3339(&expires_str) {
                // Consider token invalid 60 seconds before actual expiry
                return Utc::now() < expires_at - chrono::Duration::seconds(60);
            }
        }
        false
    }

    /// Try to refresh the access token using the stored refresh token.
    ///
    /// Uses an `AtomicBool` guard to prevent concurrent refresh attempts.
    async fn try_refresh(&self, provider_id: &str) -> Result<bool, CoreError> {
        if self
            .refresh_in_progress
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            debug!("Refresh already in progress for {provider_id}, skipping");
            return Ok(false);
        }
        struct RefreshGuard<'a>(&'a AtomicBool);
        impl<'a> Drop for RefreshGuard<'a> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _guard = RefreshGuard(&self.refresh_in_progress);

        let config = self.get_provider(provider_id)?;
        let refresh_tok = self
            .secret_store
            .retrieve(provider_id, KEY_REFRESH_TOKEN)
            .await?;

        let Some(refresh_tok) = refresh_tok else {
            debug!("no refresh token stored for {provider_id}");
            return Ok(false);
        };

        match token_exchange::refresh_token(&self.http, config, &refresh_tok).await {
            Ok(result) => {
                self.store_tokens(provider_id, &result).await?;
                info!("access token refreshed for {provider_id}");
                Ok(true)
            }
            Err(e) => {
                warn!("token refresh failed for {provider_id}: {e}");
                Ok(false)
            }
        }
    }

    /// Store tokens from an exchange result into the secret store.
    async fn store_tokens(
        &self,
        provider_id: &str,
        result: &token_exchange::TokenExchangeResult,
    ) -> Result<(), CoreError> {
        store_tokens_static(&*self.secret_store, provider_id, result).await
    }
}

#[async_trait]
impl OAuthPort for OAuthClient {
    async fn start_flow(&self, provider_id: &str) -> Result<OAuthFlowHandle, CoreError> {
        let config = self.get_provider(provider_id)?.clone();

        // Check port availability first
        if !callback_server::check_port_available(config.callback_port).await {
            return Err(CoreError::OAuthError {
                code: oneshim_core::error_codes::OAuthCode::Failed,
                provider: provider_id.into(),
                message: format!(
                    "port {} is already in use (is Codex CLI running?). \
                     Please close other applications using this port and try again.",
                    config.callback_port
                ),
            });
        }

        let pkce = pkce::generate_pkce();
        let state = pkce::generate_state();
        let flow_id = uuid::Uuid::new_v4().to_string();

        let auth_url = config
            .authorization_url(&state, &pkce.challenge)
            .map_err(|e| CoreError::OAuthError {
                code: oneshim_core::error_codes::OAuthCode::Failed,
                provider: provider_id.into(),
                message: format!("invalid authorization endpoint URL: {e}"),
            })?;

        let (cancel_tx, cancel_rx) = oneshot::channel();

        // Store active flow
        {
            let mut flows = self.active_flows.lock().await;
            flows.insert(
                flow_id.clone(),
                ActiveFlow {
                    provider_id: provider_id.to_string(),
                    pkce_verifier: pkce.verifier.clone(),
                    cancel_tx: Some(cancel_tx),
                    status: OAuthFlowStatus::Pending,
                },
            );
        }

        // Spawn background task: callback server → token exchange → store
        let flow_id_bg = flow_id.clone();
        let provider_id_bg = provider_id.to_string();
        let flows_ref = self.active_flows.clone();
        let http = self.http.clone();
        let secret_store = self.secret_store.clone();
        let verifier = pkce.verifier;

        tokio::spawn(async move {
            let result =
                callback_server::wait_for_callback(config.callback_port, state, cancel_rx).await;

            let mut flows = flows_ref.lock().await;
            let flow = flows.get_mut(&flow_id_bg);

            match result {
                Ok(callback) => {
                    // Exchange code for tokens
                    match token_exchange::exchange_code(&http, &config, &callback.code, &verifier)
                        .await
                    {
                        Ok(tokens) => {
                            // Store tokens
                            if let Err(e) =
                                store_tokens_static(&*secret_store, &provider_id_bg, &tokens).await
                            {
                                warn!("failed to store tokens: {e}");
                                if let Some(f) = flow {
                                    f.status = OAuthFlowStatus::Failed {
                                        error: format!("token storage failed: {e}"),
                                    };
                                }
                                return;
                            }
                            info!("OAuth flow completed for {provider_id_bg}");
                            if let Some(f) = flow {
                                f.status = OAuthFlowStatus::Completed;
                            }
                        }
                        Err(e) => {
                            warn!("token exchange failed: {e}");
                            if let Some(f) = flow {
                                f.status = OAuthFlowStatus::Failed {
                                    error: e.to_string(),
                                };
                            }
                        }
                    }
                }
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("cancelled") {
                        if let Some(f) = flow {
                            f.status = OAuthFlowStatus::Cancelled;
                        }
                    } else if let Some(f) = flow {
                        f.status = OAuthFlowStatus::Failed { error: msg };
                    }
                }
            }
        });

        debug!("OAuth flow started: {flow_id} (provider: {provider_id})");

        Ok(OAuthFlowHandle { flow_id, auth_url })
    }

    async fn flow_status(&self, flow_id: &str) -> Result<OAuthFlowStatus, CoreError> {
        let mut flows = self.active_flows.lock().await;
        let status = flows
            .get(flow_id)
            .map(|f| f.status.clone())
            .ok_or_else(|| CoreError::OAuthError {
                code: oneshim_core::error_codes::OAuthCode::Failed,
                provider: "unknown".into(),
                message: format!("flow {flow_id} not found"),
            })?;

        // Evict terminal flows to prevent memory leaks over long sessions.
        if matches!(
            status,
            OAuthFlowStatus::Completed
                | OAuthFlowStatus::Failed { .. }
                | OAuthFlowStatus::Cancelled
        ) {
            flows.remove(flow_id);
        }

        Ok(status)
    }

    async fn cancel_flow(&self, flow_id: &str) -> Result<(), CoreError> {
        let mut flows = self.active_flows.lock().await;
        if let Some(flow) = flows.get_mut(flow_id) {
            if let Some(tx) = flow.cancel_tx.take() {
                if let Err(e) = tx.send(()) {
                    debug!("channel send failed: {e:?}");
                }
            }
            flow.status = OAuthFlowStatus::Cancelled;
        }
        Ok(())
    }

    async fn get_access_token(&self, provider_id: &str) -> Result<Option<String>, CoreError> {
        // 1. Check if we have a stored access token
        let token = self
            .secret_store
            .retrieve(provider_id, KEY_ACCESS_TOKEN)
            .await?;

        if token.is_none() {
            return Ok(None);
        }

        // 2. Check if it's still valid
        if self.is_token_valid(provider_id).await {
            return Ok(token);
        }

        // 3. Try to refresh
        if self.try_refresh(provider_id).await? {
            return self
                .secret_store
                .retrieve(provider_id, KEY_ACCESS_TOKEN)
                .await;
        }

        // 4. Token expired and refresh failed
        Ok(None)
    }

    async fn revoke(&self, provider_id: &str) -> Result<(), CoreError> {
        info!("revoking OAuth credentials for {provider_id}");
        self.secret_store.delete_namespace(provider_id).await?;

        // Clean up any active flows for this provider
        let mut flows = self.active_flows.lock().await;
        let flow_ids: Vec<String> = flows
            .iter()
            .filter(|(_, f)| f.provider_id == provider_id)
            .map(|(id, _)| id.clone())
            .collect();
        for id in flow_ids {
            if let Some(mut flow) = flows.remove(&id) {
                if let Some(tx) = flow.cancel_tx.take() {
                    if let Err(e) = tx.send(()) {
                        debug!("channel send failed: {e:?}");
                    }
                }
            }
        }

        Ok(())
    }

    async fn connection_status(
        &self,
        provider_id: &str,
    ) -> Result<OAuthConnectionStatus, CoreError> {
        let has_token = self
            .secret_store
            .retrieve(provider_id, KEY_ACCESS_TOKEN)
            .await?
            .is_some();

        let expires_at = self
            .secret_store
            .retrieve(provider_id, KEY_EXPIRES_AT)
            .await?;

        let scopes = self
            .secret_store
            .retrieve(provider_id, KEY_SCOPES)
            .await?
            .map(|s| s.split_whitespace().map(String::from).collect())
            .unwrap_or_default();

        let has_refresh_token = self
            .secret_store
            .retrieve(provider_id, KEY_REFRESH_TOKEN)
            .await?
            .is_some();

        let connected = has_token && self.is_token_valid(provider_id).await;

        let api_base_url = self
            .providers
            .get(provider_id)
            .map(|p| p.api_base_url.clone());

        Ok(OAuthConnectionStatus {
            provider_id: provider_id.to_string(),
            connected,
            expires_at,
            scopes,
            api_base_url,
            has_refresh_token,
        })
    }

    async fn refresh_access_token(
        &self,
        provider_id: &str,
        min_valid_for_secs: i64,
    ) -> Result<RefreshResult, CoreError> {
        // 1. Check if we have a stored access token at all.
        let token = self
            .secret_store
            .retrieve(provider_id, KEY_ACCESS_TOKEN)
            .await?;
        if token.is_none() {
            return Ok(RefreshResult::NotAuthenticated);
        }

        // Prevent concurrent refresh attempts (shared with try_refresh).
        if self
            .refresh_in_progress
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            debug!("Refresh already in progress for {provider_id}, skipping");
            return Ok(RefreshResult::AlreadyFresh {
                expires_at: String::new(),
            });
        }
        struct RefreshGuard<'a>(&'a AtomicBool);
        impl<'a> Drop for RefreshGuard<'a> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _guard = RefreshGuard(&self.refresh_in_progress);

        // 2. Check if the token is still valid for the requested duration.
        if let Ok(Some(expires_str)) = self
            .secret_store
            .retrieve(provider_id, KEY_EXPIRES_AT)
            .await
        {
            if let Ok(expires_at) = chrono::DateTime::parse_from_rfc3339(&expires_str) {
                let remaining = expires_at.with_timezone(&Utc) - Utc::now();
                if remaining > chrono::Duration::seconds(min_valid_for_secs) {
                    return Ok(RefreshResult::AlreadyFresh {
                        expires_at: expires_str,
                    });
                }
            }
        }

        // 3. Check for refresh token.
        let config = self.get_provider(provider_id)?;
        let refresh_tok = self
            .secret_store
            .retrieve(provider_id, KEY_REFRESH_TOKEN)
            .await?;
        let Some(refresh_tok) = refresh_tok else {
            return Ok(RefreshResult::ReauthRequired {
                kind: OAuthErrorKind::Unknown("no_refresh_token".into()),
                reason: "no refresh token available".into(),
            });
        };

        // 4. Attempt the refresh.
        match token_exchange::refresh_token(&self.http, config, &refresh_tok).await {
            Ok(result) => {
                self.store_tokens(provider_id, &result).await?;
                let new_expires = self
                    .secret_store
                    .retrieve(provider_id, KEY_EXPIRES_AT)
                    .await?
                    .unwrap_or_default();
                info!("access token refreshed for {provider_id}");
                Ok(RefreshResult::Refreshed {
                    expires_at: new_expires,
                })
            }
            Err(CoreError::OAuthRefreshError {
                code: oneshim_core::error_codes::OAuthCode::RefreshFailed,
                kind,
                message,
                ..
            }) => {
                if kind.is_terminal() {
                    warn!("token refresh terminal failure for {provider_id}: [{kind:?}] {message}");
                    Ok(RefreshResult::ReauthRequired {
                        kind,
                        reason: message,
                    })
                } else {
                    warn!(
                        "token refresh transient failure for {provider_id}: [{kind:?}] {message}"
                    );
                    Ok(RefreshResult::TransientFailure { kind, message })
                }
            }
            Err(e) => {
                let msg = e.to_string();
                warn!("token refresh unexpected error for {provider_id}: {msg}");
                Ok(RefreshResult::TransientFailure {
                    kind: OAuthErrorKind::Unknown(msg.clone()),
                    message: msg,
                })
            }
        }
    }
}

/// Static helper for use inside the spawned task (cannot borrow `self`).
async fn store_tokens_static(
    secret_store: &dyn SecretStore,
    provider_id: &str,
    result: &token_exchange::TokenExchangeResult,
) -> Result<(), CoreError> {
    secret_store
        .store(provider_id, KEY_ACCESS_TOKEN, &result.access_token)
        .await?;
    if let Some(ref rt) = result.refresh_token {
        secret_store
            .store(provider_id, KEY_REFRESH_TOKEN, rt)
            .await?;
    }
    if let Some(expires_in) = result.expires_in {
        let expires_at = Utc::now() + chrono::Duration::seconds(expires_in as i64);
        secret_store
            .store(provider_id, KEY_EXPIRES_AT, &expires_at.to_rfc3339())
            .await?;
    }
    if let Some(ref scope) = result.scope {
        secret_store.store(provider_id, KEY_SCOPES, scope).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oneshim_core::ports::secret_store::SecretStore as SecretStoreTrait;
    use std::collections::HashMap as StdHashMap;

    /// In-memory secret store for testing.
    struct TestSecretStore {
        store: Mutex<StdHashMap<String, String>>,
    }

    impl TestSecretStore {
        fn new() -> Self {
            Self {
                store: Mutex::new(StdHashMap::new()),
            }
        }
    }

    #[async_trait]
    impl SecretStoreTrait for TestSecretStore {
        async fn store(&self, ns: &str, key: &str, value: &str) -> Result<(), CoreError> {
            self.store
                .lock()
                .await
                .insert(format!("{ns}.{key}"), value.to_string());
            Ok(())
        }
        async fn retrieve(&self, ns: &str, key: &str) -> Result<Option<String>, CoreError> {
            Ok(self.store.lock().await.get(&format!("{ns}.{key}")).cloned())
        }
        async fn delete(&self, ns: &str, key: &str) -> Result<(), CoreError> {
            self.store.lock().await.remove(&format!("{ns}.{key}"));
            Ok(())
        }
        async fn delete_namespace(&self, ns: &str) -> Result<(), CoreError> {
            let prefix = format!("{ns}.");
            self.store
                .lock()
                .await
                .retain(|k, _| !k.starts_with(&prefix));
            Ok(())
        }
    }

    /// Counter for unique test ports to avoid parallel test conflicts.
    static TEST_PORT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(19400);

    fn next_test_port() -> u16 {
        TEST_PORT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    /// Create a client that does NOT bind to real port 1455 in tests.
    fn make_client(secret_store: Arc<dyn SecretStoreTrait>) -> OAuthClient {
        OAuthClient::new(secret_store, vec![OAuthProviderConfig::openai_codex()])
    }

    /// Create a client with a unique test port for tests that call `start_flow`.
    fn make_client_with_test_port(secret_store: Arc<dyn SecretStoreTrait>) -> OAuthClient {
        let mut config = OAuthProviderConfig::openai_codex();
        config.callback_port = next_test_port();
        OAuthClient::new(secret_store, vec![config])
    }

    #[tokio::test]
    async fn start_flow_returns_valid_handle() {
        let store = Arc::new(TestSecretStore::new());
        let client = make_client_with_test_port(store);
        let handle = client.start_flow("openai").await.unwrap();

        assert!(!handle.flow_id.is_empty());
        assert!(handle.auth_url.contains("auth.openai.com"));
        assert!(handle.auth_url.contains("code_challenge_method=S256"));

        // Clean up: cancel the flow so the callback server shuts down
        client.cancel_flow(&handle.flow_id).await.unwrap();
    }

    #[tokio::test]
    async fn start_flow_unknown_provider_fails() {
        let store = Arc::new(TestSecretStore::new());
        let client = make_client(store);
        let result = client.start_flow("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn get_access_token_returns_none_when_not_connected() {
        let store = Arc::new(TestSecretStore::new());
        let client = make_client(store);
        let token = client.get_access_token("openai").await.unwrap();
        assert!(token.is_none());
    }

    #[tokio::test]
    async fn get_access_token_returns_valid_token() {
        let store = Arc::new(TestSecretStore::new());
        // Pre-store a valid token
        let expires = (Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        store
            .store("openai", KEY_ACCESS_TOKEN, "tok_test")
            .await
            .unwrap();
        store
            .store("openai", KEY_EXPIRES_AT, &expires)
            .await
            .unwrap();

        let client = make_client(store);
        let token = client.get_access_token("openai").await.unwrap();
        assert_eq!(token, Some("tok_test".to_string()));
    }

    #[tokio::test]
    async fn revoke_clears_all_secrets() {
        let store = Arc::new(TestSecretStore::new());
        store
            .store("openai", KEY_ACCESS_TOKEN, "tok")
            .await
            .unwrap();
        store
            .store("openai", KEY_REFRESH_TOKEN, "rt")
            .await
            .unwrap();

        let client = make_client(store.clone());
        client.revoke("openai").await.unwrap();

        assert!(store
            .retrieve("openai", KEY_ACCESS_TOKEN)
            .await
            .unwrap()
            .is_none());
        assert!(store
            .retrieve("openai", KEY_REFRESH_TOKEN)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn connection_status_disconnected() {
        let store = Arc::new(TestSecretStore::new());
        let client = make_client(store);
        let status = client.connection_status("openai").await.unwrap();
        assert!(!status.connected);
        assert_eq!(status.provider_id, "openai");
        assert!(!status.has_refresh_token);
    }

    #[tokio::test]
    async fn connection_status_connected() {
        let store = Arc::new(TestSecretStore::new());
        let expires = (Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        store
            .store("openai", KEY_ACCESS_TOKEN, "tok")
            .await
            .unwrap();
        store
            .store("openai", KEY_EXPIRES_AT, &expires)
            .await
            .unwrap();
        store
            .store("openai", KEY_SCOPES, "openid profile")
            .await
            .unwrap();
        store
            .store("openai", KEY_REFRESH_TOKEN, "rt_test")
            .await
            .unwrap();

        let client = make_client(store);
        let status = client.connection_status("openai").await.unwrap();
        assert!(status.connected);
        assert_eq!(status.scopes, vec!["openid", "profile"]);
        assert!(status.has_refresh_token);
    }

    #[tokio::test]
    async fn flow_status_returns_pending() {
        let store = Arc::new(TestSecretStore::new());
        let client = make_client_with_test_port(store);
        let handle = client.start_flow("openai").await.unwrap();

        let status = client.flow_status(&handle.flow_id).await.unwrap();
        assert_eq!(status, OAuthFlowStatus::Pending);

        client.cancel_flow(&handle.flow_id).await.unwrap();
    }

    #[tokio::test]
    async fn cancel_flow_sets_cancelled() {
        let store = Arc::new(TestSecretStore::new());
        let client = make_client_with_test_port(store);
        let handle = client.start_flow("openai").await.unwrap();

        client.cancel_flow(&handle.flow_id).await.unwrap();

        // Give the background task a moment to update
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let status = client.flow_status(&handle.flow_id).await.unwrap();
        assert_eq!(status, OAuthFlowStatus::Cancelled);
    }

    #[tokio::test]
    async fn refresh_guard_prevents_concurrent_refresh() {
        let store = Arc::new(TestSecretStore::new());
        let client = make_client(store);

        // Manually set the guard to simulate an in-progress refresh.
        client.refresh_in_progress.store(true, Ordering::Release);
        let result = client.try_refresh("openai").await.unwrap();
        assert!(
            !result,
            "try_refresh should return false when guard is held"
        );

        // Release the guard.
        client.refresh_in_progress.store(false, Ordering::Release);
    }

    #[tokio::test]
    async fn refresh_access_token_returns_not_authenticated_when_no_token() {
        let store = Arc::new(TestSecretStore::new());
        let client = make_client(store);
        let result = client.refresh_access_token("openai", 300).await.unwrap();
        assert!(matches!(result, RefreshResult::NotAuthenticated));
    }

    #[tokio::test]
    async fn refresh_access_token_returns_already_fresh_when_not_expiring() {
        let store = Arc::new(TestSecretStore::new());
        let expires = (Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        store
            .store("openai", KEY_ACCESS_TOKEN, "tok")
            .await
            .unwrap();
        store
            .store("openai", KEY_EXPIRES_AT, &expires)
            .await
            .unwrap();

        let client = make_client(store);
        let result = client.refresh_access_token("openai", 300).await.unwrap();
        match result {
            RefreshResult::AlreadyFresh { expires_at } => {
                assert_eq!(expires_at, expires);
            }
            other => panic!("expected AlreadyFresh, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn refresh_access_token_returns_reauth_when_no_refresh_token() {
        let store = Arc::new(TestSecretStore::new());
        // Token exists but is about to expire, no refresh token.
        let expires = (Utc::now() + chrono::Duration::seconds(30)).to_rfc3339();
        store
            .store("openai", KEY_ACCESS_TOKEN, "tok")
            .await
            .unwrap();
        store
            .store("openai", KEY_EXPIRES_AT, &expires)
            .await
            .unwrap();

        let client = make_client(store);
        let result = client.refresh_access_token("openai", 300).await.unwrap();
        match result {
            RefreshResult::ReauthRequired { kind, reason } => {
                assert!(reason.contains("no refresh token"));
                assert!(matches!(kind, OAuthErrorKind::Unknown(ref s) if s == "no_refresh_token"));
            }
            other => panic!("expected ReauthRequired, got {other:?}"),
        }
    }
}
