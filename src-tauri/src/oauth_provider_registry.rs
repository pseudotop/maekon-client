#[cfg(feature = "analysis")]
use maekon_api_contracts::ai_providers::ProviderTransportSpec;
#[cfg(feature = "analysis")]
use maekon_api_contracts::provider_specs::{
    self, provider_surface_catalog, provider_surface_spec, ProviderSurfaceSpec,
    ProviderTransportKind, SurfaceExecutionKind,
};
#[cfg(feature = "analysis")]
use maekon_core::config::{AiAccessMode, AiProviderConfig, AiProviderType, ExternalApiEndpoint};
#[cfg(feature = "analysis")]
use maekon_core::error::CoreError;
#[cfg(feature = "analysis")]
use maekon_core::provider_surface::default_provider_surface_id;
#[cfg(feature = "analysis")]
use maekon_network::oauth::provider_config::OAuthProviderConfig;
#[cfg(feature = "analysis")]
use tracing::warn;

#[cfg(feature = "analysis")]
#[derive(Clone, Copy)]
struct ManagedOAuthProviderFactory {
    vendor_id: &'static str,
    build: fn(&ProviderSurfaceSpec) -> Option<OAuthProviderConfig>,
}

#[cfg(feature = "analysis")]
fn managed_oauth_provider_factories() -> [ManagedOAuthProviderFactory; 2] {
    [
        ManagedOAuthProviderFactory {
            vendor_id: "openai",
            build: build_openai_managed_oauth_provider,
        },
        ManagedOAuthProviderFactory {
            vendor_id: "google",
            build: build_google_managed_oauth_provider,
        },
    ]
}

#[cfg(feature = "analysis")]
pub fn configured_oauth_provider_configs() -> Vec<OAuthProviderConfig> {
    let mut providers: Vec<OAuthProviderConfig> = managed_oauth_surface_specs()
        .into_iter()
        .flatten()
        .filter_map(build_managed_oauth_provider_config)
        .collect();
    providers.extend(connector_oauth_provider_configs());
    providers
}

/// Environment variable carrying the app-owned Google Desktop OAuth client id
/// for the calendar connector. Google publishes no shared public client id for
/// third-party desktop apps, so the operator provisions one (same posture as
/// the AI surfaces' `MAEKON_GOOGLE_OAUTH_CLIENT_ID`, but a separate credential:
/// the calendar consent screen must show calendar scopes only).
#[cfg(feature = "analysis")]
const GOOGLE_CALENDAR_OAUTH_CLIENT_ID_ENV: &str = "MAEKON_GOOGLE_CALENDAR_OAUTH_CLIENT_ID";

/// OAuth provider configs for the built-in connectors (ADR-034 P4, #9855).
///
/// Iterates `maekon_integration::connectors::builtin_connectors()` — the
/// compile-time registry of essential tools — and builds the matching provider
/// config for each entry whose credential is provisioned. Two properties:
///
/// - **Feature-gated at the source**: a build without
///   `connector-google-calendar` has no calendar row in the registry, so no
///   provider config either. This function needs no cfg of its own per entry.
/// - **Absent credential = absent provider, not an error**: the connector is
///   compiled in but cannot start an OAuth flow until the operator provisions
///   the client id. Nothing user-facing advertises the flow (the MK-EXT IPC
///   surface stays retired per #9639), so an unprovisioned build is inert
///   rather than a dead button.
#[cfg(feature = "analysis")]
fn connector_oauth_provider_configs() -> Vec<OAuthProviderConfig> {
    maekon_integration::connectors::builtin_connectors()
        .iter()
        .filter_map(|connector| {
            if connector.oauth_provider_id == maekon_core::ports::oauth::GOOGLE_CALENDAR_PROVIDER_ID
            {
                google_calendar_oauth_client_id().map(OAuthProviderConfig::google_calendar_readonly)
            } else {
                // A registry entry this wiring does not recognise is a wiring
                // gap, not a silent skip — the registry doc names this exact
                // hand-off, and the paired test in maekon-integration expects
                // the composition root to keep up.
                warn!(
                    provider_id = connector.oauth_provider_id,
                    "built-in connector has no OAuth wiring in the composition root"
                );
                None
            }
        })
        .collect()
}

