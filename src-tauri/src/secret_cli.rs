//! CLI secret projection helpers.
//!
//! Explicit local-user surface for exporting provider credentials into
//! process-scoped environment variables for CLI compatibility.

use std::path::Path;

use oneshim_core::config::{CredentialAuthMode, CredentialBackendKind, ExternalApiEndpoint};
use oneshim_core::config_manager::ConfigManager;
use oneshim_core::ports::credential_source::CredentialSource;
use oneshim_core::ports::secret_projection::{
    provider_api_key_temp_file_consumer_id, ProjectionPurpose, SecretProjectionPort,
    SecretProjectionRequest, SecretProjectionResult,
};
use oneshim_core::provider_surface::provider_vendor_id_or_default;
use oneshim_storage::process_env_projection::{
    provider_api_key_cli_template, ProcessEnvSecretProjection,
};
use oneshim_storage::temp_file_projection::{
    provider_api_key_temp_file_template, TempFileSecretProjection,
};
use tempfile::Builder;

use crate::provider_secret_backend::{create_os_secret_store, create_secret_store_for_binding};

const CONFIG_FILE_NAME: &str = "config.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SecretSurface {
    Llm,
    Ocr,
}

impl SecretSurface {
    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "llm" => Some(Self::Llm),
            "ocr" => Some(Self::Ocr),
            _ => None,
        }
    }

    fn profile_id(self) -> &'static str {
        match self {
            Self::Llm => "llm",
            Self::Ocr => "ocr",
        }
    }
}

pub fn run(args: &[String], config_dir: &Path) -> i32 {
    match args.first().map(String::as_str) {
        Some("status") => cmd_status(&args[1..], config_dir),
        Some("env") => cmd_env(&args[1..], config_dir),
        Some("file") => cmd_file(&args[1..], config_dir),
        Some("exec") => cmd_exec(&args[1..], config_dir),
        _ => {
            eprintln!("Usage: oneshim secret <status|env|file|exec> ...");
            eprintln!();
            eprintln!("Commands:");
            eprintln!(
                "  status <llm|ocr>    Show credential backend, projection, and resolution state"
            );
            eprintln!(
                "  env <llm|ocr>       Emit shell export lines for the configured provider API key"
            );
            eprintln!(
                "  file <llm|ocr>      Materialize a temp file containing the configured provider API key"
            );
            eprintln!(
                "  exec <llm|ocr> -- <command...>    Run a child command with projected provider credentials"
            );
            1
        }
    }
}

fn cmd_status(args: &[String], config_dir: &Path) -> i32 {
    let Some(surface) = args.first().and_then(|value| SecretSurface::parse(value)) else {
        eprintln!("Usage: oneshim secret status <llm|ocr>");
        return 1;
    };

    let config_manager = match ConfigManager::with_path(config_dir.join(CONFIG_FILE_NAME)) {
        Ok(manager) => manager,
        Err(err) => {
            eprintln!("Error: failed to load config: {err}");
            return 1;
        }
    };
    let config = config_manager.get();
    let Some(endpoint) = endpoint_for_surface(&config.ai_provider, surface) else {
        eprintln!(
            "Error: no {} provider endpoint is configured. Save a remote provider in Settings first.",
            surface.profile_id()
        );
        return 1;
    };

    match inspect_surface_status(endpoint, surface, config_dir) {
        Ok(status) => {
            println!("{}", format_surface_status(surface, &status));
            0
        }
        Err(err) => {
            eprintln!("Error: {err}");
            1
        }
    }
}

fn cmd_env(args: &[String], config_dir: &Path) -> i32 {
    let Some(surface) = args.first().and_then(|value| SecretSurface::parse(value)) else {
        eprintln!("Usage: oneshim secret env <llm|ocr>");
        return 1;
    };

    let config_manager = match ConfigManager::with_path(config_dir.join(CONFIG_FILE_NAME)) {
        Ok(manager) => manager,
        Err(err) => {
            eprintln!("Error: failed to load config: {err}");
            return 1;
        }
    };
    let config = config_manager.get();
    let Some(endpoint) = endpoint_for_surface(&config.ai_provider, surface) else {
        eprintln!(
            "Error: no {} provider endpoint is configured. Save a remote provider in Settings first.",
            surface.profile_id()
        );
        return 1;
    };

    if let Err(message) = ensure_projection_allowed(endpoint, surface) {
        eprintln!("Error: {message}");
        return 1;
    }

    let env_vars = match resolve_env_projection(endpoint, surface, config_dir) {
        Ok(env_vars) => env_vars,
        Err(err) => {
            eprintln!("Error: {err}");
            return 1;
        }
    };

    for (name, value) in env_vars {
        println!("export {name}={}", shell_quote(&value));
    }

    0
}

