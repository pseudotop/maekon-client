#[cfg(feature = "server")]
use oneshim_core::config::LlmProviderType;
use oneshim_core::config::{AiProviderConfig, PiiFilterLevel};
use oneshim_core::error::CoreError;
use oneshim_core::ports::secret_store::SecretStoreSet;
use oneshim_core::provider_surface::{provider_surface_spec, ProviderSurfaceTransport};
use tracing::warn;

use super::ocr_resolver::{best_local_ocr_provider, resolve_ocr_provider};
use super::types::{
    AiProviderAdapters, ExternalOcrPrivacyGuard, OcrProviderResolution, ProviderSource,
};
use crate::provider_adapters::llm_resolver::resolve_llm_provider;

pub(super) fn resolve_direct_surface_adapters(
    config: &AiProviderConfig,
    pii_filter_level: PiiFilterLevel,
    external_ocr_privacy_guard: Option<ExternalOcrPrivacyGuard>,
    secret_stores: Option<SecretStoreSet>,
) -> Result<AiProviderAdapters, CoreError> {
    let (ocr, ocr_source, ocr_fallback_reason) = resolve_ocr_provider(
        config,
        pii_filter_level,
        external_ocr_privacy_guard,
        secret_stores.clone(),
    )?;
    let (llm, llm_source, llm_fallback_reason) = resolve_llm_provider(config, secret_stores)?;

    Ok(AiProviderAdapters {
        ocr,
        llm,
        ocr_source,
        llm_source,
        ocr_fallback_reason,
        llm_fallback_reason,
    })
}

pub(super) fn configured_ocr_surface_transport(
    config: &AiProviderConfig,
) -> Option<(String, ProviderSurfaceTransport)> {
    config
        .ocr_api
        .as_ref()
        .and_then(|endpoint| endpoint.surface_id.as_deref())
        .and_then(|surface_id| {
            provider_surface_spec(surface_id).map(|spec| (spec.id.to_string(), spec.transport))
        })
}

#[cfg(feature = "server")]
pub(super) fn configured_llm_surface_transport(
    config: &AiProviderConfig,
) -> Option<(String, ProviderSurfaceTransport)> {
    config
        .llm_api
        .as_ref()
        .and_then(|endpoint| endpoint.surface_id.as_deref())
        .and_then(|surface_id| {
            provider_surface_spec(surface_id).map(|spec| (spec.id.to_string(), spec.transport))
        })
}

#[cfg(feature = "server")]
pub(super) fn llm_uses_managed_oauth(config: &AiProviderConfig) -> bool {
    if config.llm_provider != LlmProviderType::Remote {
        return false;
    }

    match configured_llm_surface_transport(config) {
        Some((_, ProviderSurfaceTransport::ManagedOAuth)) => true,
        Some(_) => false,
        None => true,
    }
}

pub(super) fn unsupported_ocr_surface_runtime(
    config: &AiProviderConfig,
    surface_id: &str,
    transport: ProviderSurfaceTransport,
) -> OcrProviderResolution {
    let runtime_label = match transport {
        ProviderSurfaceTransport::DirectApi => "direct_http",
        ProviderSurfaceTransport::ManagedOAuth => "managed_oauth",
        ProviderSurfaceTransport::SubprocessCli => "subprocess_cli",
    };
    let reason = format!(
        "Selected OCR provider surface '{surface_id}' uses {runtime_label}, but an OCR runtime adapter for that transport is not implemented yet."
    );

    if config.fallback_to_local {
        warn!(
            fallback_reason = %reason,
            "OCR runtime unavailable for selected provider surface, falling back to local OCR"
        );
        return Ok((
            best_local_ocr_provider(),
            ProviderSource::LocalFallback,
            Some(reason),
        ));
    }

    Err(CoreError::Config {
        code: oneshim_core::error_codes::ConfigCode::Invalid,
        message: reason,
    })
}
