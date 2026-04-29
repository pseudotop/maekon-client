use oneshim_api_contracts::provider_specs::{
    list_subprocess_surface_specs, provider_surface_spec as catalog_surface_spec,
    SurfaceCapabilityKind,
};
use oneshim_core::config::AiProviderConfig;
use oneshim_core::config::AiProviderType;
use oneshim_core::provider_surface::provider_type_from_vendor_id;

use super::auth_probe::probe_cli_surface;
use super::runtime::runtime_ready_for_auth_status;
use super::{
    catalog_subprocess_transport, find_executable, DetectedSubprocessCli, ProbedSubprocessCli,
};

pub fn detect_known_cli_surfaces() -> Vec<DetectedSubprocessCli> {
    list_subprocess_surface_specs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|surface| {
            let transport = catalog_subprocess_transport(&surface.surface_id).ok()?;
            transport
                .executable_candidates
                .iter()
                .find_map(|candidate| find_executable(candidate))
                .map(|executable_path| DetectedSubprocessCli {
                    surface_id: surface.surface_id.clone(),
                    executable_path,
                })
        })
        .collect()
}

pub fn probe_known_cli_surfaces() -> Vec<ProbedSubprocessCli> {
    detect_known_cli_surfaces()
        .into_iter()
        .map(probe_cli_surface)
        .collect()
}

pub fn select_cli_surface_for_config(
    config: &AiProviderConfig,
    detected: &[ProbedSubprocessCli],
) -> Option<DetectedSubprocessCli> {
    select_cli_surface_for_capability(config, detected, SurfaceCapabilityKind::Llm)
}

pub fn select_cli_surface_for_capability(
    config: &AiProviderConfig,
    detected: &[ProbedSubprocessCli],
    capability: SurfaceCapabilityKind,
) -> Option<DetectedSubprocessCli> {
    if let Some(surface_id) = preferred_cli_surface_for_capability(config, capability) {
        return detected
            .iter()
            .find(|surface| {
                surface
                    .detected
                    .surface_id
                    .eq_ignore_ascii_case(&surface_id)
                    && runtime_ready_for_auth_status(
                        &surface.detected.surface_id,
                        surface.auth_status,
                        capability,
                    )
            })
            .map(|surface| surface.detected.clone());
    }

    detected
        .iter()
        .find(|surface| {
            runtime_ready_for_auth_status(
                &surface.detected.surface_id,
                surface.auth_status,
                capability,
            )
        })
        .map(|surface| surface.detected.clone())
}

pub fn preferred_cli_surface_for_config(config: &AiProviderConfig) -> Option<String> {
    preferred_cli_surface_for_capability(config, SurfaceCapabilityKind::Llm)
}

pub fn preferred_cli_surface_for_capability(
    config: &AiProviderConfig,
    capability: SurfaceCapabilityKind,
) -> Option<String> {
    endpoint_for_capability(config, capability)
        .and_then(|endpoint| {
            endpoint
                .surface_id
                .as_deref()
                .and_then(surface_for_provider_surface_id)
                .filter(|surface_id| {
                    endpoint.provider_type == AiProviderType::Generic
                        || surface_for_provider_type(endpoint.provider_type, capability).as_deref()
                            == Some(surface_id.as_str())
                })
        })
        .or_else(|| {
            endpoint_for_capability(config, capability)
                .map(|endpoint| endpoint.provider_type)
                .filter(|provider_type| *provider_type != AiProviderType::Generic)
                .and_then(|provider_type| surface_for_provider_type(provider_type, capability))
        })
}

pub fn probe_for_surface_id<'a>(
    probed: &'a [ProbedSubprocessCli],
    surface_id: &str,
) -> Option<&'a ProbedSubprocessCli> {
    probed
        .iter()
        .find(|surface| surface.detected.surface_id.eq_ignore_ascii_case(surface_id))
}

