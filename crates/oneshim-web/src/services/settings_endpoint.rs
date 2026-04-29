use oneshim_api_contracts::provider_specs::{
    default_surface_id_for_access_mode as default_surface_id_from_catalog,
    known_model_capability_warning, resolved_model_capability_status,
    resolved_ocr_requires_structured_output_model,
    resolved_surface_requires_explicit_model_selection, resolved_surface_supports_model_selection,
    surface_supports_capability as surface_supports_capability_from_catalog,
    validate_known_model_capability, SurfaceCapabilityKind, SurfaceModelCapabilityKind,
};
use oneshim_api_contracts::settings::ExternalApiSettings;
use oneshim_core::config::{
    AiAccessMode, AiProviderType, CredentialAuthMode, CredentialBackendKind, CredentialBinding,
    ExternalApiEndpoint,
};
use oneshim_core::provider_surface::{
    canonical_provider_surface_id, provider_surface_spec, provider_vendor_id_or_default,
    ProviderSurfaceTransport,
};
use tracing::warn;

use crate::error::ApiError;
use crate::services::settings_assembler::is_masked_key;
use crate::services::settings_validation::{
    is_managed_auth_mode, parse_ai_provider_type, parse_credential_auth_mode,
    parse_credential_backend_kind,
};

#[derive(Clone, Copy)]
pub(crate) enum ApiEndpointKind {
    Ocr,
    Llm,
}

impl ApiEndpointKind {
    pub(crate) fn profile_id(self) -> &'static str {
        match self {
            Self::Ocr => "ocr",
            Self::Llm => "llm",
        }
    }
}

pub(crate) fn normalize_ai_access_mode_for_settings(value: AiAccessMode) -> AiAccessMode {
    value.normalized_for_ai_surfaces()
}

pub(crate) fn provider_type_id(value: AiProviderType) -> &'static str {
    provider_vendor_id_or_default(value)
}

pub(crate) fn api_settings_to_endpoint(
    settings: &ExternalApiSettings,
    existing_endpoint: Option<&ExternalApiEndpoint>,
    access_mode: AiAccessMode,
    endpoint_kind: ApiEndpointKind,
) -> Result<ExternalApiEndpoint, ApiError> {
    let provider_type = parse_ai_provider_type(&settings.provider_type)?;
    let surface_id = resolve_endpoint_surface_id(
        settings,
        existing_endpoint,
        provider_type,
        access_mode,
        endpoint_kind,
    )?;
    let existing_key = existing_endpoint
        .map(|endpoint| endpoint.api_key.as_str())
        .unwrap_or("");
    let api_key = if is_managed_auth_mode(&settings.auth_mode)
        || surface_id.as_deref().is_some_and(surface_uses_no_auth)
    {
        String::new()
    } else if is_masked_key(&settings.api_key_masked) || settings.api_key_masked.is_empty() {
        existing_key.to_string()
    } else {
        settings.api_key_masked.clone()
    };

    validate_surface_model_selection(
        provider_type,
        surface_id.as_deref(),
        endpoint_kind,
        settings.model.as_deref(),
    )?;

    let credential =
        updated_credential_binding(settings, existing_endpoint, surface_id.as_deref())?;
    validate_endpoint_model_compatibility(
        provider_type,
        surface_id.as_deref(),
        endpoint_kind,
        settings.model.as_deref(),
    )?;

    Ok(ExternalApiEndpoint {
        endpoint: settings.endpoint.clone(),
        api_key,
        model: settings.model.clone(),
        timeout_secs: settings.timeout_secs,
        provider_type,
        surface_id,
        credential,
    })
}

fn validate_surface_model_selection(
    provider_type: AiProviderType,
    surface_id: Option<&str>,
    endpoint_kind: ApiEndpointKind,
    model: Option<&str>,
) -> Result<(), ApiError> {
    let capability = match endpoint_kind {
        ApiEndpointKind::Ocr => SurfaceCapabilityKind::Ocr,
        ApiEndpointKind::Llm => SurfaceCapabilityKind::Llm,
    };
    let normalized_model = model.map(str::trim).filter(|value| !value.is_empty());

    let supports_model =
        resolved_surface_supports_model_selection(provider_type, surface_id, capability)
            .map_err(ApiError::BadRequest)?;
    if normalized_model.is_some() && !supports_model {
        let target = match endpoint_kind {
            ApiEndpointKind::Ocr => "OCR",
            ApiEndpointKind::Llm => "LLM",
        };
        let surface_label = surface_id.unwrap_or("default surface");
        return Err(ApiError::BadRequest(format!(
            "Provider surface '{surface_label}' does not support configurable {target} model selection."
        )));
    }

    if normalized_model.is_none()
        && resolved_surface_requires_explicit_model_selection(provider_type, surface_id, capability)
            .map_err(ApiError::BadRequest)?
    {
        let target = match endpoint_kind {
            ApiEndpointKind::Ocr => "OCR",
            ApiEndpointKind::Llm => "LLM",
        };
        let surface_label = surface_id.unwrap_or("default surface");
        return Err(ApiError::BadRequest(format!(
            "Provider surface '{surface_label}' requires an explicit {target} model selection."
        )));
    }

    if let Some(model) = normalized_model {
        validate_known_model_capability(provider_type, surface_id, capability, model)
            .map_err(ApiError::BadRequest)?;
    }

    Ok(())
}