fn cmd_file(args: &[String], config_dir: &Path) -> i32 {
    let Some(surface) = args.first().and_then(|value| SecretSurface::parse(value)) else {
        eprintln!("Usage: oneshim secret file <llm|ocr>");
        return 1;
    };

    let config_manager = match ConfigManager::with_path(config_dir.join(CONFIG_FILE_NAME)) {
        Ok(manager) => manager,
        Err(err) => {
            eprintln!("Error: failed to load config: {err}");
            return 1;
        }
    };
    let config = config_manager.get();
    let Some(endpoint) = endpoint_for_surface(&config.ai_provider, surface) else {
        eprintln!(
            "Error: no {} provider endpoint is configured. Save a remote provider in Settings first.",
            surface.profile_id()
        );
        return 1;
    };

    if let Err(message) = ensure_projection_allowed(endpoint, surface) {
        eprintln!("Error: {message}");
        return 1;
    }

    match resolve_temp_file_projection(endpoint, surface, config_dir) {
        Ok(path) => {
            println!("{}", path.display());
            0
        }
        Err(err) => {
            eprintln!("Error: {err}");
            1
        }
    }
}

fn cmd_exec(args: &[String], config_dir: &Path) -> i32 {
    let Ok((surface, command)) = parse_exec_args(args) else {
        eprintln!("Usage: oneshim secret exec <llm|ocr> -- <command...>");
        return 1;
    };

    let config_manager = match ConfigManager::with_path(config_dir.join(CONFIG_FILE_NAME)) {
        Ok(manager) => manager,
        Err(err) => {
            eprintln!("Error: failed to load config: {err}");
            return 1;
        }
    };
    let config = config_manager.get();
    let Some(endpoint) = endpoint_for_surface(&config.ai_provider, surface) else {
        eprintln!(
            "Error: no {} provider endpoint is configured. Save a remote provider in Settings first.",
            surface.profile_id()
        );
        return 1;
    };

    if let Err(message) = ensure_projection_allowed(endpoint, surface) {
        eprintln!("Error: {message}");
        return 1;
    }

    let env_vars = match resolve_env_projection(endpoint, surface, config_dir) {
        Ok(env_vars) => env_vars,
        Err(err) => {
            eprintln!("Error: {err}");
            return 1;
        }
    };

    let mut child = std::process::Command::new(&command[0]);
    child.args(&command[1..]);
    child.envs(env_vars);

    match child.status() {
        Ok(status) => status.code().unwrap_or(1),
        Err(err) => {
            eprintln!("Error: failed to launch '{}': {err}", command[0]);
            1
        }
    }
}

fn endpoint_for_surface(
    config: &oneshim_core::config::AiProviderConfig,
    surface: SecretSurface,
) -> Option<&ExternalApiEndpoint> {
    match surface {
        SecretSurface::Llm => config.llm_api.as_ref(),
        SecretSurface::Ocr => config.ocr_api.as_ref(),
    }
}