fn endpoint_for_capability(
    config: &AiProviderConfig,
    capability: SurfaceCapabilityKind,
) -> Option<&oneshim_core::config::ExternalApiEndpoint> {
    match capability {
        SurfaceCapabilityKind::Llm => config.llm_api.as_ref(),
        SurfaceCapabilityKind::Ocr => config.ocr_api.as_ref(),
    }
}

fn surface_for_provider_type(
    provider_type: AiProviderType,
    capability: SurfaceCapabilityKind,
) -> Option<String> {
    list_subprocess_surface_specs()
        .ok()?
        .into_iter()
        .filter(|surface| {
            provider_type_from_vendor_id(&surface.provider_type) == Some(provider_type)
        })
        .filter(|surface| match capability {
            SurfaceCapabilityKind::Llm => surface.supports.llm,
            SurfaceCapabilityKind::Ocr => surface.supports.ocr,
        })
        .max_by_key(|surface| surface.preferred_for_product_auth)
        .map(|surface| surface.surface_id.clone())
}

fn surface_for_provider_surface_id(raw: &str) -> Option<String> {
    let normalized = raw.trim();
    if normalized.is_empty() {
        return None;
    }
    catalog_surface_spec(normalized).ok().and_then(|surface| {
        catalog_subprocess_transport(&surface.surface_id)
            .ok()
            .map(|_| surface.surface_id.clone())
    })
}

#[cfg(test)]
mod tests {
    use super::super::SubprocessCliAuthStatus;
    use super::*;
    use oneshim_core::config::AiProviderType;
    use std::path::PathBuf;

    fn endpoint(
        provider_type: AiProviderType,
        model: Option<&str>,
    ) -> oneshim_core::config::ExternalApiEndpoint {
        oneshim_core::config::ExternalApiEndpoint {
            endpoint: "https://example.invalid".to_string(),
            api_key: String::new(),
            model: model.map(|value| value.to_string()),
            timeout_secs: 30,
            provider_type,
            surface_id: None,
            credential: None,
        }
    }

    fn probed(surface_id: &str, auth_status: SubprocessCliAuthStatus) -> ProbedSubprocessCli {
        use super::super::runtime::cli_id_for_surface_id;
        ProbedSubprocessCli {
            detected: DetectedSubprocessCli {
                surface_id: surface_id.to_string(),
                executable_path: PathBuf::from(format!(
                    "/tmp/{}",
                    cli_id_for_surface_id(surface_id).unwrap_or_else(|_| surface_id.to_string())
                )),
            },
            auth_status,
            auth_detail: None,
        }
    }

    #[test]
    fn selects_provider_matching_surface_when_available() {
        let config = AiProviderConfig {
            llm_api: Some(endpoint(AiProviderType::Anthropic, None)),
            ..AiProviderConfig::default()
        };
        let surfaces = vec![
            probed(
                "provider_surface.openai.subprocess_cli",
                SubprocessCliAuthStatus::Authenticated,
            ),
            probed(
                "provider_surface.anthropic.subprocess_cli",
                SubprocessCliAuthStatus::Authenticated,
            ),
        ];

        let resolved = select_cli_surface_for_config(&config, &surfaces).unwrap();
        assert_eq!(
            resolved.surface_id,
            "provider_surface.anthropic.subprocess_cli"
        );
    }

    #[test]
    fn falls_back_to_first_runtime_supported_surface() {
        let config = AiProviderConfig::default();
        let surfaces = vec![
            probed(
                "provider_surface.google.subprocess_cli",
                SubprocessCliAuthStatus::Unknown,
            ),
            probed(
                "provider_surface.openai.subprocess_cli",
                SubprocessCliAuthStatus::Authenticated,
            ),
        ];

        let resolved = select_cli_surface_for_config(&config, &surfaces).unwrap();
        assert_eq!(
            resolved.surface_id,
            "provider_surface.google.subprocess_cli"
        );
    }

