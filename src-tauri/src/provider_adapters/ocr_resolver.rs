use std::sync::Arc;

use maekon_core::config::{AiProviderConfig, OcrProviderType, PiiFilterLevel};
use maekon_core::error::CoreError;
#[cfg(feature = "analysis")]
use maekon_core::ports::credential_source::CredentialSource;
#[cfg(feature = "analysis")]
use maekon_core::ports::oauth::OAuthPort;
use maekon_core::ports::ocr_provider::OcrProvider;
use maekon_core::ports::secret_store::SecretStoreSet;
use maekon_core::provider_surface::ProviderSurfaceTransport;
#[cfg(feature = "analysis")]
use maekon_network::ai_ocr_client::RemoteOcrProvider;
use maekon_vision::local_ocr_provider::LocalOcrProvider;
use tracing::{info, warn};

use maekon_api_contracts::provider_specs::SurfaceCapabilityKind;

use super::guarded_ocr::GuardedOcrProvider;
#[cfg(feature = "analysis")]
use super::helpers::require_endpoint_config;
#[cfg(feature = "analysis")]
use super::helpers::resolve_remote_with_optional_fallback;
use super::surface::{configured_ocr_surface_transport, unsupported_ocr_surface_runtime};
use super::types::{ExternalOcrPrivacyGuard, OcrProviderResolution, ProviderSource};
#[cfg(feature = "analysis")]
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
        if let Some(native) = maekon_vision::native_ocr::create_native_ocr() {
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
    // D7 (#4812 / E20-20): shared workspace-wide registry from the composition
    // root, forwarded to the remote-OCR fallback path below.
    breaker_registry: Arc<crate::breaker_registry::CircuitBreakerRegistry>,
) -> OcrProviderResolution {
    if let Some((surface_id, transport)) = configured_ocr_surface_transport(config) {
        if transport == ProviderSurfaceTransport::SubprocessCli {
            if let Some(surface) =
                select_cli_surface_for_capability(config, detected, SurfaceCapabilityKind::Ocr)
            {
                let privacy_guard =
                    external_ocr_privacy_guard
                        .clone()
                        .ok_or_else(|| CoreError::Config {
                            code: maekon_core::error_codes::ConfigCode::Missing,
                            message: "Provider-owned CLI OCR requires a runtime privacy guard."
                                .to_string(),
                        })?;
                let provider =
                    Arc::new(SubprocessOcrProvider::new(surface, config)) as Arc<dyn OcrProvider>;
                return Ok((
                    Arc::new(GuardedOcrProvider::new(
                        provider,
                        privacy_guard,
                        config.allow_unredacted_external_ocr,
                        config.ocr_validation.clone(),
                    )) as Arc<dyn OcrProvider>,
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
                    SubprocessCliAuthStatus::Unsupported => format!(
                        "Selected OCR provider surface '{surface_id}' uses installed {cli_label}, but the active plan or provider mode does not support headless CLI use."
                    ),
                    SubprocessCliAuthStatus::StaleSession => format!(
                        "Selected OCR provider surface '{surface_id}' uses installed {cli_label}, but the session appears expired or stale. Refresh provider-owned CLI sign-in first."
                    ),
                    SubprocessCliAuthStatus::InteractiveRequired => format!(
                        "Selected OCR provider surface '{surface_id}' uses installed {cli_label}, but an interactive sign-in step is required before headless use."
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
                    code: maekon_core::error_codes::ConfigCode::Invalid,
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
            breaker_registry,
        ),
    }
}

#[allow(unused_variables)]
pub(super) fn resolve_ocr_provider(
    config: &AiProviderConfig,
    pii_filter_level: PiiFilterLevel,
    external_ocr_privacy_guard: Option<ExternalOcrPrivacyGuard>,
    secret_stores: Option<SecretStoreSet>,
    // D7 (#4812 / E20-20): shared workspace-wide registry from the composition root.
    breaker_registry: Arc<crate::breaker_registry::CircuitBreakerRegistry>,
) -> OcrProviderResolution {
    match config.ocr_provider {
        OcrProviderType::Local => Ok((best_local_ocr_provider(), ProviderSource::Local, None)),
        OcrProviderType::Remote => {
            if let Some((surface_id, transport)) = configured_ocr_surface_transport(config) {
                if transport != ProviderSurfaceTransport::DirectApi {
                    return unsupported_ocr_surface_runtime(config, &surface_id, transport);
                }
            }
            #[cfg(feature = "analysis")]
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
                                    code: maekon_core::error_codes::ConfigCode::Invalid,
                                    message: "Remote OCR provider requires a runtime privacy guard"
                                        .to_string(),
                                }
                            })?;
                        let remote = Arc::new(RemoteOcrProvider::new_with_credential(
                            endpoint,
                            credential,
                            // D7 (#4812 / E20-20): shared workspace-wide registry
                            // from the composition root (iter-011 done).
                            breaker_registry.clone(),
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
            #[cfg(not(feature = "analysis"))]
            {
                // Iter-109: feature-gate disabled = service unavailable.
                Err(CoreError::ServiceUnavailable {
                    code: maekon_core::error_codes::ServiceCode::Unavailable,
                    message: "Remote OCR provider requires the 'analysis' feature".to_string(),
                })
            }
        }
    }
}

#[cfg(feature = "analysis")]
pub(super) fn resolve_ocr_provider_oauth(
    config: &AiProviderConfig,
    _pii_filter_level: PiiFilterLevel,
    external_ocr_privacy_guard: Option<ExternalOcrPrivacyGuard>,
    oauth_port: Arc<dyn OAuthPort>,
    // D7 (#4812 / E20-20): shared workspace-wide registry from the composition root.
    breaker_registry: Arc<crate::breaker_registry::CircuitBreakerRegistry>,
) -> OcrProviderResolution {
    let endpoint = require_endpoint_config(config.ocr_api.as_ref(), "ocr_api")?;
    let provider_id = managed_oauth_provider_id_for_endpoint(
        endpoint,
        maekon_api_contracts::provider_specs::ProviderTransportKind::Ocr,
    )?;
    let api_base_url = managed_oauth_transport_url_for_endpoint(
        endpoint,
        maekon_api_contracts::provider_specs::ProviderTransportKind::Ocr,
    )?;
    let credential = CredentialSource::ManagedOAuth {
        provider_id,
        oauth_port,
        api_base_url,
    };
    let privacy_guard = external_ocr_privacy_guard.ok_or_else(|| CoreError::Config {
        code: maekon_core::error_codes::ConfigCode::Invalid,
        message: "Remote OCR provider requires a runtime privacy guard".to_string(),
    })?;
    let remote = Arc::new(RemoteOcrProvider::new_with_credential(
        endpoint,
        credential,
        // D7 (#4812 / E20-20): shared workspace-wide registry (iter-011 done).
        breaker_registry,
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
