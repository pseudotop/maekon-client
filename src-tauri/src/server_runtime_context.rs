#[cfg(feature = "server")]
use anyhow::Result;
#[cfg(feature = "server")]
use oneshim_api_contracts::integration::IntegrationOutboundRuntimeStatus;
#[cfg(feature = "server")]
use oneshim_core::config::{AiAccessMode, AppConfig};
#[cfg(feature = "server")]
use oneshim_core::config_manager::ConfigManager;
#[cfg(feature = "server")]
use oneshim_core::ports::oauth::OAuthPort;
#[cfg(feature = "server")]
use oneshim_core::ports::secret_store::{SecretStore, SecretStoreSet};
#[cfg(feature = "server")]
use oneshim_network::oauth::refresh_coordinator::TokenRefreshCoordinator;
#[cfg(feature = "server")]
use oneshim_network::oauth::OAuthClient;
#[cfg(feature = "server")]
use oneshim_storage::sqlite::SqliteStorage;
#[cfg(feature = "server")]
use std::path::Path;
#[cfg(feature = "server")]
use std::sync::Arc;

#[cfg(feature = "server")]
use crate::agent_runtime::AgentRuntimeBuilder;
#[cfg(feature = "server")]
use crate::background_runtime::BackgroundRuntimeCoordinator;
#[cfg(feature = "server")]
use crate::integration_runtime::{
    IntegrationRuntimeBindings, IntegrationRuntimeBuilder, IntegrationRuntimeBundle,
};
#[cfg(feature = "server")]
use crate::oauth_provider_registry::{
    configured_oauth_provider_configs, configured_oauth_provider_ids,
};
#[cfg(feature = "server")]
use crate::provider_secret_backend::{
    build_provider_secret_store_set, create_os_secret_store, resolve_provider_secret_backend,
    ProviderSecretBackendResolution,
};
#[cfg(feature = "server")]
use crate::runtime_state::{ManagedStateBuilder, ManagedStateCapabilityProfile, OAuthCoordinator};
#[cfg(feature = "server")]
use crate::web_server_runtime::{WebServerRuntimeBuilder, WebServerServerSupport};

#[cfg(feature = "server")]
pub(crate) struct ServerBootstrapContext {
    pub(crate) provider_secret_backend: ProviderSecretBackendResolution,
    pub(crate) provider_secret_stores: SecretStoreSet,
    pub(crate) integration_runtime: IntegrationRuntimeBundle,
    pub(crate) integration_bindings: IntegrationRuntimeBindings,
    pub(crate) oauth_port: Option<Arc<dyn OAuthPort>>,
    pub(crate) oauth_coordinator: Option<Arc<TokenRefreshCoordinator>>,
    pub(crate) oauth_provider_ids: Vec<String>,
}

#[cfg(feature = "server")]
impl ServerBootstrapContext {
    pub(crate) fn build(config: &AppConfig, data_dir_path: &Path) -> Result<Self> {
        let config_dir =
            ConfigManager::config_dir().unwrap_or_else(|_| data_dir_path.to_path_buf());
        let desktop_secret_store = create_os_secret_store(&config_dir);
        let provider_secret_backend =
            resolve_provider_secret_backend(&config_dir, desktop_secret_store.clone())?;
        let provider_secret_stores = build_provider_secret_store_set(
            &config_dir,
            desktop_secret_store.clone(),
            &provider_secret_backend,
        );
        let integration_runtime =
            IntegrationRuntimeBuilder::new(config, &config_dir, desktop_secret_store.clone())
                .build()?;
        let integration_bindings = integration_runtime.bindings();
        let oauth_port = desktop_secret_store.clone().map(create_oauth_port);
        let oauth_provider_ids = configured_oauth_provider_ids();
        let oauth_coordinator =
            build_oauth_coordinator(config, oauth_port.as_ref().map(Arc::clone));

        Ok(Self {
            provider_secret_backend,
            provider_secret_stores,
            integration_runtime,
            integration_bindings,
            oauth_port,
            oauth_coordinator,
            oauth_provider_ids,
        })
    }

