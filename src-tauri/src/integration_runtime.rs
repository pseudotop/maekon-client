use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use oneshim_api_contracts::integration::IntegrationOutboundRuntimeStatus;
use oneshim_core::config::AppConfig;
use oneshim_core::error::CoreError;
use oneshim_core::models::integration::{
    default_integration_runtime_scopes, IntegrationAuthProfileKind, IntegrationAuthScheme,
};
use oneshim_core::ports::integration::{
    IntegrationAuditPort, IntegrationAuthPort, IntegrationCheckpointStorePort,
    IntegrationEgressPort, IntegrationEgressSignalPort, IntegrationInboxPort,
    IntegrationInboxSignalPort, IntegrationInboxStorePort, IntegrationInsightProducerPort,
    IntegrationOutboxPort, IntegrationPromptPresenterPort, IntegrationPromptReceiptStorePort,
    IntegrationRuntimeTelemetryPort, IntegrationSessionPort, LocalSuggestionQueryPort,
};
use oneshim_core::ports::secret_store::SecretStore;
use oneshim_network::integration::{
    assemble_https_transport, EnvIntegrationAuthPort, IntegrationEgressCoordinator,
    IntegrationInboxCoordinator, IntegrationInsightProducerCoordinator,
    IntegrationProducerRuntimeLoop, IntegrationProducerRuntimeLoopProfile, IntegrationRuntimeLoop,
    IntegrationRuntimeLoopProfile, IntegrationRuntimeTelemetryHandle,
    IntegrationSessionCoordinator, IntegrationSessionRuntimeProfile, OidcDeviceFlowAuthConfig,
    OidcDeviceFlowIntegrationAuthPort, PolicyAwareIntegrationEgressCoordinator,
};
use oneshim_storage::integration_state_store::{
    FileIntegrationStateStore, IntegrationStateStorePolicy,
};
use tokio::sync::watch;

use crate::integration_insight_source::LocalSuggestionIntegrationSource;
use crate::integration_policy::DefaultIntegrationEgressPolicy;
use crate::integration_prompt_delivery::{
    IntegrationInboxDeliveryCoordinator, IntegrationInboxDeliveryLoop,
    IntegrationInboxDeliveryLoopProfile,
};

#[derive(Clone)]
pub(crate) struct IntegrationRuntimeBindings {
    pub status: IntegrationOutboundRuntimeStatus,
    pub auth: Option<Arc<dyn IntegrationAuthPort>>,
    pub session: Option<Arc<dyn IntegrationSessionPort>>,
    pub outbox: Option<Arc<dyn IntegrationOutboxPort>>,
    pub inbox: Option<Arc<dyn IntegrationInboxPort>>,
    pub inbox_store: Option<Arc<dyn IntegrationInboxStorePort>>,
    pub audit: Option<Arc<dyn IntegrationAuditPort>>,
    pub telemetry: Option<Arc<dyn IntegrationRuntimeTelemetryPort>>,
}

#[derive(Clone)]
pub(crate) struct IntegrationBackgroundLoops {
    runtime: Option<IntegrationRuntimeLoop>,
    producer: Option<IntegrationProducerRuntimeLoop>,
    delivery: Option<IntegrationInboxDeliveryLoop>,
}

impl IntegrationBackgroundLoops {
    pub(crate) fn spawn_on(
        &self,
        handle: &tokio::runtime::Handle,
        shutdown_tx: &watch::Sender<bool>,
    ) {
        if let Some(runtime_loop) = self.runtime.clone() {
            let shutdown_rx = shutdown_tx.subscribe();
            handle.spawn(async move {
                runtime_loop.run(shutdown_rx).await;
            });
        }

        if let Some(producer_loop) = self.producer.clone() {
            let shutdown_rx = shutdown_tx.subscribe();
            handle.spawn(async move {
                producer_loop.run(shutdown_rx).await;
            });
        }

        if let Some(delivery_loop) = self.delivery.clone() {
            let shutdown_rx = shutdown_tx.subscribe();
            handle.spawn(async move {
                delivery_loop.run(shutdown_rx).await;
            });
        }
    }
}

