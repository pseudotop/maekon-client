use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use oneshim_core::error::CoreError;
use oneshim_core::models::integration::{
    IntegrationRuntimeLaneTelemetry, IntegrationRuntimeTelemetry,
};
use oneshim_core::ports::integration::IntegrationRuntimeTelemetryPort;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Copy)]
pub enum IntegrationRuntimeLane {
    Connect,
    Heartbeat,
    Egress,
    Inbox,
}

#[derive(Clone, Default)]
pub struct IntegrationRuntimeTelemetryHandle {
    inner: Arc<RwLock<IntegrationRuntimeTelemetry>>,
}

impl IntegrationRuntimeTelemetryHandle {
    pub async fn record_success(&self, lane: IntegrationRuntimeLane) {
        let mut telemetry = self.inner.write().await;
        let lane_state = lane_state_mut(&mut telemetry, lane);
        lane_state.consecutive_failures = 0;
        lane_state.last_success_at = Some(Utc::now());
        lane_state.backoff_until = None;
        lane_state.last_error = None;
    }

    pub async fn record_failure(
        &self,
        lane: IntegrationRuntimeLane,
        error: &CoreError,
        delay: Duration,
    ) {
        let message = error.to_string();
        let mut telemetry = self.inner.write().await;
        let lane_state = lane_state_mut(&mut telemetry, lane);
        lane_state.consecutive_failures = lane_state.consecutive_failures.saturating_add(1);
        lane_state.last_failure_at = Some(Utc::now());
        lane_state.backoff_until = chrono::Duration::from_std(delay)
            .ok()
            .map(|delay| Utc::now() + delay);
        lane_state.last_error = Some(message);
    }
}

fn lane_state_mut(
    telemetry: &mut IntegrationRuntimeTelemetry,
    lane: IntegrationRuntimeLane,
) -> &mut IntegrationRuntimeLaneTelemetry {
    match lane {
        IntegrationRuntimeLane::Connect => &mut telemetry.connect,
        IntegrationRuntimeLane::Heartbeat => &mut telemetry.heartbeat,
        IntegrationRuntimeLane::Egress => &mut telemetry.egress,
        IntegrationRuntimeLane::Inbox => &mut telemetry.inbox,
    }
}

#[async_trait]
impl IntegrationRuntimeTelemetryPort for IntegrationRuntimeTelemetryHandle {
    async fn snapshot(&self) -> Result<IntegrationRuntimeTelemetry, CoreError> {
        Ok(self.inner.read().await.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn telemetry_records_success_and_failure() {
        let telemetry = IntegrationRuntimeTelemetryHandle::default();
        telemetry
            .record_failure(
                IntegrationRuntimeLane::Egress,
                &CoreError::ServiceUnavailable {
                    code: oneshim_core::error_codes::ServiceCode::Unavailable,
                    message: "egress unavailable".to_string(),
                },
                Duration::from_secs(5),
            )
            .await;

        let failed = telemetry.snapshot().await.unwrap();
        assert_eq!(failed.egress.consecutive_failures, 1);
        assert!(failed.egress.last_failure_at.is_some());
        assert!(failed.egress.backoff_until.is_some());
        assert!(failed
            .egress
            .last_error
            .as_deref()
            .is_some_and(|message| message.contains("egress unavailable")));

        telemetry
            .record_success(IntegrationRuntimeLane::Egress)
            .await;

        let recovered = telemetry.snapshot().await.unwrap();
        assert_eq!(recovered.egress.consecutive_failures, 0);
        assert!(recovered.egress.last_success_at.is_some());
        assert!(recovered.egress.backoff_until.is_none());
        assert!(recovered.egress.last_error.is_none());
    }
}
