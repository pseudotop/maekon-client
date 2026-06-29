use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use maekon_api_contracts::integration::IntegrationOutboundRuntimeStatus;
use maekon_core::config::AppConfig;
use maekon_core::error::CoreError;
use maekon_core::models::integration::{
    default_integration_runtime_scopes, IntegrationAuthProfileKind, IntegrationAuthScheme,
};
use maekon_core::ports::consent_manager::ConsentManagerPort;
use maekon_core::ports::integration::{
    IntegrationAuditPort, IntegrationAuthPort, IntegrationCheckpointStorePort,
    IntegrationEgressPort, IntegrationEgressSignalPort, IntegrationInboxPort,
    IntegrationInboxSignalPort, IntegrationInboxStorePort, IntegrationInsightProducerPort,
    IntegrationOutboxPort, IntegrationPromptPresenterPort, IntegrationRuntimeTelemetryPort,
    IntegrationSessionPort, LocalSuggestionQueryPort,
};
use maekon_core::ports::secret_store::SecretStore;
use maekon_core::{error_codes::ConsentCode, models::integration::IntegrationAckCursor};
use maekon_network::integration::{
    assemble_https_transport, EnvIntegrationAuthPort, IntegrationEgressCoordinator,
    IntegrationInboxCoordinator, IntegrationInsightProducerCoordinator,
    IntegrationProducerRuntimeLoop, IntegrationProducerRuntimeLoopProfile, IntegrationRuntimeLoop,
    IntegrationRuntimeLoopProfile, IntegrationRuntimeTelemetryHandle,
    IntegrationSessionCoordinator, IntegrationSessionRuntimeProfile, OidcDeviceFlowAuthConfig,
    OidcDeviceFlowIntegrationAuthPort, PolicyAwareIntegrationEgressCoordinator,
};
use maekon_storage::encryption::EncryptionKey;
use maekon_storage::integration_state_store::{
    FileIntegrationStateStore, IntegrationStateStorePolicy,
};
use parking_lot::RwLock;
use tokio::sync::watch;

use crate::integration_insight_source::LocalSuggestionIntegrationSource;
use crate::integration_policy::DefaultIntegrationEgressPolicy;
use crate::integration_prompt_delivery::{
    IntegrationInboxDeliveryCoordinator, IntegrationInboxDeliveryLoop,
    IntegrationInboxDeliveryLoopProfile,
};

/// #6936: PII level for integration insight egress to a third-party SaaS backend.
///
/// integration insight summaries/derived tags leave the device, so they must be
/// floored through the egress PII SSOT `ExternalDataPolicy::effective_egress_pii_level`
/// exactly like the sibling off-device paths (external LLM, window titles, OCR,
/// remote-embedding #6914). Using the raw `privacy.pii_filter_level` leaked verbatim
/// content under the reachable `AllowFiltered + Off` config; this floors it to Basic.
fn integration_egress_pii_level(config: &AppConfig) -> maekon_core::config::PiiFilterLevel {
    config
        .ai_provider
        .external_data_policy
        .effective_egress_pii_level(config.privacy.pii_filter_level)
}

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
    consent_gate: IntegrationConsentGate,
    device_id: Option<String>,
    max_batch_size: usize,
    produce_interval: Duration,
    inbox_refresh_interval: Duration,
}

impl IntegrationRuntimeBundle {
    pub(crate) fn bindings(&self) -> IntegrationRuntimeBindings {
        self.bindings.clone()
    }