pub(crate) struct IntegrationRuntimeBundle {
    bindings: IntegrationRuntimeBindings,
    egress: Option<Arc<dyn IntegrationEgressPort>>,
    checkpoint_store: Option<Arc<dyn IntegrationCheckpointStorePort>>,
    runtime_loop: Option<IntegrationRuntimeLoop>,
    device_id: Option<String>,
    max_batch_size: usize,
    produce_interval: Duration,
    inbox_refresh_interval: Duration,
}

impl IntegrationRuntimeBundle {
    pub(crate) fn bindings(&self) -> IntegrationRuntimeBindings {
        self.bindings.clone()
    }

    pub(crate) fn background_loops(
        &self,
        suggestion_query: Arc<dyn LocalSuggestionQueryPort>,
        prompt_presenter: Arc<dyn IntegrationPromptPresenterPort>,
    ) -> IntegrationBackgroundLoops {
        let producer = match (
            self.egress.clone(),
            self.checkpoint_store.clone(),
            self.device_id.clone(),
        ) {
            (Some(egress), Some(checkpoint_store), Some(device_id)) => {
                let producer = Arc::new(IntegrationInsightProducerCoordinator::new(
                    Arc::new(LocalSuggestionIntegrationSource::new(
                        device_id,
                        suggestion_query,
                    )),
                    checkpoint_store,
                    egress,
                    self.max_batch_size,
                )) as Arc<dyn IntegrationInsightProducerPort>;
                Some(IntegrationProducerRuntimeLoop::new(
                    producer,
                    IntegrationProducerRuntimeLoopProfile {
                        produce_interval: self.produce_interval,
                    },
                ))
            }
            _ => None,
        };

        let delivery = self.bindings.inbox_store.clone().map(|inbox_store| {
            let delivery = Arc::new(IntegrationInboxDeliveryCoordinator::new(
                inbox_store,
                prompt_presenter,
                self.max_batch_size,
            ));
            IntegrationInboxDeliveryLoop::new(
                delivery,
                IntegrationInboxDeliveryLoopProfile {
                    delivery_interval: self.inbox_refresh_interval,
                },
            )
        });

        IntegrationBackgroundLoops {
            runtime: self.runtime_loop.clone(),
            producer,
            delivery,
        }
    }
}

pub(crate) struct IntegrationRuntimeBuilder<'a> {
    config: &'a AppConfig,
    config_dir: &'a Path,
    secret_store: Option<Arc<dyn SecretStore>>,
}

impl<'a> IntegrationRuntimeBuilder<'a> {
    pub(crate) fn new(
        config: &'a AppConfig,
        config_dir: &'a Path,
        secret_store: Option<Arc<dyn SecretStore>>,
    ) -> Self {
        Self {
            config,
            config_dir,
            secret_store,
        }
    }

    pub(crate) fn build(&self) -> Result<IntegrationRuntimeBundle, CoreError> {
        let integration = &self.config.integration;
        let bootstrap_url = non_empty_config_value(integration.bootstrap_url.as_deref());
        let auth_token_env_var = non_empty_config_value(integration.auth_token_env_var.as_deref());
        let oidc_client_id =
            non_empty_config_value(integration.oidc_device_flow.client_id.as_deref());
        let oidc_device_authorization_url = non_empty_config_value(
            integration
                .oidc_device_flow
                .device_authorization_url
                .as_deref(),
        );
        let oidc_token_url =
            non_empty_config_value(integration.oidc_device_flow.token_url.as_deref());

        let preferred_transports = if integration.preferred_transports.is_empty() {
            IntegrationSessionRuntimeProfile::default().preferred_transports
        } else {
            integration.preferred_transports.clone()
        };

        let mut supported_auth_schemes = if integration.supported_auth_schemes.is_empty() {
            vec![IntegrationAuthScheme::BearerToken]
        } else {
            integration.supported_auth_schemes.clone()
        };
        if supported_auth_schemes.is_empty() {
            supported_auth_schemes.push(IntegrationAuthScheme::BearerToken);
        }

        let auth_source_configured = match integration.auth_profile_kind {
            IntegrationAuthProfileKind::EnvToken => auth_token_env_var.is_some(),
            IntegrationAuthProfileKind::OidcDeviceFlow => {
                oidc_client_id.is_some()
                    && oidc_device_authorization_url.is_some()
                    && oidc_token_url.is_some()
            }
        };
        let auth_material_available = match integration.auth_profile_kind {
            IntegrationAuthProfileKind::EnvToken => auth_token_env_var
                .as_deref()
                .and_then(|env_var| std::env::var(env_var).ok())
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false),
            IntegrationAuthProfileKind::OidcDeviceFlow => false,
        };