pub(crate) fn default_surface_id_for_endpoint(
    provider_type: AiProviderType,
    access_mode: AiAccessMode,
    endpoint_kind: ApiEndpointKind,
) -> Option<&'static str> {
    let direct_default = || {
        default_surface_id_from_catalog(
            provider_type,
            AiAccessMode::ProviderApiKey,
            match endpoint_kind {
                ApiEndpointKind::Ocr => SurfaceCapabilityKind::Ocr,
                ApiEndpointKind::Llm => SurfaceCapabilityKind::Llm,
            },
        )
        .ok()
        .flatten()
    };

    default_surface_id_from_catalog(
        provider_type,
        access_mode,
        match endpoint_kind {
            ApiEndpointKind::Ocr => SurfaceCapabilityKind::Ocr,
            ApiEndpointKind::Llm => SurfaceCapabilityKind::Llm,
        },
    )
    .ok()
    .flatten()
    .or_else(|| {
        (matches!(endpoint_kind, ApiEndpointKind::Ocr)
            || matches!(access_mode, AiAccessMode::ProviderOAuth))
        .then(direct_default)
        .flatten()
    })
}

fn resolve_endpoint_surface_id(
    settings: &ExternalApiSettings,
    existing_endpoint: Option<&ExternalApiEndpoint>,
    provider_type: AiProviderType,
    access_mode: AiAccessMode,
    endpoint_kind: ApiEndpointKind,
) -> Result<Option<String>, ApiError> {
    let auth_mode = parse_credential_auth_mode(&settings.auth_mode)?;
    let requested = settings
        .surface_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| existing_endpoint.and_then(|endpoint| endpoint.surface_id.clone()));

    let default_surface =
        default_surface_id_for_endpoint(provider_type, access_mode, endpoint_kind)
            .map(str::to_string);
    let resolved = requested.or(default_surface);

    let Some(surface_id) = resolved else {
        return Ok(None);
    };

    let Some(spec) = provider_surface_spec(&surface_id) else {
        return Err(ApiError::BadRequest(format!(
            "Unsupported ai_provider.api.surface_id value: {surface_id}"
        )));
    };

    if spec.provider_type != provider_type {
        return Err(ApiError::BadRequest(format!(
            "Provider surface '{surface_id}' does not match provider_type '{}'.",
            provider_type_id(provider_type)
        )));
    }

    let expected_transport = transport_for_auth_mode(auth_mode);

    if spec.transport != expected_transport {
        return Err(ApiError::BadRequest(format!(
            "Provider surface '{surface_id}' is incompatible with access_mode '{:?}' for this endpoint.",
            access_mode
        )));
    }

    if !access_mode_allows_surface_transport(access_mode, endpoint_kind, spec.transport) {
        let target = match endpoint_kind {
            ApiEndpointKind::Ocr => "OCR",
            ApiEndpointKind::Llm => "LLM",
        };
        return Err(ApiError::BadRequest(format!(
            "Provider surface '{surface_id}' is incompatible with access_mode '{:?}' for the selected {target} endpoint.",
            access_mode
        )));
    }

    let required_capability = match endpoint_kind {
        ApiEndpointKind::Ocr => SurfaceCapabilityKind::Ocr,
        ApiEndpointKind::Llm => SurfaceCapabilityKind::Llm,
    };
    if !surface_supports_capability_from_catalog(&surface_id, required_capability)
        .map_err(ApiError::BadRequest)?
    {
        return Err(ApiError::BadRequest(format!(
            "Provider surface '{surface_id}' does not support the selected endpoint capability."
        )));
    }

    Ok(canonical_provider_surface_id(&surface_id).map(str::to_string))
}

