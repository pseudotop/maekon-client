//! Shared `IntegrationSessionPort` test double (#7729 ctd-W2 G2).
//!
//! Canonicalizes 3 previously hand-rolled `MockSessionPort` copies that had
//! silently DIVERGED: `runtime_loop.rs`, `inbox_coordinator.rs`, and
//! `egress_coordinator/tests.rs`. The `egress_coordinator`/`inbox_coordinator`
//! copies were byte-for-byte identical and correct: `store_ack_cursor` upserts
//! by `stream_id` into `IntegrationSessionState.ack_cursors`. The
//! `runtime_loop.rs` copy was BUGGY: its `store_ack_cursor` ignored the
//! `cursor` argument entirely (`_cursor`) and returned the current session
//! unchanged, silently dropping every acknowledged cursor.
//!
//! Empirically, `IntegrationRuntimeLoop`'s production code never calls
//! `session_port.store_ack_cursor()` (only `connect`/`current_session`/
//! `heartbeat`), so the bug was dead/inert rather than an active theater
//! assertion — no `runtime_loop.rs` test exercised the buggy path, so none
//! needed correcting. It nonetheless violated the `IntegrationSessionPort`
//! contract and would have caused false-positive passes for any future test
//! added against it, so the canonical fake below always persists the cursor.
//!
//! `connect()`/`heartbeat()` use `runtime_loop.rs`'s richer "cold start" +
//! call-counter behavior (needed by the runtime-loop test suite); this is
//! safe to share because `egress_coordinator`/`inbox_coordinator` never call
//! `connect()`/`heartbeat()` in their tested code paths (verified: their
//! production coordinators only ever call `current_session()` and
//! `store_ack_cursor()` on the session port).

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::Mutex;

use maekon_core::error::CoreError;
use maekon_core::models::integration::{
    IntegrationAckCursor, IntegrationAuthScheme, IntegrationCapabilityScope,
    IntegrationSessionState, IntegrationSessionStatus, IntegrationTransportKind,
};
use maekon_core::ports::integration::IntegrationSessionPort;

/// In-memory `IntegrationSessionPort` fake shared by the integration
/// runtime-loop, inbox-coordinator, and egress-coordinator test suites.
#[derive(Default)]
pub(crate) struct FakeIntegrationSessionPort {
    state: Arc<Mutex<Option<IntegrationSessionState>>>,
    pub(crate) connect_calls: Arc<Mutex<usize>>,
    pub(crate) heartbeat_calls: Arc<Mutex<usize>>,
}

impl FakeIntegrationSessionPort {
    /// Empty fake — `current_session()` returns `None` until `connect()` is
    /// called. Mirrors the `runtime_loop.rs` "cold start" test flow.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Pre-seed a connected session. Mirrors the `egress_coordinator.rs` /
    /// `inbox_coordinator.rs` "already connected" test flow — those
    /// coordinators never call `connect()` directly (only `current_session()`
    /// and `store_ack_cursor()`), so `connect()` is not exercised once a
    /// session is pre-seeded this way.
    pub(crate) fn with_session(session: IntegrationSessionState) -> Self {
        Self {
            state: Arc::new(Mutex::new(Some(session))),
            ..Self::default()
        }
    }

    /// Replace the current session wholesale, post-construction. For tests
    /// that need to seed a session on an already-`Arc`-wrapped fake (e.g. one
    /// already passed into a runtime loop constructor).
    pub(crate) async fn set_session(&self, session: IntegrationSessionState) {
        *self.state.lock().await = Some(session);
    }

    /// Mutate the current session's `status` in place. Panics if no session
    /// is set yet (mirrors the direct-field-access pattern this replaces —
    /// callers only use this after `connect()`/`with_session` has run).
    pub(crate) async fn set_status(&self, status: IntegrationSessionStatus) {
        let mut guard = self.state.lock().await;
        guard
            .as_mut()
            .expect("set_status called before a session exists")
            .status = status;
    }
}

