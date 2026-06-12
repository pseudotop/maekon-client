mod codex_ui_approval_hook;
mod fallback_llm;
mod guarded_analysis;
mod guarded_conversation;
mod guarded_llm;
mod guarded_ocr;
mod helpers;
mod llm_resolver;
mod ocr_resolver;
mod surface;
mod types;

#[cfg(all(test, feature = "analysis"))]
mod tests;

pub(crate) use codex_ui_approval_hook::{CodexApprovalRegistry, CodexUiApprovalHook};
pub(crate) use guarded_conversation::{ConversationContentGuard, GuardedConversationSession};
pub use types::{AiProviderAdapters, ExternalOcrPrivacyGuard, ProviderSource};

use std::sync::Arc;

use maekon_core::config::{AiAccessMode, AiProviderConfig, PiiFilterLevel};
use maekon_core::error::CoreError;
use maekon_core::ports::analysis_provider::AnalysisProvider;
use maekon_core::ports::llm_provider::LlmCallHealth;
#[cfg(feature = "analysis")]
use maekon_core::ports::oauth::OAuthPort;
use maekon_core::ports::secret_store::SecretStoreSet;
#[cfg(feature = "analysis")]
use maekon_core::provider_surface::ProviderSurfaceTransport;
use ocr_resolver::best_local_ocr_provider;

use llm_resolver::resolve_cli_subscription_llm_provider_with_detected;
use llm_resolver::resolve_local_model_llm_provider;
#[cfg(feature = "analysis")]
use llm_resolver::{resolve_llm_provider, resolve_llm_provider_oauth};
use ocr_resolver::resolve_cli_subscription_ocr_provider;
#[cfg(feature = "analysis")]
use ocr_resolver::{resolve_ocr_provider, resolve_ocr_provider_oauth};
use surface::resolve_direct_surface_adapters;
#[cfg(feature = "analysis")]
use surface::{configured_ocr_surface_transport, llm_uses_managed_oauth};
use types::ProviderSource as PS;

use crate::subprocess_provider::probe_known_cli_surfaces;

pub(crate) fn guard_external_analysis_provider(
    provider: Arc<dyn AnalysisProvider>,
    privacy_guard: Option<ExternalOcrPrivacyGuard>,
) -> Result<Arc<dyn AnalysisProvider>, CoreError> {
    let guard = privacy_guard.ok_or_else(|| CoreError::Config {
        code: maekon_core::error_codes::ConfigCode::Missing,
        message: "External analysis provider requires a runtime privacy guard.".to_string(),
    })?;
    Ok(Arc::new(guarded_analysis::GuardedAnalysisProvider::new(
        provider, guard,
    )) as Arc<dyn AnalysisProvider>)
}

