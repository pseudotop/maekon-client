// C1: BYOK/OAuth direct-provider adapter context — available in 'analysis' builds.
//
// Extracts the user-provider assets that were previously bundled inside
// ServerBootstrapContext so they are reachable in default (analysis-only)
// builds without requiring the 'server' (Oneshim transport) feature.
//
// Dependency chain:
//   maekon-network (analysis-gated) → OAuthClient, TokenRefreshCoordinator
//   maekon-storage (always)         → create_os_secret_store
//   maekon-core    (always)         → SecretStoreSet, OAuthPort

#[cfg(feature = "analysis")]
use anyhow::Result;
#[cfg(feature = "analysis")]
use maekon_core::config::{AiAccessMode, AppConfig};
#[cfg(feature = "analysis")]
use maekon_core::config_manager::ConfigManager;
#[cfg(feature = "analysis")]
use maekon_core::ports::oauth::OAuthPort;
#[cfg(feature = "analysis")]
use maekon_core::ports::secret_store::{SecretStore, SecretStoreSet};
#[cfg(feature = "analysis")]
use maekon_network::oauth::refresh_coordinator::TokenRefreshCoordinator;
#[cfg(feature = "analysis")]
use maekon_network::oauth::OAuthClient;
#[cfg(feature = "analysis")]
use std::path::Path;
#[cfg(feature = "analysis")]
use std::sync::Arc;

#[cfg(feature = "analysis")]
use crate::agent_runtime::AgentRuntimeBundle;
#[cfg(feature = "analysis")]
use crate::oauth_provider_registry::{
    configured_oauth_provider_configs, configured_oauth_provider_ids,
};
#[cfg(feature = "analysis")]
use crate::provider_secret_backend::{
    build_provider_secret_store_set, create_os_secret_store, resolve_provider_secret_backend,
    ProviderSecretBackendResolution,
};
#[cfg(feature = "analysis")]
use crate::runtime_state::{
    secret_backend_capabilities, ManagedStateBuilder, ManagedStateCapabilityProfile,
    OAuthCoordinator,
};

/// User-provider credential and OAuth assets — analysis-only builds.
///
/// Holds BYOK secret-store resolution and OAuth coordinator so the agent and
/// web-server paths can wire BYOK/OAuth provider adapters without the 'server'
/// (Oneshim transport) feature being enabled.
#[cfg(feature = "analysis")]
pub(crate) struct ProviderRuntimeContext {
    pub(crate) provider_secret_backend: ProviderSecretBackendResolution,
    pub(crate) provider_secret_stores: SecretStoreSet,
    pub(crate) oauth_port: Option<Arc<dyn OAuthPort>>,
    pub(crate) oauth_coordinator: OAuthCoordinator,
    pub(crate) oauth_provider_ids: Vec<String>,
}

#[cfg(feature = "analysis")]
impl ProviderRuntimeContext {
    /// Build the provider context from config + data directory.
    ///
    /// Called unconditionally during bootstrap so that analysis-only builds
    /// (default features, no 'server') still wire BYOK/OAuth adapters.
    /// The 'server' bootstrap path calls this via BootstrapRuntimeBundle.provider
    /// and additionally constructs the integration runtime on top.
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

        // Edge case (design verify): oauth_port must be created unconditionally
        // when the OS keychain exists so capability reporting is accurate even
        // in non-OAuth access_mode configurations.
        let oauth_port = desktop_secret_store.clone().map(create_oauth_port);
        let oauth_provider_ids = configured_oauth_provider_ids();
        let oauth_coordinator =
            build_oauth_coordinator(config, oauth_port.as_ref().map(Arc::clone));

