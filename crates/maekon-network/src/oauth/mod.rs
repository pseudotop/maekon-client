//! OAuth client implementation — generic runtime for provider-managed credentials.
//!
//! Coordinates PKCE, loopback callback server, token exchange, and secure
//! storage via the `SecretStore` port.

pub mod callback_server;
pub mod pkce;
pub mod provider_config;
pub mod refresh_coordinator;
#[cfg(test)]
mod revoke_race_tests;
pub mod token_exchange;

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::{oneshot, Mutex};
use tracing::{debug, info, warn};

use maekon_core::error::CoreError;
use maekon_core::ports::oauth::{
    OAuthConnectionStatus, OAuthErrorKind, OAuthFlowHandle, OAuthFlowStatus, OAuthPort,
    RefreshResult,
};
use maekon_core::ports::secret_store::SecretStore;

use self::provider_config::OAuthProviderConfig;

/// Active OAuth flow state.
struct ActiveFlow {
    provider_id: String,
    // Actual PKCE enforcement uses a separately-cloned `verifier` local at the
    // token-exchange call site (not this field) — this is a retained copy for
    // potential retry/resume, not read back today.
    #[allow(dead_code)]
    pkce_verifier: String,
    cancel_tx: Option<oneshot::Sender<()>>,
    /// Background task handle retained so `OAuthClient::drop` can abort
    /// immediately rather than waiting for the cancel channel to be noticed.
    handle: Option<tokio::task::JoinHandle<()>>,
    status: OAuthFlowStatus,
}

/// SecretStore key names.
const KEY_ACCESS_TOKEN: &str = "access_token";
const KEY_REFRESH_TOKEN: &str = "refresh_token";
const KEY_EXPIRES_AT: &str = "expires_at";
const KEY_SCOPES: &str = "scopes";

/// Upper bound (30 days) on a server-supplied OAuth `expires_in` before it is
/// turned into a `chrono::Duration`. A hostile/buggy provider value (u64::MAX
/// casts to -1; a huge-but-positive value overflows `TimeDelta`/`DateTime`)
/// would otherwise panic or yield a nonsense expiry. 30 days is far longer than
/// any realistic access-token lifetime and is trivially within range (#6201 sibling).
const MAX_OAUTH_TTL_SECS: u64 = 60 * 60 * 24 * 30;

/// Outcome of a `try_refresh` attempt.
///
/// Distinguishes a contention skip (another refresh for the **same provider**
/// already holds that provider's refresh lock) from a genuine refresh failure.
/// A contention skip must NOT be reported as unauthenticated: the caller's
/// previously-read access token is within the 60s safety buffer and may still
/// be valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefreshOutcome {
    /// A fresh access token was successfully stored.
    Refreshed,
    /// A concurrent refresh for the same provider held that provider's lock, so
    /// this attempt was skipped. No conclusion about authentication can be drawn
    /// from this.
    SkippedContention,
    /// The refresh was attempted (or precluded) and did not yield a new token
    /// (e.g. no stored refresh token, or the network exchange failed).
    Failed,
}