pub(crate) fn derive_credential_auth_mode(
    surface_id: Option<&str>,
    access_mode: AiAccessMode,
    endpoint_kind: ApiEndpointKind,
) -> CredentialAuthMode {
    if let Some(surface_id) = surface_id {
        if let Some(spec) = provider_surface_spec(surface_id) {
            return match spec.transport {
                ProviderSurfaceTransport::ManagedOAuth => CredentialAuthMode::ManagedOAuth,
                ProviderSurfaceTransport::SubprocessCli => CredentialAuthMode::CliBridge,
                ProviderSurfaceTransport::DirectApi => CredentialAuthMode::ApiKey,
            };
        }
    }

    match (access_mode, endpoint_kind) {
        (AiAccessMode::ProviderOAuth, ApiEndpointKind::Llm) => CredentialAuthMode::ManagedOAuth,
        (AiAccessMode::ProviderSubscriptionCli, ApiEndpointKind::Llm) => {
            CredentialAuthMode::CliBridge
        }
        _ => CredentialAuthMode::ApiKey,
    }
}

fn validate_endpoint_model_compatibility(
    provider_type: AiProviderType,
    surface_id: Option<&str>,
    endpoint_kind: ApiEndpointKind,
    model: Option<&str>,
) -> Result<(), ApiError> {
    let Some(model) = model.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(());
    };

    let capability = match endpoint_kind {
        ApiEndpointKind::Ocr => SurfaceCapabilityKind::Ocr,
        ApiEndpointKind::Llm => SurfaceCapabilityKind::Llm,
    };
    if let Some(message) =
        known_model_capability_warning(provider_type, surface_id, capability, model)
            .map_err(ApiError::BadRequest)?
    {
        warn!(
            provider = ?provider_type,
            surface_id = ?surface_id,
            model = %model,
            "{message}"
        );
    }
    validate_known_model_capability(provider_type, surface_id, capability, model)
        .map_err(ApiError::BadRequest)?;

    if matches!(endpoint_kind, ApiEndpointKind::Ocr)
        && resolved_ocr_requires_structured_output_model(provider_type, surface_id)
            .map_err(ApiError::BadRequest)?
    {
        match resolved_model_capability_status(
            provider_type,
            surface_id,
            SurfaceModelCapabilityKind::StructuredOutput,
            model,
        )
        .map_err(ApiError::BadRequest)?
        {
            oneshim_api_contracts::ai_providers::ProviderModelSupportStatus::Unsupported => {
                return Err(ApiError::BadRequest(format!(
                    "Model '{model}' is not marked as supporting structured JSON output required by the selected OCR surface."
                )));
            }
            oneshim_api_contracts::ai_providers::ProviderModelSupportStatus::Unknown => {
                warn!(
                    provider = ?provider_type,
                    surface_id = ?surface_id,
                    model = %model,
                    "OCR surface requires structured output, but the selected model's support is unknown."
                );
            }
            oneshim_api_contracts::ai_providers::ProviderModelSupportStatus::Supported => {}
        }
    }

    Ok(())
}

fn transport_for_auth_mode(auth_mode: CredentialAuthMode) -> ProviderSurfaceTransport {
    match auth_mode {
        CredentialAuthMode::ManagedOAuth => ProviderSurfaceTransport::ManagedOAuth,
        CredentialAuthMode::CliBridge => ProviderSurfaceTransport::SubprocessCli,
        CredentialAuthMode::ApiKey => ProviderSurfaceTransport::DirectApi,
    }
}

fn access_mode_allows_surface_transport(
    access_mode: AiAccessMode,
    endpoint_kind: ApiEndpointKind,
    transport: ProviderSurfaceTransport,
) -> bool {
    let access_mode = normalize_ai_access_mode_for_settings(access_mode);
    match endpoint_kind {
        ApiEndpointKind::Llm => match access_mode {
            AiAccessMode::ProviderOAuth => matches!(
                transport,
                ProviderSurfaceTransport::DirectApi | ProviderSurfaceTransport::ManagedOAuth
            ),
            AiAccessMode::ProviderSubscriptionCli => {
                transport == ProviderSurfaceTransport::SubprocessCli
            }
            AiAccessMode::ProviderApiKey | AiAccessMode::LocalModel => {
                transport == ProviderSurfaceTransport::DirectApi
            }
        },
        ApiEndpointKind::Ocr => match access_mode {
            AiAccessMode::ProviderOAuth => matches!(
                transport,
                ProviderSurfaceTransport::DirectApi | ProviderSurfaceTransport::ManagedOAuth
            ),
            AiAccessMode::ProviderSubscriptionCli => matches!(
                transport,
                ProviderSurfaceTransport::DirectApi | ProviderSurfaceTransport::SubprocessCli
            ),
            AiAccessMode::ProviderApiKey | AiAccessMode::LocalModel => {
                transport == ProviderSurfaceTransport::DirectApi
            }
        },
    }
}

