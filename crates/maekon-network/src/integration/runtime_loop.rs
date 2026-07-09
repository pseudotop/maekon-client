use std::sync::Arc;
use std::time::Duration;

use futures::future::pending;
use maekon_core::error::CoreError;
use maekon_core::models::integration::{
    IntegrationCapabilityScope, IntegrationSessionState, IntegrationSessionStatus,
};
use maekon_core::ports::integration::{
    IntegrationEgressPort, IntegrationEgressSignalPort, IntegrationInboxPort,
    IntegrationInboxSignalPort, IntegrationSessionPort,
};
use tokio::sync::watch;
use tracing::warn;

use super::runtime_telemetry::{IntegrationRuntimeLane, IntegrationRuntimeTelemetryHandle};
use crate::error::NetworkError;
use crate::resilience::{scale_duration, RetryBackoffGate, RetryBackoffPolicy};

/// Convert a `&CoreError` to a `NetworkError` so it can be passed to
/// `RetryBackoffGate::on_failure`, which matches on `NetworkError::RateLimited`
/// to honour server-specified retry-after delays.
fn core_to_network_error(e: &CoreError) -> NetworkError {
    match e {
        CoreError::RateLimit {
            code: maekon_core::error_codes::NetworkCode::RateLimit,
            retry_after_secs,
        } => NetworkError::RateLimited {
            retry_after_secs: *retry_after_secs,
        },
        CoreError::RequestTimeout {
            code: maekon_core::error_codes::NetworkCode::Timeout,
            timeout_ms,
        } => NetworkError::Timeout {
            timeout_ms: *timeout_ms,
        },
        CoreError::ServiceUnavailable {
            code: maekon_core::error_codes::ServiceCode::Unavailable,
            message: msg,
        } => NetworkError::ServiceUnavailable(msg.clone()),
        CoreError::Auth {
            code: maekon_core::error_codes::AuthCode::Failed,
            message: msg,
        } => NetworkError::Auth(msg.clone()),
        other => NetworkError::Http(other.to_string()),
    }
}

#[derive(Debug, Clone)]
pub struct IntegrationRuntimeLoopProfile {
    pub requested_scopes: Vec<IntegrationCapabilityScope>,
    pub connect_retry_interval: Duration,
    pub heartbeat_interval: Duration,
    pub egress_interval: Duration,
    pub inbox_refresh_interval: Duration,
}

impl Default for IntegrationRuntimeLoopProfile {
    fn default() -> Self {
        Self {
            requested_scopes: vec![
                IntegrationCapabilityScope::InsightWrite,
                IntegrationCapabilityScope::PromptRead,
                IntegrationCapabilityScope::PromptAck,
                IntegrationCapabilityScope::SessionManage,
            ],
            connect_retry_interval: Duration::from_secs(15),
            heartbeat_interval: Duration::from_secs(30),
            egress_interval: Duration::from_secs(15),
            inbox_refresh_interval: Duration::from_secs(15),
        }
    }
}

/// Upper bound on how far the egress/inbox backstop polls are stretched while
/// the integration is quiet (#6516). 4× the base interval (e.g. 15s → 60s).
const POLL_BACKOFF_MAX_FACTOR: u32 = 4;

/// Adaptive backoff for the egress/inbox *backstop* polls (#6516).
///
/// Those polls fire every base interval to flush queued egress / pull the remote
/// inbox even without a push signal. When the integration is quiet (consecutive
/// polls find nothing) we stretch the next poll up to `base * max_factor`, so an
/// idle integration stops waking the network every base interval. Any real work
/// (a non-empty cycle) or a push signal snaps the cadence back to base, so
/// delivery latency stays bounded and recovers the moment traffic resumes.
///
/// Deliberately scoped: heartbeat is NOT backed off (it keeps the session
/// alive), and this keys on *integration traffic* — never on capture-pause,
/// which is a privacy control for screen capture, not a reason to stop syncing.
struct PollBackoff {
    base: Duration,
    max_factor: u32,
    empty_streak: u32,
}

impl PollBackoff {
    fn new(base: Duration, max_factor: u32) -> Self {
        Self {
            base,
            max_factor: max_factor.max(1),
            empty_streak: 0,
        }
    }

