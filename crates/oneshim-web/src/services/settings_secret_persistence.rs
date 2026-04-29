use oneshim_core::config::{
    AiAccessMode, AiProviderProfileConfig, AppConfig, CredentialAuthMode, CredentialBackendKind,
    CredentialBinding, ExternalApiEndpoint, SecretRef,
};
use oneshim_core::ports::secret_store::{provider_api_key_secret_ref, SecretStore, SecretStoreSet};
use std::sync::Arc;

use crate::error::ApiError;
use crate::services::settings_endpoint::{
    derive_credential_auth_mode, normalize_ai_access_mode_for_settings, provider_type_id,
    surface_uses_no_auth, ApiEndpointKind,
};

pub(crate) async fn persist_api_key_bindings(
    config: &mut AppConfig,
    secret_store: Option<Arc<dyn SecretStore>>,
    secret_stores: Option<&SecretStoreSet>,
    default_backend_kind: CredentialBackendKind,
) -> Result<(), ApiError> {
    let ocr_profile_id = config
        .ai_provider
        .active_secret_profile_id_or(ApiEndpointKind::Ocr.profile_id())
        .to_string();
    let llm_profile_id = config
        .ai_provider
        .active_secret_profile_id_or(ApiEndpointKind::Llm.profile_id())
        .to_string();
    let mut active_profile = AiProviderProfileConfig {
        access_mode: config.ai_provider.access_mode,
        ocr_provider: config.ai_provider.ocr_provider,
        llm_provider: config.ai_provider.llm_provider,
        ocr_api: config.ai_provider.ocr_api.clone(),
        llm_api: config.ai_provider.llm_api.clone(),
        external_data_policy: config.ai_provider.external_data_policy,
        allow_unredacted_external_ocr: config.ai_provider.allow_unredacted_external_ocr,
        ocr_validation: config.ai_provider.ocr_validation.clone(),
        scene_action_override: config.ai_provider.scene_action_override.clone(),
        scene_intelligence: config.ai_provider.scene_intelligence.clone(),
        fallback_to_local: config.ai_provider.fallback_to_local,
    };
    persist_ai_provider_profile_bindings(
        &mut active_profile,
        ocr_profile_id.as_str(),
        llm_profile_id.as_str(),
        secret_store.clone(),
        secret_stores,
        default_backend_kind,
    )
    .await?;
    config.ai_provider.ocr_api = active_profile.ocr_api;
    config.ai_provider.llm_api = active_profile.llm_api;

    for saved_profile in config.ai_provider.saved_profiles.iter_mut() {
        persist_saved_profile_bindings(
            &mut saved_profile.ai_provider,
            saved_profile.profile_id.as_str(),
            secret_store.clone(),
            secret_stores,
            default_backend_kind,
        )
        .await?;
    }

    Ok(())
}

async fn persist_saved_profile_bindings(
    profile: &mut AiProviderProfileConfig,
    profile_id: &str,
    secret_store: Option<Arc<dyn SecretStore>>,
    secret_stores: Option<&SecretStoreSet>,
    default_backend_kind: CredentialBackendKind,
) -> Result<(), ApiError> {
    let access_mode = normalize_ai_access_mode_for_settings(profile.access_mode);

    if let Some(endpoint) = profile.ocr_api.as_mut() {
        persist_api_key_binding(
            endpoint,
            access_mode,
            ApiEndpointKind::Ocr,
            profile_id,
            secret_store.clone(),
            secret_stores,
            default_backend_kind,
        )
        .await?;
    }

    if let Some(endpoint) = profile.llm_api.as_mut() {
        persist_api_key_binding(
            endpoint,
            access_mode,
            ApiEndpointKind::Llm,
            profile_id,
            secret_store,
            secret_stores,
            default_backend_kind,
        )
        .await?;
    }

    Ok(())
}