#[cfg(feature = "analysis")]
fn google_calendar_oauth_client_id() -> Option<String> {
    std::env::var(GOOGLE_CALENDAR_OAUTH_CLIENT_ID_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(feature = "analysis")]
fn managed_oauth_surface_specs() -> Result<Vec<&'static ProviderSurfaceSpec>, String> {
    let catalog = provider_surface_catalog()?;
    Ok(catalog
        .surfaces
        .iter()
        .filter(|surface| {
            surface.execution_kind == SurfaceExecutionKind::ManagedHttp
                && surface
                    .credential_kind
                    .eq_ignore_ascii_case("managed_oauth")
        })
        .collect())
}

#[cfg(feature = "analysis")]
fn build_managed_oauth_provider_config(
    surface: &ProviderSurfaceSpec,
) -> Option<OAuthProviderConfig> {
    let factory = managed_oauth_provider_factories()
        .into_iter()
        .find(|factory| factory.vendor_id.eq_ignore_ascii_case(&surface.vendor_id));
    let Some(factory) = factory else {
        warn!(
            surface_id = %surface.surface_id,
            vendor_id = %surface.vendor_id,
            "Managed OAuth surface is present in the catalog but no runtime provider factory is registered."
        );
        return None;
    };
    (factory.build)(surface)
}

#[cfg(feature = "analysis")]
pub fn configured_oauth_provider_ids() -> Vec<String> {
    configured_oauth_provider_configs()
        .into_iter()
        .map(|provider| provider.provider_id)
        .collect()
}

#[cfg(feature = "analysis")]
pub fn selected_managed_oauth_provider_ids(
    config: &AiProviderConfig,
) -> Result<Vec<String>, CoreError> {
    let mut provider_ids = Vec::new();

    if let Some(endpoint) = config.llm_api.as_ref() {
        maybe_push_managed_provider(&mut provider_ids, endpoint, ProviderTransportKind::Llm)?;
    } else if config.llm_provider == maekon_core::config::LlmProviderType::Remote {
        if let Some(surface_id) =
            default_provider_surface_id(AiProviderType::OpenAi, AiAccessMode::ProviderOAuth)
        {
            // Iter-107: catalog-miss = NotFound (consistent with iter-94
            // session_manager/factory.rs fixes).
            let surface = provider_surface_spec(surface_id).map_err(|msg| CoreError::NotFound {
                code: maekon_core::error_codes::NotFoundCode::ResourceMissing,
                resource_type: "provider_surface".to_string(),
                id: format!("{surface_id}: {msg}"),
            })?;
            provider_ids.push(surface.vendor_id.clone());
        } else {
            provider_ids.push("openai".to_string());
        }
    }

    if let Some(endpoint) = config.ocr_api.as_ref() {
        maybe_push_managed_provider(&mut provider_ids, endpoint, ProviderTransportKind::Ocr)?;
    }

    Ok(provider_ids)
}

#[cfg(feature = "analysis")]
pub fn managed_oauth_provider_id_for_endpoint(
    endpoint: &ExternalApiEndpoint,
    _kind: ProviderTransportKind,
) -> Result<String, CoreError> {
    Ok(managed_oauth_surface(endpoint)?.vendor_id.clone())
}

#[cfg(feature = "analysis")]
pub fn managed_oauth_transport_url_for_endpoint(
    endpoint: &ExternalApiEndpoint,
    kind: ProviderTransportKind,
) -> Result<String, CoreError> {
    Ok(managed_oauth_transport_spec(endpoint, kind)?.url.clone())
}

#[cfg(feature = "analysis")]
fn managed_oauth_transport_spec(
    endpoint: &ExternalApiEndpoint,
    kind: ProviderTransportKind,
) -> Result<&ProviderTransportSpec, CoreError> {
    managed_oauth_surface(endpoint)?;
    // Iter-107: transport catalog miss = NotFound.
    let spec = provider_specs::resolved_transport_spec(
        endpoint.provider_type,
        endpoint.surface_id.as_deref(),
        kind,
    )
    .map_err(|msg| CoreError::NotFound {
        code: maekon_core::error_codes::NotFoundCode::ResourceMissing,
        resource_type: "provider_transport".to_string(),
        id: format!("{:?}/{kind:?}: {msg}", endpoint.provider_type),
    })?;

    Ok(spec)
}