    /// Delay until the next backstop poll after a completed cycle. `did_work`
    /// means the cycle flushed or received something. Empty cycles grow the
    /// delay exponentially (capped at `base * max_factor`); any work resets it.
    fn after_cycle(&mut self, did_work: bool) -> Duration {
        if did_work {
            self.empty_streak = 0;
        } else {
            self.empty_streak = self.empty_streak.saturating_add(1);
        }
        self.current_delay()
    }

    /// Snap back to the base cadence — called when a push signal shows the
    /// integration is active again.
    fn reset_to_base(&mut self) -> Duration {
        self.empty_streak = 0;
        self.base
    }

    fn current_delay(&self) -> Duration {
        // 1×, 2×, 4×, … capped at max_factor. Cap the shift to avoid overflow.
        let factor = 1u32
            .checked_shl(self.empty_streak.min(16))
            .unwrap_or(u32::MAX)
            .min(self.max_factor);
        self.base * factor
    }
}

/// #7617 (LOW finding #7): outcome of a single `run_heartbeat_cycle` attempt.
/// Distinguishes "a heartbeat was actually transmitted" from "there was no
/// ready session to heartbeat" so the caller does not record a false success
/// on the Heartbeat telemetry lane while the integration is actually down
/// (e.g. session `Failed` after a connect failure, or not yet connected).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeartbeatOutcome {
    /// A heartbeat was sent to a Connected/Degraded session.
    Sent,
    /// No ready session existed; no heartbeat was transmitted.
    Skipped,
}

#[derive(Clone)]
pub struct IntegrationRuntimeLoop {
    session: Arc<dyn IntegrationSessionPort>,
    egress: Arc<dyn IntegrationEgressPort>,
    inbox: Arc<dyn IntegrationInboxPort>,
    egress_signal: Option<Arc<dyn IntegrationEgressSignalPort>>,
    inbox_signal: Option<Arc<dyn IntegrationInboxSignalPort>>,
    telemetry: Option<IntegrationRuntimeTelemetryHandle>,
    profile: IntegrationRuntimeLoopProfile,
}

impl IntegrationRuntimeLoop {
    pub fn new(
        session: Arc<dyn IntegrationSessionPort>,
        egress: Arc<dyn IntegrationEgressPort>,
        inbox: Arc<dyn IntegrationInboxPort>,
        egress_signal: Option<Arc<dyn IntegrationEgressSignalPort>>,
        inbox_signal: Option<Arc<dyn IntegrationInboxSignalPort>>,
        telemetry: Option<IntegrationRuntimeTelemetryHandle>,
        profile: IntegrationRuntimeLoopProfile,
    ) -> Self {
        Self {
            session,
            egress,
            inbox,
            egress_signal,
            inbox_signal,
            telemetry,
            profile,
        }
    }

    fn session_satisfies_scopes(
        session: &IntegrationSessionState,
        requested_scopes: &[IntegrationCapabilityScope],
    ) -> bool {
        // #7617 (MED finding #2): `Degraded` is EXCLUDED from readiness on
        // purpose. `IntegrationSessionCoordinator::heartbeat` only ever moves
        // a session to `Degraded` after a heartbeat transport failure (see
        // session_coordinator.rs), so a Degraded session is by definition
        // "last known bad" — treating it as ready here permanently skips the
        // coordinator's own Degraded-revalidation/reconnect path (#6204),
        // since `ensure_session_ready` never calls `connect()` again. Letting
        // `ensure_session_ready` fall through to `connect()` on ANY Degraded
        // session is cheap and safe: `IntegrationSessionCoordinator::connect`
        // already reuses the session with a single revalidation heartbeat
        // when it succeeds, and only pays for a full reconnect (with prior
        // binding eviction) when that heartbeat also fails. This is also the
        // recovery path for finding #1: an unexpectedly dropped live_channel
        // fails the next heartbeat, which flips the session to Degraded here,
        // which this check now escalates into an actual reconnect attempt.
        matches!(session.status, IntegrationSessionStatus::Connected)
            && !session.session_id.is_empty()
            && requested_scopes
                .iter()
                .all(|scope| session.granted_scopes.contains(scope))
    }