        Ok(Self {
            provider_secret_backend,
            provider_secret_stores,
            oauth_port,
            oauth_coordinator,
            oauth_provider_ids,
        })
    }

    /// Wire provider credentials and OAuth state into the agent runtime builder.
    pub(crate) fn configure_agent_builder(
        &self,
        builder: AgentRuntimeBundle,
    ) -> AgentRuntimeBundle {
        builder
            .with_oauth_coordinator(self.oauth_coordinator.clone())
            .with_provider_secret_stores(self.provider_secret_stores.clone())
    }

    /// Wire provider credentials and OAuth capability profile into the Tauri
    /// managed state builder.
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
    }

    /// Build a `SecretBackendCapabilities` snapshot for the web-server bindings.
    pub(crate) fn secret_backend_capabilities(
        &self,
    ) -> crate::runtime_state::SecretBackendCapabilities {
        secret_backend_capabilities(
            self.oauth_port.is_some(),
            self.oauth_provider_ids.clone(),
            self.provider_secret_backend.backend_kind,
            self.provider_secret_backend.fallback_backend_kind,
        )
    }
}

/// Build an `OAuthPort` from the OS-level secret store.
#[cfg(feature = "analysis")]
fn create_oauth_port(secret_store: Arc<dyn SecretStore>) -> Arc<dyn OAuthPort> {
    let providers = configured_oauth_provider_configs();
    Arc::new(OAuthClient::new(secret_store, providers)) as Arc<dyn OAuthPort>
}

/// Build the OAuth refresh coordinator when the access mode is `ProviderOAuth`.
///
/// Returns `None` for any other access mode so the background scheduler loop
/// never starts an unnecessary OAuth refresh cycle.
#[cfg(feature = "analysis")]
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

#[cfg(all(test, feature = "analysis"))]
mod tests {
    use super::*;
    use maekon_core::config::{AiAccessMode, AppConfig};

    fn test_config() -> AppConfig {
        AppConfig::default_config()
    }

    fn test_config_with_access_mode(access_mode: AiAccessMode) -> AppConfig {
        let mut config = AppConfig::default_config();
        config.ai_provider.access_mode = access_mode;
        config
    }

    /// C1 wiring test: ProviderRuntimeContext::build() succeeds in analysis builds.
    ///
    /// With the default access_mode (InlineApiKey), the coordinator is None.
    /// correction #1: this is the NEW wiring test added by C1 — the existing
    /// `managed_state_builder_defaults_to_unavailable_secret_backend` test is
    /// NOT modified (the default is still Unavailable; C1 only adds wiring).
    #[test]
    fn provider_runtime_context_builds_without_panic() {
        let config = test_config();
        let temp_dir = tempfile::tempdir().expect("tmp dir");
        // Must not panic — even without a real OS keychain the build path must
        // succeed (keychain absence yields Unavailable backend, not an error).
        let ctx = ProviderRuntimeContext::build(&config, temp_dir.path())
            .expect("ProviderRuntimeContext::build must not fail");
        // In non-OAuth access_mode the coordinator is always None.
        assert!(ctx.oauth_coordinator.is_none());
        // provider_ids list is populated from the registry (may be empty in test).
        let _ = &ctx.oauth_provider_ids;
    }

    /// secret_backend_capabilities() must not panic regardless of keychain state.
    #[test]
    fn secret_backend_capabilities_is_callable_after_build() {
        let config = test_config();
        let temp_dir = tempfile::tempdir().expect("tmp dir");
        let Ok(ctx) = ProviderRuntimeContext::build(&config, temp_dir.path()) else {
            return;
        };
        let caps = ctx.secret_backend_capabilities();
        let _ = caps;
    }

    /// oauth_coordinator is None for non-OAuth access modes.
    #[test]
    fn oauth_coordinator_is_none_for_non_oauth_access_mode() {
        let config = test_config_with_access_mode(AiAccessMode::ProviderApiKey);
        let temp_dir = tempfile::tempdir().expect("tmp dir");
        let Ok(ctx) = ProviderRuntimeContext::build(&config, temp_dir.path()) else {
            return;
        };
        assert!(
            ctx.oauth_coordinator.is_none(),
            "oauth_coordinator must be None for ProviderApiKey mode"
        );
    }
}
