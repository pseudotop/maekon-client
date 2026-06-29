//! Token types, in-memory state, and the `get_token` / `is_authenticated` accessors.
//!
//! Internal types (`TokenResponse`, `TokenState`) remain private to the `auth`
//! module family.  Only `TokenManager` is re-exported from `auth/mod.rs`.

use chrono::{DateTime, Duration, Utc};
use maekon_core::error::CoreError;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing::warn;

use crate::http_client::build_reqwest_client;
use maekon_core::config::TlsConfig;

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
}

impl TokenManager {
    // ── Constructors ──────────────────────────────────────────────────────────

    /// Legacy constructor — uses a default `reqwest::Client` with no TLS policy.
    ///
    /// Prefer [`TokenManager::new_with_tls`] in production code so that the
    /// same TLS settings (HTTPS-only, no certificate-validation bypass) are applied to
    /// credential requests as to all other network calls.
    ///
    /// This constructor is retained for backward compatibility and unit tests
    /// that talk to `mockito` HTTP servers.
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
            client: crate::outbound::hardened_client_builder().build().expect(
                "TokenManager HTTP client must build with redirects disabled (#7068/#6892)",
            ),
            state: Arc::new(RwLock::new(None)),
            refresh_lock: Arc::new(Mutex::new(())),
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