    async fn ensure_session_ready(&self) -> Result<(), CoreError> {
        if let Some(current) = self.session.current_session().await? {
            if Self::session_satisfies_scopes(&current, &self.profile.requested_scopes) {
                return Ok(());
            }
        }

        self.session
            .connect(self.profile.requested_scopes.clone())
            .await
            .map(|_| ())
    }

    async fn run_connect_cycle(&self) -> Result<(), CoreError> {
        self.ensure_session_ready().await
    }

    async fn run_egress_cycle(&self) -> Result<usize, CoreError> {
        self.ensure_session_ready().await?;
        self.egress.flush().await
    }

    async fn run_inbox_cycle(&self) -> Result<usize, CoreError> {
        self.ensure_session_ready().await?;
        self.inbox.refresh().await
    }

    async fn wait_for_egress_signal(&self) -> Result<bool, CoreError> {
        match self.egress_signal.as_ref() {
            Some(signal) => {
                signal
                    .wait_for_pending_egress(self.profile.egress_interval)
                    .await
            }
            None => pending::<Result<bool, CoreError>>().await,
        }
    }

    async fn wait_for_inbox_signal(&self) -> Result<bool, CoreError> {
        match self.inbox_signal.as_ref() {
            Some(signal) => {
                signal
                    .wait_for_remote_prompt_signal(self.profile.inbox_refresh_interval)
                    .await
            }
            None => pending::<Result<bool, CoreError>>().await,
        }
    }

    async fn run_heartbeat_cycle(&self) -> Result<HeartbeatOutcome, CoreError> {
        let Some(current) = self.session.current_session().await? else {
            return Ok(HeartbeatOutcome::Skipped);
        };

        if matches!(
            current.status,
            IntegrationSessionStatus::Connected | IntegrationSessionStatus::Degraded
        ) && !current.session_id.is_empty()
        {
            self.session.heartbeat(&current.session_id).await?;
            return Ok(HeartbeatOutcome::Sent);
        }

        Ok(HeartbeatOutcome::Skipped)
    }

    async fn record_cycle_success(&self, lane: IntegrationRuntimeLane) {
        if let Some(telemetry) = self.telemetry.as_ref() {
            telemetry.record_success(lane).await;
        }
    }

    async fn record_cycle_failure(
        &self,
        lane: IntegrationRuntimeLane,
        error: &CoreError,
        delay: Duration,
    ) {
        if let Some(telemetry) = self.telemetry.as_ref() {
            telemetry.record_failure(lane, error, delay).await;
        }
    }

