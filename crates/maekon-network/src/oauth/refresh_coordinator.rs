//! Token refresh coordinator — orchestrates automatic access-token renewal
//! with failure tracking, backoff, and event emission.
//!
//! This is an orchestration utility, NOT a port adapter. It wraps an
//! `OAuthPort` implementation and adds retry/backoff/event-emission
//! semantics on top of the raw `refresh_access_token` call.
//!
//! # Per-provider isolation
//!
//! State (in_progress / backoff_until / consecutive_transient_failures) is
//! tracked independently for each provider. A 1-year backoff on provider A
//! does NOT block refresh attempts on provider B.
//!
//! # NotAuthenticated vs ReauthRequired
//!
//! `RefreshResult::NotAuthenticated` means the secret store has no token at
//! all for that provider — the user has never connected it.  This is a silent
//! no-op: no `ReauthRequired` event is fired, no alert is shown.
//!
//! `RefreshResult::ReauthRequired` (or three consecutive transient failures)
//! means the provider HAD a token that has since become invalid — the user
//! must reconnect. Only this case fires `TokenEvent::ReauthRequired`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{broadcast, Mutex};
use tracing::{debug, info, warn};

use maekon_core::ports::oauth::{OAuthPort, RefreshResult, TokenEvent};

/// Maximum consecutive transient failures before escalating to reauth.
const MAX_CONSECUTIVE_FAILURES: u8 = 3;

/// Seconds before expiry at which a refresh is considered necessary.
const REFRESH_THRESHOLD_SECS: i64 = 300;

/// Outcome of a `check_and_refresh` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshOutcome {
    /// Token is still fresh — no refresh was attempted.
    NotNeeded,
    /// Token was successfully refreshed.
    Refreshed,
    /// Refresh failed on a transient error (attempt count attached).
    Failed { attempt: u8 },
    /// User must re-authenticate (terminal failure or too many transients).
    ///
    /// Only produced when the provider had a token that became invalid.
    /// Never produced for the `NotAuthenticated` (never connected) case.
    ReauthRequired,
    /// Provider has never been connected — no token exists in the store.
    ///
    /// Distinct from `ReauthRequired`: no notification or Tauri event is
    /// emitted. The coordinator records a long backoff so the loop does not
    /// busy-poll an unconnected provider.
    NotConnected,
    /// Another refresh is already in progress.
    AlreadyInProgress,
    /// Backoff period has not elapsed since the last failure.
    BackingOff,
}

/// Per-provider mutable state.
struct ProviderRefreshState {
    in_progress: bool,
    consecutive_transient_failures: u8,
    last_attempt: Option<Instant>,
    backoff_until: Option<Instant>,
}

impl ProviderRefreshState {
    fn new() -> Self {
        Self {
            in_progress: false,
            consecutive_transient_failures: 0,
            last_attempt: None,
            backoff_until: None,
        }
    }

    /// Calculate backoff duration based on the current failure count.
    ///
    /// Returns `None` when the failure count has reached the maximum,
    /// signalling that the caller should escalate to reauth instead.
    fn backoff_duration(failures: u8) -> Option<Duration> {
        match failures {
            1 => Some(Duration::from_secs(120)),
            2 => Some(Duration::from_secs(300)),
            _ => None,
        }
    }
}

/// Guards the per-provider state map.  All providers share a single `Mutex`
/// wrapping a `HashMap`; the per-provider entry is fetched (or inserted) by
/// key each time.  The Phase-1 lock pattern is preserved: lock is acquired to
/// check preconditions, released before the network call, then re-acquired to
/// process results.
struct RefreshStateMap(HashMap<String, ProviderRefreshState>);

impl RefreshStateMap {
    fn new() -> Self {
        Self(HashMap::new())
    }

    fn entry(&mut self, provider_id: &str) -> &mut ProviderRefreshState {
        self.0
            .entry(provider_id.to_string())
            .or_insert_with(ProviderRefreshState::new)
    }

    /// Clear all provider states (used by `reset()`).
    fn clear(&mut self) {
        self.0.clear();
    }
}

/// Coordinates automatic token refresh with per-provider failure tracking
/// and backoff.
pub struct TokenRefreshCoordinator {
    oauth_port: Arc<dyn OAuthPort>,
    state: Mutex<RefreshStateMap>,
    event_tx: broadcast::Sender<TokenEvent>,
}

