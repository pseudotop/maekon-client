#[cfg(feature = "server")]
use oneshim_api_contracts::ai_providers::ProviderTransportSpec;
#[cfg(feature = "server")]
use oneshim_api_contracts::provider_specs::{
    self, parse_surface_execution_kind, provider_surface_catalog, provider_surface_spec,
    ProviderSurfaceSpec, ProviderTransportKind, SurfaceExecutionKind,
};
#[cfg(feature = "server")]
use oneshim_core::config::{AiAccessMode, AiProviderConfig, AiProviderType, ExternalApiEndpoint};
#[cfg(feature = "server")]
use oneshim_core::error::CoreError;
#[cfg(feature = "server")]
use oneshim_core::provider_surface::default_provider_surface_id;
#[cfg(feature = "server")]
use oneshim_network::oauth::provider_config::OAuthProviderConfig;
#[cfg(feature = "server")]
use tracing::warn;

#[cfg(feature = "server")]
#[derive(Clone, Copy)]
struct ManagedOAuthProviderFactory {
    vendor_id: &'static str,
    build: fn(&ProviderSurfaceSpec) -> Option<OAuthProviderConfig>,
}

#[cfg(feature = "server")]
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

#[cfg(feature = "server")]
pub fn configured_oauth_provider_configs() -> Vec<OAuthProviderConfig> {
    managed_oauth_surface_specs()
        .into_iter()
        .flatten()
        .filter_map(build_managed_oauth_provider_config)
        .collect()
}

#[cfg(feature = "server")]
fn managed_oauth_surface_specs() -> Result<Vec<&'static ProviderSurfaceSpec>, String> {
    let catalog = provider_surface_catalog()?;
    Ok(catalog
        .surfaces
        .iter()
        .filter(|surface| {
            parse_surface_execution_kind(&surface.execution_kind).ok()
                == Some(SurfaceExecutionKind::ManagedHttp)
                && surface
                    .credential_kind
                    .eq_ignore_ascii_case("managed_oauth")
        })
        .collect())
}

#[cfg(feature = "server")]
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

#[cfg(feature = "server")]
pub fn configured_oauth_provider_ids() -> Vec<String> {
    configured_oauth_provider_configs()
        .into_iter()
        .map(|provider| provider.provider_id)
        .collect()
}

#[cfg(feature = "server")]
pub fn selected_managed_oauth_provider_ids(
    config: &AiProviderConfig,
) -> Result<Vec<String>, CoreError> {
    let mut provider_ids = Vec::new();

    if let Some(endpoint) = config.llm_api.as_ref() {
        maybe_push_managed_provider(&mut provider_ids, endpoint, ProviderTransportKind::Llm)?;
    } else if config.llm_provider == oneshim_core::config::LlmProviderType::Remote {
        if let Some(surface_id) =
            default_provider_surface_id(AiProviderType::OpenAi, AiAccessMode::ProviderOAuth)
        {
            // Iter-107: catalog-miss = NotFound (consistent with iter-94
            // session_manager/factory.rs fixes).
            let surface = provider_surface_spec(surface_id).map_err(|msg| CoreError::NotFound {
                code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
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

#[cfg(feature = "server")]
pub fn managed_oauth_provider_id_for_endpoint(
    endpoint: &ExternalApiEndpoint,
    _kind: ProviderTransportKind,
) -> Result<String, CoreError> {
    Ok(managed_oauth_surface(endpoint)?.vendor_id.clone())
}

#[cfg(feature = "server")]
pub fn managed_oauth_transport_url_for_endpoint(
    endpoint: &ExternalApiEndpoint,
    kind: ProviderTransportKind,
) -> Result<String, CoreError> {
    Ok(managed_oauth_transport_spec(endpoint, kind)?.url.clone())
}

#[cfg(feature = "server")]
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
        code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
        resource_type: "provider_transport".to_string(),
        id: format!("{:?}/{kind:?}: {msg}", endpoint.provider_type),
    })?;

    Ok(spec)
}

#[cfg(feature = "server")]
fn managed_oauth_surface(
    endpoint: &ExternalApiEndpoint,
) -> Result<&oneshim_api_contracts::provider_specs::ProviderSurfaceSpec, CoreError> {
    let surface =
        provider_surface_spec(
            endpoint
                .surface_id
                .as_deref()
                .ok_or_else(|| CoreError::Config {
                    code: oneshim_core::error_codes::ConfigCode::Missing,
                    message: "Managed OAuth endpoint is missing provider surface metadata."
                        .to_string(),
                })?,
        )
        // Iter-107: surface-id-not-in-catalog = NotFound.
        .map_err(|msg| CoreError::NotFound {
            code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
            resource_type: "provider_surface".to_string(),
            id: msg,
        })?;
    // Iter-107: catalog metadata has bad execution_kind value = invalid
    // catalog data (programmer/catalog bug). Config::Invalid fits the
    // "catalog contains an unsupported value" semantic better than Internal.
    if parse_surface_execution_kind(&surface.execution_kind).map_err(|msg| CoreError::Config {
        code: oneshim_core::error_codes::ConfigCode::Invalid,
        message: msg,
    })? != SurfaceExecutionKind::ManagedHttp
    {
        return Err(CoreError::Config {
            code: oneshim_core::error_codes::ConfigCode::Invalid,
            message: "Selected provider surface does not use managed OAuth transport.".to_string(),
        });
    }
    Ok(surface)
}

#[cfg(feature = "server")]
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

#[cfg(feature = "server")]
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

#[cfg(feature = "server")]
fn google_oauth_client_id(surface: &ProviderSurfaceSpec) -> Option<String> {
    configured_provisioning_env_value(surface, 0)
}

#[cfg(feature = "server")]
fn build_openai_managed_oauth_provider(
    _surface: &ProviderSurfaceSpec,
) -> Option<OAuthProviderConfig> {
    Some(OAuthProviderConfig::openai_codex())
}

#[cfg(feature = "server")]
fn build_google_managed_oauth_provider(
    surface: &ProviderSurfaceSpec,
) -> Option<OAuthProviderConfig> {
    google_oauth_client_id(surface).map(OAuthProviderConfig::google_cloud_vision)
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;
    use oneshim_core::config::{
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
            access_mode: oneshim_core::config::AiAccessMode::ProviderOAuth,
            ocr_provider: oneshim_core::config::OcrProviderType::Remote,
            llm_provider: oneshim_core::config::LlmProviderType::Local,
            ocr_api: Some(managed_google_ocr_endpoint()),
            ..AiProviderConfig::default()
        };

        let providers =
            selected_managed_oauth_provider_ids(&config).expect("provider IDs should resolve");
        assert_eq!(providers, vec!["google".to_string()]);
    }

    #[test]
    fn google_oauth_client_id_uses_surface_provisioning_env_var() {
        std::env::set_var("ONESHIM_GOOGLE_OAUTH_CLIENT_ID", "test-google-client-id");
        let surface = provider_surface_spec(&managed_surface_id_for(AiProviderType::Google))
            .expect("google managed OAuth surface should exist");
        let client_id = google_oauth_client_id(surface);
        std::env::remove_var("ONESHIM_GOOGLE_OAUTH_CLIENT_ID");
        assert_eq!(client_id.as_deref(), Some("test-google-client-id"));
    }
}
