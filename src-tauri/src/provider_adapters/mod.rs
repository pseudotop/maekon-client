mod guarded_ocr;
mod helpers;
mod llm_resolver;
mod ocr_resolver;
mod surface;
mod types;

#[cfg(all(test, feature = "server"))]
mod tests;

pub use types::{AiProviderAdapters, ExternalOcrPrivacyGuard, ProviderSource};

use std::sync::Arc;

use ocr_resolver::best_local_ocr_provider;
use oneshim_automation::local_llm::LocalLlmProvider;
use oneshim_core::config::{AiAccessMode, AiProviderConfig, PiiFilterLevel};
use oneshim_core::error::CoreError;
#[cfg(feature = "server")]
use oneshim_core::ports::oauth::OAuthPort;
use oneshim_core::ports::secret_store::SecretStoreSet;
#[cfg(feature = "server")]
use oneshim_core::provider_surface::ProviderSurfaceTransport;

use llm_resolver::resolve_cli_subscription_llm_provider_with_detected;
#[cfg(feature = "server")]
use llm_resolver::{resolve_llm_provider, resolve_llm_provider_oauth};
use ocr_resolver::resolve_cli_subscription_ocr_provider;
#[cfg(feature = "server")]
use ocr_resolver::{resolve_ocr_provider, resolve_ocr_provider_oauth};
use surface::resolve_direct_surface_adapters;
#[cfg(feature = "server")]
use surface::{configured_ocr_surface_transport, llm_uses_managed_oauth};
use types::ProviderSource as PS;

use crate::subprocess_provider::probe_known_cli_surfaces;

pub fn resolve_ai_provider_adapters(
    config: &AiProviderConfig,
    pii_filter_level: PiiFilterLevel,
    external_ocr_privacy_guard: Option<ExternalOcrPrivacyGuard>,
    secret_stores: Option<SecretStoreSet>,
    #[cfg(feature = "server")] oauth_port: Option<Arc<dyn OAuthPort>>,
) -> Result<AiProviderAdapters, CoreError> {
    match config.access_mode.normalized_for_ai_surfaces() {
        AiAccessMode::LocalModel => Ok(AiProviderAdapters {
            ocr: best_local_ocr_provider(),
            llm: Arc::new(LocalLlmProvider::new()),
            ocr_source: PS::Local,
            llm_source: PS::Local,
            ocr_fallback_reason: None,
            llm_fallback_reason: None,
        }),
        AiAccessMode::ProviderSubscriptionCli => {
            let probed = probe_known_cli_surfaces();
            let (ocr, ocr_source, ocr_fallback_reason) = resolve_cli_subscription_ocr_provider(
                config,
                pii_filter_level,
                external_ocr_privacy_guard.clone(),
                secret_stores.clone(),
                &probed,
            )?;
            let (llm, llm_source, llm_fallback_reason) =
                resolve_cli_subscription_llm_provider_with_detected(config, &probed)?;
            Ok(AiProviderAdapters {
                ocr,
                llm,
                ocr_source,
                llm_source,
                ocr_fallback_reason,
                llm_fallback_reason,
            })
        }
        AiAccessMode::ProviderApiKey => resolve_direct_surface_adapters(
            config,
            pii_filter_level,
            external_ocr_privacy_guard,
            secret_stores,
        ),
        AiAccessMode::ProviderOAuth => {
            #[cfg(feature = "server")]
            {
                let ocr_uses_managed = matches!(
                    configured_ocr_surface_transport(config),
                    Some((_, ProviderSurfaceTransport::ManagedOAuth))
                );
                let llm_uses_managed = llm_uses_managed_oauth(config);
                let oauth = if ocr_uses_managed || llm_uses_managed {
                    Some(oauth_port.ok_or_else(|| {
                        CoreError::Config { code: oneshim_core::error_codes::ConfigCode::Invalid, message: "ProviderOAuth mode requires an initialized OAuth runtime for managed provider surfaces.".to_string(), }
                    })?)
                } else {
                    None
                };
                let (ocr, ocr_source, ocr_fallback_reason) = if ocr_uses_managed {
                    resolve_ocr_provider_oauth(
                        config,
                        pii_filter_level,
                        external_ocr_privacy_guard.clone(),
                        oauth.clone().ok_or_else(|| CoreError::Config {
                            code: oneshim_core::error_codes::ConfigCode::Invalid,
                            message: "oauth runtime required for managed OCR mode".into(),
                        })?,
                    )?
                } else {
                    resolve_ocr_provider(
                        config,
                        pii_filter_level,
                        external_ocr_privacy_guard.clone(),
                        secret_stores.clone(),
                    )?
                };
                let (llm, llm_source, llm_fallback_reason) = if llm_uses_managed {
                    resolve_llm_provider_oauth(
                        config,
                        oauth.clone().ok_or_else(|| CoreError::Config {
                            code: oneshim_core::error_codes::ConfigCode::Invalid,
                            message: "oauth runtime required for managed LLM mode".into(),
                        })?,
                    )?
                } else {
                    resolve_llm_provider(config, secret_stores.clone())?
                };
                Ok(AiProviderAdapters {
                    ocr,
                    llm,
                    ocr_source,
                    llm_source,
                    ocr_fallback_reason,
                    llm_fallback_reason,
                })
            }
            #[cfg(not(feature = "server"))]
            {
                // Iter-109: feature-gate disabled = service unavailable.
                Err(CoreError::ServiceUnavailable {
                    code: oneshim_core::error_codes::ServiceCode::Unavailable,
                    message: "ProviderOAuth mode requires the 'server' feature".to_string(),
                })
            }
        }
    }
}