        let dpop_proof_factory = oneshim_network::integration::build_proof_factory(
            &supported_auth_schemes,
            self.secret_store.clone(),
        );

        let auth: Option<Arc<dyn IntegrationAuthPort>> = match integration.auth_profile_kind {
            IntegrationAuthProfileKind::EnvToken => auth_token_env_var.clone().map(|env_var| {
                Arc::new(EnvIntegrationAuthPort::new(
                    env_var,
                    supported_auth_schemes.first().cloned().unwrap_or_default(),
                    None,
                    integration.resource_indicator.clone(),
                )) as Arc<dyn IntegrationAuthPort>
            }),
            IntegrationAuthProfileKind::OidcDeviceFlow => match (
                oidc_client_id.clone(),
                oidc_device_authorization_url.clone(),
                oidc_token_url.clone(),
            ) {
                (Some(client_id), Some(device_authorization_url), Some(token_url)) => {
                    Some(Arc::new(OidcDeviceFlowIntegrationAuthPort::new(
                        OidcDeviceFlowAuthConfig {
                            client_id,
                            device_authorization_url,
                            token_url,
                            default_scopes: integration.oidc_device_flow.scopes.clone(),
                            resource_indicator: non_empty_config_value(
                                integration.resource_indicator.as_deref(),
                            ),
                            scheme: supported_auth_schemes.first().cloned().unwrap_or_default(),
                            request_timeout: Duration::from_secs(integration.request_timeout_secs),
                        },
                        dpop_proof_factory.clone(),
                        self.secret_store.clone(),
                    )?) as Arc<dyn IntegrationAuthPort>)
                }
                _ => None,
            },
        };

        let runtime_configured = integration.enabled
            && bootstrap_url.is_some()
            && auth.is_some()
            && !preferred_transports.is_empty();

        let status = IntegrationOutboundRuntimeStatus {
            enabled: integration.enabled,
            bootstrap_configured: bootstrap_url.is_some(),
            auth_source_configured,
            auth_material_available,
            runtime_configured,
            resource_indicator_configured: integration
                .resource_indicator
                .as_deref()
                .map(str::trim)
                .is_some_and(|value| !value.is_empty()),
            auth_profile_kind: integration.auth_profile_kind.clone(),
            preferred_transports: preferred_transports.clone(),
            supported_auth_schemes: supported_auth_schemes.clone(),
            outbox_pending_count: None,
            inbox_pending_count: None,
            outbox_ack_cursor: None,
            inbox_ack_cursor: None,
            auth_status: None,
            current_session: None,
            runtime_telemetry: None,
        };

        let mut bundle = IntegrationRuntimeBundle {
            bindings: IntegrationRuntimeBindings {
                status,
                auth: auth.clone(),
                session: None,
                outbox: None,
                inbox: None,
                inbox_store: None,
                audit: None,
                telemetry: None,
            },
            egress: None,
            checkpoint_store: None,
            runtime_loop: None,
            device_id: None,
            max_batch_size: integration.max_batch_size,
            produce_interval: Duration::from_secs(integration.produce_interval_secs),
            inbox_refresh_interval: Duration::from_secs(integration.inbox_refresh_interval_secs),
        };

        if !runtime_configured {
            return Ok(bundle);
        }

