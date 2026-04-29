use std::sync::Arc;

use oneshim_core::config::{AiProviderConfig, OcrProviderType, PiiFilterLevel};
use oneshim_core::error::CoreError;
#[cfg(feature = "server")]
use oneshim_core::ports::credential_source::CredentialSource;
#[cfg(feature = "server")]
use oneshim_core::ports::oauth::OAuthPort;
use oneshim_core::ports::ocr_provider::OcrProvider;
use oneshim_core::ports::secret_store::SecretStoreSet;
use oneshim_core::provider_surface::ProviderSurfaceTransport;
#[cfg(feature = "server")]
use oneshim_network::ai_ocr_client::RemoteOcrProvider;
use oneshim_vision::local_ocr_provider::LocalOcrProvider;
use tracing::{info, warn};

use oneshim_api_contracts::provider_specs::SurfaceCapabilityKind;

#[cfg(feature = "server")]
use super::guarded_ocr::GuardedOcrProvider;
#[cfg(feature = "server")]
use super::helpers::require_endpoint_config;
#[cfg(feature = "server")]
use super::helpers::resolve_remote_with_optional_fallback;
use super::surface::{configured_ocr_surface_transport, unsupported_ocr_surface_runtime};
use super::types::{ExternalOcrPrivacyGuard, OcrProviderResolution, ProviderSource};
#[cfg(feature = "server")]
use crate::oauth_provider_registry::{
    managed_oauth_provider_id_for_endpoint, managed_oauth_transport_url_for_endpoint,
};
use crate::subprocess_provider::{
    cli_id_for_surface_id, probe_for_surface_id, select_cli_surface_for_capability,
    ProbedSubprocessCli, SubprocessCliAuthStatus, SubprocessOcrProvider,
};

/// Create the best available local OCR provider.
///
/// When the `native-vision` feature is enabled, attempts to use platform-native
/// OCR (macOS Vision.framework / Windows WinRT Media.Ocr) first. Falls back to
/// Tesseract-based `LocalOcrProvider` when native OCR is unavailable.
pub(super) fn best_local_ocr_provider() -> Arc<dyn OcrProvider> {
    #[cfg(feature = "native-vision")]
    {
        if let Some(native) = oneshim_vision::native_ocr::create_native_ocr() {
            info!(
                provider = native.provider_name(),
                "Using platform-native OCR provider"
            );
            return native;
        }
    }
    Arc::new(LocalOcrProvider::new())
}

pub(super) fn resolve_cli_subscription_ocr_provider(
    config: &AiProviderConfig,
    pii_filter_level: PiiFilterLevel,
    external_ocr_privacy_guard: Option<ExternalOcrPrivacyGuard>,
    secret_stores: Option<SecretStoreSet>,
    detected: &[ProbedSubprocessCli],
) -> OcrProviderResolution {
    if let Some((surface_id, transport)) = configured_ocr_surface_transport(config) {
        if transport == ProviderSurfaceTransport::SubprocessCli {
            if let Some(surface) =
                select_cli_surface_for_capability(config, detected, SurfaceCapabilityKind::Ocr)
            {
                return Ok((
                    Arc::new(SubprocessOcrProvider::new(surface, config)) as Arc<dyn OcrProvider>,
                    ProviderSource::CliSubscription,
                    None,
                ));
            }

            if let Some(surface) = probe_for_surface_id(detected, &surface_id) {
                let cli_label = cli_id_for_surface_id(&surface.detected.surface_id)
                    .unwrap_or_else(|_| surface.detected.surface_id.clone());
                let reason = match surface.auth_status {
                    SubprocessCliAuthStatus::Authenticated => format!(
                        "Selected OCR provider surface '{surface_id}' uses installed {cli_label}, but the OCR subprocess runtime could not be selected."
                    ),
                    SubprocessCliAuthStatus::Unauthenticated => format!(
                        "Selected OCR provider surface '{surface_id}' uses installed {cli_label}, but the CLI is not authenticated. Sign in through the provider-owned CLI first."
                    ),
                    SubprocessCliAuthStatus::Unknown => format!(
                        "Selected OCR provider surface '{surface_id}' uses installed {cli_label}, but authentication status could not be verified."
                    ),
                };
                if config.fallback_to_local {
                    warn!(
                        fallback_reason = %reason,
                        "CLI OCR runtime unavailable, falling back to local OCR"
                    );
                    return Ok((
                        best_local_ocr_provider(),
                        ProviderSource::LocalFallback,
                        Some(reason),
                    ));
                }
                return Err(CoreError::Config {
                    code: oneshim_core::error_codes::ConfigCode::Invalid,
                    message: reason,
                });
            }

            return unsupported_ocr_surface_runtime(config, &surface_id, transport);
        }
    }

    match config.ocr_provider {
        OcrProviderType::Local => Ok((best_local_ocr_provider(), ProviderSource::Local, None)),
        OcrProviderType::Remote => resolve_ocr_provider(
            config,
            pii_filter_level,
            external_ocr_privacy_guard,
            secret_stores,
        ),
    }
}

