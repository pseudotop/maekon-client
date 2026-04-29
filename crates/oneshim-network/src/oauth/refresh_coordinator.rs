//! Token refresh coordinator — orchestrates automatic access-token renewal
//! with failure tracking, backoff, and event emission.
//!
//! This is an orchestration utility, NOT a port adapter. It wraps an
//! `OAuthPort` implementation and adds retry/backoff/event-emission
//! semantics on top of the raw `refresh_access_token` call.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{broadcast, Mutex};
use tracing::{debug, info, warn};

use oneshim_core::ports::oauth::{OAuthPort, RefreshResult, TokenEvent};

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
    ReauthRequired,
    /// Another refresh is already in progress.
    AlreadyInProgress,
    /// Backoff period has not elapsed since the last failure.
    BackingOff,
}

/// Internal mutable state guarded by a tokio Mutex.
struct RefreshState {
    in_progress: bool,
    consecutive_transient_failures: u8,
    last_attempt: Option<Instant>,
    backoff_until: Option<Instant>,
}

impl RefreshState {
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

/// Coordinates automatic token refresh with failure tracking and backoff.
pub struct TokenRefreshCoordinator {
    oauth_port: Arc<dyn OAuthPort>,
    state: Mutex<RefreshState>,
    event_tx: broadcast::Sender<TokenEvent>,
}

impl TokenRefreshCoordinator {
    /// Create a new coordinator wrapping the given `OAuthPort`.
    pub fn new(oauth_port: Arc<dyn OAuthPort>, event_tx: broadcast::Sender<TokenEvent>) -> Self {
        Self {
            oauth_port,
            state: Mutex::new(RefreshState::new()),
            event_tx,
        }
    }

    /// Subscribe to token lifecycle events emitted by this coordinator.
    pub fn subscribe(&self) -> broadcast::Receiver<TokenEvent> {
        self.event_tx.subscribe()
    }

    /// Reset internal state after successful manual re-authentication.
    ///
    /// Clears backoff timers and failure counters so the background
    /// refresh loop resumes normal operation immediately.
    pub async fn reset(&self) {
        let mut state = self.state.lock().await;
        state.in_progress = false;
        state.consecutive_transient_failures = 0;
        state.last_attempt = None;
        state.backoff_until = None;
    }

    /// Check whether a refresh is needed and perform it if so.
    ///
    /// This is the main entry point called by the scheduler loop.
    pub async fn check_and_refresh(&self, provider_id: &str) -> RefreshOutcome {
        // --- Phase 1: acquire lock, check preconditions ---
        {
            let mut state = self.state.lock().await;

            if state.in_progress {
                debug!("refresh already in progress for {provider_id}");
                return RefreshOutcome::AlreadyInProgress;
            }

            if let Some(until) = state.backoff_until {
                if Instant::now() < until {
                    debug!("backing off refresh for {provider_id}");
                    return RefreshOutcome::BackingOff;
                }
                // Backoff period elapsed — clear it.
                state.backoff_until = None;
            }

            state.in_progress = true;
            state.last_attempt = Some(Instant::now());
        }
        // Lock released — the actual network call happens without holding it.

        // --- Phase 2: call the port ---
        let result = self
            .oauth_port
            .refresh_access_token(provider_id, REFRESH_THRESHOLD_SECS)
            .await;

        // --- Phase 3: re-acquire lock, process result ---
        let mut state = self.state.lock().await;
        state.in_progress = false;

        match result {
            Ok(RefreshResult::AlreadyFresh { .. }) => {
                if state.consecutive_transient_failures > 0 {
                    info!(
                        "auto-recovery: resetting failure counter for {provider_id} \
                         (was {})",
                        state.consecutive_transient_failures
                    );
                    state.consecutive_transient_failures = 0;
                    state.backoff_until = None;
                }
                RefreshOutcome::NotNeeded
            }

            Ok(RefreshResult::Refreshed { expires_at }) => {
                state.consecutive_transient_failures = 0;
                state.backoff_until = None;
                let _ = self.event_tx.send(TokenEvent::Refreshed {
                    provider_id: provider_id.to_string(),
                    expires_at,
                });
                RefreshOutcome::Refreshed
            }

            Ok(RefreshResult::NotAuthenticated) => {
                state.backoff_until = Some(Instant::now() + Duration::from_secs(86400 * 365));
                let _ = self.event_tx.send(TokenEvent::ReauthRequired {
                    provider_id: provider_id.to_string(),
                });
                RefreshOutcome::ReauthRequired
            }

            Ok(RefreshResult::ReauthRequired { kind, reason }) => {
                // Terminal — set backoff far into the future to prevent retries.
                warn!("terminal refresh failure for {provider_id}: [{kind:?}] {reason}");
                state.backoff_until = Some(Instant::now() + Duration::from_secs(86400 * 365));
                let _ = self.event_tx.send(TokenEvent::ReauthRequired {
                    provider_id: provider_id.to_string(),
                });
                RefreshOutcome::ReauthRequired
            }

            Ok(RefreshResult::TransientFailure { kind, message }) => {
                debug!("transient refresh failure for {provider_id}: [{kind:?}] {message}");
                self.handle_transient_failure(&mut state, provider_id, &message)
            }

            Err(e) => {
                warn!("refresh_access_token returned error for {provider_id}: {e}");
                self.handle_transient_failure(&mut state, provider_id, &e.to_string())
            }
        }
    }