    pub(crate) fn set_consent_manager(&self, consent_manager: Arc<dyn ConsentManagerPort>) {
        self.consent_gate.set_consent_manager(consent_manager);
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

#[derive(Clone, Default)]
struct IntegrationConsentGate {
    consent_manager: Arc<RwLock<Option<Arc<dyn ConsentManagerPort>>>>,
}

impl IntegrationConsentGate {
    fn set_consent_manager(&self, consent_manager: Arc<dyn ConsentManagerPort>) {
        *self.consent_manager.write() = Some(consent_manager);
    }

    fn telemetry_permitted(&self) -> bool {
        self.consent_manager
            .read()
            .as_ref()
            .map(|consent| consent.effective_permissions().telemetry)
            .unwrap_or(false)
    }
}

struct ConsentGatedIntegrationEgress {
    inner: Arc<dyn IntegrationEgressPort>,
    gate: IntegrationConsentGate,
}

impl ConsentGatedIntegrationEgress {
    #[cfg(test)]
    fn new(
        inner: Arc<dyn IntegrationEgressPort>,
        consent_manager: Arc<dyn ConsentManagerPort>,
    ) -> Self {
        let gate = IntegrationConsentGate::default();
        gate.set_consent_manager(consent_manager);
        Self { inner, gate }
    }

    fn with_gate(inner: Arc<dyn IntegrationEgressPort>, gate: IntegrationConsentGate) -> Self {
        Self { inner, gate }
    }

    fn consent_required() -> CoreError {
        CoreError::ConsentRequired {
            code: ConsentCode::Required,
            message: "Telemetry consent is required before integration egress can leave the device"
                .to_string(),
        }
    }
}

#[async_trait::async_trait]
impl IntegrationEgressPort for ConsentGatedIntegrationEgress {
    async fn enqueue_message(
        &self,
        envelope: maekon_core::models::integration::IntegrationEnvelope,
        payload: maekon_core::models::integration::IntegrationOutboundPayload,
    ) -> Result<(), CoreError> {
        if !self.gate.telemetry_permitted() {
            return Err(Self::consent_required());
        }
        self.inner.enqueue_message(envelope, payload).await
    }

    async fn flush(&self) -> Result<usize, CoreError> {
        if !self.gate.telemetry_permitted() {
            return Ok(0);
        }
        self.inner.flush().await
    }

    async fn last_ack_cursor(&self) -> Result<Option<IntegrationAckCursor>, CoreError> {
        self.inner.last_ack_cursor().await
    }
}

pub(crate) struct IntegrationRuntimeBuilder<'a> {
    config: &'a AppConfig,
    config_dir: &'a Path,
    secret_store: Option<Arc<dyn SecretStore>>,
    /// #7073: shared AES-256-GCM key used to encrypt the integration state store
    /// at rest. `None` only in tests / degraded fallback; the production
    /// composition root supplies the same `.db_key` used for SQLite and frames.
    encryption_key: Option<Arc<EncryptionKey>>,
}

impl<'a> IntegrationRuntimeBuilder<'a> {
    pub(crate) fn new(
        config: &'a AppConfig,
        config_dir: &'a Path,
        secret_store: Option<Arc<dyn SecretStore>>,
        encryption_key: Option<Arc<EncryptionKey>>,
    ) -> Self {
        Self {
            config,
            config_dir,
            secret_store,
            encryption_key,
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

        let dpop_proof_factory = maekon_network::integration::build_proof_factory(
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

        let consent_gate = IntegrationConsentGate::default();
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
            consent_gate: consent_gate.clone(),
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
                code: maekon_core::error_codes::ConfigCode::Invalid,
                message: "runtime_configured requires bootstrap_url".into(),
            })?,
            Duration::from_secs(integration.request_timeout_secs),
            auth.clone().ok_or_else(|| CoreError::Config {
                code: maekon_core::error_codes::ConfigCode::Invalid,
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
                // Inherit the store-layer outbox cap default (drop-oldest bound
                // that keeps the persisted outbox bounded over long-running
                // sessions even when egress is enqueued directly at the store).
                ..Default::default()
            },
            // #7073: encrypt the integration state at rest with the shared key.
            self.encryption_key.clone(),
        )?;
        let session_store = Arc::new(integration_state_store.session_store())
            as Arc<dyn maekon_core::ports::integration::IntegrationSessionStorePort>;
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
        let policy_egress = Arc::new(
            PolicyAwareIntegrationEgressCoordinator::new(
                base_egress as Arc<dyn IntegrationEgressPort>,
                Arc::new(DefaultIntegrationEgressPolicy),
                audit.clone(),
            )
            .with_pii_sanitizer(
                Arc::new(maekon_vision::privacy::VisionPiiSanitizer),
                // #6936: integration insight summaries/tags egress to a third-party
                // SaaS backend, so they must pass the egress PII floor SSOT (see
                // integration_egress_pii_level) like the sibling off-device paths.
                integration_egress_pii_level(self.config),
            ),
        ) as Arc<dyn IntegrationEgressPort>;
        let egress = Arc::new(ConsentGatedIntegrationEgress::with_gate(
            policy_egress,
            consent_gate,
        )) as Arc<dyn IntegrationEgressPort>;
        // #6198: thread the policy-aware egress coordinator into the inbox so
        // acknowledge/dismiss prompt receipts (including free-text reasons) are
        // PII-sanitized and egress-policy-authorized like every other outbound
        // payload, instead of being written straight to the receipt store.
        let base_inbox = Arc::new(IntegrationInboxCoordinator::new(
            device_id.clone(),
            session.clone(),
            inbox_store.clone(),
            egress.clone(),
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
    use async_trait::async_trait;
    use maekon_core::consent::{ConsentManager, ConsentPermissions};
    use maekon_core::models::integration::{
        InsightPacket, InsightSourceWindow, IntegrationCapabilityScope, IntegrationEnvelope,
        IntegrationMessageType, IntegrationOrigin, IntegrationOutboundPayload,
        IntegrationPrivacyClassification,
    };
    use maekon_core::ports::consent_manager::ConsentManagerPort;
    use serial_test::serial;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    #[derive(Default)]
    struct CountingIntegrationEgress {
        enqueued: AtomicUsize,
        flushed: AtomicUsize,
    }

    #[async_trait]
    impl IntegrationEgressPort for CountingIntegrationEgress {
        async fn enqueue_message(
            &self,
            _envelope: IntegrationEnvelope,
            _payload: IntegrationOutboundPayload,
        ) -> Result<(), CoreError> {
            self.enqueued.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        async fn flush(&self) -> Result<usize, CoreError> {
            self.flushed.fetch_add(1, Ordering::Relaxed);
            Ok(1)
        }

        async fn last_ack_cursor(
            &self,
        ) -> Result<Option<maekon_core::models::integration::IntegrationAckCursor>, CoreError>
        {
            Ok(None)
        }
    }

    fn integration_envelope() -> IntegrationEnvelope {
        IntegrationEnvelope {
            envelope_id: "env-test".to_string(),
            schema_version: "1".to_string(),
            message_type: IntegrationMessageType::InsightPacket,
            timestamp: chrono::Utc::now(),
            nonce: "nonce".to_string(),
            origin: IntegrationOrigin {
                device_id: "device-test".to_string(),
                workspace_id: None,
                session_id: None,
                source: "test".to_string(),
            },
            capability_scope: IntegrationCapabilityScope::InsightWrite,
        }
    }

    fn integration_payload() -> IntegrationOutboundPayload {
        let now = chrono::Utc::now();
        IntegrationOutboundPayload::Insight(InsightPacket {
            packet_id: "packet-test".to_string(),
            summary: "derived local summary".to_string(),
            derived_tags: vec!["test".to_string()],
            source_window: InsightSourceWindow {
                started_at: now,
                ended_at: now,
            },
            privacy_classification: IntegrationPrivacyClassification::DerivedSummary,
            audit_reference_id: None,
        })
    }

    fn consent_with_telemetry(telemetry: bool) -> (Arc<dyn ConsentManagerPort>, TempDir) {
        let temp_dir = TempDir::new().expect("temp dir");
        let manager = Arc::new(ConsentManager::new(temp_dir.path().join("consent.json")));
        if telemetry {
            manager
                .grant_consent(
                    ConsentPermissions {
                        telemetry: true,
                        ..Default::default()
                    },
                    30,
                )
                .expect("grant telemetry consent");
        }
        (manager, temp_dir)
    }

    #[tokio::test]
    async fn integration_egress_enqueue_requires_telemetry_consent() {
        let inner = Arc::new(CountingIntegrationEgress::default());
        let (consent, _dir) = consent_with_telemetry(false);
        let gated = ConsentGatedIntegrationEgress::new(inner.clone(), consent);

        let err = gated
            .enqueue_message(integration_envelope(), integration_payload())
            .await
            .expect_err("enqueue must fail closed without telemetry consent");

        assert!(matches!(err, CoreError::ConsentRequired { .. }));
        assert_eq!(inner.enqueued.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn integration_egress_flush_is_noop_after_telemetry_consent_revoked() {
        let inner = Arc::new(CountingIntegrationEgress::default());
        let (consent, _dir) = consent_with_telemetry(true);
        let gated = ConsentGatedIntegrationEgress::new(inner.clone(), consent.clone());

        consent
            .revoke_consent()
            .expect("revocation should persist in temp consent file");

        let flushed = gated
            .flush()
            .await
            .expect("flush should not error on denial");

        assert_eq!(flushed, 0);
        assert_eq!(inner.flushed.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn integration_egress_allows_after_telemetry_consent_granted() {
        let inner = Arc::new(CountingIntegrationEgress::default());
        let (consent, _dir) = consent_with_telemetry(true);
        let gated = ConsentGatedIntegrationEgress::new(inner.clone(), consent);

        gated
            .enqueue_message(integration_envelope(), integration_payload())
            .await
            .expect("telemetry consent should allow enqueue");
        let flushed = gated
            .flush()
            .await
            .expect("telemetry consent should allow flush");

        assert_eq!(flushed, 1);
        assert_eq!(inner.enqueued.load(Ordering::Relaxed), 1);
        assert_eq!(inner.flushed.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn build_runtime_requires_bootstrap_url_for_runtime() {
        let temp_dir = TempDir::new().expect("temp dir");
        let mut config = AppConfig::default_config();
        config.integration.enabled = true;
        config.integration.auth_token_env_var = Some("MAEKON_TEST_INTEGRATION_TOKEN".to_string());

        let runtime = IntegrationRuntimeBuilder::new(&config, temp_dir.path(), None, None)
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

    /// #6936 regression guard: integration insight egress PII level must be floored
    /// through effective_egress_pii_level. AllowFiltered + Off → Basic (not raw Off),
    /// and must differ from the raw level (else the floor isn't applied = the bug).
    #[test]
    fn integration_egress_pii_level_floors_allow_filtered_off() {
        use maekon_core::config::{ExternalDataPolicy, PiiFilterLevel};
        let mut config = AppConfig::default_config();
        config.ai_provider.external_data_policy = ExternalDataPolicy::AllowFiltered;
        config.privacy.pii_filter_level = PiiFilterLevel::Off;

        let level = super::integration_egress_pii_level(&config);
        assert_eq!(
            level,
            PiiFilterLevel::Basic,
            "AllowFiltered + Off must floor integration egress to Basic"
        );
        assert_ne!(
            level, config.privacy.pii_filter_level,
            "egress level equal to raw pii_filter_level means the floor wasn't applied (bug)"
        );

        // PiiFilterStrict policy pins Strict regardless of configured level.
        config.ai_provider.external_data_policy = ExternalDataPolicy::PiiFilterStrict;
        assert_eq!(
            super::integration_egress_pii_level(&config),
            PiiFilterLevel::Strict
        );
    }

    #[test]
    fn build_runtime_preserves_dpop_signing() {
        let temp_dir = TempDir::new().expect("temp dir");
        let mut config = AppConfig::default_config();
        config.integration.enabled = true;
        config.integration.bootstrap_url =
            Some("https://integration.example.com/bootstrap".to_string());
        config.integration.auth_token_env_var = Some("MAEKON_TEST_INTEGRATION_TOKEN".to_string());
        config.integration.supported_auth_schemes = vec![IntegrationAuthScheme::DpopBearer];

        let runtime = IntegrationRuntimeBuilder::new(&config, temp_dir.path(), None, None)
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

    // `#[serial]`: this test mutates process-global env (`set_var`/`remove_var`),
    // which is `unsafe` and races other tests in the binary. Serialize it.
    #[test]
    #[serial]
    fn build_runtime_reports_auth_material_presence() {
        let temp_dir = TempDir::new().expect("temp dir");
        let mut config = AppConfig::default_config();
        config.integration.enabled = true;
        config.integration.bootstrap_url =
            Some("https://integration.example.com/bootstrap".to_string());
        config.integration.auth_token_env_var = Some("MAEKON_TEST_INTEGRATION_TOKEN".to_string());

        // SAFETY: set_var mutates process-global environment, which is unsound
        // if another thread reads/writes the environment concurrently. The
        // `#[serial]` attribute (see comment above) guarantees no other test in
        // this binary runs while this one holds the env, so there is no
        // concurrent access.
        unsafe {
            std::env::set_var("MAEKON_TEST_INTEGRATION_TOKEN", "token-value");
        }
        let runtime = IntegrationRuntimeBuilder::new(&config, temp_dir.path(), None, None)
            .build()
            .unwrap();
        // SAFETY: same `#[serial]` exclusivity as the set_var above — no other
        // thread is touching the environment while we remove this key.
        unsafe {
            std::env::remove_var("MAEKON_TEST_INTEGRATION_TOKEN");
        }

        assert!(runtime.bindings().status.auth_material_available);
    }
}