fn resolve_env_projection(
    endpoint: &ExternalApiEndpoint,
    surface: SecretSurface,
    config_dir: &Path,
) -> Result<Vec<(String, String)>, String> {
    let template =
        provider_api_key_cli_template(endpoint.provider_type).map_err(|err| err.to_string())?;
    let desktop_secret_store = create_os_secret_store(config_dir);
    let secret_store = create_secret_store_for_binding(
        endpoint.credential.as_ref(),
        config_dir,
        desktop_secret_store,
    )
    .map_err(|err| err.to_string())?;
    let runtime = build_runtime()?;

    if let (Some(secret_store), Some(secret_ref)) = (
        secret_store.clone(),
        endpoint
            .credential
            .as_ref()
            .and_then(|binding| binding.secret_ref.clone()),
    ) {
        let projection =
            ProcessEnvSecretProjection::with_default_provider_api_key_cli_templates(secret_store);
        let request = SecretProjectionRequest {
            namespace: secret_ref.namespace,
            key: secret_ref.key,
            target: oneshim_core::ports::secret_projection::ProjectionTarget::ProcessEnv,
            purpose: ProjectionPurpose::ProviderCliExecution,
            consumer_id: template.consumer_id.clone(),
        };

        if let Ok(SecretProjectionResult::EnvVars(envs)) =
            runtime.block_on(projection.project(request))
        {
            return Ok(envs);
        }
    }

    let source = CredentialSource::from_api_key_endpoint_for_profile(
        endpoint,
        Some(surface.profile_id()),
        secret_store,
    )
    .map_err(|err| err.to_string())?;

    let resolved = runtime
        .block_on(source.resolve_bearer_token())
        .map_err(|err| err.to_string())?;

    Ok(template
        .env_names
        .into_iter()
        .map(|name| (name, resolved.clone()))
        .collect())
}

fn resolve_temp_file_projection(
    endpoint: &ExternalApiEndpoint,
    surface: SecretSurface,
    config_dir: &Path,
) -> Result<std::path::PathBuf, String> {
    let template =
        provider_api_key_temp_file_template(endpoint.provider_type, surface.profile_id())
            .map_err(|err| err.to_string())?;
    let desktop_secret_store = create_os_secret_store(config_dir);
    let secret_store = create_secret_store_for_binding(
        endpoint.credential.as_ref(),
        config_dir,
        desktop_secret_store,
    )
    .map_err(|err| err.to_string())?;
    let runtime = build_runtime()?;

    if let Some(binding) = endpoint.credential.as_ref() {
        match binding.backend_kind {
            CredentialBackendKind::OsSecretStore | CredentialBackendKind::FileSecretStore => {
                let Some(secret_store) = secret_store.clone() else {
                    return Err(format!(
                        "configured secret backend {:?} is unavailable for {} projection",
                        binding.backend_kind,
                        surface.profile_id()
                    ));
                };
                let Some(secret_ref) = binding.secret_ref.as_ref() else {
                    return Err(format!(
                        "{} secret binding is missing a secret reference",
                        surface.profile_id()
                    ));
                };
                let request = SecretProjectionRequest {
                    namespace: secret_ref.namespace.clone(),
                    key: secret_ref.key.clone(),
                    target: oneshim_core::ports::secret_projection::ProjectionTarget::TempFile,
                    purpose: ProjectionPurpose::ProviderCliExecution,
                    consumer_id: template.consumer_id.clone(),
                };
                let projection =
                    TempFileSecretProjection::with_default_provider_api_key_cli_templates(
                        secret_store,
                        config_dir,
                    );
                return match runtime.block_on(projection.project(request)) {
                    Ok(SecretProjectionResult::TempFile { path, .. }) => Ok(path),
                    Ok(_) => Err("temp-file projection returned an unexpected result".to_string()),
                    Err(err) => Err(format!(
                        "failed to project managed temp file for {}: {err}",
                        surface.profile_id()
                    )),
                };
            }
            CredentialBackendKind::Env => {
                let Some(secret_store) = secret_store.clone() else {
                    return Err(format!(
                        "environment-backed secret store is unavailable for {} projection",
                        surface.profile_id()
                    ));
                };
                let request = SecretProjectionRequest::provider_api_key_temp_file(
                    provider_vendor_id_or_default(endpoint.provider_type),
                    surface.profile_id(),
                    provider_api_key_temp_file_consumer_id(
                        provider_vendor_id_or_default(endpoint.provider_type),
                        surface.profile_id(),
                    )
                    .map_err(|err| err.to_string())?,
                )
                .map_err(|err| err.to_string())?;
                let projection =
                    TempFileSecretProjection::with_default_provider_api_key_cli_templates(
                        secret_store,
                        config_dir,
                    );
                return match runtime.block_on(projection.project(request)) {
                    Ok(SecretProjectionResult::TempFile { path, .. }) => Ok(path),
                    Ok(_) => Err("temp-file projection returned an unexpected result".to_string()),
                    Err(err) => Err(format!(
                        "failed to project environment-backed temp file for {}: {err}",
                        surface.profile_id()
                    )),
                };
            }
            CredentialBackendKind::BridgeManaged => {
                return Err(
                    "bridge-managed provider credentials do not support temp-file projection yet"
                        .to_string(),
                );
            }
            CredentialBackendKind::Unavailable => {}
        }
    }

    let source = CredentialSource::from_api_key_endpoint_for_profile(
        endpoint,
        Some(surface.profile_id()),
        secret_store,
    )
    .map_err(|err| err.to_string())?;
    let resolved = runtime
        .block_on(source.resolve_bearer_token())
        .map_err(|err| err.to_string())?;

    materialize_plaintext_temp_file(
        &resolved,
        template
            .file_name_hint
            .as_deref()
            .unwrap_or("provider-api-key"),
    )
}

