use std::sync::Arc;

use async_trait::async_trait;
use oneshim_core::error::CoreError;
use oneshim_core::models::integration::{
    IntegrationAckCursor, IntegrationAuthScheme, IntegrationCapabilityScope,
    IntegrationSessionState, IntegrationSessionStatus, IntegrationTransportKind,
};
use oneshim_core::ports::integration::{IntegrationSessionPort, IntegrationSessionStorePort};
use tokio::sync::RwLock;

use super::transport::{IntegrationTransportClient, IntegrationTransportConnectRequest};
use tracing::debug;

#[derive(Debug, Clone)]
pub struct IntegrationSessionRuntimeProfile {
    pub client_version: String,
    pub device_label: Option<String>,
    pub preferred_transports: Vec<IntegrationTransportKind>,
    pub supported_auth_schemes: Vec<IntegrationAuthScheme>,
    pub resource_indicator: Option<String>,
}

impl Default for IntegrationSessionRuntimeProfile {
    fn default() -> Self {
        Self {
            client_version: env!("CARGO_PKG_VERSION").to_string(),
            device_label: None,
            preferred_transports: vec![
                IntegrationTransportKind::WebSocket,
                IntegrationTransportKind::HttpsSse,
                IntegrationTransportKind::HttpsLongPoll,
            ],
            supported_auth_schemes: vec![
                IntegrationAuthScheme::DpopBearer,
                IntegrationAuthScheme::BearerToken,
            ],
            resource_indicator: None,
        }
    }
}

pub struct IntegrationSessionCoordinator {
    device_id: String,
    profile: IntegrationSessionRuntimeProfile,
    transport: Arc<dyn IntegrationTransportClient>,
    session_store: Option<Arc<dyn IntegrationSessionStorePort>>,
    state: Arc<RwLock<Option<IntegrationSessionState>>>,
}

impl IntegrationSessionCoordinator {
    pub fn new(
        device_id: impl Into<String>,
        transport: Arc<dyn IntegrationTransportClient>,
    ) -> Self {
        Self::new_with_profile(
            device_id,
            transport,
            IntegrationSessionRuntimeProfile::default(),
        )
    }

    pub fn new_with_profile(
        device_id: impl Into<String>,
        transport: Arc<dyn IntegrationTransportClient>,
        profile: IntegrationSessionRuntimeProfile,
    ) -> Self {
        Self::new_with_profile_and_store(device_id, transport, profile, None)
    }

    pub fn new_with_profile_and_store(
        device_id: impl Into<String>,
        transport: Arc<dyn IntegrationTransportClient>,
        profile: IntegrationSessionRuntimeProfile,
        session_store: Option<Arc<dyn IntegrationSessionStorePort>>,
    ) -> Self {
        Self {
            device_id: device_id.into(),
            profile,
            transport,
            session_store,
            state: Arc::new(RwLock::new(None)),
        }
    }

    fn scopes_satisfied(
        state: &IntegrationSessionState,
        requested_scopes: &[IntegrationCapabilityScope],
    ) -> bool {
        requested_scopes
            .iter()
            .all(|scope| state.granted_scopes.contains(scope))
    }

    async fn set_failed_state(&self, requested_scopes: Vec<IntegrationCapabilityScope>) {
        let state = IntegrationSessionState {
            session_id: String::new(),
            device_id: self.device_id.clone(),
            status: IntegrationSessionStatus::Failed,
            transport_kind: IntegrationTransportKind::default(),
            auth_scheme: IntegrationAuthScheme::default(),
            connected_at: None,
            last_heartbeat_at: None,
            requested_scopes,
            granted_scopes: Vec::new(),
            ack_cursors: Vec::new(),
        };
        let mut guard = self.state.write().await;
        *guard = Some(state.clone());
        drop(guard);
        if let Err(e) = self.persist_state(state).await {
            debug!("persist_state failed: {e}");
        }
    }

    async fn persist_state(&self, state: IntegrationSessionState) -> Result<(), CoreError> {
        if let Some(store) = &self.session_store {
            store.store(state).await?;
        }
        Ok(())
    }

    async fn clear_persisted_state(&self) -> Result<(), CoreError> {
        if let Some(store) = &self.session_store {
            store.clear().await?;
        }
        Ok(())
    }