        let transport_assembly = assemble_https_transport(
            bootstrap_url.ok_or_else(|| CoreError::Config {
                code: oneshim_core::error_codes::ConfigCode::Invalid,
                message: "runtime_configured requires bootstrap_url".into(),
            })?,
            Duration::from_secs(integration.request_timeout_secs),
            auth.clone().ok_or_else(|| CoreError::Config {
                code: oneshim_core::error_codes::ConfigCode::Invalid,
                message: "runtime_configured requires an integration auth port".into(),
            })?,
            &supported_auth_schemes,
            self.secret_store.clone(),
        )?;

        let integration_state_store = FileIntegrationStateStore::with_policy(
            integration_state_store_path(self.config_dir),
            IntegrationStateStorePolicy {
                max_stored_prompts: integration.max_stored_prompts,
                redact_completed_prompt_bodies: integration.redact_completed_prompt_bodies,
            },
        )?;
        let session_store = Arc::new(integration_state_store.session_store())
            as Arc<dyn oneshim_core::ports::integration::IntegrationSessionStorePort>;
        let egress_transport = transport_assembly.egress_transport;
        let inbox_transport = transport_assembly.inbox_transport;
        let transport = transport_assembly.session_transport;

        let device_id = integration
            .device_id
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| derive_integration_device_id(self.config_dir));

        let session = Arc::new(IntegrationSessionCoordinator::new_with_profile_and_store(
            device_id.clone(),
            transport,
            IntegrationSessionRuntimeProfile {
                client_version: env!("CARGO_PKG_VERSION").to_string(),
                device_label: non_empty_config_value(integration.device_label.as_deref()),
                preferred_transports,
                supported_auth_schemes,
                resource_indicator: non_empty_config_value(
                    integration.resource_indicator.as_deref(),
                ),
            },
            Some(session_store),
        )) as Arc<dyn IntegrationSessionPort>;

        let outbox =
            Arc::new(integration_state_store.outbox_store()) as Arc<dyn IntegrationOutboxPort>;
        let inbox_store =
            Arc::new(integration_state_store.inbox_store()) as Arc<dyn IntegrationInboxStorePort>;
        let receipt_store = Arc::new(integration_state_store.inbox_store())
            as Arc<dyn IntegrationPromptReceiptStorePort>;
        let checkpoint_store = Arc::new(integration_state_store.checkpoint_store())
            as Arc<dyn IntegrationCheckpointStorePort>;
        let audit =
            Arc::new(integration_state_store.audit_store()) as Arc<dyn IntegrationAuditPort>;
        let runtime_telemetry = IntegrationRuntimeTelemetryHandle::default();

        let base_egress = Arc::new(IntegrationEgressCoordinator::new(
            session.clone(),
            outbox.clone(),
            egress_transport,
            integration.max_batch_size,
        ));
        let egress_signal = base_egress.clone() as Arc<dyn IntegrationEgressSignalPort>;
        let egress = Arc::new(PolicyAwareIntegrationEgressCoordinator::new(
            base_egress as Arc<dyn IntegrationEgressPort>,
            Arc::new(DefaultIntegrationEgressPolicy),
            audit.clone(),
        )) as Arc<dyn IntegrationEgressPort>;
        let base_inbox = Arc::new(IntegrationInboxCoordinator::new(
            device_id.clone(),
            session.clone(),
            inbox_store.clone(),
            receipt_store,
            inbox_transport,
            integration.max_batch_size,
        ));
        let inbox_signal = base_inbox.clone() as Arc<dyn IntegrationInboxSignalPort>;
        let inbox = base_inbox as Arc<dyn IntegrationInboxPort>;

        bundle.bindings.session = Some(session.clone());
        bundle.bindings.outbox = Some(outbox);
        bundle.bindings.inbox = Some(inbox.clone());
        bundle.bindings.inbox_store = Some(inbox_store);
        bundle.bindings.audit = Some(audit.clone());
        bundle.bindings.telemetry =
            Some(Arc::new(runtime_telemetry.clone()) as Arc<dyn IntegrationRuntimeTelemetryPort>);
        bundle.egress = Some(egress.clone());
        bundle.checkpoint_store = Some(checkpoint_store);
        bundle.runtime_loop = Some(IntegrationRuntimeLoop::new(
            session,
            egress,
            inbox,
            Some(egress_signal),
            Some(inbox_signal),
            Some(runtime_telemetry),
            IntegrationRuntimeLoopProfile {
                requested_scopes: default_integration_runtime_scopes(),
                connect_retry_interval: Duration::from_secs(integration.connect_retry_secs),
                heartbeat_interval: Duration::from_secs(integration.heartbeat_interval_secs),
                egress_interval: Duration::from_secs(integration.sync_interval_secs),
                inbox_refresh_interval: Duration::from_secs(
                    integration.inbox_refresh_interval_secs,
                ),
            },
        ));
        bundle.device_id = Some(device_id);

        Ok(bundle)
    }
}

