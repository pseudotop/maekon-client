//! Token types, in-memory state, and the `get_token` / `is_authenticated` accessors.
//!
//! Internal types (`TokenResponse`, `TokenState`) remain private to the `auth`
//! module family.  Only `TokenManager` is re-exported from `auth/mod.rs`.

use chrono::{DateTime, Duration, Utc};
use maekon_core::error::CoreError;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing::warn;

use crate::http_client::build_reqwest_client;
use maekon_core::config::TlsConfig;
use maekon_core::ports::secret_store::{
    SecretStore, ONESHIM_ACCESS_TOKEN_SECRET_KEY, ONESHIM_AUTH_SECRET_NAMESPACE,
    ONESHIM_EXPIRES_AT_SECRET_KEY, ONESHIM_IDENTIFIER_SECRET_KEY,
    ONESHIM_ORGANIZATION_ID_SECRET_KEY, ONESHIM_REFRESH_TOKEN_SECRET_KEY,
};

use crate::error::NetworkError;

// ── Constants ───────────────────────────────────────────────────────────────

/// Upper bound (in seconds) for a server-supplied token TTL (`expires_in`).
///
/// `expires_in` is server-controlled; feeding an adversarial/huge value (e.g.
/// `i64::MAX`) straight into `chrono::Duration::seconds()` panics (TimeDelta
/// out-of-range) or overflows the subsequent `Utc::now() + Duration` addition.
/// Clamping to 30 days keeps the resulting `Duration`/`DateTime` trivially in
/// range while remaining far longer than any realistic access-token lifetime.
pub(super) const MAX_TOKEN_TTL_SECS: i64 = 60 * 60 * 24 * 30;

// ── Wire types ────────────────────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
pub(super) struct TokenResponse {
    pub(super) access_token: String,
    pub(super) refresh_token: Option<String>,
    pub(super) expires_in: Option<i64>,
}

#[derive(Debug, Clone)]
pub(super) struct TokenState {
    pub(super) access_token: String,
    pub(super) refresh_token: Option<String>,
    pub(super) expires_at: DateTime<Utc>,
    /// Login identifier (username/email) the session was issued for. Carried
    /// across refreshes — the server returns token material only.
    pub(super) identifier: Option<String>,
    pub(super) organization_id: Option<String>,
}

/// Non-secret session metadata for UI status display.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub identifier: Option<String>,
    pub organization_id: Option<String>,
}

// ── TokenManager struct ───────────────────────────────────────────────────────

#[derive(Clone)]
pub struct TokenManager {
    pub(super) base_url: String,
    pub(super) client: reqwest::Client,
    pub(super) state: Arc<RwLock<Option<TokenState>>>,
    /// Serializes concurrent auto-refreshes triggered from [`get_token`] so that
    /// only one `/refresh` POST fires per expiry window.  Without it, many callers
    /// hitting `get_token` near expiry (e.g. concurrent gRPC RPCs that each fetch a
    /// token) would race to refresh with the same refresh token, which a rotating
    /// server rejects on the second use.  Shared across clones via `Arc`.
    pub(super) refresh_lock: Arc<Mutex<()>>,
    /// Optional durable backing for the session, attached via
    /// [`TokenManager::with_persistence`]. `None` keeps the manager purely
    /// in-memory, which is the behaviour every pre-#9459 call site relies on.
    pub(super) persistence: Option<Arc<dyn SecretStore>>,
    /// Serializes [`persist_current_state`] so a persist that is already in
    /// flight cannot write its snapshot after a concurrent one finished. A
    /// background refresh and a user logout otherwise interleave as: refresh
    /// snapshots the new session, logout clears the state and completes
    /// `delete_namespace`, then the refresh's remaining writes land — leaving a
    /// logged-out client with a full session in the keychain that the next
    /// launch happily restores. Because the snapshot is taken *inside* this
    /// lock, whichever persist runs last always observes the true current
    /// state, so the store converges on it.
    ///
    /// Deliberately NOT `refresh_lock`: [`get_token`] holds that one across
    /// `refresh()`, which itself calls `persist_current_state` — reusing it
    /// would deadlock, since `tokio::sync::Mutex` is not reentrant.
    ///
    /// [`persist_current_state`]: TokenManager::persist_current_state
    /// [`get_token`]: TokenManager::get_token
    pub(super) persist_lock: Arc<Mutex<()>>,
    /// Monotonic session-generation (epoch) counter (#9491).
    ///
    /// Every session transition — `login_with_org`, `logout`,
    /// `logout_all_sessions`, [`restore_persisted_session`] — bumps this via
    /// [`bump_session_generation`] **while holding the `state` write lock**, so
    /// any observer that reads state and generation under the same lock always
    /// sees the two move together. `refresh()` captures the generation at entry
    /// (under the state read lock, alongside the snapshot it carries forward)
    /// and re-reads it under the state write lock before committing the
    /// rotation; a different value means the session it was refreshing no
    /// longer exists, so the rotated tokens are discarded.
    ///
    /// This subsumes a bare `state.is_none()` post-logout check, which only
    /// catches logout -> `None`. The interleaving it misses is logout ->
    /// re-login: a stale refresh landing after the user has signed back in
    /// (possibly as a *different* account) would otherwise overwrite the new
    /// session's tokens, identifier, and organization — in memory and in the
    /// keychain — with the previous account's.
    ///
    /// Shared across clones via `Arc`, like `state` itself: a refresh spawned
    /// off one clone must observe the logout/login performed through another.
    ///
    /// [`restore_persisted_session`]: TokenManager::restore_persisted_session
    /// [`bump_session_generation`]: TokenManager::bump_session_generation
    pub(super) session_generation: Arc<AtomicU64>,
}