impl TokenRefreshCoordinator {
    /// Create a new coordinator wrapping the given `OAuthPort`.
    pub fn new(oauth_port: Arc<dyn OAuthPort>, event_tx: broadcast::Sender<TokenEvent>) -> Self {
        Self {
            oauth_port,
            state: Mutex::new(RefreshStateMap::new()),
            event_tx,
        }
    }

    /// Subscribe to token lifecycle events emitted by this coordinator.
    pub fn subscribe(&self) -> broadcast::Receiver<TokenEvent> {
        self.event_tx.subscribe()
    }

    /// Reset internal state after successful manual re-authentication.
    ///
    /// Clears backoff timers and failure counters for ALL providers so the
    /// background refresh loop resumes normal operation immediately.
    /// Signature is unchanged — callers at `src-tauri/src/commands/integration.rs`
    /// are not affected.
    pub async fn reset(&self) {
        let mut state = self.state.lock().await;
        state.clear();
    }

    /// Check whether a refresh is needed for `provider_id` and perform it if so.
    ///
    /// State is tracked independently per provider: backoff or in-progress on
    /// one provider does not affect any other provider.
    ///
    /// Returns `RefreshOutcome::NotConnected` (silently) when the provider has
    /// never been authenticated.  `RefreshOutcome::ReauthRequired` is returned
    /// — and `TokenEvent::ReauthRequired` is fired — only when a token that
    /// previously existed has become invalid.
    pub async fn check_and_refresh(&self, provider_id: &str) -> RefreshOutcome {
        // --- Phase 1: acquire lock, check per-provider preconditions ---
        {
            let mut state_map = self.state.lock().await;
            let ps = state_map.entry(provider_id);

            if ps.in_progress {
                debug!("refresh already in progress for {provider_id}");
                return RefreshOutcome::AlreadyInProgress;
            }

            if let Some(until) = ps.backoff_until {
                if Instant::now() < until {
                    debug!("backing off refresh for {provider_id}");
                    return RefreshOutcome::BackingOff;
                }
                // Backoff period elapsed — clear it.
                ps.backoff_until = None;
            }

            ps.in_progress = true;
            ps.last_attempt = Some(Instant::now());
        }
        // Lock released — the actual network call happens without holding it.

        // --- Phase 2: call the port ---
        let result = self
            .oauth_port
            .refresh_access_token(provider_id, REFRESH_THRESHOLD_SECS)
            .await;

        // --- Phase 3: re-acquire lock, process result per provider ---
        let mut state_map = self.state.lock().await;
        let ps = state_map.entry(provider_id);
        ps.in_progress = false;

        match result {
            Ok(RefreshResult::AlreadyFresh { .. }) => {
                if ps.consecutive_transient_failures > 0 {
                    info!(
                        "auto-recovery: resetting failure counter for {provider_id} \
                         (was {})",
                        ps.consecutive_transient_failures
                    );
                    ps.consecutive_transient_failures = 0;
                    ps.backoff_until = None;
                }
                RefreshOutcome::NotNeeded
            }

            Ok(RefreshResult::Refreshed { expires_at }) => {
                ps.consecutive_transient_failures = 0;
                ps.backoff_until = None;
                let _ = self.event_tx.send(TokenEvent::Refreshed {
                    provider_id: provider_id.to_string(),
                    expires_at,
                });
                RefreshOutcome::Refreshed
            }

            Ok(RefreshResult::NotAuthenticated) => {
                // The provider was never connected — no token exists.
                // This is NOT a re-authentication request: do NOT fire
                // ReauthRequired, do NOT show a notification.
                // Record a long backoff so the loop does not busy-poll.
                ps.backoff_until = Some(Instant::now() + Duration::from_secs(86400 * 365));
                RefreshOutcome::NotConnected
            }

            Ok(RefreshResult::ReauthRequired { kind, reason }) => {
                // Terminal — token existed but is now invalid.
                warn!("terminal refresh failure for {provider_id}: [{kind:?}] {reason}");
                ps.backoff_until = Some(Instant::now() + Duration::from_secs(86400 * 365));
                let _ = self.event_tx.send(TokenEvent::ReauthRequired {
                    provider_id: provider_id.to_string(),
                });
                RefreshOutcome::ReauthRequired
            }

            Ok(RefreshResult::TransientFailure { kind, message }) => {
                debug!("transient refresh failure for {provider_id}: [{kind:?}] {message}");
                Self::handle_transient_failure_for(ps, provider_id, &message, &self.event_tx)
            }

            Err(e) => {
                warn!("refresh_access_token returned error for {provider_id}: {e}");
                Self::handle_transient_failure_for(ps, provider_id, &e.to_string(), &self.event_tx)
            }
        }
    }

