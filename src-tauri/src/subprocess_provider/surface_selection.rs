use maekon_api_contracts::provider_specs::{
    list_subprocess_surface_specs, provider_surface_spec as catalog_surface_spec,
    SurfaceCapabilityKind,
};
use maekon_core::config::AiProviderConfig;
use maekon_core::config::AiProviderType;
use maekon_core::provider_surface::provider_type_from_vendor_id;
use std::path::{Path, PathBuf};

use super::auth_probe::probe_cli_surface;
use super::runtime::runtime_ready_for_auth_status;
use super::{
    catalog_subprocess_transport, find_executable, DetectedSubprocessCli, ProbedSubprocessCli,
    SubprocessCliDependencyStatus, SubprocessCliDiscoveryReport, SubprocessCliVersionStatus,
};

const CLAUDE_SUBPROCESS_SURFACE_ID: &str = "provider_surface.anthropic.subprocess_cli";
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const CLAUDE_CODE_GIT_BASH_ENV: &str = "CLAUDE_CODE_GIT_BASH_PATH";

pub fn detect_known_cli_surfaces() -> Vec<DetectedSubprocessCli> {
    list_subprocess_surface_specs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|surface| {
            let transport = catalog_subprocess_transport(&surface.surface_id).ok()?;
            let candidates = transport
                .executable_candidates
                .iter()
                .filter_map(|candidate| {
                    find_executable(candidate).map(|executable_path| {
                        let discovery = cli_discovery_report_for_candidate(
                            &surface.surface_id,
                            candidate,
                            &executable_path,
                        );
                        DetectedCliCandidate {
                            surface_id: surface.surface_id.clone(),
                            candidate_name: candidate.clone(),
                            executable_path,
                            dependency_status: discovery.dependency_status,
                        }
                    })
                })
                .collect();
            choose_cli_candidate(candidates)
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

pub(crate) fn cli_discovery_report_for_detected(
    detected: &DetectedSubprocessCli,
) -> SubprocessCliDiscoveryReport {
    let candidate_name = detected
        .executable_path
        .file_stem()
        .and_then(|value| value.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| "unknown".to_string());
    cli_discovery_report_for_candidate(
        &detected.surface_id,
        &candidate_name,
        &detected.executable_path,
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DetectedCliCandidate {
    surface_id: String,
    candidate_name: String,
    executable_path: PathBuf,
    dependency_status: SubprocessCliDependencyStatus,
}

fn choose_cli_candidate(candidates: Vec<DetectedCliCandidate>) -> Option<DetectedSubprocessCli> {
    let selected = candidates
        .iter()
        .find(|candidate| dependency_status_allows_invocation(candidate.dependency_status))
        .or_else(|| candidates.first())?;
    Some(DetectedSubprocessCli {
        surface_id: selected.surface_id.clone(),
        executable_path: selected.executable_path.clone(),
    })
}

fn dependency_status_allows_invocation(status: SubprocessCliDependencyStatus) -> bool {
    matches!(
        status,
        SubprocessCliDependencyStatus::Ready | SubprocessCliDependencyStatus::NotRequired
    )
}

fn cli_discovery_report_for_candidate(
    surface_id: &str,
    candidate_name: &str,
    executable_path: &Path,
) -> SubprocessCliDiscoveryReport {
    let dependency = dependency_report_for_surface(surface_id);
    SubprocessCliDiscoveryReport {
        candidate_name: candidate_name.to_string(),
        executable_path: executable_path.display().to_string(),
        executable_hint: executable_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or(candidate_name)
            .to_string(),
        version_status: SubprocessCliVersionStatus::NotChecked,
        dependency_status: dependency.dependency_status,
        status_reason: dependency.status_reason,
        env_refresh_required: dependency.env_refresh_required,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CliDependencyReport {
    dependency_status: SubprocessCliDependencyStatus,
    status_reason: Option<String>,
    env_refresh_required: bool,
}

fn dependency_report_for_surface(surface_id: &str) -> CliDependencyReport {
    if surface_id != CLAUDE_SUBPROCESS_SURFACE_ID {
        return CliDependencyReport {
            dependency_status: SubprocessCliDependencyStatus::NotRequired,
            status_reason: None,
            env_refresh_required: false,
        };
    }

    #[cfg(windows)]
    {
        evaluate_windows_claude_dependency_from_env(
            std::env::var_os(CLAUDE_CODE_GIT_BASH_ENV).map(PathBuf::from),
            read_windows_user_env_path(CLAUDE_CODE_GIT_BASH_ENV),
            windows_claude_shell_dependency_available(),
        )
    }

    #[cfg(not(windows))]
    {
        CliDependencyReport {
            dependency_status: SubprocessCliDependencyStatus::Ready,
            status_reason: Some("claude_code_shell_dependency_not_required".to_string()),
            env_refresh_required: false,
        }
    }
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn evaluate_windows_claude_dependency_from_env(
    process_git_bash_path: Option<PathBuf>,
    user_git_bash_path: Option<PathBuf>,
    shell_dependency_available: bool,
) -> CliDependencyReport {
    if let Some(path) = process_git_bash_path {
        if path.is_file() {
            return CliDependencyReport {
                dependency_status: SubprocessCliDependencyStatus::Ready,
                status_reason: Some("claude_code_git_bash_path_ready".to_string()),
                env_refresh_required: false,
            };
        }
        if user_git_bash_path
            .as_ref()
            .is_some_and(|user_path| user_path.is_file() && user_path != &path)
        {
            return CliDependencyReport {
                dependency_status: SubprocessCliDependencyStatus::StaleProcessEnv,
                status_reason: Some("claude_code_git_bash_path_requires_restart".to_string()),
                env_refresh_required: true,
            };
        }
        return CliDependencyReport {
            dependency_status: SubprocessCliDependencyStatus::Missing,
            status_reason: Some("claude_code_git_bash_path_invalid".to_string()),
            env_refresh_required: false,
        };
    }

    if user_git_bash_path.is_some() {
        return CliDependencyReport {
            dependency_status: SubprocessCliDependencyStatus::StaleProcessEnv,
            status_reason: Some("claude_code_git_bash_path_requires_restart".to_string()),
            env_refresh_required: true,
        };
    }

    if shell_dependency_available {
        return CliDependencyReport {
            dependency_status: SubprocessCliDependencyStatus::Ready,
            status_reason: Some("claude_code_shell_dependency_ready".to_string()),
            env_refresh_required: false,
        };
    }

    CliDependencyReport {
        dependency_status: SubprocessCliDependencyStatus::Missing,
        status_reason: Some("claude_code_shell_dependency_missing".to_string()),
        env_refresh_required: true,
    }
}

#[cfg(windows)]
fn windows_claude_shell_dependency_available() -> bool {
    find_executable("pwsh").is_some()
        || [
            r"C:\Program Files\Git\bin\bash.exe",
            r"C:\Program Files\Git\usr\bin\bash.exe",
            r"C:\Program Files (x86)\Git\bin\bash.exe",
            r"C:\Program Files (x86)\Git\usr\bin\bash.exe",
        ]
        .iter()
        .map(Path::new)
        .any(Path::is_file)
}

#[cfg(windows)]
fn read_windows_user_env_path(name: &str) -> Option<PathBuf> {
    use std::ffi::{OsStr, OsString};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ,
        REG_EXPAND_SZ, REG_SZ,
    };

    fn to_wide(value: &str) -> Vec<u16> {
        OsStr::new(value)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    let subkey_wide = to_wide("Environment");
    let value_wide = to_wide(name);

    // SAFETY: Registry handles are opened read-only and closed before return.
    // UTF-16 key/value buffers stay alive for each call, and the output buffer is sized from
    // RegQueryValueExW's byte count before the second read.
    unsafe {
        let mut hkey: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey_wide.as_ptr(),
            0,
            KEY_READ,
            &mut hkey,
        ) != 0
        {
            return None;
        }

        let mut value_type: u32 = 0;
        let mut byte_len: u32 = 0;
        let size_result = RegQueryValueExW(
            hkey,
            value_wide.as_ptr(),
            std::ptr::null_mut(),
            &mut value_type,
            std::ptr::null_mut(),
            &mut byte_len,
        );
        if size_result != 0 || !matches!(value_type, REG_SZ | REG_EXPAND_SZ) || byte_len < 2 {
            RegCloseKey(hkey);
            return None;
        }

        let mut buffer = vec![0_u16; (byte_len as usize / 2).saturating_add(1)];
        let read_result = RegQueryValueExW(
            hkey,
            value_wide.as_ptr(),
            std::ptr::null_mut(),
            &mut value_type,
            buffer.as_mut_ptr() as *mut u8,
            &mut byte_len,
        );
        RegCloseKey(hkey);
        if read_result != 0 || !matches!(value_type, REG_SZ | REG_EXPAND_SZ) {
            return None;
        }

        let terminator = buffer
            .iter()
            .position(|value| *value == 0)
            .unwrap_or(buffer.len());
        let value = OsString::from_wide(&buffer[..terminator]);
        (!value.is_empty()).then(|| PathBuf::from(value))
    }
}

fn endpoint_for_capability(
    config: &AiProviderConfig,
    capability: SurfaceCapabilityKind,
) -> Option<&maekon_core::config::ExternalApiEndpoint> {
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
    use super::super::{SubprocessCliAuthStatus, SubprocessCliDependencyStatus};
    use super::*;
    use maekon_core::config::AiProviderType;
    use std::path::PathBuf;

    fn endpoint(
        provider_type: AiProviderType,
        model: Option<&str>,
    ) -> maekon_core::config::ExternalApiEndpoint {
        maekon_core::config::ExternalApiEndpoint {
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

    #[test]
    fn chooses_dependency_ready_candidate_over_broken_first_candidate() {
        let selected = choose_cli_candidate(vec![
            DetectedCliCandidate {
                surface_id: "provider_surface.anthropic.subprocess_cli".to_string(),
                candidate_name: "claude".to_string(),
                executable_path: PathBuf::from("C:/fake/claude.exe"),
                dependency_status: SubprocessCliDependencyStatus::Missing,
            },
            DetectedCliCandidate {
                surface_id: "provider_surface.anthropic.subprocess_cli".to_string(),
                candidate_name: "claude-code".to_string(),
                executable_path: PathBuf::from("C:/fake/claude-code.exe"),
                dependency_status: SubprocessCliDependencyStatus::Ready,
            },
        ])
        .expect("working candidate should be selected");

        assert_eq!(
            selected.executable_path,
            PathBuf::from("C:/fake/claude-code.exe")
        );
    }

    #[test]
    fn claude_dependency_marks_user_env_as_stale_when_process_env_is_missing() {
        let report = evaluate_windows_claude_dependency_from_env(
            None,
            Some(PathBuf::from("C:/Program Files/Git/bin/bash.exe")),
            false,
        );

        assert_eq!(
            report.dependency_status,
            SubprocessCliDependencyStatus::StaleProcessEnv
        );
        assert!(report.env_refresh_required);
    }

    #[test]
    fn claude_dependency_marks_user_env_as_stale_when_process_env_is_invalid() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let user_shell = temp_dir.path().join("bash.exe");
        std::fs::write(&user_shell, b"fake shell").expect("fake shell");

        let report = evaluate_windows_claude_dependency_from_env(
            Some(PathBuf::from("C:/missing/bash.exe")),
            Some(user_shell),
            false,
        );

        assert_eq!(
            report.dependency_status,
            SubprocessCliDependencyStatus::StaleProcessEnv
        );
        assert!(report.env_refresh_required);
    }
}
