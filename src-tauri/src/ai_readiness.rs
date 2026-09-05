//! Privacy-safe AI readiness projection for desktop product surfaces (#11735).
//!
//! This module never invokes a provider. It combines bounded feature probes,
//! live configuration, and consent booleans into the typed core contract.

use crate::feature_capabilities::{
    FeatureAvailability, FeatureCapability, FeatureCapabilitySnapshot, ProviderCliReadiness,
};
use maekon_core::ai_readiness::{
    evaluate_ai_readiness, AiCapabilityId, AiConsentField, AiConsentReadiness,
    AiInvocationGuardState, AiModelAvailability, AiProviderAuthReadiness, AiProviderDetection,
    AiProviderInvocationReadiness, AiReadinessDimensions, AiReadinessSnapshot,
    AiRuntimeApplyRequirement,
};
use maekon_core::config::{
    AiAccessMode, AiProviderConfig, AiProviderType, AppConfig, ExternalApiEndpoint, OcrProviderType,
};

/// Build the shared Chat, OCR-derived suggestion, and summary snapshot.
#[cfg(test)]
pub(crate) fn build_ai_readiness_snapshot(
    provider_snapshot: &FeatureCapabilitySnapshot,
    current: &AppConfig,
    boot: &AppConfig,
    consent: &maekon_core::consent::ConsentPermissions,
) -> AiReadinessSnapshot {
    build_ai_readiness_snapshot_with_local_preflight(
        provider_snapshot,
        current,
        boot,
        consent,
        None,
    )
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct LocalChatPreflight {
    detection: AiProviderDetection,
    auth: AiProviderAuthReadiness,
    invocation: AiProviderInvocationReadiness,
    model_availability: AiModelAvailability,
}

/// Probe only loopback Ollama metadata endpoints. This never invokes a model,
/// submits a prompt, consumes provider tokens, or turns a generic capability
/// read into unconsented external egress.
#[cfg(feature = "analysis")]
pub(crate) async fn probe_local_chat_preflight(config: &AiProviderConfig) -> LocalChatPreflight {
    use crate::session_manager::factory::{
        negotiate_local_llm_model, resolve_local_llm_target, NegotiationOutcome,
    };

    let target = resolve_local_llm_target(config);
    if !maekon_http_core::outbound::host_is_loopback(&target.base_url) {
        return unavailable_local_chat_preflight();
    }
    let resolved_model = target
        .default_model
        .clone()
        .or_else(|| {
            maekon_api_contracts::provider_specs::resolved_default_model(
                AiProviderType::Ollama,
                None,
                maekon_api_contracts::provider_specs::SurfaceCapabilityKind::Llm,
            )
            .ok()
            .flatten()
        })
        .unwrap_or_else(|| "qwen3:8b".to_string());
    let timeout = std::time::Duration::from_secs(2);
    match maekon_network::ollama_discovery::probe_ollama(&target.base_url, timeout).await {
        maekon_network::ollama_discovery::OllamaProbe::Ollama { .. } => {
            let installed =
                maekon_network::ollama_discovery::list_installed_models(&target.base_url, timeout)
                    .await;
            // The configured model is the session default, not a per-session
            // advanced override, so the same-family fallback remains valid.
            let (_, outcome) =
                negotiate_local_llm_model(&resolved_model, false, installed.as_deref());
            let model_availability = match outcome {
                NegotiationOutcome::ExactMatch
                | NegotiationOutcome::FamilyFallback { .. }
                | NegotiationOutcome::ExplicitOverride => AiModelAvailability::Available,
                NegotiationOutcome::NotInstalled { .. } => AiModelAvailability::Unavailable,
                NegotiationOutcome::ListUnavailable => AiModelAvailability::Unverified,
            };
            LocalChatPreflight {
                detection: AiProviderDetection::Detected,
                auth: AiProviderAuthReadiness::NotRequired,
                invocation: AiProviderInvocationReadiness::Ready,
                model_availability,
            }
        }
        maekon_network::ollama_discovery::OllamaProbe::NotOllama
        | maekon_network::ollama_discovery::OllamaProbe::Unreachable => {
            unavailable_local_chat_preflight()
        }
    }
}

#[cfg(not(feature = "analysis"))]
// The changed-line mutation lane runs the default feature set, where this
// no-analysis adapter is cfg-elided. Its fail-closed values are exercised by
// `non_analysis_build_fails_closed_for_local_chat_preflight` in the canonical
// no-default-features cross-check instead.
#[mutants::skip]
pub(crate) async fn probe_local_chat_preflight(_config: &AiProviderConfig) -> LocalChatPreflight {
    unavailable_local_chat_preflight()
}

fn unavailable_local_chat_preflight() -> LocalChatPreflight {
    LocalChatPreflight {
        detection: AiProviderDetection::NotDetected,
        auth: AiProviderAuthReadiness::NotRequired,
        invocation: AiProviderInvocationReadiness::Unavailable,
        model_availability: AiModelAvailability::Unavailable,
    }
}

pub(crate) fn build_ai_readiness_snapshot_with_local_preflight(
    provider_snapshot: &FeatureCapabilitySnapshot,
    current: &AppConfig,
    boot: &AppConfig,
    consent: &maekon_core::consent::ConsentPermissions,
    local_preflight: Option<LocalChatPreflight>,
) -> AiReadinessSnapshot {
    let access_mode = current.ai_provider.access_mode.normalized_for_ai_surfaces();
    let cli_axes =
        selected_cli_provider_axes(provider_snapshot, current.ai_provider.llm_api.as_ref());
    let apply_pending = provider_selection_fingerprint(&current.ai_provider)
        != provider_selection_fingerprint(&boot.ai_provider);

    let chat_subprocess = evaluate_ai_readiness(
        AiCapabilityId::ChatSubprocess,
        dimensions(
            access_mode,
            access_mode == AiAccessMode::ProviderSubscriptionCli,
            true,
            cli_axes,
            AiModelAvailability::NotRequired,
            ReadinessRuntime {
                compiled_capability: cfg!(feature = "analysis"),
                runtime_flag_enabled: true,
                consent: Vec::new(),
                apply_requirement: AiRuntimeApplyRequirement::Restart,
                apply_pending,
            },
        ),
    );

    let chat_http = evaluate_ai_readiness(
        AiCapabilityId::ChatHttpApi,
        dimensions(
            access_mode,
            matches!(
                access_mode,
                AiAccessMode::ProviderApiKey | AiAccessMode::ProviderOAuth
            ),
            endpoint_or_profile_configured(&current.ai_provider),
            http_provider_axes(provider_snapshot, &current.ai_provider),
            configured_model_availability(current.ai_provider.llm_api.as_ref()),
            ReadinessRuntime {
                compiled_capability: cfg!(feature = "analysis"),
                runtime_flag_enabled: true,
                consent: Vec::new(),
                apply_requirement: AiRuntimeApplyRequirement::Restart,
                apply_pending,
            },
        ),
    );

    let local_axes = local_preflight.map_or_else(
        || local_provider_axes(provider_snapshot),
        |preflight| ProviderReadinessAxes {
            detection: preflight.detection,
            auth: preflight.auth,
            invocation: preflight.invocation,
        },
    );
    let chat_local = evaluate_ai_readiness(
        AiCapabilityId::ChatLocalLlm,
        dimensions(
            access_mode,
            access_mode == AiAccessMode::LocalModel,
            true,
            local_axes,
            local_preflight.map_or_else(
                || configured_local_model_availability(&current.ai_provider),
                |preflight| preflight.model_availability,
            ),
            ReadinessRuntime {
                compiled_capability: cfg!(feature = "analysis"),
                runtime_flag_enabled: true,
                consent: Vec::new(),
                apply_requirement: AiRuntimeApplyRequirement::Restart,
                apply_pending,
            },
        ),
    );

    let ocr_local = ocr_uses_local_runtime(&current.ai_provider);
    let ocr_axes = ocr_provider_axes(provider_snapshot, &current.ai_provider, ocr_local);
    let ocr_capture = evaluate_ai_readiness(
        AiCapabilityId::OcrCapture,
        dimensions(
            access_mode,
            true,
            ocr_local
                || current
                    .ai_provider
                    .ocr_api
                    .as_ref()
                    .is_some_and(|value| !value.endpoint.trim().is_empty()),
            ocr_axes,
            if ocr_local {
                AiModelAvailability::NotRequired
            } else {
                configured_model_availability(current.ai_provider.ocr_api.as_ref())
            },
            ReadinessRuntime {
                compiled_capability: if ocr_local {
                    provider_snapshot.ocr_available
                } else {
                    cfg!(feature = "analysis")
                },
                runtime_flag_enabled: current.vision.ocr_enabled,
                consent: vec![AiConsentReadiness {
                    field: AiConsentField::OcrProcessing,
                    granted: consent.ocr_processing,
                }],
                apply_requirement: AiRuntimeApplyRequirement::RuntimeApplied,
                apply_pending: false,
            },
        ),
    );

    let ready_cli_mode_mismatch = access_mode == AiAccessMode::ProviderApiKey
        && !endpoint_or_profile_configured(&current.ai_provider)
        && cli_axes.invocation == AiProviderInvocationReadiness::Ready;
    let suggestion = evaluate_ai_readiness(
        AiCapabilityId::OcrSuggestionAnalysis,
        dimensions(
            access_mode,
            analysis_access_mode_compatible(&current.ai_provider, ready_cli_mode_mismatch),
            analysis_endpoint_or_profile_configured(&current.ai_provider),
            analysis_provider_axes(provider_snapshot, &current.ai_provider),
            match access_mode {
                AiAccessMode::ProviderSubscriptionCli => AiModelAvailability::NotRequired,
                AiAccessMode::LocalModel => {
                    configured_local_model_availability(&current.ai_provider)
                }
                _ => configured_model_availability(current.ai_provider.llm_api.as_ref()),
            },
            ReadinessRuntime {
                compiled_capability: cfg!(all(feature = "analysis", feature = "local-suggestions")),
                runtime_flag_enabled: current.analysis.enabled,
                consent: vec![
                    AiConsentReadiness {
                        field: AiConsentField::OcrProcessing,
                        granted: consent.ocr_processing,
                    },
                    AiConsentReadiness {
                        field: AiConsentField::ActivityPatternLearning,
                        granted: consent.activity_pattern_learning,
                    },
                ],
                apply_requirement: AiRuntimeApplyRequirement::HotRewire,
                apply_pending: apply_pending && current.analysis.enabled && boot.analysis.enabled,
            },
        ),
    );

    let summary_axes = summary_provider_axes(provider_snapshot, &current.ai_provider);
    let summary_dimensions = dimensions(
        access_mode,
        summary_access_mode_compatible(&current.ai_provider, ready_cli_mode_mismatch),
        summary_endpoint_or_profile_configured(&current.ai_provider),
        summary_axes,
        summary_model_availability(&current.ai_provider),
        ReadinessRuntime {
            // The shipped analysis adapter supplies the loopback embedding
            // fallback even when the optional native `embedding` crate is not
            // compiled. Requiring that feature here would falsely block the
            // production release profile.
            compiled_capability: cfg!(feature = "analysis"),
            runtime_flag_enabled: summary_runtime_enabled(current),
            consent: vec![AiConsentReadiness {
                field: AiConsentField::ActivityPatternLearning,
                granted: consent.activity_pattern_learning,
            }],
            apply_requirement: AiRuntimeApplyRequirement::Restart,
            apply_pending: summary_startup_fingerprint(current)
                != summary_startup_fingerprint(boot),
        },
    );
    let segment_summary =
        evaluate_ai_readiness(AiCapabilityId::SegmentSummary, summary_dimensions.clone());
    let daily_narrative = evaluate_ai_readiness(AiCapabilityId::DailyNarrative, summary_dimensions);

    AiReadinessSnapshot::new(vec![
        chat_subprocess,
        chat_http,
        chat_local,
        ocr_capture,
        suggestion,
        segment_summary,
        daily_narrative,
    ])
}

struct ReadinessRuntime {
    compiled_capability: bool,
    runtime_flag_enabled: bool,
    consent: Vec<AiConsentReadiness>,
    apply_requirement: AiRuntimeApplyRequirement,
    apply_pending: bool,
}

fn dimensions(
    selected_access_mode: AiAccessMode,
    access_mode_compatible: bool,
    endpoint_or_profile_configured: bool,
    axes: ProviderReadinessAxes,
    model_availability: AiModelAvailability,
    runtime: ReadinessRuntime,
) -> AiReadinessDimensions {
    let guard = AiInvocationGuardState::EnforcedAtInvocation;
    AiReadinessDimensions {
        compiled_capability: runtime.compiled_capability,
        selected_access_mode,
        access_mode_compatible,
        endpoint_or_profile_configured,
        provider_detection: axes.detection,
        provider_auth: axes.auth,
        provider_invocation: axes.invocation,
        model_availability,
        runtime_flag_enabled: runtime.runtime_flag_enabled,
        consent: runtime.consent,
        apply_requirement: runtime.apply_requirement,
        apply_pending: runtime.apply_pending,
        // Readiness is evidence, never authority. Invocation keeps enforcing
        // the existing privacy, egress, budget, and audit gates.
        privacy_gate: guard,
        egress_gate: guard,
        budget_gate: guard,
        audit_gate: guard,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProviderReadinessAxes {
    detection: AiProviderDetection,
    auth: AiProviderAuthReadiness,
    invocation: AiProviderInvocationReadiness,
}

fn cli_axes_from_readiness(readiness: &[ProviderCliReadiness]) -> ProviderReadinessAxes {
    if readiness.is_empty()
        || readiness
            .iter()
            .all(|value| *value == ProviderCliReadiness::NotDetected)
    {
        return ProviderReadinessAxes {
            detection: AiProviderDetection::NotDetected,
            auth: AiProviderAuthReadiness::Required,
            invocation: AiProviderInvocationReadiness::Unavailable,
        };
    }

    let invocation = if readiness.contains(&ProviderCliReadiness::InvocationReady) {
        AiProviderInvocationReadiness::Ready
    } else if readiness.iter().any(|value| {
        matches!(
            value,
            ProviderCliReadiness::AuthReady | ProviderCliReadiness::AuthUnverified
        )
    }) {
        AiProviderInvocationReadiness::Unverified
    } else {
        AiProviderInvocationReadiness::Unavailable
    };
    let auth = if readiness.iter().any(|value| {
        matches!(
            value,
            ProviderCliReadiness::InvocationReady | ProviderCliReadiness::AuthReady
        )
    }) {
        AiProviderAuthReadiness::Ready
    } else if readiness.contains(&ProviderCliReadiness::AuthUnverified) {
        AiProviderAuthReadiness::Unverified
    } else {
        AiProviderAuthReadiness::Required
    };
    ProviderReadinessAxes {
        detection: AiProviderDetection::Detected,
        auth,
        invocation,
    }
}

fn selected_cli_provider_axes(
    snapshot: &FeatureCapabilitySnapshot,
    endpoint: Option<&ExternalApiEndpoint>,
) -> ProviderReadinessAxes {
    if let Some(endpoint) = endpoint {
        let selected =
            selected_feature(snapshot, endpoint).and_then(|feature| feature.provider_cli_readiness);
        // An explicit selection is authoritative. Falling back to another
        // detected CLI would report the wrong provider as invocation-ready.
        return selected.map_or_else(
            || cli_axes_from_readiness(&[]),
            |readiness| cli_axes_from_readiness(&[readiness]),
        );
    }

    let readiness: Vec<ProviderCliReadiness> = snapshot
        .features
        .iter()
        .filter_map(|feature| feature.provider_cli_readiness)
        .collect();
    cli_axes_from_readiness(&readiness)
}

fn http_provider_axes(
    snapshot: &FeatureCapabilitySnapshot,
    config: &AiProviderConfig,
) -> ProviderReadinessAxes {
    http_endpoint_axes(
        snapshot,
        config.llm_api.as_ref(),
        config.access_mode == AiAccessMode::ProviderOAuth,
    )
}

fn http_endpoint_axes(
    snapshot: &FeatureCapabilitySnapshot,
    endpoint: Option<&ExternalApiEndpoint>,
    oauth_mode: bool,
) -> ProviderReadinessAxes {
    let Some(endpoint) = endpoint else {
        return ProviderReadinessAxes {
            detection: AiProviderDetection::NotDetected,
            auth: AiProviderAuthReadiness::Required,
            invocation: AiProviderInvocationReadiness::Unavailable,
        };
    };
    if endpoint.endpoint.trim().is_empty() {
        return ProviderReadinessAxes {
            detection: AiProviderDetection::NotDetected,
            auth: AiProviderAuthReadiness::Required,
            invocation: AiProviderInvocationReadiness::Unavailable,
        };
    }

    let auth = if oauth_mode {
        selected_feature(snapshot, endpoint)
            .map(|feature| match feature.availability {
                FeatureAvailability::Available => AiProviderAuthReadiness::Ready,
                FeatureAvailability::Unavailable => AiProviderAuthReadiness::Required,
                FeatureAvailability::PartiallyAvailable => AiProviderAuthReadiness::Unverified,
            })
            .unwrap_or(AiProviderAuthReadiness::Unverified)
    } else if endpoint_has_credential(endpoint) {
        AiProviderAuthReadiness::Ready
    } else {
        AiProviderAuthReadiness::Required
    };
    let invocation = selected_feature(snapshot, endpoint)
        .map(|feature| match feature.availability {
            FeatureAvailability::Unavailable => AiProviderInvocationReadiness::Unavailable,
            FeatureAvailability::Available | FeatureAvailability::PartiallyAvailable => {
                AiProviderInvocationReadiness::Unverified
            }
        })
        .unwrap_or(AiProviderInvocationReadiness::Unverified);
    ProviderReadinessAxes {
        detection: AiProviderDetection::NotRequired,
        auth,
        invocation,
    }
}

fn local_provider_axes(snapshot: &FeatureCapabilitySnapshot) -> ProviderReadinessAxes {
    let local = snapshot.features.iter().find(|feature| {
        feature.feature_id.contains(".ollama.")
            || feature
                .requires
                .iter()
                .any(|requirement| requirement == "local_server:ollama")
    });
    match local {
        Some(feature) => ProviderReadinessAxes {
            detection: if feature.availability == FeatureAvailability::Unavailable {
                AiProviderDetection::NotDetected
            } else {
                AiProviderDetection::Detected
            },
            auth: AiProviderAuthReadiness::NotRequired,
            invocation: if feature.availability == FeatureAvailability::Unavailable {
                AiProviderInvocationReadiness::Unavailable
            } else {
                AiProviderInvocationReadiness::Unverified
            },
        },
        None => ProviderReadinessAxes {
            detection: AiProviderDetection::NotDetected,
            auth: AiProviderAuthReadiness::NotRequired,
            invocation: AiProviderInvocationReadiness::Unavailable,
        },
    }
}

fn ocr_provider_axes(
    snapshot: &FeatureCapabilitySnapshot,
    config: &AiProviderConfig,
    ocr_local: bool,
) -> ProviderReadinessAxes {
    if ocr_local {
        return ProviderReadinessAxes {
            detection: AiProviderDetection::NotRequired,
            auth: AiProviderAuthReadiness::NotRequired,
            invocation: AiProviderInvocationReadiness::NotRequired,
        };
    }

    if config.access_mode.normalized_for_ai_surfaces() == AiAccessMode::ProviderSubscriptionCli
        && config.ocr_api.as_ref().is_some_and(|endpoint| {
            selected_feature(snapshot, endpoint)
                .is_none_or(|feature| feature.provider_cli_readiness.is_some())
        })
    {
        return selected_cli_provider_axes(snapshot, config.ocr_api.as_ref());
    }

    http_endpoint_axes(
        snapshot,
        config.ocr_api.as_ref(),
        config.access_mode == AiAccessMode::ProviderOAuth,
    )
}

fn ocr_uses_local_runtime(config: &AiProviderConfig) -> bool {
    if config.access_mode.normalized_for_ai_surfaces() == AiAccessMode::LocalModel {
        return true;
    }
    let explicit_cli_selection = config.access_mode.normalized_for_ai_surfaces()
        == AiAccessMode::ProviderSubscriptionCli
        && config.ocr_api.is_some();
    !explicit_cli_selection && config.ocr_provider == OcrProviderType::Local
}

fn analysis_provider_axes(
    snapshot: &FeatureCapabilitySnapshot,
    config: &AiProviderConfig,
) -> ProviderReadinessAxes {
    match config.access_mode.normalized_for_ai_surfaces() {
        AiAccessMode::ProviderSubscriptionCli => {
            selected_cli_provider_axes(snapshot, config.llm_api.as_ref())
        }
        AiAccessMode::LocalModel => local_provider_axes(snapshot),
        _ => http_provider_axes(snapshot, config),
    }
}

fn analysis_access_mode_compatible(config: &AiProviderConfig, cli_mode_mismatch: bool) -> bool {
    if cli_mode_mismatch {
        return false;
    }
    match config.access_mode.normalized_for_ai_surfaces() {
        AiAccessMode::LocalModel => config
            .llm_api
            .as_ref()
            .is_none_or(|endpoint| endpoint.provider_type == AiProviderType::Ollama),
        _ => true,
    }
}

fn summary_provider_axes(
    snapshot: &FeatureCapabilitySnapshot,
    config: &AiProviderConfig,
) -> ProviderReadinessAxes {
    match config.llm_api.as_ref() {
        None => local_provider_axes(snapshot),
        Some(_) => analysis_provider_axes(snapshot, config),
    }
}

fn summary_access_mode_compatible(config: &AiProviderConfig, cli_mode_mismatch: bool) -> bool {
    match config.llm_api.as_ref() {
        None => config.access_mode.normalized_for_ai_surfaces() == AiAccessMode::LocalModel,
        Some(_) => analysis_access_mode_compatible(config, cli_mode_mismatch),
    }
}

fn summary_endpoint_or_profile_configured(config: &AiProviderConfig) -> bool {
    match config.llm_api.as_ref() {
        None => true,
        Some(_) => analysis_endpoint_or_profile_configured(config),
    }
}

fn summary_model_availability(config: &AiProviderConfig) -> AiModelAvailability {
    match config.llm_api.as_ref() {
        None => AiModelAvailability::Unverified,
        Some(_) if config.access_mode == AiAccessMode::ProviderSubscriptionCli => {
            AiModelAvailability::NotRequired
        }
        endpoint => configured_model_availability(endpoint),
    }
}

fn summary_runtime_enabled(config: &AppConfig) -> bool {
    config.analysis.enabled
        && config.analysis.tiered_memory.enabled
        && config.analysis.embedding.enabled
        && config.analysis.embedding.llm_summary_enabled
}
fn selected_feature<'a>(
    snapshot: &'a FeatureCapabilitySnapshot,
    endpoint: &ExternalApiEndpoint,
) -> Option<&'a FeatureCapability> {
    let surface_id = endpoint.surface_id.as_deref()?.trim();
    snapshot
        .features
        .iter()
        .find(|feature| feature.feature_id == surface_id)
}

fn endpoint_or_profile_configured(config: &AiProviderConfig) -> bool {
    config
        .llm_api
        .as_ref()
        .is_some_and(|value| !value.endpoint.trim().is_empty())
        || config
            .active_profile_id
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
}

fn analysis_endpoint_or_profile_configured(config: &AiProviderConfig) -> bool {
    match config.access_mode.normalized_for_ai_surfaces() {
        AiAccessMode::ProviderSubscriptionCli | AiAccessMode::LocalModel => true,
        _ => endpoint_or_profile_configured(config),
    }
}

fn endpoint_has_credential(endpoint: &ExternalApiEndpoint) -> bool {
    !endpoint.api_key.trim().is_empty()
        || endpoint
            .credential
            .as_ref()
            .and_then(|binding| binding.secret_ref.as_ref())
            .is_some()
}

fn configured_model_availability(endpoint: Option<&ExternalApiEndpoint>) -> AiModelAvailability {
    match endpoint.and_then(|value| value.model.as_deref()) {
        Some(model) if !model.trim().is_empty() => AiModelAvailability::Unverified,
        Some(_) | None => AiModelAvailability::Unavailable,
    }
}

fn configured_local_model_availability(config: &AiProviderConfig) -> AiModelAvailability {
    config
        .llm_api
        .as_ref()
        .map_or(AiModelAvailability::Unverified, |endpoint| {
            configured_model_availability(Some(endpoint))
        })
}

#[derive(Debug, PartialEq, Eq)]
struct ProviderSelectionFingerprint<'a> {
    access_mode: AiAccessMode,
    active_profile_id: Option<&'a str>,
    endpoint: Option<&'a str>,
    model: Option<&'a str>,
    provider_type: Option<AiProviderType>,
    surface_id: Option<&'a str>,
    credential_present: bool,
}