/// OAuth client implementing `OAuthPort`.
pub struct OAuthClient {
    http: reqwest::Client,
    secret_store: Arc<dyn SecretStore>,
    providers: HashMap<String, OAuthProviderConfig>,
    active_flows: Arc<Mutex<HashMap<String, ActiveFlow>>>,
    /// Per-provider refresh serialization. Each provider gets its own
    /// `tokio::sync::Mutex<()>` so a slow refresh of provider A no longer blocks
    /// (or skips) a refresh of provider B. Previously a single process-global
    /// `AtomicBool` guarded all providers, which serialized refreshes across
    /// unrelated providers. The outer `Mutex<HashMap<..>>` is only held briefly
    /// to look up / lazily create a provider's lock — never across a refresh.
    refresh_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl OAuthClient {
    /// Create a new OAuthClient with provider configurations.
    pub fn new(secret_store: Arc<dyn SecretStore>, providers: Vec<OAuthProviderConfig>) -> Self {
        let provider_map: HashMap<String, OAuthProviderConfig> = providers
            .into_iter()
            .map(|p| (p.provider_id.clone(), p))
            .collect();

        Self {
            // #7068/#6892: build the OAuth HTTP client with redirect following
            // disabled by construction. This `http` client POSTs credential
            // bodies — the authorization code + PKCE verifier (exchange_code) and
            // the long-lived refresh_token (refresh_token) — to operator-
            // configurable token endpoints (OAuthProviderConfig.token_endpoint).
            // reqwest's default policy follows 30x and re-sends the request body
            // verbatim on a 307/308 (it strips only standard auth headers on a
            // cross-host hop, not the form body), so a malicious/MITM/open-
            // redirecting token endpoint could exfiltrate those credentials to
            // the redirect target. `hardened_client_builder()` (= redirect
            // Policy::none) closes that hole. The redirect-only build cannot fail
            // (see outbound::hardened_client_builder), so a build error is a
            // fail-loud invariant violation rather than a silent fall back to a
            // redirect-following client.
            // #8045 C3: this single `http` client is shared across every
            // configured provider, so it cannot statically commit to
            // `https_only` without breaking loopback dev/test IdPs (and the
            // loopback redirect regression test below). The credential-exfil-on-
            // 30x threat it guards is already closed by redirect=none; cleartext
            // egress to a remote token endpoint stays out of scope for the shared
            // builder here.
            http: crate::outbound::hardened_client_builder(
                crate::outbound::TransportPolicy::AllowLoopbackCleartext,
            )
            // #9504 review (A3): the token-exchange/refresh POSTs run while the
            // per-provider refresh lock is held, and `revoke()` now waits on
            // that lock — an untimed request would let a hung token endpoint
            // block credential revocation indefinitely. 30s bounds the stall.
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|error| {
                panic!(
                    "OAuth HTTP client must build with redirects disabled (#7068/#6892): {error}"
                )
            }),
            secret_store,
            providers: provider_map,
            active_flows: Arc::new(Mutex::new(HashMap::new())),
            refresh_locks: Mutex::new(HashMap::new()),
        }
    }