impl TokenManager {
    // ── Constructors ──────────────────────────────────────────────────────────

    /// Legacy constructor — uses a default `reqwest::Client` with no TLS policy.
    ///
    /// **Test and example code only.** Every production wiring call site
    /// (`build_server_transports` in `src-tauri/src/agent_runtime_support.rs`,
    /// and — since #7733 — `build_suggestion_manager` in
    /// `src-tauri/src/app_runtime_launch/suggestion_wiring.rs`) uses
    /// [`TokenManager::new_with_tls`] exclusively and fails loud instead of
    /// falling back to this constructor on a `[tls]` config error. Do NOT add
    /// a new production fallback onto this constructor — silently downgrading
    /// TLS policy enforcement on a privacy product is a security regression,
    /// not a graceful degrade. It remains available for unit/integration tests
    /// and examples that talk to `mockito`/loopback stub HTTP servers where
    /// `[tls]` policy enforcement is not the thing under test.
    #[deprecated(note = "Use new_with_tls() for TLS enforcement")]
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            // #7068/#6892: even this deprecated/test-only constructor builds a
            // credential-bearing client — TokenManager POSTs the login password
            // and the server refresh_token in the request body (auth/refresh.rs).
            // Build it via the hardened builder (redirect Policy::none) so the
            // by-construction invariant holds across every TokenManager
            // constructor. The redirect-only build cannot fail, so a build error
            // is a fail-loud invariant violation rather than a silent fall back
            // to a redirect-following client.
            // #8045 C3: even this deprecated/test constructor gets the https_only
            // backstop derived from `base_url` — loopback stub servers (mockito)
            // keep cleartext, any remote base_url is HTTPS-only.
            client: maekon_http_core::outbound::hardened_client_builder(
                maekon_http_core::outbound::TransportPolicy::for_endpoint(base_url),
            )
                .build()
                .unwrap_or_else(|error| {
                    panic!(
                        "TokenManager HTTP client must build with redirects disabled (#7068/#6892): {error}"
                    )
                }),
            state: Arc::new(RwLock::new(None)),
            refresh_lock: Arc::new(Mutex::new(())),
            persistence: None,
            persist_lock: Arc::new(Mutex::new(())),
            session_generation: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Preferred production constructor — accepts a pre-built `reqwest::Client`
    /// so the caller can apply the canonical TLS policy via
    /// [`build_reqwest_client`].
    ///
    /// # Example
    /// ```no_run
    /// use maekon_network::auth::TokenManager;
    /// use maekon_network::http_client::build_reqwest_client;
    /// use maekon_core::config::TlsConfig;
    ///
    /// let tls = TlsConfig::default();
    /// let client = build_reqwest_client(&tls, None).unwrap();
    /// let tm = TokenManager::new_with_client("https://api.example.com", client);
    /// ```
    pub fn new_with_client(base_url: &str, client: reqwest::Client) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client,
            state: Arc::new(RwLock::new(None)),
            refresh_lock: Arc::new(Mutex::new(())),
            persistence: None,
            persist_lock: Arc::new(Mutex::new(())),
            session_generation: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Convenience constructor that builds a TLS-aware `reqwest::Client`
    /// internally using the canonical [`build_reqwest_client`] helper.
    ///
    /// `timeout` is applied as a per-request timeout.  Pass `None` to omit a
    /// global timeout (not recommended for auth endpoints — they should be
    /// short-lived).
    pub fn new_with_tls(
        base_url: &str,
        tls: &TlsConfig,
        timeout: Option<std::time::Duration>,
    ) -> Result<Self, NetworkError> {
        let client = build_reqwest_client(tls, timeout)?;
        Ok(Self::new_with_client(base_url, client))
    }

