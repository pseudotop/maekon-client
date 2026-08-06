// P2 PR-A (B2), inherited with the modules ADR-034 P3 moved here from
// maekon-network (which carries the same crate-wide allow with the original
// rationale): `significant_drop_tightening` is accepted crate-wide. The flagged
// sites are tokio::sync Mutex guards held across await points on purpose —
// signing-key initialization and live-channel inbound state must stay locked
// across the async boundary (intentional atomicity), and clippy's "tighten via
// single-usage" rewrite produces invalid Rust on this shape (confirmed on
// similar sites in PR #468). The nursery lint's false-positive rate here
// outweighs its diagnostic value.
#![allow(clippy::significant_drop_tightening)]

pub mod auth;
pub mod cloudevents;
pub mod connectors;
pub mod egress_coordinator;
// Google Calendar read-only Context Source connector (MK-EXT-01.C01 #8590).
pub mod google_calendar;
pub mod http_transport;
pub mod inbox_coordinator;
pub mod live_channel;
pub mod policy_egress;
pub mod producer_coordinator;
pub mod producer_loop;
pub mod runtime_loop;
pub mod runtime_telemetry;
pub mod session_coordinator;
// #7729 ctd-W2 G2: canonical `FakeIntegrationSessionPort`, shared by
// runtime_loop/inbox_coordinator/egress_coordinator's test modules.
#[cfg(test)]
pub(crate) mod test_support;
pub mod transport;
pub mod transport_assembly;

pub use auth::{
    Ed25519DpopProofFactory, EnvIntegrationAuthPort, NoopIntegrationRequestProofFactory,
    OidcDeviceFlowAuthConfig, OidcDeviceFlowIntegrationAuthPort, StaticIntegrationAuthPort,
    StaticIntegrationRequestProofFactory,
};
pub use cloudevents::{
    insight_to_cloudevent, outbound_message_to_cloudevent, prompt_from_cloudevent,
    prompt_receipt_to_cloudevent, IntegrationCloudEvent, IntegrationOutboundCloudEventBatch,
    IntegrationOutboundCloudEventBatchItem, PromptCloudEventBatch,
};
pub use egress_coordinator::IntegrationEgressCoordinator;
pub use http_transport::{
    HttpsIntegrationEgressTransportClient, HttpsIntegrationInboxTransportClient,
    HttpsIntegrationSessionBindings, HttpsIntegrationTransportClient,
    HttpsIntegrationTransportConfig,
};
pub use inbox_coordinator::IntegrationInboxCoordinator;
pub use live_channel::WebSocketIntegrationSessionChannel;
pub use policy_egress::PolicyAwareIntegrationEgressCoordinator;
pub use producer_coordinator::IntegrationInsightProducerCoordinator;
pub use producer_loop::{IntegrationProducerRuntimeLoop, IntegrationProducerRuntimeLoopProfile};
pub use runtime_loop::{IntegrationRuntimeLoop, IntegrationRuntimeLoopProfile};
pub use runtime_telemetry::{IntegrationRuntimeLane, IntegrationRuntimeTelemetryHandle};
pub use session_coordinator::{IntegrationSessionCoordinator, IntegrationSessionRuntimeProfile};
pub use transport::{
    IntegrationEgressTransportClient, IntegrationEgressTransportResponse,
    IntegrationInboxTransportClient, IntegrationInboxTransportResponse, IntegrationRequestProof,
    IntegrationRequestProofFactory, IntegrationTransportClient, IntegrationTransportConnectRequest,
    IntegrationTransportConnectResponse,
};
pub use transport_assembly::{
    assemble_https_transport, build_proof_factory, IntegrationTransportAssembly,
};