    async fn load_persisted_state(&self) -> Result<Option<IntegrationSessionState>, CoreError> {
        if let Some(existing) = self.state.read().await.clone() {
            return Ok(Some(existing));
        }

        let Some(store) = &self.session_store else {
            return Ok(None);
        };

        let loaded = store.load().await?;
        if let Some(state) = loaded.clone() {
            let mut guard = self.state.write().await;
            if guard.is_none() {
                *guard = Some(state);
            }
        }
        Ok(loaded)
    }
}

#[async_trait]
impl IntegrationSessionPort for IntegrationSessionCoordinator {
    async fn connect(
        &self,
        requested_scopes: Vec<IntegrationCapabilityScope>,
    ) -> Result<IntegrationSessionState, CoreError> {
        if let Some(existing) = self.load_persisted_state().await? {
            if Self::scopes_satisfied(&existing, &requested_scopes) {
                match existing.status {
                    IntegrationSessionStatus::Connected => return Ok(existing),
                    IntegrationSessionStatus::Degraded if !existing.session_id.is_empty() => {
                        if let Ok(state) = self.heartbeat(&existing.session_id).await {
                            return Ok(state);
                        }
                    }
                    _ => {}
                }
            }
        }

        {
            let mut guard = self.state.write().await;
            *guard = Some(IntegrationSessionState {
                session_id: String::new(),
                device_id: self.device_id.clone(),
                status: IntegrationSessionStatus::Connecting,
                transport_kind: self
                    .profile
                    .preferred_transports
                    .first()
                    .cloned()
                    .unwrap_or_default(),
                auth_scheme: self
                    .profile
                    .supported_auth_schemes
                    .last()
                    .cloned()
                    .unwrap_or_default(),
                connected_at: None,
                last_heartbeat_at: None,
                requested_scopes: requested_scopes.clone(),
                granted_scopes: Vec::new(),
                ack_cursors: Vec::new(),
            });
        }

        let response = match self
            .transport
            .connect(IntegrationTransportConnectRequest {
                device_id: self.device_id.clone(),
                client_version: self.profile.client_version.clone(),
                device_label: self.profile.device_label.clone(),
                requested_scopes: requested_scopes.clone(),
                preferred_transports: self.profile.preferred_transports.clone(),
                supported_auth_schemes: self.profile.supported_auth_schemes.clone(),
                resource_indicator: self.profile.resource_indicator.clone(),
            })
            .await
        {
            Ok(response) => response,
            Err(err) => {
                self.set_failed_state(requested_scopes).await;
                return Err(err);
            }
        };

        let state = IntegrationSessionState {
            session_id: response.session_id,
            device_id: self.device_id.clone(),
            status: IntegrationSessionStatus::Connected,
            transport_kind: response.transport_kind,
            auth_scheme: response.auth_scheme,
            connected_at: Some(response.connected_at),
            last_heartbeat_at: Some(response.connected_at),
            requested_scopes,
            granted_scopes: response.granted_scopes,
            ack_cursors: Vec::new(),
        };

        let mut guard = self.state.write().await;
        *guard = Some(state.clone());
        drop(guard);
        self.persist_state(state.clone()).await?;
        Ok(state)
    }

    async fn current_session(&self) -> Result<Option<IntegrationSessionState>, CoreError> {
        self.load_persisted_state().await
    }

    async fn heartbeat(&self, session_id: &str) -> Result<IntegrationSessionState, CoreError> {
        let heartbeat_at = match self.transport.heartbeat(session_id).await {
            Ok(value) => value,
            Err(err) => {
                let mut guard = self.state.write().await;
                if let Some(state) = guard.as_mut() {
                    if state.session_id == session_id {
                        state.status = IntegrationSessionStatus::Degraded;
                    }
                }
                return Err(err);
            }
        };

        let mut guard = self.state.write().await;
        let state = guard.as_mut().ok_or_else(|| CoreError::NotFound {
            code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
            resource_type: "integration_session".to_string(),
            id: session_id.to_string(),
        })?;

        if state.session_id != session_id {
            return Err(CoreError::NotFound {
                code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
                resource_type: "integration_session".to_string(),
                id: session_id.to_string(),
            });
        }

        state.status = IntegrationSessionStatus::Connected;
        state.last_heartbeat_at = Some(heartbeat_at);
        let updated = state.clone();
        drop(guard);
        self.persist_state(updated.clone()).await?;
        Ok(updated)
    }