    /// Shared logic for transient-failure and unexpected-error paths.
    fn handle_transient_failure(
        &self,
        state: &mut RefreshState,
        provider_id: &str,
        message: &str,
    ) -> RefreshOutcome {
        state.consecutive_transient_failures += 1;
        let attempt = state.consecutive_transient_failures;

        if attempt >= MAX_CONSECUTIVE_FAILURES {
            warn!(
                "transient failure #{attempt} for {provider_id}: {message} — \
                 escalating to reauth"
            );
            state.backoff_until = Some(Instant::now() + Duration::from_secs(86400 * 365));
            let _ = self.event_tx.send(TokenEvent::ReauthRequired {
                provider_id: provider_id.to_string(),
            });
            RefreshOutcome::ReauthRequired
        } else {
            // Safe: the `attempt >= MAX` branch above handles the None case,
            // so `backoff_duration` always returns Some here. Fallback to 120s
            // as a defensive default in case the match arms are ever reordered.
            let backoff =
                RefreshState::backoff_duration(attempt).unwrap_or(Duration::from_secs(120));
            state.backoff_until = Some(Instant::now() + backoff);

            warn!(
                "transient failure #{attempt} for {provider_id}: {message} — \
                 backing off for {}s",
                backoff.as_secs()
            );

            let _ = self.event_tx.send(TokenEvent::RefreshFailed {
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
    use oneshim_core::error::CoreError;
    use oneshim_core::ports::oauth::{
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
                code: oneshim_core::error_codes::NetworkCode::Generic,
                message: format!(
                    "start_flow not supported in MockOAuthPort (provider: {provider_id})"
                ),
            })
        }
        async fn flow_status(&self, flow_id: &str) -> Result<OAuthFlowStatus, CoreError> {
            warn!("MockOAuthPort::flow_status called unexpectedly for {flow_id}");
            Err(CoreError::Network {
                code: oneshim_core::error_codes::NetworkCode::Generic,
                message: format!("flow_status not supported in MockOAuthPort (flow: {flow_id})"),
            })
        }
        async fn cancel_flow(&self, flow_id: &str) -> Result<(), CoreError> {
            warn!("MockOAuthPort::cancel_flow called unexpectedly for {flow_id}");
            Err(CoreError::Network {
                code: oneshim_core::error_codes::NetworkCode::Generic,
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
                code: oneshim_core::error_codes::NetworkCode::Generic,
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
        coord.state.lock().await.backoff_until = None;

        // Failure 2
        let outcome = coord.check_and_refresh("openai").await;
        assert_eq!(outcome, RefreshOutcome::Failed { attempt: 2 });
        let _ = rx.try_recv(); // consume RefreshFailed event

        // Clear backoff for failure 3
        coord.state.lock().await.backoff_until = None;

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
        coord.state.lock().await.backoff_until = None;

        // Failure 2
        let outcome = coord.check_and_refresh("openai").await;
        assert_eq!(outcome, RefreshOutcome::Failed { attempt: 2 });
        coord.state.lock().await.backoff_until = None;

        // Now the server recovers — mock returns AlreadyFresh
        mock.set_result(RefreshResult::AlreadyFresh {
            expires_at: expiry_in_secs(3600),
        })
        .await;

        let outcome = coord.check_and_refresh("openai").await;
        assert_eq!(outcome, RefreshOutcome::NotNeeded);

        // Verify the counter was reset.
        let state = coord.state.lock().await;
        assert_eq!(
            state.consecutive_transient_failures, 0,
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
                    code: oneshim_core::error_codes::NetworkCode::Generic,
                    message: format!(
                    "start_flow not supported in CountingMockOAuthPort (provider: {provider_id})"
                ),
                })
            }
            async fn flow_status(&self, flow_id: &str) -> Result<OAuthFlowStatus, CoreError> {
                warn!("CountingMockOAuthPort::flow_status called unexpectedly for {flow_id}");
                Err(CoreError::Network {
                    code: oneshim_core::error_codes::NetworkCode::Generic,
                    message: format!(
                        "flow_status not supported in CountingMockOAuthPort (flow: {flow_id})"
                    ),
                })
            }
            async fn cancel_flow(&self, flow_id: &str) -> Result<(), CoreError> {
                warn!("CountingMockOAuthPort::cancel_flow called unexpectedly for {flow_id}");
                Err(CoreError::Network {
                    code: oneshim_core::error_codes::NetworkCode::Generic,
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
                    code: oneshim_core::error_codes::NetworkCode::Generic,
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
            RefreshState::backoff_duration(1),
            Some(Duration::from_secs(120))
        );
        assert_eq!(
            RefreshState::backoff_duration(2),
            Some(Duration::from_secs(300))
        );
        assert_eq!(RefreshState::backoff_duration(3), None);
        assert_eq!(RefreshState::backoff_duration(4), None);
    }

    #[tokio::test]
    async fn mock_start_flow_returns_error() {
        let mock = Arc::new(MockOAuthPort::new(RefreshResult::AlreadyFresh {
            expires_at: expiry_in_secs(3600),
        }));
        let result = mock.start_flow("openai").await;
        assert!(result.is_err());
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
        let result = mock.flow_status("flow-123").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn mock_cancel_flow_returns_error() {
        let mock = Arc::new(MockOAuthPort::new(RefreshResult::AlreadyFresh {
            expires_at: expiry_in_secs(3600),
        }));
        let result = mock.cancel_flow("flow-123").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn mock_get_access_token_returns_none() {
        let mock = Arc::new(MockOAuthPort::new(RefreshResult::AlreadyFresh {
            expires_at: expiry_in_secs(3600),
        }));
        let result = mock.get_access_token("openai").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), None);
    }

    #[tokio::test]
    async fn mock_revoke_returns_error() {
        let mock = Arc::new(MockOAuthPort::new(RefreshResult::AlreadyFresh {
            expires_at: expiry_in_secs(3600),
        }));
        let result = mock.revoke("openai").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn mock_connection_status_returns_disconnected() {
        let mock = Arc::new(MockOAuthPort::new(RefreshResult::AlreadyFresh {
            expires_at: expiry_in_secs(3600),
        }));
        let result = mock.connection_status("openai").await;
        assert!(result.is_ok());
        let status = result.unwrap();
        assert_eq!(status.provider_id, "openai");
        assert!(!status.connected);
        assert!(status.scopes.is_empty());
        assert!(!status.has_refresh_token);
    }

    #[tokio::test]
    async fn not_authenticated_triggers_reauth() {
        let mock = Arc::new(MockOAuthPort::new(RefreshResult::NotAuthenticated));
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
    async fn port_error_treated_as_transient() {
        // When the OAuthPort returns Err(...), the coordinator should treat
        // it the same as a transient failure.
        struct ErrorMockOAuthPort;

        #[async_trait]
        impl OAuthPort for ErrorMockOAuthPort {
            async fn start_flow(&self, _: &str) -> Result<OAuthFlowHandle, CoreError> {
                Err(CoreError::Network {
                    code: oneshim_core::error_codes::NetworkCode::Generic,
                    message: "mock".into(),
                })
            }
            async fn flow_status(&self, _: &str) -> Result<OAuthFlowStatus, CoreError> {
                Err(CoreError::Network {
                    code: oneshim_core::error_codes::NetworkCode::Generic,
                    message: "mock".into(),
                })
            }
            async fn cancel_flow(&self, _: &str) -> Result<(), CoreError> {
                Err(CoreError::Network {
                    code: oneshim_core::error_codes::NetworkCode::Generic,
                    message: "mock".into(),
                })
            }
            async fn get_access_token(&self, _: &str) -> Result<Option<String>, CoreError> {
                Ok(None)
            }
            async fn revoke(&self, _: &str) -> Result<(), CoreError> {
                Err(CoreError::Network {
                    code: oneshim_core::error_codes::NetworkCode::Generic,
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
                    code: oneshim_core::error_codes::NetworkCode::Generic,
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