async fn persist_ai_provider_profile_bindings(
    profile: &mut AiProviderProfileConfig,
    ocr_profile_id: &str,
    llm_profile_id: &str,
    secret_store: Option<Arc<dyn SecretStore>>,
    secret_stores: Option<&SecretStoreSet>,
    default_backend_kind: CredentialBackendKind,
) -> Result<(), ApiError> {
    let access_mode = normalize_ai_access_mode_for_settings(profile.access_mode);

    if let Some(endpoint) = profile.ocr_api.as_mut() {
        persist_api_key_binding(
            endpoint,
            access_mode,
            ApiEndpointKind::Ocr,
            ocr_profile_id,
            secret_store.clone(),
            secret_stores,
            default_backend_kind,
        )
        .await?;
    }

    if let Some(endpoint) = profile.llm_api.as_mut() {
        persist_api_key_binding(
            endpoint,
            access_mode,
            ApiEndpointKind::Llm,
            llm_profile_id,
            secret_store,
            secret_stores,
            default_backend_kind,
        )
        .await?;
    }

    Ok(())
}

async fn persist_api_key_binding(
    endpoint: &mut ExternalApiEndpoint,
    access_mode: AiAccessMode,
    endpoint_kind: ApiEndpointKind,
    profile_id: &str,
    secret_store: Option<Arc<dyn SecretStore>>,
    secret_stores: Option<&SecretStoreSet>,
    default_backend_kind: CredentialBackendKind,
) -> Result<(), ApiError> {
    if endpoint
        .surface_id
        .as_deref()
        .is_some_and(surface_uses_no_auth)
    {
        endpoint.api_key.clear();
        endpoint.credential = None;
        return Ok(());
    }

    let auth_mode = endpoint
        .credential
        .as_ref()
        .map(|binding| binding.auth_mode)
        .unwrap_or_else(|| {
            derive_credential_auth_mode(endpoint.surface_id.as_deref(), access_mode, endpoint_kind)
        });

    if auth_mode != CredentialAuthMode::ApiKey {
        return Ok(());
    }

    let api_key = endpoint.api_key.trim();
    if api_key.is_empty() {
        return Ok(());
    }

    let backend_kind = endpoint
        .credential
        .as_ref()
        .map(|binding| binding.backend_kind)
        .unwrap_or(default_backend_kind);

    match backend_kind {
        CredentialBackendKind::Env => {
            return Err(ApiError::BadRequest(
                "Environment-backed provider credentials are read-only; update the environment source instead.".to_string(),
            ));
        }
        CredentialBackendKind::Unavailable => {
            return Err(ApiError::BadRequest(
                "No writable provider secret backend is available; configure a secret backend before saving API keys.".to_string(),
            ));
        }
        CredentialBackendKind::BridgeManaged => {
            return Err(ApiError::BadRequest(
                "Bridge-managed credentials cannot be edited from Settings.".to_string(),
            ));
        }
        CredentialBackendKind::OsSecretStore | CredentialBackendKind::FileSecretStore => {}
    }

    let secret_store = secret_stores
        .and_then(|stores| stores.for_binding(endpoint.credential.as_ref()))
        .or(secret_store);

    let Some(secret_store) = secret_store else {
        return Err(ApiError::Internal(
            "Writable provider secret backend was selected, but no secret store was initialized."
                .to_string(),
        ));
    };

    let (namespace, key) =
        provider_api_key_secret_ref(provider_type_id(endpoint.provider_type), profile_id)
            .map_err(ApiError::from)?;

    secret_store
        .store(&namespace, key, api_key)
        .await
        .map_err(|e| {
            ApiError::Internal(format!("Failed to persist API key to secret store: {e}"))
        })?;

    endpoint.credential = Some(CredentialBinding {
        auth_mode: CredentialAuthMode::ApiKey,
        backend_kind,
        secret_ref: Some(SecretRef {
            namespace,
            key: key.to_string(),
        }),
        projection_enabled: endpoint
            .credential
            .as_ref()
            .map(|binding| binding.projection_enabled)
            .unwrap_or(false),
    });
    endpoint.api_key.clear();

    Ok(())
}
