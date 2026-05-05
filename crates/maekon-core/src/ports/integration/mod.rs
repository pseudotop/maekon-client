//! Integration domain ports for outbound egress, inbound inbox delivery, and
//! session/auth orchestration.
//!
//! Split into sub-modules per ADR-003 (Directory Module Pattern):
//! - `session` — session lifecycle and telemetry
//! - `auth` — device authorization and auth resolution
//! - `egress` — outbound message queue, policy, and audit
//! - `inbox` — inbound prompt delivery and lifecycle
//! - `insight` — insight production, source, and checkpointing

mod auth;
mod egress;
mod inbox;
mod insight;
mod session;

// Re-export all public items so that `use crate::ports::integration::*` still works.

pub use auth::IntegrationAuthPort;

pub use egress::{
    IntegrationAuditPort, IntegrationEgressDecision, IntegrationEgressPolicyPort,
    IntegrationEgressPort, IntegrationEgressSignalPort, IntegrationOutboxPort,
};

pub use inbox::{
    IntegrationInboxPort, IntegrationInboxSignalPort, IntegrationInboxStorePort,
    IntegrationPromptPresenterPort, IntegrationPromptReceiptStorePort,
};

pub use insight::{
    IntegrationCheckpointStorePort, IntegrationInsightProducerPort, IntegrationInsightSourcePort,
    LocalSuggestionQueryPort,
};

pub use session::{
    IntegrationRuntimeTelemetryPort, IntegrationSessionPort, IntegrationSessionStorePort,
};