    // ── Session generation (#9491) ────────────────────────────────────────────

    /// Record that the session was replaced (logged in, logged out, restored).
    ///
    /// **Call site contract:** invoke this while holding the `state` write lock
    /// that performs the transition, never before taking it or after releasing
    /// it. Anything else lets a reader pair a new state with an old generation
    /// (or vice versa), which is exactly the window
    /// [`session_generation`] exists to close.
    ///
    /// [`session_generation`]: TokenManager::session_generation
    pub(super) fn bump_session_generation(&self) {
        self.session_generation.fetch_add(1, Ordering::SeqCst);
    }

    // ── Session persistence (#9459) ───────────────────────────────────────────

    /// Builder-style: attach a `SecretStore` so login/refresh/logout persist the
    /// session material under `oneshim/auth/default`. Call before Arc-wrapping.
    #[must_use]
    pub fn with_persistence(mut self, store: Arc<dyn SecretStore>) -> Self {
        self.persistence = Some(store);
        self
    }

    /// Best-effort persistence of the current in-memory session. Failures are
    /// warn-logged, never propagated — auth must not fail because keychain I/O did.
    ///
    /// Serialized on [`persist_lock`]: the guard is taken *before* the snapshot
    /// so a concurrent logout can neither interleave its `delete_namespace`
    /// between these writes nor be overwritten by them.
    ///
    /// [`persist_lock`]: TokenManager::persist_lock
    pub(super) async fn persist_current_state(&self) {
        let Some(store) = self.persistence.as_ref() else {
            return;
        };
        let _persist_guard = self.persist_lock.lock().await;
        let snapshot = { self.state.read().await.clone() };
        let result: Result<(), CoreError> = async {
            match snapshot {
                Some(s) => {
                    store
                        .store(
                            ONESHIM_AUTH_SECRET_NAMESPACE,
                            ONESHIM_ACCESS_TOKEN_SECRET_KEY,
                            &s.access_token,
                        )
                        .await?;
                    match s.refresh_token.as_deref() {
                        Some(rt) => {
                            store
                                .store(
                                    ONESHIM_AUTH_SECRET_NAMESPACE,
                                    ONESHIM_REFRESH_TOKEN_SECRET_KEY,
                                    rt,
                                )
                                .await?
                        }
                        None => {
                            store
                                .delete(
                                    ONESHIM_AUTH_SECRET_NAMESPACE,
                                    ONESHIM_REFRESH_TOKEN_SECRET_KEY,
                                )
                                .await?
                        }
                    }
                    store
                        .store(
                            ONESHIM_AUTH_SECRET_NAMESPACE,
                            ONESHIM_EXPIRES_AT_SECRET_KEY,
                            &s.expires_at.to_rfc3339(),
                        )
                        .await?;
                    match s.identifier.as_deref() {
                        Some(v) => {
                            store
                                .store(
                                    ONESHIM_AUTH_SECRET_NAMESPACE,
                                    ONESHIM_IDENTIFIER_SECRET_KEY,
                                    v,
                                )
                                .await?
                        }
                        None => {
                            store
                                .delete(
                                    ONESHIM_AUTH_SECRET_NAMESPACE,
                                    ONESHIM_IDENTIFIER_SECRET_KEY,
                                )
                                .await?
                        }
                    }
                    match s.organization_id.as_deref() {
                        Some(v) => {
                            store
                                .store(
                                    ONESHIM_AUTH_SECRET_NAMESPACE,
                                    ONESHIM_ORGANIZATION_ID_SECRET_KEY,
                                    v,
                                )
                                .await?
                        }
                        None => {
                            store
                                .delete(
                                    ONESHIM_AUTH_SECRET_NAMESPACE,
                                    ONESHIM_ORGANIZATION_ID_SECRET_KEY,
                                )
                                .await?
                        }
                    }
                    Ok(())
                }
                None => store.delete_namespace(ONESHIM_AUTH_SECRET_NAMESPACE).await,
            }
        }
        .await;
        if let Err(e) = result {
            warn!(
                err.code = %e.code(),
                "session persistence write failed (auth state unaffected)"
            );
        }
    }