#[derive(Debug, PartialEq, Eq)]
struct SummaryStartupFingerprint<'a> {
    provider: ProviderSelectionFingerprint<'a>,
    analysis_enabled: bool,
    tiered_memory_enabled: bool,
    embedding_enabled: bool,
    llm_summary_enabled: bool,
    embedding_provider: &'a maekon_core::config::EmbeddingProviderType,
    local_model: &'a str,
    remote_endpoint: Option<&'a str>,
    remote_model: Option<&'a str>,
    remote_dimensions: Option<usize>,
    remote_credential_present: bool,
    min_segment_for_summary_secs: u64,
}

fn provider_selection_fingerprint(config: &AiProviderConfig) -> ProviderSelectionFingerprint<'_> {
    ProviderSelectionFingerprint {
        access_mode: config.access_mode,
        active_profile_id: config.active_profile_id.as_deref(),
        endpoint: config.llm_api.as_ref().map(|value| value.endpoint.as_str()),
        model: config
            .llm_api
            .as_ref()
            .and_then(|value| value.model.as_deref()),
        provider_type: config.llm_api.as_ref().map(|value| value.provider_type),
        surface_id: config
            .llm_api
            .as_ref()
            .and_then(|value| value.surface_id.as_deref()),
        credential_present: config.llm_api.as_ref().is_some_and(endpoint_has_credential),
    }
}

fn summary_startup_fingerprint(config: &AppConfig) -> SummaryStartupFingerprint<'_> {
    SummaryStartupFingerprint {
        provider: provider_selection_fingerprint(&config.ai_provider),
        analysis_enabled: config.analysis.enabled,
        tiered_memory_enabled: config.analysis.tiered_memory.enabled,
        embedding_enabled: config.analysis.embedding.enabled,
        llm_summary_enabled: config.analysis.embedding.llm_summary_enabled,
        embedding_provider: &config.analysis.embedding.provider,
        local_model: config.analysis.embedding.local_model.as_str(),
        remote_endpoint: config.analysis.embedding.remote_endpoint.as_deref(),
        remote_model: config.analysis.embedding.remote_model.as_deref(),
        remote_dimensions: config.analysis.embedding.remote_dimensions,
        remote_credential_present: config.analysis.embedding.remote_credential.is_some(),
        min_segment_for_summary_secs: config.analysis.embedding.min_segment_for_summary_secs,
    }
}

#[cfg(test)]
#[path = "ai_readiness_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "ai_readiness_summary_tests.rs"]
mod summary_tests;
