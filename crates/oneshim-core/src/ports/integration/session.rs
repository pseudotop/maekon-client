//! Integration session management ports.
//!
//! # Errors (all traits in this module)
//! - `CoreError::Auth` (wire: `auth.failed`) — session auth token
//!   rejection (401/403), missing/expired credentials surfaced through
//!   the transport layer.
//! - `CoreError::RequestTimeout` (wire: `network.timeout`) — connect,
//!   heartbeat, or ack-cursor store exceeding transport timeout.
//! - `CoreError::Network` (wire: `network.generic`) — pre-response
//!   transport failures (DNS, refused connection) that don't match the
//!   timeout/rate-limit specific variants.
//! - `CoreError::ServiceUnavailable` (wire: `service.unavailable`) —
//!   502/503 from the integration backend, or running with the
//!   feature flag disabled.
//! - `CoreError::Storage` (wire: `storage.failed`) — persisted
//!   session-state write/read failures in `IntegrationSessionStorePort`.
//! - `IntegrationSessionStorePort::load` returns `Ok(None)` on first
//!   launch (no persisted state), not Err.
//! - `IntegrationRuntimeTelemetryPort::snapshot` returns sentinel
//!   zeros for loops that have never run rather than Err.

use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::integration::{
    IntegrationAckCursor, IntegrationCapabilityScope, IntegrationRuntimeTelemetry,
    IntegrationSessionState,
};

#[async_trait]
pub trait IntegrationSessionPort: Send + Sync {
    /// Connect or resume an outbound integration session for the requested scopes.
    async fn connect(
        &self,
        requested_scopes: Vec<IntegrationCapabilityScope>,
    ) -> Result<IntegrationSessionState, CoreError>;

    /// Return the current session state, if one exists.
    async fn current_session(&self) -> Result<Option<IntegrationSessionState>, CoreError>;

    /// Send a liveness heartbeat and return the refreshed session state.
    async fn heartbeat(&self, session_id: &str) -> Result<IntegrationSessionState, CoreError>;

    /// Persist the latest acknowledged cursor for the active session.
    async fn store_ack_cursor(
        &self,
        session_id: &str,
        cursor: IntegrationAckCursor,
    ) -> Result<IntegrationSessionState, CoreError>;

    /// Disconnect an established integration session.
    async fn disconnect(&self, session_id: &str) -> Result<(), CoreError>;
}

#[async_trait]
pub trait IntegrationSessionStorePort: Send + Sync {
    /// Load the last persisted integration session state, if one exists.
    async fn load(&self) -> Result<Option<IntegrationSessionState>, CoreError>;

    /// Persist the latest integration session state snapshot.
    async fn store(&self, state: IntegrationSessionState) -> Result<(), CoreError>;

    /// Clear any persisted integration session state.
    async fn clear(&self) -> Result<(), CoreError>;
}

#[async_trait]
pub trait IntegrationRuntimeTelemetryPort: Send + Sync {
    /// Return the latest runtime telemetry snapshot for integration background loops.
    async fn snapshot(&self) -> Result<IntegrationRuntimeTelemetry, CoreError>;
}