    #[test]
    fn allows_unknown_auth_status_when_surface_has_no_probe() {
        let config = AiProviderConfig {
            llm_api: Some(endpoint(AiProviderType::Google, None)),
            ..AiProviderConfig::default()
        };
        let surfaces = vec![probed(
            "provider_surface.google.subprocess_cli",
            SubprocessCliAuthStatus::Unknown,
        )];

        let resolved = select_cli_surface_for_config(&config, &surfaces).unwrap();
        assert_eq!(
            resolved.surface_id,
            "provider_surface.google.subprocess_cli"
        );
    }

    #[test]
    fn selects_provider_matching_ocr_surface_when_available() {
        let config = AiProviderConfig {
            ocr_api: Some(endpoint(AiProviderType::OpenAi, Some("gpt-5.4"))),
            ..AiProviderConfig::default()
        };
        let surfaces = vec![
            probed(
                "provider_surface.openai.subprocess_cli",
                SubprocessCliAuthStatus::Authenticated,
            ),
            probed(
                "provider_surface.anthropic.subprocess_cli",
                SubprocessCliAuthStatus::Authenticated,
            ),
        ];

        let resolved =
            select_cli_surface_for_capability(&config, &surfaces, SurfaceCapabilityKind::Ocr)
                .unwrap();
        assert_eq!(
            resolved.surface_id,
            "provider_surface.openai.subprocess_cli"
        );
    }

    #[test]
    fn does_not_switch_to_a_different_vendor_when_matching_surface_requires_auth() {
        let config = AiProviderConfig {
            llm_api: Some(endpoint(AiProviderType::OpenAi, None)),
            ..AiProviderConfig::default()
        };
        let surfaces = vec![
            probed(
                "provider_surface.openai.subprocess_cli",
                SubprocessCliAuthStatus::Unauthenticated,
            ),
            probed(
                "provider_surface.anthropic.subprocess_cli",
                SubprocessCliAuthStatus::Authenticated,
            ),
        ];

        let resolved = select_cli_surface_for_config(&config, &surfaces);
        assert!(resolved.is_none());
    }

    #[test]
    fn prefers_explicit_surface_id_when_provider_type_is_generic() {
        let mut llm_endpoint = endpoint(AiProviderType::Generic, None);
        llm_endpoint.surface_id = Some("provider_surface.anthropic.subprocess_cli".to_string());
        let config = AiProviderConfig {
            llm_api: Some(llm_endpoint),
            ..AiProviderConfig::default()
        };
        let surfaces = vec![
            probed(
                "provider_surface.openai.subprocess_cli",
                SubprocessCliAuthStatus::Authenticated,
            ),
            probed(
                "provider_surface.anthropic.subprocess_cli",
                SubprocessCliAuthStatus::Authenticated,
            ),
        ];

        let resolved = select_cli_surface_for_config(&config, &surfaces).unwrap();
        assert_eq!(
            resolved.surface_id,
            "provider_surface.anthropic.subprocess_cli"
        );
    }

    #[test]
    fn ignores_explicit_surface_id_when_it_conflicts_with_provider_type() {
        let mut llm_endpoint = endpoint(AiProviderType::OpenAi, None);
        llm_endpoint.surface_id = Some("provider_surface.anthropic.subprocess_cli".to_string());
        let config = AiProviderConfig {
            llm_api: Some(llm_endpoint),
            ..AiProviderConfig::default()
        };
        let surfaces = vec![
            probed(
                "provider_surface.openai.subprocess_cli",
                SubprocessCliAuthStatus::Authenticated,
            ),
            probed(
                "provider_surface.anthropic.subprocess_cli",
                SubprocessCliAuthStatus::Authenticated,
            ),
        ];

        let resolved = select_cli_surface_for_config(&config, &surfaces).unwrap();
        assert_eq!(
            resolved.surface_id,
            "provider_surface.openai.subprocess_cli"
        );
    }
}
