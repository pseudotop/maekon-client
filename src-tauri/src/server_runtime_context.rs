// C1: ServerBootstrapContext now holds only integration-runtime assets.
// Provider assets (BYOK secret stores, OAuth port/coordinator) have been
// extracted to ProviderRuntimeContext (#[cfg(feature = "analysis")]) so they
// are available in default builds without the 'server' feature.
#[cfg(feature = "server")]
use anyhow::Result;
#[cfg(feature = "server")]
use maekon_api_contracts::integration::IntegrationOutboundRuntimeStatus;
#[cfg(feature = "server")]
use maekon_core::config::AppConfig;
#[cfg(feature = "server")]
use maekon_core::config_manager::ConfigManager;
#[cfg(feature = "server")]
use maekon_storage::sqlite::SqliteStorage;
#[cfg(feature = "server")]
use std::path::Path;
#[cfg(feature = "server")]
use std::sync::Arc;

#[cfg(feature = "server")]
use crate::background_runtime::BackgroundRuntimeCoordinator;
#[cfg(feature = "server")]
use crate::integration_runtime::{
    IntegrationRuntimeBindings, IntegrationRuntimeBuilder, IntegrationRuntimeBundle,
};
#[cfg(feature = "server")]
use crate::provider_secret_backend::create_os_secret_store;
#[cfg(feature = "server")]
use crate::runtime_state::ManagedStateBuilder;
#[cfg(feature = "server")]
use crate::web_server_runtime::{WebServerRuntimeBuilder, WebServerServerSupport};

/// Integration-only bootstrap context for the 'server' feature.
///
/// Provider assets (BYOK secret-store resolution, OAuth port/coordinator) are
/// now in ProviderRuntimeContext so that analysis-only builds can wire them
/// without enabling the full server transport stack.
#[cfg(feature = "server")]
pub(crate) struct ServerBootstrapContext {
    pub(crate) integration_runtime: IntegrationRuntimeBundle,
    pub(crate) integration_bindings: IntegrationRuntimeBindings,
}

#[cfg(feature = "server")]
impl ServerBootstrapContext {
    pub(crate) fn build(config: &AppConfig, data_dir_path: &Path) -> Result<Self> {
        let config_dir =
            ConfigManager::config_dir().unwrap_or_else(|_| data_dir_path.to_path_buf());
        // create_os_secret_store is used here only to construct the integration
        // transport (OIDC device flow auth).  The provider credential store is
        // constructed separately in ProviderRuntimeContext::build.
        let desktop_secret_store = create_os_secret_store(&config_dir);
        let integration_runtime =
            IntegrationRuntimeBuilder::new(config, &config_dir, desktop_secret_store).build()?;
        let integration_bindings = integration_runtime.bindings();

        Ok(Self {
            integration_runtime,
            integration_bindings,
        })
    }

    pub(crate) fn integration_runtime_status(&self) -> IntegrationOutboundRuntimeStatus {
        self.integration_bindings.status.clone()
    }
}

/// Integration-only launch context for the 'server' feature.
///
/// Wraps integration bindings for use in the app_runtime_launch composition
/// root.  Provider wiring is handled separately via ProviderRuntimeContext
/// (called unconditionally under #[cfg(feature = "analysis")]).
#[cfg(feature = "server")]
pub(crate) struct ServerLaunchContext {
    pub(crate) integration_runtime: IntegrationRuntimeBundle,
    pub(crate) integration_bindings: IntegrationRuntimeBindings,
}

#[cfg(feature = "server")]
impl ServerLaunchContext {
    pub(crate) fn from_bootstrap(server: ServerBootstrapContext) -> Self {
        Self {
            integration_runtime: server.integration_runtime,
            integration_bindings: server.integration_bindings,
        }
    }

    pub(crate) fn spawn_integration_loops(
        &self,
        background_runtime: &BackgroundRuntimeCoordinator<'_>,
        sqlite_storage: Arc<SqliteStorage>,
    ) {
        background_runtime.spawn_integration_loops(&self.integration_runtime, sqlite_storage);
    }

    /// Wire only the integration auth bindings into the state builder.
    ///
    /// Provider wiring (with_oauth, with_secret_backend_profile) is done by
    /// ProviderRuntimeContext::configure_state_builder under
    /// #[cfg(feature = "analysis")] in app_runtime_launch.
    pub(crate) fn configure_state_builder(
        &self,
        builder: ManagedStateBuilder,
    ) -> ManagedStateBuilder {
        builder.with_integration(
            self.integration_bindings.auth.clone(),
            self.integration_bindings.session.clone(),
        )
    }

    /// Wire integration bindings and provider credentials into the web-server builder.
    ///
    /// Accepts the provider secret-store and oauth_port as explicit parameters
    /// so this method can be called from app_runtime_launch without coupling the
    /// two contexts at the type level.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn configure_web_server_builder<'a>(
        &self,
        builder: WebServerRuntimeBuilder<'a>,
        secret_backend_capabilities: crate::runtime_state::SecretBackendCapabilities,
        secret_store: Option<Arc<dyn maekon_core::ports::secret_store::SecretStore>>,
        secret_stores: Option<maekon_core::ports::secret_store::SecretStoreSet>,
        default_backend_kind: Option<maekon_core::config::CredentialBackendKind>,
        oauth_port: Option<Arc<dyn maekon_core::ports::oauth::OAuthPort>>,
    ) -> WebServerRuntimeBuilder<'a> {
        builder
            .with_server_support(WebServerServerSupport::new(
                self.integration_bindings.auth.clone(),
                self.integration_bindings.session.clone(),
                self.integration_bindings.outbox.clone(),
                self.integration_bindings.inbox.clone(),
                self.integration_bindings.inbox_store.clone(),
                self.integration_bindings.audit.clone(),
                self.integration_bindings.telemetry.clone(),
                secret_store,
                secret_stores,
                default_backend_kind,
                oauth_port,
            ))
            .with_secret_backend_capabilities(secret_backend_capabilities)
    }
}