pub(crate) fn derive_credential_backend_kind(
    auth_mode: CredentialAuthMode,
    default_backend_kind: CredentialBackendKind,
) -> CredentialBackendKind {
    match auth_mode {
        CredentialAuthMode::ManagedOAuth => CredentialBackendKind::OsSecretStore,
        CredentialAuthMode::CliBridge => CredentialBackendKind::BridgeManaged,
        CredentialAuthMode::ApiKey => default_backend_kind,
    }
}

fn updated_credential_binding(
    settings: &ExternalApiSettings,
    existing_endpoint: Option<&ExternalApiEndpoint>,
    resolved_surface_id: Option<&str>,
) -> Result<Option<CredentialBinding>, ApiError> {
    if resolved_surface_id.is_some_and(surface_uses_no_auth) {
        return Ok(None);
    }

    let auth_mode = parse_credential_auth_mode(&settings.auth_mode)?;
    let backend_kind = parse_credential_backend_kind(&settings.backend_kind)?;
    validate_projection_binding(auth_mode, backend_kind, settings.projection_enabled)?;

    if matches!(
        auth_mode,
        CredentialAuthMode::ManagedOAuth | CredentialAuthMode::CliBridge
    ) {
        return Ok(Some(CredentialBinding {
            auth_mode,
            backend_kind,
            secret_ref: None,
            projection_enabled: false,
        }));
    }

    if let Some(mut binding) = existing_endpoint.and_then(|endpoint| endpoint.credential.clone()) {
        if binding.auth_mode != auth_mode {
            binding = CredentialBinding {
                auth_mode,
                backend_kind,
                secret_ref: None,
                projection_enabled: settings.projection_enabled,
            };
            return Ok(Some(binding));
        }
        if binding.backend_kind != backend_kind {
            return Err(ApiError::BadRequest(
                "Changing provider credential auth mode or backend for an existing endpoint is not supported from Settings. Use the dedicated migration or reconnect flow instead.".to_string(),
            ));
        }
        binding.projection_enabled = settings.projection_enabled;
        return Ok(Some(binding));
    }

    if settings.projection_enabled || backend_kind != CredentialBackendKind::Unavailable {
        return Ok(Some(CredentialBinding {
            auth_mode,
            backend_kind,
            secret_ref: None,
            projection_enabled: settings.projection_enabled,
        }));
    }

    Ok(None)
}

fn validate_projection_binding(
    auth_mode: CredentialAuthMode,
    backend_kind: CredentialBackendKind,
    projection_enabled: bool,
) -> Result<(), ApiError> {
    if !projection_enabled {
        return Ok(());
    }

    if auth_mode != CredentialAuthMode::ApiKey {
        return Err(ApiError::BadRequest(
            "Projection can only be enabled for API-key credentials.".to_string(),
        ));
    }

    if !matches!(
        backend_kind,
        CredentialBackendKind::OsSecretStore | CredentialBackendKind::FileSecretStore
    ) {
        return Err(ApiError::BadRequest(format!(
            "Projection is not supported for backend kind {:?}.",
            backend_kind
        )));
    }

    Ok(())
}

pub(crate) fn surface_uses_no_auth(surface_id: &str) -> bool {
    oneshim_api_contracts::provider_specs::provider_surface_spec(surface_id)
        .map(|surface| {
            surface
                .llm_transport
                .as_ref()
                .is_some_and(|transport| transport.auth_scheme.eq_ignore_ascii_case("none"))
                || surface
                    .ocr_transport
                    .as_ref()
                    .is_some_and(|transport| transport.auth_scheme.eq_ignore_ascii_case("none"))
        })
        .unwrap_or(false)
}

pub(crate) fn can_edit_secret(
    auth_mode: CredentialAuthMode,
    backend_kind: CredentialBackendKind,
) -> bool {
    matches!(auth_mode, CredentialAuthMode::ApiKey)
        && matches!(
            backend_kind,
            CredentialBackendKind::OsSecretStore | CredentialBackendKind::FileSecretStore
        )
}

pub(crate) fn credential_auth_mode_to_wire(value: CredentialAuthMode) -> &'static str {
    match value {
        CredentialAuthMode::ApiKey => "api_key",
        CredentialAuthMode::ManagedOAuth => "managed_oauth",
        CredentialAuthMode::CliBridge => "cli_bridge",
    }
}

pub(crate) fn credential_backend_kind_to_wire(value: CredentialBackendKind) -> &'static str {
    match value {
        CredentialBackendKind::OsSecretStore => "os_secret_store",
        CredentialBackendKind::FileSecretStore => "file_secret_store",
        CredentialBackendKind::Env => "env",
        CredentialBackendKind::BridgeManaged => "bridge_managed",
        CredentialBackendKind::Unavailable => "unavailable",
    }
}
