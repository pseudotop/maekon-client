//! Transport assembly — creates and bundles all integration transport adapters
//! behind opaque Arc handles. External consumers (src-tauri) receive pre-cast
//! trait objects without importing the transport trait definitions.

use std::sync::Arc;
use std::time::Duration;

use oneshim_core::error::CoreError;
use oneshim_core::models::integration::IntegrationAuthScheme;
use oneshim_core::ports::integration::IntegrationAuthPort;
use oneshim_core::ports::secret_store::SecretStore;

use super::auth::{Ed25519DpopProofFactory, NoopIntegrationRequestProofFactory};
use super::transport::{
    IntegrationEgressTransportClient, IntegrationInboxTransportClient,
    IntegrationRequestProofFactory, IntegrationTransportClient,
};
use super::{HttpsIntegrationTransportClient, HttpsIntegrationTransportConfig};

/// Build a proof factory based on supported auth schemes.
/// Returns an opaque Arc so callers don't need to import the trait.
pub fn build_proof_factory(
    supported_auth_schemes: &[IntegrationAuthScheme],
    secret_store: Option<Arc<dyn SecretStore>>,
) -> Arc<dyn IntegrationRequestProofFactory> {
    if supported_auth_schemes.contains(&IntegrationAuthScheme::DpopBearer) {
        Arc::new(Ed25519DpopProofFactory::new(secret_store))
    } else {
        Arc::new(NoopIntegrationRequestProofFactory)
    }
}

/// Pre-assembled integration transport components.
/// All fields are trait-object Arcs ready for coordinator injection.
pub struct IntegrationTransportAssembly {
    pub session_transport: Arc<dyn IntegrationTransportClient>,
    pub egress_transport: Arc<dyn IntegrationEgressTransportClient>,
    pub inbox_transport: Arc<dyn IntegrationInboxTransportClient>,
}

/// Create transport assembly from HTTPS configuration.
///
/// Handles internal wiring (proof factory, sub-transports) so callers
/// never need to import the raw transport trait names.
pub fn assemble_https_transport(
    bootstrap_url: String,
    request_timeout: Duration,
    auth: Arc<dyn IntegrationAuthPort>,
    supported_auth_schemes: &[IntegrationAuthScheme],
    secret_store: Option<Arc<dyn SecretStore>>,
) -> Result<IntegrationTransportAssembly, CoreError> {
    let proof_factory = build_proof_factory(supported_auth_schemes, secret_store);

    let transport = HttpsIntegrationTransportClient::new(
        HttpsIntegrationTransportConfig::new(bootstrap_url, request_timeout),
        auth,
        proof_factory,
    )?;

    let egress =
        Arc::new(transport.egress_transport()) as Arc<dyn IntegrationEgressTransportClient>;
    let inbox = Arc::new(transport.inbox_transport()) as Arc<dyn IntegrationInboxTransportClient>;
    let session = Arc::new(transport) as Arc<dyn IntegrationTransportClient>;

    Ok(IntegrationTransportAssembly {
        session_transport: session,
        egress_transport: egress,
        inbox_transport: inbox,
    })
}