#[allow(unused_variables)]
pub(super) fn resolve_ocr_provider(
    config: &AiProviderConfig,
    pii_filter_level: PiiFilterLevel,
    external_ocr_privacy_guard: Option<ExternalOcrPrivacyGuard>,
    secret_stores: Option<SecretStoreSet>,
) -> OcrProviderResolution {
    match config.ocr_provider {
        OcrProviderType::Local => Ok((best_local_ocr_provider(), ProviderSource::Local, None)),
        OcrProviderType::Remote => {
            if let Some((surface_id, transport)) = configured_ocr_surface_transport(config) {
                if transport != ProviderSurfaceTransport::DirectApi {
                    return unsupported_ocr_surface_runtime(config, &surface_id, transport);
                }
            }
            #[cfg(feature = "server")]
            {
                resolve_remote_with_optional_fallback(
                    "ocr",
                    config.fallback_to_local,
                    || {
                        let endpoint = require_endpoint_config(config.ocr_api.as_ref(), "ocr_api")?;
                        let secret_store = secret_stores
                            .as_ref()
                            .and_then(|stores| stores.for_binding(endpoint.credential.as_ref()));
                        let profile_id = config.active_secret_profile_id_or("ocr");
                        let credential = CredentialSource::from_api_key_endpoint_for_profile(
                            endpoint,
                            Some(profile_id),
                            secret_store,
                        )?;
                        let privacy_guard =
                            external_ocr_privacy_guard.clone().ok_or_else(|| {
                                CoreError::Config {
                                    code: oneshim_core::error_codes::ConfigCode::Invalid,
                                    message: "Remote OCR provider requires a runtime privacy guard"
                                        .to_string(),
                                }
                            })?;
                        let remote = Arc::new(RemoteOcrProvider::new_with_credential(
                            endpoint,
                            credential,
                            // D7: per-adapter registry; iter-011 will consolidate
                            // to a workspace-wide shared registry.
                            oneshim_network::CircuitBreakerRegistry::new(),
                        )?) as Arc<dyn OcrProvider>;
                        Ok(Arc::new(GuardedOcrProvider::new(
                            remote,
                            privacy_guard,
                            config.allow_unredacted_external_ocr,
                            config.ocr_validation.clone(),
                        )) as Arc<dyn OcrProvider>)
                    },
                    || best_local_ocr_provider(),
                )
            }
            #[cfg(not(feature = "server"))]
            {
                // Iter-109: feature-gate disabled = service unavailable.
                Err(CoreError::ServiceUnavailable {
                    code: oneshim_core::error_codes::ServiceCode::Unavailable,
                    message: "Remote OCR provider requires the 'server' feature".to_string(),
                })
            }
        }
    }
}

#[cfg(feature = "server")]
pub(super) fn resolve_ocr_provider_oauth(
    config: &AiProviderConfig,
    _pii_filter_level: PiiFilterLevel,
    external_ocr_privacy_guard: Option<ExternalOcrPrivacyGuard>,
    oauth_port: Arc<dyn OAuthPort>,
) -> OcrProviderResolution {
    let endpoint = require_endpoint_config(config.ocr_api.as_ref(), "ocr_api")?;
    let provider_id = managed_oauth_provider_id_for_endpoint(
        endpoint,
        oneshim_api_contracts::provider_specs::ProviderTransportKind::Ocr,
    )?;
    let api_base_url = managed_oauth_transport_url_for_endpoint(
        endpoint,
        oneshim_api_contracts::provider_specs::ProviderTransportKind::Ocr,
    )?;
    let credential = CredentialSource::ManagedOAuth {
        provider_id,
        oauth_port,
        api_base_url,
    };
    let privacy_guard = external_ocr_privacy_guard.ok_or_else(|| CoreError::Config {
        code: oneshim_core::error_codes::ConfigCode::Invalid,
        message: "Remote OCR provider requires a runtime privacy guard".to_string(),
    })?;
    let remote = Arc::new(RemoteOcrProvider::new_with_credential(
        endpoint,
        credential,
        // D7: per-adapter registry; iter-011 will consolidate.
        oneshim_network::CircuitBreakerRegistry::new(),
    )?) as Arc<dyn OcrProvider>;
    Ok((
        Arc::new(GuardedOcrProvider::new(
            remote,
            privacy_guard,
            config.allow_unredacted_external_ocr,
            config.ocr_validation.clone(),
        )) as Arc<dyn OcrProvider>,
        ProviderSource::OAuth,
        None,
    ))
}