    pub(crate) fn integration_runtime_status(&self) -> IntegrationOutboundRuntimeStatus {
        self.integration_bindings.status.clone()
    }
}

#[cfg(feature = "server")]
pub(crate) struct ServerLaunchContext {
    provider_secret_backend: ProviderSecretBackendResolution,
    provider_secret_stores: SecretStoreSet,
    integration_runtime: IntegrationRuntimeBundle,
    integration_bindings: IntegrationRuntimeBindings,
    oauth_port: Option<Arc<dyn OAuthPort>>,
    oauth_coordinator: OAuthCoordinator,
    oauth_provider_ids: Vec<String>,
}

#[cfg(feature = "server")]
impl ServerLaunchContext {
    pub(crate) fn from_bootstrap(server: ServerBootstrapContext) -> Self {
        Self {
            provider_secret_backend: server.provider_secret_backend,
            provider_secret_stores: server.provider_secret_stores,
            integration_runtime: server.integration_runtime,
            integration_bindings: server.integration_bindings,
            oauth_port: server.oauth_port,
            oauth_coordinator: server.oauth_coordinator,
            oauth_provider_ids: server.oauth_provider_ids,
        }
    }

    pub(crate) fn spawn_integration_loops(
        &self,
        background_runtime: &BackgroundRuntimeCoordinator<'_>,
        sqlite_storage: Arc<SqliteStorage>,
    ) {
        background_runtime.spawn_integration_loops(&self.integration_runtime, sqlite_storage);
    }

    pub(crate) fn configure_agent_builder<'a>(
        &self,
        builder: AgentRuntimeBuilder<'a>,
    ) -> AgentRuntimeBuilder<'a> {
        builder.with_oauth_coordinator(self.oauth_coordinator.clone())
    }

    pub(crate) fn configure_web_server_builder<'a>(
        &self,
        builder: WebServerRuntimeBuilder<'a>,
    ) -> WebServerRuntimeBuilder<'a> {
        builder.with_server_support(WebServerServerSupport::new(
            self.integration_bindings.auth.clone(),
            self.integration_bindings.session.clone(),
            self.integration_bindings.outbox.clone(),
            self.integration_bindings.inbox.clone(),
            self.integration_bindings.inbox_store.clone(),
            self.integration_bindings.audit.clone(),
            self.integration_bindings.telemetry.clone(),
            self.provider_secret_backend.secret_store.clone(),
            Some(self.provider_secret_stores.clone()),
            Some(self.provider_secret_backend.backend_kind),
            self.oauth_port.clone(),
        ))
    }

    pub(crate) fn configure_state_builder(
        &self,
        builder: ManagedStateBuilder,
    ) -> ManagedStateBuilder {
        builder
            .with_oauth(self.oauth_port.clone(), self.oauth_coordinator.clone())
            .with_secret_backend_profile(ManagedStateCapabilityProfile {
                oauth_provider_ids: self.oauth_provider_ids.clone(),
                provider_backend_kind: self.provider_secret_backend.backend_kind,
                fallback_backend_kind: self.provider_secret_backend.fallback_backend_kind,
            })
            .with_integration(
                self.integration_bindings.auth.clone(),
                self.integration_bindings.session.clone(),
            )
    }
}

#[cfg(feature = "server")]
fn create_oauth_port(secret_store: Arc<dyn SecretStore>) -> Arc<dyn OAuthPort> {
    let providers = configured_oauth_provider_configs();
    Arc::new(OAuthClient::new(secret_store, providers)) as Arc<dyn OAuthPort>
}

#[cfg(feature = "server")]
fn build_oauth_coordinator(
    config: &AppConfig,
    oauth_port: Option<Arc<dyn OAuthPort>>,
) -> Option<Arc<TokenRefreshCoordinator>> {
    if !matches!(config.ai_provider.access_mode, AiAccessMode::ProviderOAuth) {
        return None;
    }

    oauth_port.map(|port| {
        let (token_event_tx, _) = tokio::sync::broadcast::channel(32);
        Arc::new(TokenRefreshCoordinator::new(port, token_event_tx))
    })
}
