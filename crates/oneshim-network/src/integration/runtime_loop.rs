use std::sync::Arc;
use std::time::Duration;

use futures::future::pending;
use oneshim_core::error::CoreError;
use oneshim_core::models::integration::{
    IntegrationCapabilityScope, IntegrationSessionState, IntegrationSessionStatus,
};
use oneshim_core::ports::integration::{
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
            code: oneshim_core::error_codes::NetworkCode::RateLimit,
            retry_after_secs,
        } => NetworkError::RateLimited {
            retry_after_secs: *retry_after_secs,
        },
        CoreError::RequestTimeout {
            code: oneshim_core::error_codes::NetworkCode::Timeout,
            timeout_ms,
        } => NetworkError::Timeout {
            timeout_ms: *timeout_ms,
        },
        CoreError::ServiceUnavailable {
            code: oneshim_core::error_codes::ServiceCode::Unavailable,
            message: msg,
        } => NetworkError::ServiceUnavailable(msg.clone()),
        CoreError::Auth {
            code: oneshim_core::error_codes::AuthCode::Failed,
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
        matches!(
            session.status,
            IntegrationSessionStatus::Connected | IntegrationSessionStatus::Degraded
        ) && !session.session_id.is_empty()
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

    async fn run_heartbeat_cycle(&self) -> Result<(), CoreError> {
        let Some(current) = self.session.current_session().await? else {
            return Ok(());
        };

        if matches!(
            current.status,
            IntegrationSessionStatus::Connected | IntegrationSessionStatus::Degraded
        ) && !current.session_id.is_empty()
        {
            self.session.heartbeat(&current.session_id).await?;
        }

        Ok(())
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
                    if let Err(error) = self.run_heartbeat_cycle().await {
                        let delay = heartbeat_gate.on_failure(now, &core_to_network_error(&error));
                        self.record_cycle_failure(IntegrationRuntimeLane::Heartbeat, &error, delay).await;
                        warn!(error = %error, retry_in_ms = delay.as_millis() as u64, "integration runtime heartbeat cycle failed");
                    } else {
                        heartbeat_gate.on_success();
                        self.record_cycle_success(IntegrationRuntimeLane::Heartbeat).await;
                    }
                }
                _ = egress_interval.tick() => {
                    let now = tokio::time::Instant::now();
                    if !egress_gate.is_ready(now) {
                        continue;
                    }
                    if let Err(error) = self.run_egress_cycle().await {
                        let delay = egress_gate.on_failure(now, &core_to_network_error(&error));
                        self.record_cycle_failure(IntegrationRuntimeLane::Egress, &error, delay).await;
                        warn!(error = %error, retry_in_ms = delay.as_millis() as u64, "integration runtime egress cycle failed");
                    } else {
                        egress_gate.on_success();
                        self.record_cycle_success(IntegrationRuntimeLane::Egress).await;
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
                    if let Err(error) = self.run_inbox_cycle().await {
                        let delay = inbox_gate.on_failure(now, &core_to_network_error(&error));
                        self.record_cycle_failure(IntegrationRuntimeLane::Inbox, &error, delay).await;
                        warn!(error = %error, retry_in_ms = delay.as_millis() as u64, "integration runtime inbox cycle failed");
                    } else {
                        inbox_gate.on_success();
                        self.record_cycle_success(IntegrationRuntimeLane::Inbox).await;
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
    use oneshim_core::models::integration::{
        IntegrationAckCursor, IntegrationAuthScheme, IntegrationSessionStatus,
        IntegrationTransportKind,
    };

    #[derive(Default)]
    struct MockSessionPort {
        current: Arc<Mutex<Option<IntegrationSessionState>>>,
        connect_calls: Arc<Mutex<usize>>,
        heartbeat_calls: Arc<Mutex<usize>>,
    }

    #[async_trait]
    impl IntegrationSessionPort for MockSessionPort {
        async fn connect(
            &self,
            requested_scopes: Vec<IntegrationCapabilityScope>,
        ) -> Result<IntegrationSessionState, CoreError> {
            *self.connect_calls.lock().await += 1;
            let session = IntegrationSessionState {
                session_id: "session-runtime".to_string(),
                device_id: "device-1".to_string(),
                status: IntegrationSessionStatus::Connected,
                transport_kind: IntegrationTransportKind::WebSocket,
                auth_scheme: IntegrationAuthScheme::BearerToken,
                connected_at: Some(Utc::now()),
                last_heartbeat_at: None,
                requested_scopes: requested_scopes.clone(),
                granted_scopes: requested_scopes,
                ack_cursors: Vec::new(),
            };
            *self.current.lock().await = Some(session.clone());
            Ok(session)
        }

        async fn current_session(&self) -> Result<Option<IntegrationSessionState>, CoreError> {
            Ok(self.current.lock().await.clone())
        }

        async fn heartbeat(&self, _session_id: &str) -> Result<IntegrationSessionState, CoreError> {
            *self.heartbeat_calls.lock().await += 1;
            let session =
                self.current
                    .lock()
                    .await
                    .clone()
                    .ok_or_else(|| CoreError::ServiceUnavailable {
                        code: oneshim_core::error_codes::ServiceCode::Unavailable,
                        message: "integration session missing".to_string(),
                    })?;
            Ok(session)
        }

        async fn store_ack_cursor(
            &self,
            _session_id: &str,
            _cursor: IntegrationAckCursor,
        ) -> Result<IntegrationSessionState, CoreError> {
            self.current_session()
                .await?
                .ok_or_else(|| CoreError::ServiceUnavailable {
                    code: oneshim_core::error_codes::ServiceCode::Unavailable,
                    message: "integration session missing".to_string(),
                })
        }

        async fn disconnect(&self, _session_id: &str) -> Result<(), CoreError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct MockEgressPort {
        flush_calls: Arc<Mutex<usize>>,
    }

    #[async_trait]
    impl IntegrationEgressPort for MockEgressPort {
        async fn enqueue_message(
            &self,
            _envelope: oneshim_core::models::integration::IntegrationEnvelope,
            _payload: oneshim_core::models::integration::IntegrationOutboundPayload,
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
        ) -> Result<Vec<oneshim_core::models::integration::StoredProactivePrompt>, CoreError>
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
        let session = Arc::new(MockSessionPort::default());
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
        let session = Arc::new(MockSessionPort::default());
        session
            .current
            .lock()
            .await
            .replace(IntegrationSessionState {
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
            });
        let runtime = IntegrationRuntimeLoop::new(
            session.clone(),
            Arc::new(MockEgressPort::default()),
            Arc::new(MockInboxPort::default()),
            None,
            None,
            None,
            IntegrationRuntimeLoopProfile::default(),
        );

        runtime.run_heartbeat_cycle().await.unwrap();

        assert_eq!(*session.connect_calls.lock().await, 0);
        assert_eq!(*session.heartbeat_calls.lock().await, 1);
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
        let session = Arc::new(MockSessionPort::default());
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
        let session = Arc::new(MockSessionPort::default());
        session
            .current
            .lock()
            .await
            .replace(IntegrationSessionState {
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
            });
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
}