#[cfg(feature = "analysis")]
fn managed_oauth_surface(
    endpoint: &ExternalApiEndpoint,
) -> Result<&maekon_api_contracts::provider_specs::ProviderSurfaceSpec, CoreError> {
    let surface =
        provider_surface_spec(
            endpoint
                .surface_id
                .as_deref()
                .ok_or_else(|| CoreError::Config {
                    code: maekon_core::error_codes::ConfigCode::Missing,
                    message: "Managed OAuth endpoint is missing provider surface metadata."
                        .to_string(),
                })?,
        )
        // Iter-107: surface-id-not-in-catalog = NotFound.
        .map_err(|msg| CoreError::NotFound {
            code: maekon_core::error_codes::NotFoundCode::ResourceMissing,
            resource_type: "provider_surface".to_string(),
            id: msg,
        })?;
    if surface.execution_kind != SurfaceExecutionKind::ManagedHttp {
        return Err(CoreError::Config {
            code: maekon_core::error_codes::ConfigCode::Invalid,
            message: "Selected provider surface does not use managed OAuth transport.".to_string(),
        });
    }
    Ok(surface)
}

#[cfg(feature = "analysis")]
fn maybe_push_managed_provider(
    provider_ids: &mut Vec<String>,
    endpoint: &ExternalApiEndpoint,
    kind: ProviderTransportKind,
) -> Result<(), CoreError> {
    match managed_oauth_transport_spec(endpoint, kind) {
        Ok(_) => {
            let provider_id = managed_oauth_provider_id_for_endpoint(endpoint, kind)?;
            if !provider_ids
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(&provider_id))
            {
                provider_ids.push(provider_id);
            }
            Ok(())
        }
        Err(CoreError::Config { .. }) => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(feature = "analysis")]