fn materialize_plaintext_temp_file(
    secret: &str,
    file_name_hint: &str,
) -> Result<std::path::PathBuf, String> {
    let prefix = format!(
        "oneshim-{}-",
        file_name_hint
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() {
                    ch.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .trim_matches('-')
    );

    let temp_file = Builder::new()
        .prefix(&prefix)
        .suffix(".secret")
        .tempfile()
        .map_err(|err| format!("failed to create temp secret file: {err}"))?;
    std::fs::write(temp_file.path(), secret)
        .map_err(|err| format!("failed to write temp secret file: {err}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        std::fs::set_permissions(temp_file.path(), std::fs::Permissions::from_mode(0o600))
            .map_err(|err| format!("failed to secure temp secret file: {err}"))?;
    }

    temp_file
        .keep()
        .map(|value| value.1)
        .map_err(|err| format!("failed to persist temp secret file: {}", err.error))
}

fn ensure_projection_allowed(
    endpoint: &ExternalApiEndpoint,
    surface: SecretSurface,
) -> Result<(), String> {
    let Some(binding) = endpoint.credential.as_ref() else {
        return Ok(());
    };

    if binding.auth_mode != CredentialAuthMode::ApiKey {
        return Err(format!(
            "{} is not configured for API-key projection. Current auth mode: {:?}",
            surface.profile_id(),
            binding.auth_mode
        ));
    }

    if matches!(
        binding.backend_kind,
        CredentialBackendKind::OsSecretStore | CredentialBackendKind::FileSecretStore
    ) && !binding.projection_enabled
    {
        return Err(format!(
            "{} secret projection is disabled. Enable CLI projection in Settings first.",
            surface.profile_id()
        ));
    }

    if binding.backend_kind == CredentialBackendKind::BridgeManaged {
        return Err(format!(
            "{} secret projection is not available for bridge-managed credentials yet.",
            surface.profile_id()
        ));
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SecretSurfaceStatus {
    provider_type: String,
    auth_mode: String,
    backend_kind: String,
    projection_enabled: bool,
    plaintext_present: bool,
    secret_ref_present: bool,
    resolved_secret_available: bool,
}

fn inspect_surface_status(
    endpoint: &ExternalApiEndpoint,
    surface: SecretSurface,
    config_dir: &Path,
) -> Result<SecretSurfaceStatus, String> {
    let binding = endpoint.credential.as_ref();
    let auth_mode = binding
        .map(|value| value.auth_mode)
        .unwrap_or(CredentialAuthMode::ApiKey);
    let backend_kind = binding
        .map(|value| value.backend_kind)
        .unwrap_or(CredentialBackendKind::Unavailable);
    let projection_enabled = binding
        .map(|value| value.projection_enabled)
        .unwrap_or(false);
    let secret_ref_present = binding
        .and_then(|value| value.secret_ref.as_ref())
        .is_some();
    let plaintext_present = !endpoint.api_key.trim().is_empty();

    let desktop_secret_store = create_os_secret_store(config_dir);
    let secret_store = create_secret_store_for_binding(binding, config_dir, desktop_secret_store)
        .map_err(|err| err.to_string())?;
    let runtime = build_runtime()?;
    let resolved_secret_available = match CredentialSource::from_api_key_endpoint_for_profile(
        endpoint,
        Some(surface.profile_id()),
        secret_store,
    )
    .map_err(|err| err.to_string())
    {
        Ok(source) => runtime.block_on(source.resolve_bearer_token()).is_ok(),
        Err(_) => false,
    };

    Ok(SecretSurfaceStatus {
        provider_type: format!("{:?}", endpoint.provider_type),
        auth_mode: format!("{auth_mode:?}"),
        backend_kind: format!("{backend_kind:?}"),
        projection_enabled,
        plaintext_present,
        secret_ref_present,
        resolved_secret_available,
    })
}

fn format_surface_status(surface: SecretSurface, status: &SecretSurfaceStatus) -> String {
    format!(
        "surface={}\nprovider_type={}\nauth_mode={}\nbackend_kind={}\nprojection_enabled={}\nplaintext_present={}\nsecret_ref_present={}\nresolved_secret_available={}",
        surface.profile_id(),
        status.provider_type,
        status.auth_mode,
        status.backend_kind,
        status.projection_enabled,
        status.plaintext_present,
        status.secret_ref_present,
        status.resolved_secret_available,
    )
}

fn build_runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| format!("failed to build CLI runtime: {err}"))
}

fn shell_quote(value: &str) -> String {
    let escaped = value.replace('\'', r#"'\"'\"'"#);
    format!("'{escaped}'")
}

fn parse_exec_args(args: &[String]) -> Result<(SecretSurface, &[String]), ()> {
    let Some(surface) = args.first().and_then(|value| SecretSurface::parse(value)) else {
        return Err(());
    };

    if args.get(1).map(String::as_str) != Some("--") {
        return Err(());
    }

    let command = &args[2..];
    if command.is_empty() {
        return Err(());
    }

    Ok((surface, command))
}

#[cfg(test)]
mod tests {
    use super::*;
    use oneshim_core::config::{
        AiProviderConfig, AiProviderType, CredentialAuthMode, CredentialBackendKind,
        CredentialBinding,
    };
    use tempfile::TempDir;

    #[test]
    fn surface_parser_accepts_llm_and_ocr() {
        assert_eq!(SecretSurface::parse("llm"), Some(SecretSurface::Llm));
        assert_eq!(SecretSurface::parse("ocr"), Some(SecretSurface::Ocr));
        assert_eq!(SecretSurface::parse("other"), None);
    }

    #[test]
    fn endpoint_for_surface_returns_expected_endpoint() {
        let endpoint = ExternalApiEndpoint {
            endpoint: "https://api.example.com/v1".to_string(),
            api_key: "sk-test".to_string(),
            model: Some("gpt-5.4".to_string()),
            timeout_secs: 30,
            provider_type: AiProviderType::OpenAi,
            surface_id: None,
            credential: None,
        };
        let config = AiProviderConfig {
            llm_api: Some(endpoint.clone()),
            ocr_api: Some(endpoint),
            ..AiProviderConfig::default()
        };

        assert!(endpoint_for_surface(&config, SecretSurface::Llm).is_some());
        assert!(endpoint_for_surface(&config, SecretSurface::Ocr).is_some());
    }

    #[test]
    fn shell_quote_escapes_single_quotes() {
        assert_eq!(shell_quote("sk-test"), "'sk-test'");
        assert_eq!(shell_quote("a'b"), "'a'\\\"'\\\"'b'");
    }

    #[test]
    fn materialize_plaintext_temp_file_writes_secret() {
        let path = materialize_plaintext_temp_file("sk-temp", "openai-api-key").unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "sk-temp");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn parse_exec_args_requires_surface_separator_and_command() {
        let args = vec!["llm".to_string(), "--".to_string(), "codex".to_string()];
        let (surface, command) = parse_exec_args(&args).unwrap();
        assert_eq!(surface, SecretSurface::Llm);
        assert_eq!(command, &["codex".to_string()]);

        assert!(parse_exec_args(&["llm".to_string()]).is_err());
        assert!(parse_exec_args(&["llm".to_string(), "codex".to_string()]).is_err());
        assert!(
            parse_exec_args(&["other".to_string(), "--".to_string(), "codex".to_string()]).is_err()
        );
    }

    #[test]
    fn config_manager_uses_explicit_config_dir() {
        let temp_dir = TempDir::new().unwrap();
        let manager = ConfigManager::with_path(temp_dir.path().join(CONFIG_FILE_NAME)).unwrap();

        assert_eq!(
            manager.config_path(),
            &temp_dir.path().join(CONFIG_FILE_NAME)
        );
    }

    #[test]
    fn file_secret_store_path_is_config_relative() {
        let temp_dir = TempDir::new().unwrap();
        let binding = CredentialBinding {
            auth_mode: CredentialAuthMode::ApiKey,
            backend_kind: CredentialBackendKind::FileSecretStore,
            secret_ref: None,
            projection_enabled: false,
        };

        let store = create_secret_store_for_binding(Some(&binding), temp_dir.path(), None).unwrap();
        assert!(store.is_some());
    }

    #[test]
    fn ensure_projection_allowed_rejects_disabled_backend_managed_projection() {
        let endpoint = ExternalApiEndpoint {
            endpoint: "https://api.openai.com/v1".to_string(),
            api_key: String::new(),
            model: Some("gpt-5.4".to_string()),
            timeout_secs: 30,
            provider_type: AiProviderType::OpenAi,
            surface_id: None,
            credential: Some(CredentialBinding {
                auth_mode: CredentialAuthMode::ApiKey,
                backend_kind: CredentialBackendKind::OsSecretStore,
                secret_ref: None,
                projection_enabled: false,
            }),
        };

        let error = ensure_projection_allowed(&endpoint, SecretSurface::Llm).unwrap_err();
        assert!(error.contains("Enable CLI projection"));
    }

    #[test]
    fn ensure_projection_allowed_accepts_enabled_backend_managed_projection() {
        let endpoint = ExternalApiEndpoint {
            endpoint: "https://api.openai.com/v1".to_string(),
            api_key: String::new(),
            model: Some("gpt-5.4".to_string()),
            timeout_secs: 30,
            provider_type: AiProviderType::OpenAi,
            surface_id: None,
            credential: Some(CredentialBinding {
                auth_mode: CredentialAuthMode::ApiKey,
                backend_kind: CredentialBackendKind::FileSecretStore,
                secret_ref: None,
                projection_enabled: true,
            }),
        };

        assert!(ensure_projection_allowed(&endpoint, SecretSurface::Llm).is_ok());
    }

    #[test]
    fn ensure_projection_allowed_rejects_bridge_managed_projection() {
        let endpoint = ExternalApiEndpoint {
            endpoint: "https://api.openai.com/v1".to_string(),
            api_key: String::new(),
            model: Some("gpt-5.4".to_string()),
            timeout_secs: 30,
            provider_type: AiProviderType::OpenAi,
            surface_id: None,
            credential: Some(CredentialBinding {
                auth_mode: CredentialAuthMode::ApiKey,
                backend_kind: CredentialBackendKind::BridgeManaged,
                secret_ref: None,
                projection_enabled: true,
            }),
        };

        let error = ensure_projection_allowed(&endpoint, SecretSurface::Llm).unwrap_err();
        assert!(error.contains("bridge-managed credentials"));
    }

    #[test]
    fn format_surface_status_renders_expected_fields() {
        let status = SecretSurfaceStatus {
            provider_type: "OpenAi".to_string(),
            auth_mode: "ApiKey".to_string(),
            backend_kind: "OsSecretStore".to_string(),
            projection_enabled: true,
            plaintext_present: false,
            secret_ref_present: true,
            resolved_secret_available: true,
        };

        let rendered = format_surface_status(SecretSurface::Llm, &status);
        assert!(rendered.contains("surface=llm"));
        assert!(rendered.contains("projection_enabled=true"));
        assert!(rendered.contains("resolved_secret_available=true"));
    }

    #[test]
    fn ensure_projection_allowed_accepts_unbound_plaintext_status_only() {
        let endpoint = ExternalApiEndpoint {
            endpoint: "https://api.openai.com/v1".to_string(),
            api_key: "sk-test".to_string(),
            model: Some("gpt-5.4".to_string()),
            timeout_secs: 30,
            provider_type: AiProviderType::OpenAi,
            surface_id: None,
            credential: None,
        };

        assert!(ensure_projection_allowed(&endpoint, SecretSurface::Llm).is_ok());
    }
}