#[async_trait]
impl IntegrationSessionPort for FakeIntegrationSessionPort {
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
        *self.state.lock().await = Some(session.clone());
        Ok(session)
    }

    async fn current_session(&self) -> Result<Option<IntegrationSessionState>, CoreError> {
        Ok(self.state.lock().await.clone())
    }

    async fn heartbeat(&self, _session_id: &str) -> Result<IntegrationSessionState, CoreError> {
        *self.heartbeat_calls.lock().await += 1;
        self.state
            .lock()
            .await
            .clone()
            .ok_or_else(|| CoreError::ServiceUnavailable {
                code: maekon_core::error_codes::ServiceCode::Unavailable,
                message: "no integration session".to_string(),
            })
    }

    /// #7729: always persists (upsert-by-`stream_id`) — the fixed version of
    /// the `runtime_loop.rs` copy's bug (see module docs).
    async fn store_ack_cursor(
        &self,
        session_id: &str,
        cursor: IntegrationAckCursor,
    ) -> Result<IntegrationSessionState, CoreError> {
        let mut guard = self.state.lock().await;
        let state = guard
            .as_mut()
            .ok_or_else(|| CoreError::ServiceUnavailable {
                code: maekon_core::error_codes::ServiceCode::Unavailable,
                message: "no integration session".to_string(),
            })?;
        if state.session_id != session_id {
            return Err(CoreError::NotFound {
                code: maekon_core::error_codes::NotFoundCode::ResourceMissing,
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
        Ok(state.clone())
    }

    async fn disconnect(&self, _session_id: &str) -> Result<(), CoreError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope() -> Vec<IntegrationCapabilityScope> {
        vec![IntegrationCapabilityScope::SessionManage]
    }

    fn cursor(stream_id: &str, value: &str) -> IntegrationAckCursor {
        IntegrationAckCursor {
            stream_id: stream_id.to_string(),
            cursor: value.to_string(),
            acknowledged_at: Utc::now(),
        }
    }

    /// #7729 revert-proof gate: this is the exact regression the
    /// `runtime_loop.rs` copy's bug would have caused. If `store_ack_cursor`
    /// regresses to ignoring its `cursor` argument, this test fails.
    #[tokio::test]
    async fn store_ack_cursor_persists_the_cursor() {
        let port = FakeIntegrationSessionPort::new();
        let session = port.connect(scope()).await.expect("connect");

        let updated = port
            .store_ack_cursor(&session.session_id, cursor("insights", "cursor-1"))
            .await
            .expect("store_ack_cursor");

        assert_eq!(updated.ack_cursors.len(), 1);
        assert_eq!(updated.ack_cursors[0].cursor, "cursor-1");

        let current = port
            .current_session()
            .await
            .expect("current_session")
            .expect("session present");
        assert_eq!(current.ack_cursors.len(), 1);
        assert_eq!(current.ack_cursors[0].cursor, "cursor-1");
    }

    #[tokio::test]
    async fn store_ack_cursor_upserts_by_stream_id() {
        let port = FakeIntegrationSessionPort::new();
        let session = port.connect(scope()).await.expect("connect");
        port.store_ack_cursor(&session.session_id, cursor("insights", "cursor-1"))
            .await
            .expect("first store");

        let updated = port
            .store_ack_cursor(&session.session_id, cursor("insights", "cursor-2"))
            .await
            .expect("second store");

        assert_eq!(updated.ack_cursors.len(), 1);
        assert_eq!(updated.ack_cursors[0].cursor, "cursor-2");
    }

    #[tokio::test]
    async fn store_ack_cursor_errors_without_a_session() {
        let port = FakeIntegrationSessionPort::new();
        let err = port
            .store_ack_cursor("session-runtime", cursor("insights", "cursor-1"))
            .await
            .expect_err("no session yet");
        assert!(matches!(err, CoreError::ServiceUnavailable { .. }));
    }

    #[tokio::test]
    async fn with_session_seeds_current_session_without_connect() {
        let seeded = IntegrationSessionState {
            session_id: "session-seed".to_string(),
            device_id: "device-1".to_string(),
            status: IntegrationSessionStatus::Connected,
            transport_kind: IntegrationTransportKind::WebSocket,
            auth_scheme: IntegrationAuthScheme::BearerToken,
            connected_at: Some(Utc::now()),
            last_heartbeat_at: None,
            requested_scopes: scope(),
            granted_scopes: scope(),
            ack_cursors: Vec::new(),
        };
        let port = FakeIntegrationSessionPort::with_session(seeded.clone());

        let current = port
            .current_session()
            .await
            .expect("current_session")
            .expect("session present");
        assert_eq!(current.session_id, "session-seed");
        assert_eq!(*port.connect_calls.lock().await, 0);
    }
}