fn non_empty_config_value(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn derive_integration_device_id(config_dir: &Path) -> String {
    use std::hash::{Hash, Hasher};

    let hostname = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| String::from("device"));
    let seed = format!("{}::{hostname}", config_dir.display());
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    seed.hash(&mut hasher);
    format!("device_{:016x}", hasher.finish())
}

fn integration_state_store_path(config_dir: &Path) -> PathBuf {
    config_dir.join("integration").join("state.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn build_runtime_requires_bootstrap_url_for_runtime() {
        let temp_dir = TempDir::new().expect("temp dir");
        let mut config = AppConfig::default_config();
        config.integration.enabled = true;
        config.integration.auth_token_env_var = Some("ONESHIM_TEST_INTEGRATION_TOKEN".to_string());

        let runtime = IntegrationRuntimeBuilder::new(&config, temp_dir.path(), None)
            .build()
            .unwrap();
        let bindings = runtime.bindings();

        assert!(bindings.status.enabled);
        assert!(!bindings.status.bootstrap_configured);
        assert!(bindings.status.auth_source_configured);
        assert!(!bindings.status.runtime_configured);
        assert!(bindings.auth.is_some());
        assert!(bindings.session.is_none());
    }

    #[test]
    fn build_runtime_preserves_dpop_signing() {
        let temp_dir = TempDir::new().expect("temp dir");
        let mut config = AppConfig::default_config();
        config.integration.enabled = true;
        config.integration.bootstrap_url =
            Some("https://integration.example.com/bootstrap".to_string());
        config.integration.auth_token_env_var = Some("ONESHIM_TEST_INTEGRATION_TOKEN".to_string());
        config.integration.supported_auth_schemes = vec![IntegrationAuthScheme::DpopBearer];

        let runtime = IntegrationRuntimeBuilder::new(&config, temp_dir.path(), None)
            .build()
            .unwrap();
        let bindings = runtime.bindings();

        assert_eq!(
            bindings.status.supported_auth_schemes,
            vec![IntegrationAuthScheme::DpopBearer]
        );
        assert!(bindings.auth.is_some());
        assert!(bindings.status.runtime_configured);
        assert!(bindings.session.is_some());
    }

    #[test]
    fn build_runtime_reports_auth_material_presence() {
        let temp_dir = TempDir::new().expect("temp dir");
        let mut config = AppConfig::default_config();
        config.integration.enabled = true;
        config.integration.bootstrap_url =
            Some("https://integration.example.com/bootstrap".to_string());
        config.integration.auth_token_env_var = Some("ONESHIM_TEST_INTEGRATION_TOKEN".to_string());

        unsafe {
            std::env::set_var("ONESHIM_TEST_INTEGRATION_TOKEN", "token-value");
        }
        let runtime = IntegrationRuntimeBuilder::new(&config, temp_dir.path(), None)
            .build()
            .unwrap();
        unsafe {
            std::env::remove_var("ONESHIM_TEST_INTEGRATION_TOKEN");
        }

        assert!(runtime.bindings().status.auth_material_available);
    }
}