    /// Load a previously persisted session into memory. Returns `true` when a
    /// usable session was restored — either a still-valid access token, or an
    /// expired one that carries a refresh token, in which case the caller should
    /// expect [`is_authenticated`] to stay `false` until the first
    /// [`get_token`] triggers a refresh. Corrupt or unrefreshable-expired
    /// material is ignored (and the caller may re-login); it is NOT deleted here
    /// to keep restore side-effect-free.
    ///
    /// [`is_authenticated`]: TokenManager::is_authenticated
    /// [`get_token`]: TokenManager::get_token
    pub async fn restore_persisted_session(&self) -> bool {
        let Some(store) = self.persistence.as_ref() else {
            return false;
        };
        let read = async {
            let access = store
                .retrieve(
                    ONESHIM_AUTH_SECRET_NAMESPACE,
                    ONESHIM_ACCESS_TOKEN_SECRET_KEY,
                )
                .await?;
            let refresh = store
                .retrieve(
                    ONESHIM_AUTH_SECRET_NAMESPACE,
                    ONESHIM_REFRESH_TOKEN_SECRET_KEY,
                )
                .await?;
            let expires = store
                .retrieve(ONESHIM_AUTH_SECRET_NAMESPACE, ONESHIM_EXPIRES_AT_SECRET_KEY)
                .await?;
            let identifier = store
                .retrieve(ONESHIM_AUTH_SECRET_NAMESPACE, ONESHIM_IDENTIFIER_SECRET_KEY)
                .await?;
            let organization_id = store
                .retrieve(
                    ONESHIM_AUTH_SECRET_NAMESPACE,
                    ONESHIM_ORGANIZATION_ID_SECRET_KEY,
                )
                .await?;
            Ok::<_, CoreError>((access, refresh, expires, identifier, organization_id))
        }
        .await;
        let (Some(access_token), refresh_token, Some(expires_raw), identifier, organization_id) =
            (match read {
                Ok(v) => v,
                Err(e) => {
                    warn!(err.code = %e.code(), "session restore read failed");
                    return false;
                }
            })
        else {
            return false;
        };
        let Some(expires_at) = DateTime::parse_from_rfc3339(&expires_raw)
            .ok()
            .map(|v| v.with_timezone(&Utc))
        else {
            warn!("session restore skipped: unparsable expires_at");
            return false;
        };
        // A session that can still refresh is worth restoring even when the
        // access token itself is at/near expiry; only drop it when there is no
        // refresh token AND the access token is already expired.
        if refresh_token.is_none() && Utc::now() >= expires_at {
            return false;
        }
        let mut state = self.state.write().await;
        *state = Some(TokenState {
            access_token,
            refresh_token,
            expires_at,
            identifier,
            organization_id,
        });
        // #9491: a restore replaces whatever session was live, so any refresh
        // already in flight against the previous one must not commit.
        self.bump_session_generation();
        true
    }

    /// Non-secret metadata of the current session, for status display.
    pub async fn session_info(&self) -> Option<SessionInfo> {
        let state = self.state.read().await;
        state.as_ref().map(|s| SessionInfo {
            identifier: s.identifier.clone(),
            organization_id: s.organization_id.clone(),
        })
    }

    // ── Token accessors ───────────────────────────────────────────────────────

    /// Return a valid access token, triggering a background refresh when the
    /// token is within 5 minutes of expiry.
    pub async fn get_token(&self) -> Result<String, CoreError> {
        let needs_refresh = {
            let state = self.state.read().await;
            match &*state {
                Some(s) => Utc::now() + Duration::minutes(5) >= s.expires_at,
                None => {
                    return Err(CoreError::Auth {
                        code: maekon_core::error_codes::AuthCode::Failed,
                        message: "Not authenticated".to_string(),
                    })
                }
            }
        };

        if needs_refresh {
            // Single-flight: serialize concurrent auto-refreshes so only one
            // `/refresh` POST fires per expiry window.  Multiple callers near
            // expiry (e.g. concurrent gRPC RPCs that each call `get_token`)
            // would otherwise race to refresh with the same refresh token, which
            // a rotating server rejects on the second use.
            let _refresh_guard = self.refresh_lock.lock().await;

            // Double-checked: another caller may have refreshed while we waited
            // for the lock — re-read expiry and skip the round-trip if fresh.
            let still_needs_refresh = {
                let state = self.state.read().await;
                match &*state {
                    Some(s) => Utc::now() + Duration::minutes(5) >= s.expires_at,
                    None => true,
                }
            };

            if still_needs_refresh {
                self.refresh().await.map_err(|e| {
                    warn!("token refresh failure: {e}");
                    CoreError::Auth {
                        code: maekon_core::error_codes::AuthCode::Failed,
                        message: format!("Automatic token refresh failed: {e}"),
                    }
                })?;
            }
        }

        let state = self.state.read().await;
        state
            .as_ref()
            .map(|s| s.access_token.clone())
            .ok_or_else(|| CoreError::Auth {
                code: maekon_core::error_codes::AuthCode::Failed,
                message: "Not authenticated".to_string(),
            })
    }

    /// Returns `true` when a non-expired token is present in memory.
    pub async fn is_authenticated(&self) -> bool {
        let state = self.state.read().await;
        state.as_ref().is_some_and(|s| Utc::now() < s.expires_at)
    }
}