pub fn resolve_ai_provider_adapters(
    config: &AiProviderConfig,
    pii_filter_level: PiiFilterLevel,
    external_ocr_privacy_guard: Option<ExternalOcrPrivacyGuard>,
    secret_stores: Option<SecretStoreSet>,
    #[cfg(feature = "analysis")] oauth_port: Option<Arc<dyn OAuthPort>>,
    // D7 (#4812 / E20-20): the single shared workspace-wide circuit-breaker
    // registry from the composition root. Every remote provider adapter resolved
    // below (OCR/LLM, direct/OAuth) clones this one Arc, so adapters targeting the
    // same endpoint converge on a single breaker (iter-011 consolidation done).
    breaker_registry: Arc<crate::breaker_registry::CircuitBreakerRegistry>,
    // Per-call LLM health handle forwarded to the LocalModel/FU-3 arms only.
    // `None` is safe and leaves those arms with no health tracking.
    llm_call_health: Option<Arc<LlmCallHealth>>,
) -> Result<AiProviderAdapters, CoreError> {
    match config.access_mode.normalized_for_ai_surfaces() {
        AiAccessMode::LocalModel => {
            // OCR: unchanged — best local OCR (Tesseract / macOS Vision).
            // LLM: C3 — route to loopback Ollama via FallbackLlmProvider;
            //      rule matcher demoted to runtime fallback.
            // Ok-degrade invariant: resolve_local_model_llm_provider never
            // returns Err (Decision 1), so this arm cannot trigger
            // should_fallback_to_noop in automation_controller_builder.
            let (llm, llm_source, llm_fallback_reason) = resolve_local_model_llm_provider(
                config,
                external_ocr_privacy_guard.clone(),
                breaker_registry,
                // #5734: wire the per-call health handle into the Ollama
                // provider so `AutomationStatusDto.llm_healthy` reflects
                // real per-call success/failure.
                llm_call_health,
            )?;
            Ok(AiProviderAdapters {
                ocr: best_local_ocr_provider(),
                llm,
                ocr_source: PS::Local,
                llm_source,
                ocr_fallback_reason: None,
                llm_fallback_reason,
            })
        }
        AiAccessMode::ProviderSubscriptionCli => {
            let probed = probe_known_cli_surfaces();
            let (ocr, ocr_source, ocr_fallback_reason) = resolve_cli_subscription_ocr_provider(
                config,
                pii_filter_level,
                external_ocr_privacy_guard.clone(),
                secret_stores.clone(),
                &probed,
                breaker_registry.clone(),
            )?;
            let (llm, llm_source, llm_fallback_reason) =
                resolve_cli_subscription_llm_provider_with_detected(
                    config,
                    external_ocr_privacy_guard.clone(),
                    &probed,
                )?;
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
            breaker_registry,
            llm_call_health,
        ),
        AiAccessMode::ProviderOAuth => {
            #[cfg(feature = "analysis")]
            {
                let ocr_uses_managed = matches!(
                    configured_ocr_surface_transport(config),
                    Some((_, ProviderSurfaceTransport::ManagedOAuth))
                );
                let llm_uses_managed = llm_uses_managed_oauth(config);
                let oauth = if ocr_uses_managed || llm_uses_managed {
                    Some(oauth_port.ok_or_else(|| {
                        CoreError::Config { code: maekon_core::error_codes::ConfigCode::Invalid, message: "ProviderOAuth mode requires an initialized OAuth runtime for managed provider surfaces.".to_string(), }
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
                            code: maekon_core::error_codes::ConfigCode::Invalid,
                            message: "oauth runtime required for managed OCR mode".into(),
                        })?,
                        breaker_registry.clone(),
                    )?
                } else {
                    resolve_ocr_provider(
                        config,
                        pii_filter_level,
                        external_ocr_privacy_guard.clone(),
                        secret_stores.clone(),
                        breaker_registry.clone(),
                    )?
                };
                let (llm, llm_source, llm_fallback_reason) = if llm_uses_managed {
                    resolve_llm_provider_oauth(
                        config,
                        external_ocr_privacy_guard.clone(),
                        oauth.clone().ok_or_else(|| CoreError::Config {
                            code: maekon_core::error_codes::ConfigCode::Invalid,
                            message: "oauth runtime required for managed LLM mode".into(),
                        })?,
                        breaker_registry,
                    )?
                } else {
                    resolve_llm_provider(
                        config,
                        external_ocr_privacy_guard.clone(),
                        secret_stores.clone(),
                        breaker_registry,
                        // OAuth arm: non-managed LLM uses resolve_llm_provider
                        // which may hit the Local sub-arm (FU-3). Forward handle.
                        llm_call_health,
                    )?
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
            #[cfg(not(feature = "analysis"))]
            {
                // Iter-109: feature-gate disabled = service unavailable.
                Err(CoreError::ServiceUnavailable {
                    code: maekon_core::error_codes::ServiceCode::Unavailable,
                    message: "ProviderOAuth mode requires the 'analysis' feature".to_string(),
                })
            }
        }
    }
}