    pub async fn run(&self, mut shutdown_rx: watch::Receiver<bool>) {
        let mut connect_interval = tokio::time::interval(self.profile.connect_retry_interval);
        let mut heartbeat_interval = tokio::time::interval(self.profile.heartbeat_interval);
        let mut egress_interval = tokio::time::interval(self.profile.egress_interval);
        let mut inbox_interval = tokio::time::interval(self.profile.inbox_refresh_interval);
        let mut connect_gate = RetryBackoffGate::new(RetryBackoffPolicy::new(
            self.profile.connect_retry_interval,
            scale_duration(self.profile.connect_retry_interval, 8),
        ));
        let mut heartbeat_gate = RetryBackoffGate::new(RetryBackoffPolicy::new(
            self.profile.heartbeat_interval,
            scale_duration(self.profile.heartbeat_interval, 4),
        ));
        let mut egress_gate = RetryBackoffGate::new(RetryBackoffPolicy::new(
            self.profile.egress_interval,
            scale_duration(self.profile.egress_interval, 8),
        ));
        let mut inbox_gate = RetryBackoffGate::new(RetryBackoffPolicy::new(
            self.profile.inbox_refresh_interval,
            scale_duration(self.profile.inbox_refresh_interval, 8),
        ));

        connect_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        heartbeat_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        egress_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        inbox_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        // #6516: quiet-backoff for the egress/inbox backstop polls. Heartbeat and
        // connect keep their base cadence (session keep-alive / cheap no-op).
        let mut egress_backoff =
            PollBackoff::new(self.profile.egress_interval, POLL_BACKOFF_MAX_FACTOR);
        let mut inbox_backoff =
            PollBackoff::new(self.profile.inbox_refresh_interval, POLL_BACKOFF_MAX_FACTOR);

        loop {
            tokio::select! {
                _ = connect_interval.tick() => {
                    let now = tokio::time::Instant::now();
                    if !connect_gate.is_ready(now) {
                        continue;
                    }
                    if let Err(error) = self.run_connect_cycle().await {
                        let delay = connect_gate.on_failure(now, &core_to_network_error(&error));
                        self.record_cycle_failure(IntegrationRuntimeLane::Connect, &error, delay).await;
                        warn!(error = %error, retry_in_ms = delay.as_millis() as u64, "integration runtime connect cycle failed");
                    } else {
                        connect_gate.on_success();
                        self.record_cycle_success(IntegrationRuntimeLane::Connect).await;
                    }
                }
                _ = heartbeat_interval.tick() => {
                    let now = tokio::time::Instant::now();
                    if !heartbeat_gate.is_ready(now) {
                        continue;
                    }
                    match self.run_heartbeat_cycle().await {
                        Ok(HeartbeatOutcome::Sent) => {
                            heartbeat_gate.on_success();
                            self.record_cycle_success(IntegrationRuntimeLane::Heartbeat).await;
                        }
                        Ok(HeartbeatOutcome::Skipped) => {
                            // #7617 (LOW finding #7): no ready session existed, so no
                            // heartbeat was actually transmitted. Deliberately do NOT
                            // call on_success()/record_cycle_success — that would make
                            // the Heartbeat telemetry lane look healthy while the
                            // integration is actually down (e.g. Failed after a connect
                            // failure). Leave the gate/telemetry state untouched rather
                            // than recording a failure either — there was no session to
                            // even attempt a heartbeat against.
                        }
                        Err(error) => {
                            let delay = heartbeat_gate.on_failure(now, &core_to_network_error(&error));
                            self.record_cycle_failure(IntegrationRuntimeLane::Heartbeat, &error, delay).await;
                            warn!(error = %error, retry_in_ms = delay.as_millis() as u64, "integration runtime heartbeat cycle failed");
                        }
                    }
                }
                _ = egress_interval.tick() => {
                    let now = tokio::time::Instant::now();
                    if !egress_gate.is_ready(now) {
                        continue;
                    }
                    match self.run_egress_cycle().await {
                        Ok(flushed) => {
                            egress_gate.on_success();
                            self.record_cycle_success(IntegrationRuntimeLane::Egress).await;
                            // #6516: stretch the next backstop flush while quiet; an
                            // empty flush grows the delay, a non-empty one resets it.
                            egress_interval.reset_after(egress_backoff.after_cycle(flushed > 0));
                        }
                        Err(error) => {
                            let delay = egress_gate.on_failure(now, &core_to_network_error(&error));
                            self.record_cycle_failure(IntegrationRuntimeLane::Egress, &error, delay).await;
                            warn!(error = %error, retry_in_ms = delay.as_millis() as u64, "integration runtime egress cycle failed");
                        }
                    }
                }
                signal = self.wait_for_egress_signal(), if self.egress_signal.is_some() => {
                    let now = tokio::time::Instant::now();
                    match signal {
                        Ok(true) if egress_gate.is_ready(now) => {
                            if let Err(error) = self.run_egress_cycle().await {
                                let delay = egress_gate.on_failure(now, &core_to_network_error(&error));
                                self.record_cycle_failure(IntegrationRuntimeLane::Egress, &error, delay).await;
                                warn!(error = %error, retry_in_ms = delay.as_millis() as u64, "integration runtime signal-driven egress cycle failed");
                            } else {
                                egress_gate.on_success();
                                self.record_cycle_success(IntegrationRuntimeLane::Egress).await;
                                // #6516: a push signal means egress is active — snap the backstop back to base.
                                egress_interval.reset_after(egress_backoff.reset_to_base());
                            }
                        }
                        Ok(_) => {}
                        Err(error) => {
                            let delay = egress_gate.on_failure(now, &core_to_network_error(&error));
                            self.record_cycle_failure(IntegrationRuntimeLane::Egress, &error, delay).await;
                            warn!(error = %error, retry_in_ms = delay.as_millis() as u64, "integration runtime egress signal wait failed");
                        }
                    }
                }
                _ = inbox_interval.tick() => {
                    let now = tokio::time::Instant::now();
                    if !inbox_gate.is_ready(now) {
                        continue;
                    }
                    match self.run_inbox_cycle().await {
                        Ok(received) => {
                            inbox_gate.on_success();
                            self.record_cycle_success(IntegrationRuntimeLane::Inbox).await;
                            // #6516: stretch the next backstop poll while quiet. The
                            // inbox push signal still fires immediately on new inbound
                            // (resetting to base), so inbound latency stays bounded.
                            inbox_interval.reset_after(inbox_backoff.after_cycle(received > 0));
                        }
                        Err(error) => {
                            let delay = inbox_gate.on_failure(now, &core_to_network_error(&error));
                            self.record_cycle_failure(IntegrationRuntimeLane::Inbox, &error, delay).await;
                            warn!(error = %error, retry_in_ms = delay.as_millis() as u64, "integration runtime inbox cycle failed");
                        }
                    }
                }
                signal = self.wait_for_inbox_signal(), if self.inbox_signal.is_some() => {
                    let now = tokio::time::Instant::now();
                    match signal {
                        Ok(true) if inbox_gate.is_ready(now) => {
                            if let Err(error) = self.run_inbox_cycle().await {
                                let delay = inbox_gate.on_failure(now, &core_to_network_error(&error));
                                self.record_cycle_failure(IntegrationRuntimeLane::Inbox, &error, delay).await;
                                warn!(error = %error, retry_in_ms = delay.as_millis() as u64, "integration runtime signal-driven inbox cycle failed");
                            } else {
                                inbox_gate.on_success();
                                self.record_cycle_success(IntegrationRuntimeLane::Inbox).await;
                                // #6516: a push signal means new inbound — snap the backstop poll back to base.
                                inbox_interval.reset_after(inbox_backoff.reset_to_base());
                            }
                        }
                        Ok(_) => {}
                        Err(error) => {
                            let delay = inbox_gate.on_failure(now, &core_to_network_error(&error));
                            self.record_cycle_failure(IntegrationRuntimeLane::Inbox, &error, delay).await;
                            warn!(error = %error, retry_in_ms = delay.as_millis() as u64, "integration runtime inbox signal wait failed");
                        }
                    }
                }
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        break;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use chrono::Utc;
    use tokio::sync::{Mutex, Notify};

    use super::*;
    use crate::integration::test_support::FakeIntegrationSessionPort;
    use maekon_core::models::integration::{
        IntegrationAckCursor, IntegrationAuthScheme, IntegrationSessionStatus,
        IntegrationTransportKind,
    };

    #[derive(Default)]
    struct MockEgressPort {
        flush_calls: Arc<Mutex<usize>>,
    }

    #[async_trait]
    impl IntegrationEgressPort for MockEgressPort {
        async fn enqueue_message(
            &self,
            _envelope: maekon_core::models::integration::IntegrationEnvelope,
            _payload: maekon_core::models::integration::IntegrationOutboundPayload,
        ) -> Result<(), CoreError> {
            Ok(())
        }

        async fn flush(&self) -> Result<usize, CoreError> {
            *self.flush_calls.lock().await += 1;
            Ok(0)
        }

        async fn last_ack_cursor(&self) -> Result<Option<IntegrationAckCursor>, CoreError> {
            Ok(None)
        }
    }

    #[derive(Default)]
    struct MockInboxPort {
        refresh_calls: Arc<Mutex<usize>>,
    }

    #[async_trait]
    impl IntegrationInboxPort for MockInboxPort {
        async fn refresh(&self) -> Result<usize, CoreError> {
            *self.refresh_calls.lock().await += 1;
            Ok(0)
        }

        async fn list_pending(
            &self,
        ) -> Result<Vec<maekon_core::models::integration::StoredProactivePrompt>, CoreError>
        {
            Ok(Vec::new())
        }

        async fn acknowledge(&self, _prompt_id: &str) -> Result<(), CoreError> {
            Ok(())
        }

        async fn dismiss(
            &self,
            _prompt_id: &str,
            _reason: Option<String>,
        ) -> Result<(), CoreError> {
            Ok(())
        }

        async fn last_ack_cursor(&self) -> Result<Option<IntegrationAckCursor>, CoreError> {
            Ok(None)
        }
    }

    #[tokio::test]
    async fn egress_cycle_connects_before_flushing() {
        let session = Arc::new(FakeIntegrationSessionPort::new());
        let egress = Arc::new(MockEgressPort::default());
        let inbox = Arc::new(MockInboxPort::default());
        let runtime = IntegrationRuntimeLoop::new(
            session.clone(),
            egress.clone(),
            inbox,
            None,
            None,
            None,
            IntegrationRuntimeLoopProfile::default(),
        );

        runtime.run_egress_cycle().await.unwrap();

        assert_eq!(*session.connect_calls.lock().await, 1);
        assert_eq!(*egress.flush_calls.lock().await, 1);
    }

    #[tokio::test]
    async fn heartbeat_cycle_uses_existing_session() {
        let session = Arc::new(FakeIntegrationSessionPort::new());
        session
            .set_session(IntegrationSessionState {
                session_id: "session-runtime".to_string(),
                device_id: "device-1".to_string(),
                status: IntegrationSessionStatus::Connected,
                transport_kind: IntegrationTransportKind::WebSocket,
                auth_scheme: IntegrationAuthScheme::BearerToken,
                connected_at: Some(Utc::now()),
                last_heartbeat_at: None,
                requested_scopes: vec![IntegrationCapabilityScope::SessionManage],
                granted_scopes: vec![IntegrationCapabilityScope::SessionManage],
                ack_cursors: Vec::new(),
            })
            .await;
        let runtime = IntegrationRuntimeLoop::new(
            session.clone(),
            Arc::new(MockEgressPort::default()),
            Arc::new(MockInboxPort::default()),
            None,
            None,
            None,
            IntegrationRuntimeLoopProfile::default(),
        );

        let outcome = runtime.run_heartbeat_cycle().await.unwrap();

        assert_eq!(outcome, HeartbeatOutcome::Sent);
        assert_eq!(*session.connect_calls.lock().await, 0);
        assert_eq!(*session.heartbeat_calls.lock().await, 1);
    }

    /// #7617 (LOW finding #7): with no current session at all, the cycle
    /// must report `Skipped` (and must NOT attempt a heartbeat call), so the
    /// caller in `run()` does not record a false telemetry success.
    #[tokio::test]
    async fn heartbeat_cycle_skips_without_session() {
        let session = Arc::new(FakeIntegrationSessionPort::new());
        let runtime = IntegrationRuntimeLoop::new(
            session.clone(),
            Arc::new(MockEgressPort::default()),
            Arc::new(MockInboxPort::default()),
            None,
            None,
            None,
            IntegrationRuntimeLoopProfile::default(),
        );

        let outcome = runtime.run_heartbeat_cycle().await.unwrap();

        assert_eq!(outcome, HeartbeatOutcome::Skipped);
        assert_eq!(*session.heartbeat_calls.lock().await, 0);
    }

    /// #7617 (LOW finding #7): a session that exists but is `Failed` (e.g.
    /// after a connect failure) must also report `Skipped` rather than
    /// silently succeeding.
    #[tokio::test]
    async fn heartbeat_cycle_skips_for_failed_session() {
        let session = Arc::new(FakeIntegrationSessionPort::new());
        session
            .set_session(IntegrationSessionState {
                session_id: String::new(),
                device_id: "device-1".to_string(),
                status: IntegrationSessionStatus::Failed,
                transport_kind: IntegrationTransportKind::WebSocket,
                auth_scheme: IntegrationAuthScheme::BearerToken,
                connected_at: None,
                last_heartbeat_at: None,
                requested_scopes: vec![IntegrationCapabilityScope::SessionManage],
                granted_scopes: Vec::new(),
                ack_cursors: Vec::new(),
            })
            .await;
        let runtime = IntegrationRuntimeLoop::new(
            session.clone(),
            Arc::new(MockEgressPort::default()),
            Arc::new(MockInboxPort::default()),
            None,
            None,
            None,
            IntegrationRuntimeLoopProfile::default(),
        );

        let outcome = runtime.run_heartbeat_cycle().await.unwrap();

        assert_eq!(outcome, HeartbeatOutcome::Skipped);
        assert_eq!(*session.heartbeat_calls.lock().await, 0);
    }

    /// #7617 (MED finding #2): `session_satisfies_scopes` must exclude
    /// `Degraded` — a Degraded session is by definition "last heartbeat
    /// failed" (see `IntegrationSessionCoordinator::heartbeat`), so treating
    /// it as ready here would permanently skip the coordinator's own
    /// Degraded-revalidation/reconnect path (#6204).
    #[test]
    fn session_satisfies_scopes_excludes_degraded() {
        let scopes = vec![IntegrationCapabilityScope::SessionManage];
        let mut state = IntegrationSessionState {
            session_id: "session-1".to_string(),
            device_id: "device-1".to_string(),
            status: IntegrationSessionStatus::Connected,
            transport_kind: IntegrationTransportKind::WebSocket,
            auth_scheme: IntegrationAuthScheme::BearerToken,
            connected_at: Some(Utc::now()),
            last_heartbeat_at: None,
            requested_scopes: scopes.clone(),
            granted_scopes: scopes.clone(),
            ack_cursors: Vec::new(),
        };
        assert!(
            IntegrationRuntimeLoop::session_satisfies_scopes(&state, &scopes),
            "a Connected session with all requested scopes granted must satisfy readiness"
        );

        state.status = IntegrationSessionStatus::Degraded;
        assert!(
            !IntegrationRuntimeLoop::session_satisfies_scopes(&state, &scopes),
            "a Degraded session must NOT satisfy readiness -- it must force \
             ensure_session_ready to call connect() again"
        );
    }

    /// #7617 (MED finding #2): `ensure_session_ready` must call `connect()`
    /// again when the current session is Degraded, instead of treating a
    /// stale Degraded session as "ready" forever. This is the wiring half of
    /// the fix; `IntegrationSessionCoordinator`'s own Degraded-revalidation
    /// behaviour (single heartbeat retry, then a full reconnect on repeated
    /// failure) is covered separately by
    /// `session_coordinator.rs::connect_revalidates_degraded_session_with_heartbeat`
    /// and `::connect_reconnects_when_degraded_heartbeat_fails` -- together
    /// these prove the full composed recovery path with finding #1 (an
    /// unexpectedly dropped live_channel fails the next heartbeat, flipping
    /// the session to Degraded, which this check now escalates into a
    /// reconnect attempt).
    #[tokio::test]
    async fn ensure_session_ready_reconnects_when_session_is_degraded() {
        let session = Arc::new(FakeIntegrationSessionPort::new());
        session
            .connect(vec![IntegrationCapabilityScope::SessionManage])
            .await
            .unwrap();
        session.set_status(IntegrationSessionStatus::Degraded).await;

        let runtime = IntegrationRuntimeLoop::new(
            session.clone(),
            Arc::new(MockEgressPort::default()),
            Arc::new(MockInboxPort::default()),
            None,
            None,
            None,
            IntegrationRuntimeLoopProfile::default(),
        );

        runtime.ensure_session_ready().await.unwrap();

        assert_eq!(
            *session.connect_calls.lock().await,
            2,
            "a Degraded session must trigger a second connect() call from \
             ensure_session_ready (1 initial + 1 reconnect triggered by this fix)"
        );
    }

    #[derive(Default)]
    struct MockEgressSignalPort {
        notify: Arc<Notify>,
    }

    #[async_trait]
    impl IntegrationEgressSignalPort for MockEgressSignalPort {
        async fn wait_for_pending_egress(&self, timeout: Duration) -> Result<bool, CoreError> {
            match tokio::time::timeout(timeout, self.notify.notified()).await {
                Ok(_) => Ok(true),
                Err(_) => Ok(false),
            }
        }
    }

    #[derive(Default)]
    struct MockInboxSignalPort {
        notify: Arc<Notify>,
    }

    #[async_trait]
    impl IntegrationInboxSignalPort for MockInboxSignalPort {
        async fn wait_for_remote_prompt_signal(
            &self,
            timeout: Duration,
        ) -> Result<bool, CoreError> {
            match tokio::time::timeout(timeout, self.notify.notified()).await {
                Ok(_) => Ok(true),
                Err(_) => Ok(false),
            }
        }
    }

    #[tokio::test]
    async fn egress_signal_triggers_flush_between_interval_ticks() {
        let session = Arc::new(FakeIntegrationSessionPort::new());
        session.connect(Vec::new()).await.unwrap();
        let egress = Arc::new(MockEgressPort::default());
        let inbox = Arc::new(MockInboxPort::default());
        let egress_signal = Arc::new(MockEgressSignalPort::default());
        let runtime = IntegrationRuntimeLoop::new(
            session,
            egress.clone(),
            inbox,
            Some(egress_signal.clone()),
            None,
            None,
            IntegrationRuntimeLoopProfile {
                egress_interval: Duration::from_secs(30),
                inbox_refresh_interval: Duration::from_secs(30),
                ..IntegrationRuntimeLoopProfile::default()
            },
        );
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let task = tokio::spawn({
            let runtime = runtime.clone();
            async move { runtime.run(shutdown_rx).await }
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        egress_signal.notify.notify_waiters();
        tokio::time::sleep(Duration::from_millis(50)).await;
        shutdown_tx.send(true).unwrap();
        task.await.unwrap();

        assert!(*egress.flush_calls.lock().await >= 2);
    }

    #[tokio::test]
    async fn inbox_signal_triggers_refresh_between_interval_ticks() {
        let session = Arc::new(FakeIntegrationSessionPort::new());
        session
            .set_session(IntegrationSessionState {
                session_id: "session-runtime".to_string(),
                device_id: "device-1".to_string(),
                status: IntegrationSessionStatus::Connected,
                transport_kind: IntegrationTransportKind::WebSocket,
                auth_scheme: IntegrationAuthScheme::BearerToken,
                connected_at: Some(Utc::now()),
                last_heartbeat_at: None,
                requested_scopes: vec![IntegrationCapabilityScope::PromptRead],
                granted_scopes: vec![IntegrationCapabilityScope::PromptRead],
                ack_cursors: Vec::new(),
            })
            .await;
        let egress = Arc::new(MockEgressPort::default());
        let inbox = Arc::new(MockInboxPort::default());
        let inbox_signal = Arc::new(MockInboxSignalPort::default());
        let runtime = IntegrationRuntimeLoop::new(
            session,
            egress,
            inbox.clone(),
            None,
            Some(inbox_signal.clone()),
            None,
            IntegrationRuntimeLoopProfile {
                egress_interval: Duration::from_secs(30),
                inbox_refresh_interval: Duration::from_secs(30),
                ..IntegrationRuntimeLoopProfile::default()
            },
        );
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let task = tokio::spawn({
            let runtime = runtime.clone();
            async move { runtime.run(shutdown_rx).await }
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        inbox_signal.notify.notify_waiters();
        tokio::time::sleep(Duration::from_millis(50)).await;
        shutdown_tx.send(true).unwrap();
        task.await.unwrap();

        assert!(*inbox.refresh_calls.lock().await >= 2);
    }

    #[test]
    fn poll_backoff_grows_on_empty_and_resets_on_work() {
        let base = Duration::from_secs(15);
        let mut backoff = PollBackoff::new(base, POLL_BACKOFF_MAX_FACTOR);

        // Empty cycles grow the delay exponentially, capped at base * max_factor.
        assert_eq!(backoff.after_cycle(false), base * 2); // 1 empty → 2×
        assert_eq!(backoff.after_cycle(false), base * 4); // 2 empties → 4× (cap)
        assert_eq!(backoff.after_cycle(false), base * 4); // stays capped at 4×

        // A non-empty cycle snaps the cadence back to base...
        assert_eq!(backoff.after_cycle(true), base);
        assert_eq!(backoff.after_cycle(false), base * 2); // ...then grows again.

        // A push signal also resets to base.
        assert_eq!(backoff.reset_to_base(), base);
        assert_eq!(backoff.after_cycle(false), base * 2);
    }

    #[test]
    fn poll_backoff_max_factor_clamped_to_at_least_one() {
        let base = Duration::from_secs(10);
        // max_factor 0 is clamped to 1 → cadence never deviates from base.
        let mut backoff = PollBackoff::new(base, 0);
        assert_eq!(backoff.after_cycle(false), base);
        assert_eq!(backoff.after_cycle(false), base);
    }
}