    /// Shared logic for transient-failure and unexpected-error paths.
    fn handle_transient_failure_for(
        ps: &mut ProviderRefreshState,
        provider_id: &str,
        message: &str,
        event_tx: &broadcast::Sender<TokenEvent>,
    ) -> RefreshOutcome {
        ps.consecutive_transient_failures += 1;
        let attempt = ps.consecutive_transient_failures;

        if attempt >= MAX_CONSECUTIVE_FAILURES {
            warn!(
                "transient failure #{attempt} for {provider_id}: {message} — \
                 escalating to reauth"
            );
            ps.backoff_until = Some(Instant::now() + Duration::from_secs(86400 * 365));
            let _ = event_tx.send(TokenEvent::ReauthRequired {
                provider_id: provider_id.to_string(),
            });
            RefreshOutcome::ReauthRequired
        } else {
            // Safe: the `attempt >= MAX` branch above handles the None case,
            // so `backoff_duration` always returns Some here. Fallback to 120s
            // as a defensive default in case the match arms are ever reordered.
            let backoff =
                ProviderRefreshState::backoff_duration(attempt).unwrap_or(Duration::from_secs(120));
            ps.backoff_until = Some(Instant::now() + backoff);

            warn!(
                "transient failure #{attempt} for {provider_id}: {message} — \
                 backing off for {}s",
                backoff.as_secs()
            );

            let _ = event_tx.send(TokenEvent::RefreshFailed {
                provider_id: provider_id.to_string(),
                attempt,
                max_attempts: MAX_CONSECUTIVE_FAILURES,
            });
            RefreshOutcome::Failed { attempt }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_trait::async_trait;
    use maekon_core::error::CoreError;
    use maekon_core::ports::oauth::{
        OAuthConnectionStatus, OAuthErrorKind, OAuthFlowHandle, OAuthFlowStatus, OAuthPort,
        RefreshResult, TokenEvent,
    };

    /// Mock OAuthPort with configurable RefreshResult for testing.
    struct MockOAuthPort {
        result: tokio::sync::Mutex<RefreshResult>,
    }

    impl MockOAuthPort {
        fn new(result: RefreshResult) -> Self {
            Self {
                result: tokio::sync::Mutex::new(result),
            }
        }

        async fn set_result(&self, result: RefreshResult) {
            *self.result.lock().await = result;
        }
    }

    #[async_trait]
    impl OAuthPort for MockOAuthPort {
        async fn start_flow(&self, provider_id: &str) -> Result<OAuthFlowHandle, CoreError> {
            warn!("MockOAuthPort::start_flow called unexpectedly for {provider_id}");
            Err(CoreError::Network {
                code: maekon_core::error_codes::NetworkCode::Generic,
                message: format!(
                    "start_flow not supported in MockOAuthPort (provider: {provider_id})"
                ),
            })
        }
        async fn flow_status(&self, flow_id: &str) -> Result<OAuthFlowStatus, CoreError> {
            warn!("MockOAuthPort::flow_status called unexpectedly for {flow_id}");
            Err(CoreError::Network {
                code: maekon_core::error_codes::NetworkCode::Generic,
                message: format!("flow_status not supported in MockOAuthPort (flow: {flow_id})"),
            })
        }
        async fn cancel_flow(&self, flow_id: &str) -> Result<(), CoreError> {
            warn!("MockOAuthPort::cancel_flow called unexpectedly for {flow_id}");
            Err(CoreError::Network {
                code: maekon_core::error_codes::NetworkCode::Generic,
                message: format!("cancel_flow not supported in MockOAuthPort (flow: {flow_id})"),
            })
        }
        async fn get_access_token(&self, provider_id: &str) -> Result<Option<String>, CoreError> {
            warn!("MockOAuthPort::get_access_token called unexpectedly for {provider_id}");
            Ok(None)
        }
        async fn revoke(&self, provider_id: &str) -> Result<(), CoreError> {
            warn!("MockOAuthPort::revoke called unexpectedly for {provider_id}");
            Err(CoreError::Network {
                code: maekon_core::error_codes::NetworkCode::Generic,
                message: format!("revoke not supported in MockOAuthPort (provider: {provider_id})"),
            })
        }
        async fn connection_status(
            &self,
            provider_id: &str,
        ) -> Result<OAuthConnectionStatus, CoreError> {
            info!("MockOAuthPort::connection_status returning disconnected for {provider_id}");
            Ok(OAuthConnectionStatus {
                provider_id: provider_id.to_string(),
                connected: false,
                expires_at: None,
                scopes: vec![],
                api_base_url: None,
                has_refresh_token: false,
            })
        }
        async fn refresh_access_token(
            &self,
            _provider_id: &str,
            _min_valid_for_secs: i64,
        ) -> Result<RefreshResult, CoreError> {
            Ok(self.result.lock().await.clone())
        }
    }

    fn expiry_in_secs(secs: i64) -> String {
        (chrono::Utc::now() + chrono::Duration::seconds(secs)).to_rfc3339()
    }

    fn make_coordinator(
        mock: Arc<MockOAuthPort>,
    ) -> (TokenRefreshCoordinator, broadcast::Receiver<TokenEvent>) {
        let (tx, rx) = broadcast::channel(16);
        let coord = TokenRefreshCoordinator::new(mock, tx);
        (coord, rx)
    }

    // Helper: clear the backoff for a named provider so successive test steps
    // are not blocked by backoff.  Replaces the old `coord.state.lock().await.backoff_until = None`
    // pattern which accessed the now-removed flat `RefreshState`.
    async fn clear_backoff(coord: &TokenRefreshCoordinator, provider_id: &str) {
        let mut state_map = coord.state.lock().await;
        let ps = state_map.entry(provider_id);
        ps.backoff_until = None;
    }

    #[tokio::test]
    async fn not_needed_when_already_fresh() {
        let mock = Arc::new(MockOAuthPort::new(RefreshResult::AlreadyFresh {
            expires_at: expiry_in_secs(3600),
        }));
        let (coord, _rx) = make_coordinator(mock);

        let outcome = coord.check_and_refresh("openai").await;
        assert_eq!(outcome, RefreshOutcome::NotNeeded);
    }

    #[tokio::test]
    async fn refreshes_when_token_expiring() {
        let mock = Arc::new(MockOAuthPort::new(RefreshResult::Refreshed {
            expires_at: expiry_in_secs(3600),
        }));
        let (coord, mut rx) = make_coordinator(mock);

        let outcome = coord.check_and_refresh("openai").await;
        assert_eq!(outcome, RefreshOutcome::Refreshed);

        // Verify event was emitted.
        let event = rx.try_recv().expect("should receive TokenEvent::Refreshed");
        match event {
            TokenEvent::Refreshed { provider_id, .. } => {
                assert_eq!(provider_id, "openai");
            }
            other => panic!("expected Refreshed event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn transient_failure_increments_counter() {
        let mock = Arc::new(MockOAuthPort::new(RefreshResult::TransientFailure {
            kind: OAuthErrorKind::NetworkError,
            message: "connection reset".into(),
        }));
        let (coord, mut rx) = make_coordinator(mock);

        let outcome = coord.check_and_refresh("openai").await;
        assert_eq!(outcome, RefreshOutcome::Failed { attempt: 1 });

        let event = rx
            .try_recv()
            .expect("should receive TokenEvent::RefreshFailed");
        match event {
            TokenEvent::RefreshFailed {
                provider_id,
                attempt,
                max_attempts,
            } => {
                assert_eq!(provider_id, "openai");
                assert_eq!(attempt, 1);
                assert_eq!(max_attempts, 3);
            }
            other => panic!("expected RefreshFailed event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn terminal_failure_immediate_reauth() {
        let mock = Arc::new(MockOAuthPort::new(RefreshResult::ReauthRequired {
            kind: OAuthErrorKind::InvalidGrant,
            reason: "invalid_grant".into(),
        }));
        let (coord, mut rx) = make_coordinator(mock);

        let outcome = coord.check_and_refresh("openai").await;
        assert_eq!(outcome, RefreshOutcome::ReauthRequired);

        let event = rx
            .try_recv()
            .expect("should receive TokenEvent::ReauthRequired");
        match event {
            TokenEvent::ReauthRequired { provider_id } => {
                assert_eq!(provider_id, "openai");
            }
            other => panic!("expected ReauthRequired event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn three_transient_failures_triggers_reauth() {
        let mock = Arc::new(MockOAuthPort::new(RefreshResult::TransientFailure {
            kind: OAuthErrorKind::NetworkError,
            message: "timeout".into(),
        }));
        let (coord, mut rx) = make_coordinator(mock);

        // Failure 1
        let outcome = coord.check_and_refresh("openai").await;
        assert_eq!(outcome, RefreshOutcome::Failed { attempt: 1 });
        let _ = rx.try_recv(); // consume RefreshFailed event

        // Clear backoff for failure 2
        clear_backoff(&coord, "openai").await;

        // Failure 2
        let outcome = coord.check_and_refresh("openai").await;
        assert_eq!(outcome, RefreshOutcome::Failed { attempt: 2 });
        let _ = rx.try_recv(); // consume RefreshFailed event

        // Clear backoff for failure 3
        clear_backoff(&coord, "openai").await;

        // Failure 3 — should escalate to reauth
        let outcome = coord.check_and_refresh("openai").await;
        assert_eq!(outcome, RefreshOutcome::ReauthRequired);

        let event = rx
            .try_recv()
            .expect("should receive ReauthRequired after 3 failures");
        match event {
            TokenEvent::ReauthRequired { provider_id } => {
                assert_eq!(provider_id, "openai");
            }
            other => panic!("expected ReauthRequired event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn auto_recovery_resets_failure_count() {
        let mock = Arc::new(MockOAuthPort::new(RefreshResult::TransientFailure {
            kind: OAuthErrorKind::NetworkError,
            message: "timeout".into(),
        }));
        let (coord, _rx) = make_coordinator(mock.clone());

        // Failure 1
        let outcome = coord.check_and_refresh("openai").await;
        assert_eq!(outcome, RefreshOutcome::Failed { attempt: 1 });
        clear_backoff(&coord, "openai").await;

        // Failure 2
        let outcome = coord.check_and_refresh("openai").await;
        assert_eq!(outcome, RefreshOutcome::Failed { attempt: 2 });
        clear_backoff(&coord, "openai").await;

        // Now the server recovers — mock returns AlreadyFresh
        mock.set_result(RefreshResult::AlreadyFresh {
            expires_at: expiry_in_secs(3600),
        })
        .await;

        let outcome = coord.check_and_refresh("openai").await;
        assert_eq!(outcome, RefreshOutcome::NotNeeded);

        // Verify the counter was reset.
        let state_map = coord.state.lock().await;
        let ps = state_map
            .0
            .get("openai")
            .expect("state entry for openai should exist");
        assert_eq!(
            ps.consecutive_transient_failures, 0,
            "failure counter should be reset after auto-recovery"
        );
    }

    #[tokio::test]
    async fn backoff_prevents_immediate_retry() {
        let call_count = Arc::new(std::sync::atomic::AtomicU8::new(0));
        let call_count_clone = call_count.clone();

        /// Mock that counts how many times refresh_access_token is called.
        struct CountingMockOAuthPort {
            call_count: Arc<std::sync::atomic::AtomicU8>,
        }

        #[async_trait]
        impl OAuthPort for CountingMockOAuthPort {
            async fn start_flow(&self, provider_id: &str) -> Result<OAuthFlowHandle, CoreError> {
                warn!("CountingMockOAuthPort::start_flow called unexpectedly for {provider_id}");
                Err(CoreError::Network {
                    code: maekon_core::error_codes::NetworkCode::Generic,
                    message: format!(
                    "start_flow not supported in CountingMockOAuthPort (provider: {provider_id})"
                ),
                })
            }
            async fn flow_status(&self, flow_id: &str) -> Result<OAuthFlowStatus, CoreError> {
                warn!("CountingMockOAuthPort::flow_status called unexpectedly for {flow_id}");
                Err(CoreError::Network {
                    code: maekon_core::error_codes::NetworkCode::Generic,
                    message: format!(
                        "flow_status not supported in CountingMockOAuthPort (flow: {flow_id})"
                    ),
                })
            }
            async fn cancel_flow(&self, flow_id: &str) -> Result<(), CoreError> {
                warn!("CountingMockOAuthPort::cancel_flow called unexpectedly for {flow_id}");
                Err(CoreError::Network {
                    code: maekon_core::error_codes::NetworkCode::Generic,
                    message: format!(
                        "cancel_flow not supported in CountingMockOAuthPort (flow: {flow_id})"
                    ),
                })
            }
            async fn get_access_token(
                &self,
                provider_id: &str,
            ) -> Result<Option<String>, CoreError> {
                warn!(
                    "CountingMockOAuthPort::get_access_token called unexpectedly for {provider_id}"
                );
                Ok(None)
            }
            async fn revoke(&self, provider_id: &str) -> Result<(), CoreError> {
                warn!("CountingMockOAuthPort::revoke called unexpectedly for {provider_id}");
                Err(CoreError::Network {
                    code: maekon_core::error_codes::NetworkCode::Generic,
                    message: format!(
                        "revoke not supported in CountingMockOAuthPort (provider: {provider_id})"
                    ),
                })
            }
            async fn connection_status(
                &self,
                provider_id: &str,
            ) -> Result<OAuthConnectionStatus, CoreError> {
                info!(
                    "CountingMockOAuthPort::connection_status returning disconnected for {provider_id}"
                );
                Ok(OAuthConnectionStatus {
                    provider_id: provider_id.to_string(),
                    connected: false,
                    expires_at: None,
                    scopes: vec![],
                    api_base_url: None,
                    has_refresh_token: false,
                })
            }
            async fn refresh_access_token(
                &self,
                _provider_id: &str,
                _min_valid_for_secs: i64,
            ) -> Result<RefreshResult, CoreError> {
                self.call_count
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Ok(RefreshResult::TransientFailure {
                    kind: OAuthErrorKind::NetworkError,
                    message: "timeout".into(),
                })
            }
        }

        let mock = Arc::new(CountingMockOAuthPort {
            call_count: call_count_clone,
        });
        let (tx, _rx) = broadcast::channel(16);
        let coord = TokenRefreshCoordinator::new(mock, tx);

        // First call — triggers transient failure and sets backoff.
        let outcome = coord.check_and_refresh("openai").await;
        assert_eq!(outcome, RefreshOutcome::Failed { attempt: 1 });

        // Second call — should be blocked by backoff.
        let outcome = coord.check_and_refresh("openai").await;
        assert_eq!(outcome, RefreshOutcome::BackingOff);

        // Port should have been called only once.
        assert_eq!(
            call_count.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "port should only be called once when backoff is active"
        );
    }

    #[test]
    fn backoff_duration_schedule() {
        assert_eq!(
            ProviderRefreshState::backoff_duration(1),
            Some(Duration::from_secs(120))
        );
        assert_eq!(
            ProviderRefreshState::backoff_duration(2),
            Some(Duration::from_secs(300))
        );
        assert_eq!(ProviderRefreshState::backoff_duration(3), None);
        assert_eq!(ProviderRefreshState::backoff_duration(4), None);
    }

    #[tokio::test]
    async fn mock_start_flow_returns_error() {
        let mock = Arc::new(MockOAuthPort::new(RefreshResult::AlreadyFresh {
            expires_at: expiry_in_secs(3600),
        }));
        let result = mock.start_flow("openai").await;
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("start_flow not supported"),
            "expected descriptive error, got: {err_msg}"
        );
    }

    #[tokio::test]
    async fn mock_flow_status_returns_error() {
        let mock = Arc::new(MockOAuthPort::new(RefreshResult::AlreadyFresh {
            expires_at: expiry_in_secs(3600),
        }));
        let err = mock.flow_status("flow-123").await.unwrap_err();
        assert!(
            matches!(
                err,
                CoreError::Network {
                    code: maekon_core::error_codes::NetworkCode::Generic,
                    ..
                }
            ),
            "flow_status must return CoreError::Network::Generic, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn mock_cancel_flow_returns_error() {
        let mock = Arc::new(MockOAuthPort::new(RefreshResult::AlreadyFresh {
            expires_at: expiry_in_secs(3600),
        }));
        let err = mock.cancel_flow("flow-123").await.unwrap_err();
        assert!(
            matches!(
                err,
                CoreError::Network {
                    code: maekon_core::error_codes::NetworkCode::Generic,
                    ..
                }
            ),
            "cancel_flow must return CoreError::Network::Generic, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn mock_get_access_token_returns_none() {
        let mock = Arc::new(MockOAuthPort::new(RefreshResult::AlreadyFresh {
            expires_at: expiry_in_secs(3600),
        }));
        let token = mock
            .get_access_token("openai")
            .await
            .expect("mock get_access_token must return Ok (no token stored)");
        assert_eq!(
            token, None,
            "mock must return None — no access token is stored"
        );
    }

    #[tokio::test]
    async fn mock_revoke_returns_error() {
        let mock = Arc::new(MockOAuthPort::new(RefreshResult::AlreadyFresh {
            expires_at: expiry_in_secs(3600),
        }));
        let err = mock.revoke("openai").await.unwrap_err();
        assert!(
            matches!(
                err,
                CoreError::Network {
                    code: maekon_core::error_codes::NetworkCode::Generic,
                    ..
                }
            ),
            "revoke must return CoreError::Network::Generic, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn mock_connection_status_returns_disconnected() {
        let mock = Arc::new(MockOAuthPort::new(RefreshResult::AlreadyFresh {
            expires_at: expiry_in_secs(3600),
        }));
        let status = mock
            .connection_status("openai")
            .await
            .expect("mock connection_status must return Ok with a disconnected status");
        assert_eq!(status.provider_id, "openai");
        assert!(!status.connected);
        assert!(status.scopes.is_empty());
        assert!(!status.has_refresh_token);
    }

    /// `NotAuthenticated` (never connected) must be silent: no event fired,
    /// outcome is `NotConnected` — NOT `ReauthRequired`.
    ///
    /// Previously this test was `not_authenticated_triggers_reauth`; the
    /// semantic has been corrected: a provider that was never connected must
    /// not produce a "re-authentication required" notification.
    #[tokio::test]
    async fn not_authenticated_is_silent_not_connected() {
        let mock = Arc::new(MockOAuthPort::new(RefreshResult::NotAuthenticated));
        let (coord, mut rx) = make_coordinator(mock);

        let outcome = coord.check_and_refresh("openai").await;

        // Outcome is NotConnected, not ReauthRequired.
        assert_eq!(
            outcome,
            RefreshOutcome::NotConnected,
            "never-connected provider must yield NotConnected, not ReauthRequired"
        );

        // No event must be emitted on the broadcast channel.
        assert!(
            matches!(
                rx.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            ),
            "no TokenEvent must be emitted for a never-connected provider"
        );
    }

    /// A genuine terminal failure (token existed, became invalid) must still
    /// fire `TokenEvent::ReauthRequired`.
    #[tokio::test]
    async fn terminal_reauth_required_fires_event() {
        let mock = Arc::new(MockOAuthPort::new(RefreshResult::ReauthRequired {
            kind: OAuthErrorKind::InvalidGrant,
            reason: "invalid_grant".into(),
        }));
        let (coord, mut rx) = make_coordinator(mock);

        let outcome = coord.check_and_refresh("openai").await;
        assert_eq!(
            outcome,
            RefreshOutcome::ReauthRequired,
            "a terminal ReauthRequired from the port must produce ReauthRequired outcome"
        );

        let event = rx
            .try_recv()
            .expect("TokenEvent::ReauthRequired must be emitted for a terminal failure");
        match event {
            TokenEvent::ReauthRequired { provider_id } => {
                assert_eq!(provider_id, "openai");
            }
            other => panic!("expected ReauthRequired event, got {other:?}"),
        }
    }

    /// Per-provider isolation: backoff on provider A must not block provider B.
    ///
    /// Steps:
    ///   1. Drive provider "google" into NotConnected (long backoff).
    ///   2. Call check_and_refresh("openai") — must not return BackingOff.
    #[tokio::test]
    async fn per_provider_isolation_backoff_does_not_cross_providers() {
        // Port always returns NotAuthenticated (simulates "never connected").
        let mock = Arc::new(MockOAuthPort::new(RefreshResult::NotAuthenticated));
        let (tx, _rx) = broadcast::channel(16);
        let coord = TokenRefreshCoordinator::new(mock.clone(), tx);

        // Drive "google" into NotConnected + 1-year backoff.
        let outcome_google = coord.check_and_refresh("google").await;
        assert_eq!(
            outcome_google,
            RefreshOutcome::NotConnected,
            "google must be NotConnected"
        );

        // Verify "google" is indeed in backoff.
        let outcome_google_again = coord.check_and_refresh("google").await;
        assert_eq!(
            outcome_google_again,
            RefreshOutcome::BackingOff,
            "google must be backing off after NotConnected"
        );

        // Now check "openai" — it must be independent of google's backoff.
        let outcome_openai = coord.check_and_refresh("openai").await;
        assert_ne!(
            outcome_openai,
            RefreshOutcome::BackingOff,
            "openai must NOT be blocked by google's backoff"
        );
        // openai also has no token, so it should be NotConnected on first call.
        assert_eq!(
            outcome_openai,
            RefreshOutcome::NotConnected,
            "openai must be NotConnected when never authenticated"
        );
    }

    #[tokio::test]
    async fn port_error_treated_as_transient() {
        // When the OAuthPort returns Err(...), the coordinator should treat
        // it the same as a transient failure.
        struct ErrorMockOAuthPort;

        #[async_trait]
        impl OAuthPort for ErrorMockOAuthPort {
            async fn start_flow(&self, _: &str) -> Result<OAuthFlowHandle, CoreError> {
                Err(CoreError::Network {
                    code: maekon_core::error_codes::NetworkCode::Generic,
                    message: "mock".into(),
                })
            }
            async fn flow_status(&self, _: &str) -> Result<OAuthFlowStatus, CoreError> {
                Err(CoreError::Network {
                    code: maekon_core::error_codes::NetworkCode::Generic,
                    message: "mock".into(),
                })
            }
            async fn cancel_flow(&self, _: &str) -> Result<(), CoreError> {
                Err(CoreError::Network {
                    code: maekon_core::error_codes::NetworkCode::Generic,
                    message: "mock".into(),
                })
            }
            async fn get_access_token(&self, _: &str) -> Result<Option<String>, CoreError> {
                Ok(None)
            }
            async fn revoke(&self, _: &str) -> Result<(), CoreError> {
                Err(CoreError::Network {
                    code: maekon_core::error_codes::NetworkCode::Generic,
                    message: "mock".into(),
                })
            }
            async fn connection_status(
                &self,
                provider_id: &str,
            ) -> Result<OAuthConnectionStatus, CoreError> {
                Ok(OAuthConnectionStatus {
                    provider_id: provider_id.to_string(),
                    connected: false,
                    expires_at: None,
                    scopes: vec![],
                    api_base_url: None,
                    has_refresh_token: false,
                })
            }
            async fn refresh_access_token(
                &self,
                _provider_id: &str,
                _min_valid_for_secs: i64,
            ) -> Result<RefreshResult, CoreError> {
                Err(CoreError::Network {
                    code: maekon_core::error_codes::NetworkCode::Generic,
                    message: "connection refused".into(),
                })
            }
        }

        let mock = Arc::new(ErrorMockOAuthPort);
        let (tx, mut rx) = broadcast::channel(16);
        let coord = TokenRefreshCoordinator::new(mock, tx);

        let outcome = coord.check_and_refresh("openai").await;
        assert_eq!(outcome, RefreshOutcome::Failed { attempt: 1 });

        let event = rx
            .try_recv()
            .expect("should receive TokenEvent::RefreshFailed");
        match event {
            TokenEvent::RefreshFailed {
                attempt,
                max_attempts,
                ..
            } => {
                assert_eq!(attempt, 1);
                assert_eq!(max_attempts, 3);
            }
            other => panic!("expected RefreshFailed event, got {other:?}"),
        }
    }
}