fn configured_provisioning_env_value(
    surface: &ProviderSurfaceSpec,
    index: usize,
) -> Option<String> {
    surface
        .provisioning
        .as_ref()
        .and_then(|provisioning| provisioning.configuration_env_vars.get(index))
        .and_then(|env_var| std::env::var(env_var).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(feature = "analysis")]
fn google_oauth_client_id(surface: &ProviderSurfaceSpec) -> Option<String> {
    configured_provisioning_env_value(surface, 0)
}

#[cfg(feature = "analysis")]
fn build_openai_managed_oauth_provider(
    _surface: &ProviderSurfaceSpec,
) -> Option<OAuthProviderConfig> {
    Some(OAuthProviderConfig::openai_codex())
}

#[cfg(feature = "analysis")]
fn build_google_managed_oauth_provider(
    surface: &ProviderSurfaceSpec,
) -> Option<OAuthProviderConfig> {
    google_oauth_client_id(surface).map(OAuthProviderConfig::google_cloud_vision)
}

// ToS invariant: managed_oauth_provider_factories registers only openai + google.
// Anthropic subscription OAuth relay is prohibited under the ADR-025/#4884 ToS policy.
// Before adding an "anthropic" vendor to this array, follow the ADR-019 §5 8-step checklist.
#[cfg(all(test, feature = "analysis"))]
mod tests {
    use super::*;
    use maekon_core::config::{
        AiProviderType, CredentialAuthMode, CredentialBackendKind, CredentialBinding,
        ExternalApiEndpoint,
    };

    fn managed_surface_id_for(provider_type: AiProviderType) -> String {
        default_provider_surface_id(provider_type, AiAccessMode::ProviderOAuth)
            .expect("managed OAuth surface should exist")
            .to_string()
    }

    fn managed_google_ocr_endpoint() -> ExternalApiEndpoint {
        ExternalApiEndpoint {
            endpoint: "https://vision.googleapis.com/v1/images:annotate".to_string(),
            api_key: String::new(),
            model: None,
            timeout_secs: 30,
            provider_type: AiProviderType::Google,
            surface_id: Some(managed_surface_id_for(AiProviderType::Google)),
            credential: Some(CredentialBinding {
                auth_mode: CredentialAuthMode::ManagedOAuth,
                backend_kind: CredentialBackendKind::OsSecretStore,
                secret_ref: None,
                projection_enabled: false,
            }),
        }
    }

    #[test]
    fn managed_oauth_url_uses_surface_transport() {
        let endpoint = managed_google_ocr_endpoint();
        let url = managed_oauth_transport_url_for_endpoint(&endpoint, ProviderTransportKind::Ocr)
            .expect("managed OAuth transport URL should resolve");
        assert_eq!(url, "https://vision.googleapis.com/v1/images:annotate");
    }

    #[test]
    fn selected_managed_oauth_provider_ids_collects_google() {
        let config = AiProviderConfig {
            access_mode: maekon_core::config::AiAccessMode::ProviderOAuth,
            ocr_provider: maekon_core::config::OcrProviderType::Remote,
            llm_provider: maekon_core::config::LlmProviderType::Local,
            ocr_api: Some(managed_google_ocr_endpoint()),
            ..AiProviderConfig::default()
        };

        let providers =
            selected_managed_oauth_provider_ids(&config).expect("provider IDs should resolve");
        assert_eq!(providers, vec!["google".to_string()]);
    }

    #[test]
    fn google_oauth_client_id_uses_surface_provisioning_env_var() {
        std::env::set_var("MAEKON_GOOGLE_OAUTH_CLIENT_ID", "test-google-client-id");
        let surface = provider_surface_spec(&managed_surface_id_for(AiProviderType::Google))
            .expect("google managed OAuth surface should exist");
        let client_id = google_oauth_client_id(surface);
        std::env::remove_var("MAEKON_GOOGLE_OAUTH_CLIENT_ID");
        assert_eq!(client_id.as_deref(), Some("test-google-client-id"));
    }

    /// ADR-034 P4 (#9855): the calendar connector's OAuth provider appears
    /// exactly when its client id is provisioned — absent credential means an
    /// inert connector, not an error and not a misconfigured provider row.
    #[test]
    fn calendar_connector_provider_follows_the_provisioned_client_id() {
        std::env::remove_var(GOOGLE_CALENDAR_OAUTH_CLIENT_ID_ENV);
        assert!(
            connector_oauth_provider_configs().is_empty(),
            "no client id must mean no provider row"
        );

        std::env::set_var(
            GOOGLE_CALENDAR_OAUTH_CLIENT_ID_ENV,
            "test-calendar-client-id",
        );
        let providers = connector_oauth_provider_configs();
        std::env::remove_var(GOOGLE_CALENDAR_OAUTH_CLIENT_ID_ENV);

        assert_eq!(providers.len(), 1);
        let p = &providers[0];
        assert_eq!(
            p.provider_id,
            maekon_core::ports::oauth::GOOGLE_CALENDAR_PROVIDER_ID,
            "provider id must be the shared core constant — the connector's \
             SecretStore namespace and this row must never drift"
        );
        assert_eq!(
            p.scopes,
            vec![maekon_core::ports::oauth::GOOGLE_CALENDAR_READONLY_SCOPE.to_string()],
            "read-only scope only (MK-EXT #8582)"
        );
        assert_eq!(p.client_id, "test-calendar-client-id");
    }

    /// ToS invariant guard (ADR-025 / #4884): managed_oauth_provider_factories must
    /// contain only openai + google.  Anthropic subscription OAuth relay is prohibited.
    ///
    /// If this test fails, a new vendor was added — review the ADR-019 §5 8-step
    /// checklist before merging.
    #[test]
    fn managed_oauth_provider_factories_does_not_contain_anthropic() {
        let factories = managed_oauth_provider_factories();
        for factory in &factories {
            let vendor = factory.vendor_id.to_lowercase();
            assert_ne!(
                vendor, "anthropic",
                "Anthropic must NOT appear in managed_oauth_provider_factories — ADR-025 ToS gate"
            );
        }
    }

    /// ToS invariant: factory count is exactly 2 (openai + google).
    ///
    /// If this count changes, update this test AND review ADR-025 before merging.
    #[test]
    fn managed_oauth_provider_factories_has_exactly_two_entries() {
        let factories = managed_oauth_provider_factories();
        assert_eq!(
            factories.len(),
            2,
            "Expected exactly 2 managed OAuth providers (openai + google); got {}",
            factories.len()
        );
    }
}