    /// Fetch (or lazily create) the per-provider refresh lock. The outer map
    /// lock is held only for the lookup/insert, never across a refresh.
    async fn refresh_lock_for(&self, provider_id: &str) -> Arc<Mutex<()>> {
        let mut locks = self.refresh_locks.lock().await;
        locks
            .entry(provider_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    fn get_provider(&self, provider_id: &str) -> Result<&OAuthProviderConfig, CoreError> {
        self.providers
            .get(provider_id)
            .ok_or_else(|| CoreError::OAuthError {
                code: maekon_core::error_codes::OAuthCode::Failed,
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
    /// Uses a **per-provider** `tokio::sync::Mutex` (via `try_lock`) to prevent
    /// concurrent refresh attempts for the *same* provider while allowing
    /// different providers to refresh independently. Returns a tri-state
    /// [`RefreshOutcome`] so callers can distinguish a contention skip (this
    /// provider's lock is held by a concurrent refresh) from a genuine failure
    /// (missing refresh token or failed exchange).
    async fn try_refresh(&self, provider_id: &str) -> Result<RefreshOutcome, CoreError> {
        let lock = self.refresh_lock_for(provider_id).await;
        // `try_lock_owned` (not `lock`) preserves the skip-on-contention
        // semantics: a refresh already running for THIS provider yields a
        // contention skip instead of blocking. The owned guard releases the
        // provider's lock on drop. Other providers are unaffected because each
        // has its own lock.
        let Ok(_guard) = lock.try_lock_owned() else {
            debug!("Refresh already in progress for {provider_id}, skipping");
            return Ok(RefreshOutcome::SkippedContention);
        };

        let config = self.get_provider(provider_id)?;
        let refresh_tok = self
            .secret_store
            .retrieve(provider_id, KEY_REFRESH_TOKEN)
            .await?;

        let Some(refresh_tok) = refresh_tok else {
            debug!("no refresh token stored for {provider_id}");
            return Ok(RefreshOutcome::Failed);
        };

        match token_exchange::refresh_token(&self.http, config, &refresh_tok).await {
            Ok(result) => {
                self.store_tokens(provider_id, &result).await?;
                info!("access token refreshed for {provider_id}");
                Ok(RefreshOutcome::Refreshed)
            }
            Err(e) => {
                warn!("token refresh failed for {provider_id}: {e}");
                Ok(RefreshOutcome::Failed)
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

impl Drop for OAuthClient {
    /// Abort all in-flight background tasks immediately on drop.
    ///
    /// Without this, background tasks would outlive the `OAuthClient` and
    /// attempt to lock `flows_ref` (a cloned `Arc`) after the client is gone.
    /// `try_lock` is used because `Drop` is synchronous — if the lock is
    /// currently held by an async task, that task will complete and notice
    /// the channel is closed on its own; abort is best-effort here.
    fn drop(&mut self) {
        if let Ok(flows) = self.active_flows.try_lock() {
            for flow in flows.values() {
                if let Some(ref h) = flow.handle {
                    h.abort();
                }
            }
        }
    }
}

#[async_trait]
impl OAuthPort for OAuthClient {
    async fn start_flow(&self, provider_id: &str) -> Result<OAuthFlowHandle, CoreError> {
        let config = self.get_provider(provider_id)?.clone();

        // Check port availability first
        if !callback_server::check_port_available(config.callback_port).await {
            return Err(CoreError::OAuthError {
                code: maekon_core::error_codes::OAuthCode::Failed,
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
        let flow_id = maekon_core::generate_id("flow");

        let auth_url = config
            .authorization_url(&state, &pkce.challenge)
            .map_err(|e| CoreError::OAuthError {
                code: maekon_core::error_codes::OAuthCode::Failed,
                provider: provider_id.into(),
                message: format!("invalid authorization endpoint URL: {e}"),
            })?;

        let (cancel_tx, cancel_rx) = oneshot::channel();

        // Spawn background task: callback server → token exchange → store
        let flow_id_bg = flow_id.clone();
        let provider_id_bg = provider_id.to_string();
        let flows_ref = self.active_flows.clone();
        let http = self.http.clone();
        let secret_store = self.secret_store.clone();
        let verifier = pkce.verifier.clone();
        // #9504 review (A1): pre-resolve THIS provider's refresh lock so the
        // spawned task can serialize its token commit against `revoke()` /
        // the refresh paths — the connect flow is the third writer of the
        // same namespace and was the one left unguarded.
        let refresh_lock = self.refresh_lock_for(provider_id).await;

        let handle = tokio::spawn(async move {
            // --- Phase 1: wait for the loopback callback (no lock held) ---
            let result =
                callback_server::wait_for_callback(config.callback_port, state, cancel_rx).await;

            // --- Phase 2: token exchange + storage WITHOUT holding flows_ref ---
            // The token exchange is an untimed network round-trip against the
            // OAuth provider. Holding the `active_flows` mutex across it would
            // block every other flow operation (flow_status, cancel_flow,
            // revoke, start_flow) for the entire duration of a slow or hung
            // provider. We therefore compute the resulting status into a local
            // and only re-acquire the lock briefly in Phase 3 to write it.
            // This mirrors the Phase-1/2/3 no-lock-across-await pattern used by
            // `refresh_coordinator::check_and_refresh`.
            let new_status = match result {
                Ok(callback) => {
                    match token_exchange::exchange_code(&http, &config, &callback.code, &verifier)
                        .await
                    {
                        Ok(tokens) => {
                            match commit_connect_tokens(
                                &refresh_lock,
                                &*secret_store,
                                &provider_id_bg,
                                &tokens,
                            )
                            .await
                            {
                                Ok(()) => {
                                    info!("OAuth flow completed for {provider_id_bg}");
                                    OAuthFlowStatus::Completed
                                }
                                Err(e) => {
                                    warn!("failed to store tokens: {e}");
                                    OAuthFlowStatus::Failed {
                                        error: format!("token storage failed: {e}"),
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            warn!("token exchange failed: {e}");
                            OAuthFlowStatus::Failed {
                                error: e.to_string(),
                            }
                        }
                    }
                }
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("cancelled") {
                        OAuthFlowStatus::Cancelled
                    } else {
                        OAuthFlowStatus::Failed { error: msg }
                    }
                }
            };

            // --- Phase 3: briefly re-acquire the lock only to write status ---
            // No `.await` other than the lock acquisition itself occurs while
            // the guard is held, so contention is bounded to a map lookup.
            {
                let mut flows = flows_ref.lock().await;
                if let Some(f) = flows.get_mut(&flow_id_bg) {
                    f.status = new_status;
                }
            }
        });

        // Store active flow — inserted AFTER spawn so the JoinHandle is
        // available immediately. The background task updates status in-place
        // under the lock only in its Phase 3 (after the callback wait AND the
        // token exchange), so inserting post-spawn is safe: this insert always
        // wins the lock first and registers the `Pending` entry before the task
        // can reach Phase 3.
        {
            let mut flows = self.active_flows.lock().await;
            flows.insert(
                flow_id.clone(),
                ActiveFlow {
                    provider_id: provider_id.to_string(),
                    pkce_verifier: pkce.verifier,
                    cancel_tx: Some(cancel_tx),
                    handle: Some(handle),
                    status: OAuthFlowStatus::Pending,
                },
            );
        }

        debug!("OAuth flow started: {flow_id} (provider: {provider_id})");

        Ok(OAuthFlowHandle { flow_id, auth_url })
    }

    async fn flow_status(&self, flow_id: &str) -> Result<OAuthFlowStatus, CoreError> {
        let mut flows = self.active_flows.lock().await;
        let status = flows
            .get(flow_id)
            .map(|f| f.status.clone())
            .ok_or_else(|| CoreError::OAuthError {
                code: maekon_core::error_codes::OAuthCode::Failed,
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
        match self.try_refresh(provider_id).await? {
            RefreshOutcome::Refreshed => {
                self.secret_store
                    .retrieve(provider_id, KEY_ACCESS_TOKEN)
                    .await
            }
            // A concurrent refresh held the guard. Do NOT report unauthenticated:
            // the token captured in step 1 is still within the 60s safety buffer
            // and may remain valid, so fall back to it rather than returning None.
            RefreshOutcome::SkippedContention => {
                debug!(
                    "refresh skipped due to contention for {provider_id}, \
                     returning previously-stored token"
                );
                Ok(token)
            }
            // 4. Token expired and refresh genuinely failed.
            RefreshOutcome::Failed => Ok(None),
        }
    }

    async fn revoke(&self, provider_id: &str) -> Result<(), CoreError> {
        info!("revoking OAuth credentials for {provider_id}");
        // #9504: serialize against every writer of this provider's namespace —
        // the two refresh paths (which hold the per-provider lock through
        // their network roundtrip AND store_tokens commit) and the connect
        // flow's `commit_connect_tokens`. Deleting the namespace around an
        // in-flight commit lets it land afterwards and re-persist the
        // just-revoked credentials (the #9481/#9491 resurrection shape —
        // TokenManager got the same treatment in #9499). Plain `lock().await`,
        // never `try_lock`: a revoke must wait out an in-flight commit, not be
        // skipped. The wait is bounded by the 30s OAuth http timeout.
        let lock = self.refresh_lock_for(provider_id).await;
        let _refresh_guard = lock.lock().await;
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
                if let Some(h) = flow.handle.take() {
                    h.abort();
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

        // Prevent concurrent refresh attempts for THIS provider (per-provider
        // lock shared with try_refresh). A refresh in flight for a *different*
        // provider does not block this one.
        let lock = self.refresh_lock_for(provider_id).await;
        let Ok(_guard) = lock.try_lock_owned() else {
            debug!("Refresh already in progress for {provider_id}, skipping");
            return Ok(RefreshResult::AlreadyFresh {
                expires_at: String::new(),
            });
        };

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
                code: maekon_core::error_codes::OAuthCode::RefreshFailed,
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
/// #9504 review (A1): the connect flow's token commit, serialized behind the
/// same per-provider refresh lock as `try_refresh` / `refresh_access_token` /
/// `revoke`. Without the guard, a revoke could interleave with the 4-write
/// store sequence and either resurrect just-revoked credentials or leave a
/// partial credential set. If the revoke wins the lock first, its flow-cleanup
/// `abort()` cancels this task while it is parked here — the commit then never
/// runs, which is the intended outcome.
async fn commit_connect_tokens(
    refresh_lock: &Arc<Mutex<()>>,
    secret_store: &dyn SecretStore,
    provider_id: &str,
    result: &token_exchange::TokenExchangeResult,
) -> Result<(), CoreError> {
    let _guard = refresh_lock.lock().await;
    store_tokens_static(secret_store, provider_id, result).await
}

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
        let ttl = expires_in.min(MAX_OAUTH_TTL_SECS) as i64;
        let expires_at = Utc::now() + chrono::Duration::seconds(ttl);
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
    use maekon_core::ports::secret_store::SecretStore as SecretStoreTrait;
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

    /// Build a provider config with a custom `provider_id` (and a unique
    /// callback port) for multi-provider tests.
    fn provider_with_id(provider_id: &str) -> OAuthProviderConfig {
        let mut config = OAuthProviderConfig::openai_codex();
        config.provider_id = provider_id.to_string();
        config.callback_port = next_test_port();
        config
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
        let err = client.start_flow("nonexistent").await.unwrap_err();
        assert!(
            matches!(
                err,
                CoreError::OAuthError {
                    code: maekon_core::error_codes::OAuthCode::Failed,
                    ..
                }
            ),
            "unknown provider must return CoreError::OAuthError::Failed, got: {err:?}"
        );
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

        // Hold the provider's per-provider refresh lock to simulate an
        // in-progress refresh for "openai".
        let held = client.refresh_lock_for("openai").await;
        let _held_guard = held.lock_owned().await;

        let result = client.try_refresh("openai").await.unwrap();
        assert_eq!(
            result,
            RefreshOutcome::SkippedContention,
            "try_refresh should report a contention skip when the provider's lock is held"
        );

        // _held_guard releases the lock on drop at end of scope.
    }

    /// Regression test for finding #12: the per-provider refresh lock must NOT
    /// serialize refreshes across DIFFERENT providers. A refresh in flight for
    /// provider A must not cause provider B's refresh to skip on contention.
    #[tokio::test]
    async fn refresh_lock_is_independent_per_provider() {
        let store = Arc::new(TestSecretStore::new());
        // Two distinct providers, each with a stored refresh token so
        // try_refresh proceeds past the "no refresh token" early return.
        let client = OAuthClient::new(
            store,
            vec![
                OAuthProviderConfig::openai_codex(),
                provider_with_id("other"),
            ],
        );

        // Simulate provider "openai" being mid-refresh by holding ITS lock.
        let openai_lock = client.refresh_lock_for("openai").await;
        let _openai_held = openai_lock.lock_owned().await;

        // Provider "openai" must report contention...
        assert_eq!(
            client.try_refresh("openai").await.unwrap(),
            RefreshOutcome::SkippedContention,
            "the provider whose lock is held must skip on contention"
        );

        // ...but provider "other" must be free to proceed. With no refresh
        // token stored it ends in Failed (NOT SkippedContention), proving its
        // lock was acquired independently of "openai".
        assert_eq!(
            client.try_refresh("other").await.unwrap(),
            RefreshOutcome::Failed,
            "a different provider must refresh independently, not skip on contention"
        );
    }

    /// Regression test for the OAuth refresh-contention bug (#6132), now
    /// per-provider: when a concurrent refresh holds the provider's refresh
    /// lock, `get_access_token` must fall back to the previously-stored token
    /// (still within the 60s safety buffer) instead of spuriously reporting
    /// `Ok(None)` ("not authenticated").
    #[tokio::test]
    async fn get_access_token_returns_stored_token_on_refresh_contention() {
        let store = Arc::new(TestSecretStore::new());
        // Store a token that is past the 60s safety buffer (so step 2's
        // `is_token_valid` returns false and step 3 attempts a refresh) but is
        // not yet actually expired — i.e. it may still be usable by the server.
        let expires = (Utc::now() + chrono::Duration::seconds(30)).to_rfc3339();
        store
            .store("openai", KEY_ACCESS_TOKEN, "tok_buffer")
            .await
            .unwrap();
        store
            .store("openai", KEY_EXPIRES_AT, &expires)
            .await
            .unwrap();

        let client = make_client(store);

        // Simulate a concurrent refresh already holding the provider's lock.
        let held = client.refresh_lock_for("openai").await;
        let _held_guard = held.lock_owned().await;

        let token = client.get_access_token("openai").await.unwrap();
        assert_eq!(
            token,
            Some("tok_buffer".to_string()),
            "contention skip must fall back to the stored token, not return None"
        );
    }

    /// Confirms the genuine-unauthenticated path is preserved: when the token
    /// is past the safety buffer, no refresh token is stored, and there is no
    /// contention, `get_access_token` still returns `None`.
    #[tokio::test]
    async fn get_access_token_returns_none_when_refresh_genuinely_fails() {
        let store = Arc::new(TestSecretStore::new());
        let expires = (Utc::now() + chrono::Duration::seconds(30)).to_rfc3339();
        store
            .store("openai", KEY_ACCESS_TOKEN, "tok_expired")
            .await
            .unwrap();
        store
            .store("openai", KEY_EXPIRES_AT, &expires)
            .await
            .unwrap();
        // No refresh token stored → try_refresh returns Failed (not contention).

        let client = make_client(store);
        let token = client.get_access_token("openai").await.unwrap();
        assert!(
            token.is_none(),
            "genuine refresh failure (no refresh token) must return None"
        );
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

    /// Verifies that dropping `OAuthClient` with an in-flight flow aborts the
    /// background task immediately (best-effort via `try_lock`).  We cannot
    /// observe abort directly in a unit test without spawning a real callback
    /// server, so we verify the structural invariant: a flow registered in
    /// `active_flows` has a `Some(handle)` that is not yet finished before
    /// drop, and the drop path calls `abort()` without panicking.
    #[tokio::test]
    async fn oauth_manager_drop_aborts_active_flow_tasks() {
        let store = Arc::new(TestSecretStore::new());
        let client = make_client_with_test_port(store);

        // Start a flow — this spawns the background task and registers the handle.
        let handle = client.start_flow("openai").await.unwrap();
        let flow_id = handle.flow_id.clone();

        // The flow should be registered with a handle.
        {
            let flows = client.active_flows.lock().await;
            let flow = flows.get(&flow_id).expect("flow registered");
            assert!(
                flow.handle.is_some(),
                "background task handle must be stored in ActiveFlow"
            );
            // Task should still be running (waiting for callback).
            if let Some(ref h) = flow.handle {
                assert!(!h.is_finished(), "task should still be running");
            }
        }

        // Drop the client — Drop impl calls h.abort() on all active flows.
        // This must not panic.
        drop(client);

        // Give the runtime one yield to process the abort signal.
        tokio::task::yield_now().await;
        // If we reach here without panic the Drop impl is correct.
    }

    /// Regression test for finding #6208: the OAuth `start_flow` background
    /// task must NOT hold the `active_flows` mutex across the untimed
    /// token-exchange `.await`. A slow or hung OAuth provider previously froze
    /// every other `active_flows` operation (flow_status, cancel_flow, revoke,
    /// start_flow) for the entire exchange duration.
    ///
    /// This test reproduces the fixed Phase-1/2/3 lock discipline directly on
    /// the real `OAuthClient::active_flows` map and `ActiveFlow` type: a
    /// background task performs a slow await (standing in for the network
    /// round-trip) while holding NO lock, then briefly re-acquires the lock
    /// only to write the terminal status. We assert that, while the slow phase
    /// is still pending, another task can acquire the lock and read the flow
    /// status promptly — which is only possible if the lock is not held across
    /// the await.
    #[tokio::test]
    async fn start_flow_task_does_not_hold_lock_across_exchange() {
        use std::sync::Arc as StdArc;
        use tokio::sync::Notify;

        let store = Arc::new(TestSecretStore::new());
        let client = make_client(store);
        let flow_id = "flow-test-6208".to_string();

        // Register a Pending flow as `start_flow` would, but without a real
        // callback server (we only exercise the lock-discipline of the
        // status write-back phase).
        {
            let mut flows = client.active_flows.lock().await;
            flows.insert(
                flow_id.clone(),
                ActiveFlow {
                    provider_id: "openai".into(),
                    pkce_verifier: "verifier".into(),
                    cancel_tx: None,
                    handle: None,
                    status: OAuthFlowStatus::Pending,
                },
            );
        }

        // `gate` stands in for the slow/hung token exchange: the background
        // task awaits it WITHOUT holding `flows_ref` (Phase 2), exactly as the
        // fixed code does between `wait_for_callback` and the status write.
        let gate = StdArc::new(Notify::new());
        let flows_ref = client.active_flows.clone();
        let flow_id_bg = flow_id.clone();
        let gate_bg = gate.clone();

        let handle = tokio::spawn(async move {
            // Phase 2: slow exchange — no lock held.
            gate_bg.notified().await;
            // Phase 3: briefly re-acquire the lock only to write status.
            let mut flows = flows_ref.lock().await;
            if let Some(f) = flows.get_mut(&flow_id_bg) {
                f.status = OAuthFlowStatus::Completed;
            }
        });

        // Yield so the spawned task reaches `gate_bg.notified().await` (its
        // Phase-2 slow wait) before we probe the lock.
        tokio::task::yield_now().await;

        // While the simulated exchange is still pending, acquiring the lock and
        // reading status must succeed promptly. A short timeout guards against
        // the pre-fix regression where the lock would be held for the whole
        // exchange. If the lock were held across the await this would time out.
        let probe = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            let flows = client.active_flows.lock().await;
            flows.get(&flow_id).map(|f| f.status.clone())
        })
        .await
        .expect("active_flows lock must be available during a slow exchange");
        assert_eq!(
            probe,
            Some(OAuthFlowStatus::Pending),
            "flow must still be Pending while the exchange is in flight"
        );

        // Complete the simulated exchange and let Phase 3 write the status.
        gate.notify_one();
        handle.await.expect("background task must finish");

        let status = client.flow_status(&flow_id).await.unwrap();
        assert_eq!(
            status,
            OAuthFlowStatus::Completed,
            "Phase 3 must write the terminal status after the exchange completes"
        );
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

    /// #7068/#6892 regression: the OAuth HTTP client must NOT follow 30x
    /// redirects. The credential-bearing token-exchange/refresh POSTs reuse
    /// `self.http` (token_exchange::exchange_code/refresh_token), and reqwest's
    /// default policy re-sends the request body verbatim on a 307/308, so a
    /// malicious/MITM/open-redirecting token endpoint could exfiltrate the
    /// authorization code + PKCE verifier and the long-lived refresh_token. The
    /// client is built via `hardened_client_builder()` (redirect=none), so a 30x
    /// must be returned as-is and the redirect target must never be contacted.
    /// This test fails before the fix (bare `reqwest::Client::new()` follows the
    /// 307 and re-POSTs the credential body to `/leaked`).
    #[tokio::test]
    async fn oauth_http_client_does_not_follow_redirects() {
        let mut server = mockito::Server::new_async().await;
        let start = server
            .mock("POST", "/token")
            .with_status(307)
            .with_header("location", "/leaked")
            .create_async()
            .await;
        // Would be reached only if the redirect were followed (and the credential
        // body re-sent) — must be called 0 times.
        let leaked = server
            .mock("POST", "/leaked")
            .with_status(200)
            .with_body("LEAKED")
            .expect(0)
            .create_async()
            .await;

        let store = Arc::new(TestSecretStore::new());
        let client = make_client(store);

        // Same-module test: access the private `http` field directly and POST a
        // credential-shaped form body to the redirecting endpoint, mirroring
        // token_exchange's `refresh_token` request.
        let resp = client
            .http
            .post(format!("{}/token", server.url()))
            .form(&[("refresh_token", "secret-rt"), ("client_id", "cid")])
            .send()
            .await
            .expect("request must be sent");

        assert_eq!(
            resp.status().as_u16(),
            307,
            "OAuth client must return the 307 as-is, not follow it"
        );
        start.assert_async().await;
        // expect(0): confirms the credential body was never re-sent to /leaked.
        leaked.assert_async().await;
    }
}