    async fn store_ack_cursor(
        &self,
        session_id: &str,
        cursor: IntegrationAckCursor,
    ) -> Result<IntegrationSessionState, CoreError> {
        let mut guard = self.state.write().await;
        let state = guard.as_mut().ok_or_else(|| CoreError::NotFound {
            code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
            resource_type: "integration_session".to_string(),
            id: session_id.to_string(),
        })?;

        if state.session_id != session_id {
            return Err(CoreError::NotFound {
                code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
                resource_type: "integration_session".to_string(),
                id: session_id.to_string(),
            });
        }

        if let Some(existing) = state
            .ack_cursors
            .iter_mut()
            .find(|existing| existing.stream_id == cursor.stream_id)
        {
            *existing = cursor;
        } else {
            state.ack_cursors.push(cursor);
        }
        let updated = state.clone();
        drop(guard);
        self.persist_state(updated.clone()).await?;
        Ok(updated)
    }

    async fn disconnect(&self, session_id: &str) -> Result<(), CoreError> {
        self.transport.disconnect(session_id).await?;
        let mut guard = self.state.write().await;
        if let Some(state) = guard.as_mut() {
            if state.session_id == session_id {
                state.status = IntegrationSessionStatus::Disconnected;
            }
        }
        *guard = None;
        drop(guard);
        self.clear_persisted_state().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use chrono::{DateTime, Utc};
    use oneshim_core::error::CoreError;
    use oneshim_core::models::integration::{
        IntegrationAckCursor, IntegrationAuthScheme, IntegrationCapabilityScope,
        IntegrationSessionState, IntegrationSessionStatus, IntegrationTransportKind,
    };
    use oneshim_core::ports::integration::IntegrationSessionStorePort;
    use tokio::sync::Mutex;

    use super::*;
    use crate::integration::transport::IntegrationTransportConnectResponse;

    struct MockTransport {
        calls: Arc<Mutex<Vec<String>>>,
        fail_connect: bool,
        fail_heartbeat: bool,
    }

    #[derive(Default)]
    struct MockSessionStore {
        state: Arc<Mutex<Option<IntegrationSessionState>>>,
    }

    #[async_trait]
    impl IntegrationSessionStorePort for MockSessionStore {
        async fn load(&self) -> Result<Option<IntegrationSessionState>, CoreError> {
            Ok(self.state.lock().await.clone())
        }

        async fn store(&self, state: IntegrationSessionState) -> Result<(), CoreError> {
            *self.state.lock().await = Some(state);
            Ok(())
        }

        async fn clear(&self) -> Result<(), CoreError> {
            *self.state.lock().await = None;
            Ok(())
        }
    }

    #[async_trait]
    impl IntegrationTransportClient for MockTransport {
        async fn connect(
            &self,
            request: IntegrationTransportConnectRequest,
        ) -> Result<IntegrationTransportConnectResponse, CoreError> {
            self.calls
                .lock()
                .await
                .push(format!("connect:{}", request.device_id));
            if self.fail_connect {
                return Err(CoreError::ServiceUnavailable {
                    code: oneshim_core::error_codes::ServiceCode::Unavailable,
                    message: "transport unavailable".to_string(),
                });
            }
            Ok(IntegrationTransportConnectResponse {
                session_id: "integration-session-1".to_string(),
                connected_at: Utc::now(),
                granted_scopes: request.requested_scopes,
                transport_kind: request
                    .preferred_transports
                    .first()
                    .cloned()
                    .unwrap_or_default(),
                auth_scheme: request
                    .supported_auth_schemes
                    .first()
                    .cloned()
                    .unwrap_or_default(),
            })
        }

        async fn heartbeat(&self, session_id: &str) -> Result<DateTime<Utc>, CoreError> {
            self.calls
                .lock()
                .await
                .push(format!("heartbeat:{session_id}"));
            if self.fail_heartbeat {
                return Err(CoreError::ServiceUnavailable {
                    code: oneshim_core::error_codes::ServiceCode::Unavailable,
                    message: "heartbeat unavailable".to_string(),
                });
            }
            Ok(Utc::now())
        }

        async fn disconnect(&self, session_id: &str) -> Result<(), CoreError> {
            self.calls
                .lock()
                .await
                .push(format!("disconnect:{session_id}"));
            Ok(())
        }
    }

    #[tokio::test]
    async fn connect_creates_connected_session() {
        let coordinator = IntegrationSessionCoordinator::new(
            "device-1",
            Arc::new(MockTransport {
                calls: Arc::new(Mutex::new(Vec::new())),
                fail_connect: false,
                fail_heartbeat: false,
            }),
        );

        let session = coordinator
            .connect(vec![IntegrationCapabilityScope::InsightWrite])
            .await
            .unwrap();

        assert_eq!(session.session_id, "integration-session-1");
        assert_eq!(session.status, IntegrationSessionStatus::Connected);
        assert_eq!(session.granted_scopes.len(), 1);
        assert_eq!(session.transport_kind, IntegrationTransportKind::WebSocket);
        assert_eq!(session.auth_scheme, IntegrationAuthScheme::DpopBearer);
    }

    #[tokio::test]
    async fn connect_reuses_existing_session_when_scopes_are_satisfied() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let coordinator = IntegrationSessionCoordinator::new(
            "device-1",
            Arc::new(MockTransport {
                calls: calls.clone(),
                fail_connect: false,
                fail_heartbeat: false,
            }),
        );

        coordinator
            .connect(vec![
                IntegrationCapabilityScope::InsightWrite,
                IntegrationCapabilityScope::PromptRead,
            ])
            .await
            .unwrap();
        coordinator
            .connect(vec![IntegrationCapabilityScope::InsightWrite])
            .await
            .unwrap();

        let recorded = calls.lock().await;
        assert_eq!(
            recorded
                .iter()
                .filter(|entry| entry.starts_with("connect:"))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn connect_failure_moves_state_to_failed() {
        let coordinator = IntegrationSessionCoordinator::new(
            "device-1",
            Arc::new(MockTransport {
                calls: Arc::new(Mutex::new(Vec::new())),
                fail_connect: true,
                fail_heartbeat: false,
            }),
        );

        let err = coordinator
            .connect(vec![IntegrationCapabilityScope::SessionManage])
            .await
            .expect_err("connect should fail");
        assert!(matches!(err, CoreError::ServiceUnavailable { .. }));

        let state = coordinator.current_session().await.unwrap().unwrap();
        assert_eq!(state.status, IntegrationSessionStatus::Failed);
    }

    #[tokio::test]
    async fn heartbeat_updates_session_state() {
        let coordinator = IntegrationSessionCoordinator::new(
            "device-1",
            Arc::new(MockTransport {
                calls: Arc::new(Mutex::new(Vec::new())),
                fail_connect: false,
                fail_heartbeat: false,
            }),
        );

        let session = coordinator
            .connect(vec![IntegrationCapabilityScope::SessionManage])
            .await
            .unwrap();

        let updated = coordinator.heartbeat(&session.session_id).await.unwrap();
        assert_eq!(updated.status, IntegrationSessionStatus::Connected);
        assert!(updated.last_heartbeat_at.is_some());
    }

    #[tokio::test]
    async fn disconnect_clears_current_session() {
        let coordinator = IntegrationSessionCoordinator::new(
            "device-1",
            Arc::new(MockTransport {
                calls: Arc::new(Mutex::new(Vec::new())),
                fail_connect: false,
                fail_heartbeat: false,
            }),
        );

        let session = coordinator
            .connect(vec![IntegrationCapabilityScope::SessionManage])
            .await
            .unwrap();

        coordinator.disconnect(&session.session_id).await.unwrap();
        assert!(coordinator.current_session().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn connect_revalidates_degraded_session_with_heartbeat() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let coordinator = IntegrationSessionCoordinator::new(
            "device-1",
            Arc::new(MockTransport {
                calls: calls.clone(),
                fail_connect: false,
                fail_heartbeat: false,
            }),
        );

        let session = coordinator
            .connect(vec![IntegrationCapabilityScope::InsightWrite])
            .await
            .unwrap();
        coordinator.state.write().await.as_mut().unwrap().status =
            IntegrationSessionStatus::Degraded;

        let reused = coordinator
            .connect(vec![IntegrationCapabilityScope::InsightWrite])
            .await
            .unwrap();

        assert_eq!(reused.session_id, session.session_id);
        let recorded = calls.lock().await;
        assert_eq!(
            recorded
                .iter()
                .filter(|entry| entry.starts_with("connect:"))
                .count(),
            1
        );
        assert_eq!(
            recorded
                .iter()
                .filter(|entry| entry.starts_with("heartbeat:"))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn connect_reconnects_when_degraded_heartbeat_fails() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let coordinator = IntegrationSessionCoordinator::new(
            "device-1",
            Arc::new(MockTransport {
                calls: calls.clone(),
                fail_connect: false,
                fail_heartbeat: true,
            }),
        );

        coordinator
            .connect(vec![IntegrationCapabilityScope::InsightWrite])
            .await
            .unwrap();
        coordinator.state.write().await.as_mut().unwrap().status =
            IntegrationSessionStatus::Degraded;

        let session = coordinator
            .connect(vec![IntegrationCapabilityScope::InsightWrite])
            .await
            .unwrap();

        assert_eq!(session.status, IntegrationSessionStatus::Connected);
        let recorded = calls.lock().await;
        assert_eq!(
            recorded
                .iter()
                .filter(|entry| entry.starts_with("connect:"))
                .count(),
            2
        );
        assert_eq!(
            recorded
                .iter()
                .filter(|entry| entry.starts_with("heartbeat:"))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn store_ack_cursor_updates_session_state() {
        let coordinator = IntegrationSessionCoordinator::new(
            "device-1",
            Arc::new(MockTransport {
                calls: Arc::new(Mutex::new(Vec::new())),
                fail_connect: false,
                fail_heartbeat: false,
            }),
        );

        let session = coordinator
            .connect(vec![IntegrationCapabilityScope::SessionManage])
            .await
            .unwrap();

        let updated = coordinator
            .store_ack_cursor(
                &session.session_id,
                IntegrationAckCursor {
                    stream_id: "insights".to_string(),
                    cursor: "cursor-1".to_string(),
                    acknowledged_at: Utc::now(),
                },
            )
            .await
            .unwrap();

        assert_eq!(updated.ack_cursors.len(), 1);
        assert_eq!(updated.ack_cursors[0].cursor, "cursor-1");
    }

    #[tokio::test]
    async fn current_session_loads_persisted_state_when_memory_is_empty() {
        let store = Arc::new(MockSessionStore::default());
        store
            .store(IntegrationSessionState {
                session_id: "persisted-session".to_string(),
                device_id: "device-1".to_string(),
                status: IntegrationSessionStatus::Connected,
                transport_kind: IntegrationTransportKind::WebSocket,
                auth_scheme: IntegrationAuthScheme::BearerToken,
                connected_at: Some(Utc::now()),
                last_heartbeat_at: Some(Utc::now()),
                requested_scopes: vec![IntegrationCapabilityScope::InsightWrite],
                granted_scopes: vec![IntegrationCapabilityScope::InsightWrite],
                ack_cursors: vec![],
            })
            .await
            .unwrap();

        let coordinator = IntegrationSessionCoordinator::new_with_profile_and_store(
            "device-1",
            Arc::new(MockTransport {
                calls: Arc::new(Mutex::new(Vec::new())),
                fail_connect: false,
                fail_heartbeat: false,
            }),
            IntegrationSessionRuntimeProfile::default(),
            Some(store),
        );

        let session = coordinator.current_session().await.unwrap().unwrap();
        assert_eq!(session.session_id, "persisted-session");
        assert_eq!(session.status, IntegrationSessionStatus::Connected);
    }

    #[tokio::test]
    async fn connect_uses_runtime_profile_metadata() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let coordinator = IntegrationSessionCoordinator::new_with_profile(
            "device-1",
            Arc::new(MockTransport {
                calls: calls.clone(),
                fail_connect: false,
                fail_heartbeat: false,
            }),
            IntegrationSessionRuntimeProfile {
                client_version: "9.9.9".to_string(),
                device_label: Some("workstation".to_string()),
                preferred_transports: vec![IntegrationTransportKind::HttpsLongPoll],
                supported_auth_schemes: vec![IntegrationAuthScheme::BearerToken],
                resource_indicator: Some("https://integration.example.com".to_string()),
            },
        );

        let session = coordinator
            .connect(vec![IntegrationCapabilityScope::SessionManage])
            .await
            .unwrap();

        assert_eq!(
            session.transport_kind,
            IntegrationTransportKind::HttpsLongPoll
        );
        assert_eq!(session.auth_scheme, IntegrationAuthScheme::BearerToken);
    }
}
