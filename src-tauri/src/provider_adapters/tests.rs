use std::sync::Arc;

use super::guarded_ocr;
use super::helpers;
use super::llm_resolver::resolve_cli_subscription_llm_provider_with_detected;
use super::ocr_resolver::resolve_cli_subscription_ocr_provider;
use super::*;
use async_trait::async_trait;
use oneshim_automation::audit::AuditLogger;
use oneshim_core::config::{
    AiAccessMode, AiProviderConfig, AiProviderType, CredentialAuthMode, CredentialBackendKind,
    CredentialBinding, ExternalApiEndpoint, ExternalDataPolicy, LlmProviderType, OcrProviderType,
    OcrValidationConfig, PiiFilterLevel, PrivacyConfig, SecretRef,
};
use oneshim_core::consent::{ConsentManager, ConsentPermissions};
use oneshim_core::error::CoreError;
use oneshim_core::models::context::{ProcessInfo, WindowInfo};
use oneshim_core::models::event::ProcessDetail;
use oneshim_core::ports::monitor::ProcessMonitor;
use oneshim_core::ports::oauth::{
    OAuthConnectionStatus, OAuthFlowHandle, OAuthFlowStatus, OAuthPort, RefreshResult,
};
use oneshim_core::ports::ocr_provider::{OcrProvider, OcrResult};
use oneshim_core::ports::secret_store::{
    provider_api_key_secret_ref, secret_env_var_name, SecretStoreSet,
};
use oneshim_storage::env_secret_store::EnvSecretStore;
use tempfile::TempDir;
use tokio::sync::RwLock;

use crate::subprocess_provider::{ProbedSubprocessCli, SubprocessCliAuthStatus};

fn remote_endpoint() -> ExternalApiEndpoint {
    ExternalApiEndpoint {
        endpoint: "https://api.example.com/v1/messages".to_string(),
        api_key: "test-api-key".to_string(),
        model: Some("test-model".to_string()),
        timeout_secs: 5,
        provider_type: AiProviderType::Generic,
        surface_id: None,
        credential: None,
    }
}

fn secret_bound_remote_endpoint(profile_id: &str) -> ExternalApiEndpoint {
    let (namespace, key) = provider_api_key_secret_ref("generic", profile_id).unwrap();
    ExternalApiEndpoint {
        api_key: String::new(),
        credential: Some(CredentialBinding {
            auth_mode: CredentialAuthMode::ApiKey,
            backend_kind: CredentialBackendKind::Env,
            secret_ref: Some(SecretRef {
                namespace,
                key: key.to_string(),
            }),
            projection_enabled: false,
        }),
        ..remote_endpoint()
    }
}

fn remote_secret_stores() -> SecretStoreSet {
    let mut snapshot = std::collections::HashMap::new();
    for profile_id in ["ocr", "llm"] {
        let (namespace, key) = provider_api_key_secret_ref("generic", profile_id).unwrap();
        snapshot.insert(
            secret_env_var_name(&namespace, key),
            "test-api-key".to_string(),
        );
    }
    let secret_store = Arc::new(EnvSecretStore::from_snapshot(snapshot));
    SecretStoreSet {
        os_secret_store: None,
        file_secret_store: None,
        env_secret_store: Some(secret_store),
        default_backend_kind: CredentialBackendKind::Env,
        fallback_backend_kind: CredentialBackendKind::Unavailable,
    }
}

struct StaticProcessMonitor {
    active_window: Option<WindowInfo>,
}

#[async_trait]
impl ProcessMonitor for StaticProcessMonitor {
    async fn get_active_window(&self) -> Result<Option<WindowInfo>, CoreError> {
        Ok(self.active_window.clone())
    }

    async fn get_top_processes(&self, _limit: usize) -> Result<Vec<ProcessInfo>, CoreError> {
        Ok(vec![])
    }

    async fn get_detailed_processes(
        &self,
        _foreground_pid: Option<u32>,
        _top_n: usize,
    ) -> Result<Vec<ProcessDetail>, CoreError> {
        Ok(vec![])
    }
}

fn write_consent(path: &std::path::Path, ocr_permitted: bool) {
    let mut consent_manager = ConsentManager::new(path.to_path_buf());
    if !ocr_permitted {
        return;
    }

    consent_manager
        .grant_consent(
            ConsentPermissions {
                ocr_processing: true,
                screen_capture: true,
                ..Default::default()
            },
            30,
        )
        .expect("Failed to write consent");
}

fn make_external_ocr_guard(
    ocr_permitted: bool,
    active_window: Option<WindowInfo>,
    audit_logger: Option<Arc<RwLock<AuditLogger>>>,
) -> (ExternalOcrPrivacyGuard, TempDir) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let consent_path = temp_dir.path().join("consent.json");
    write_consent(&consent_path, ocr_permitted);

    (
        ExternalOcrPrivacyGuard::new(
            consent_path,
            PiiFilterLevel::Standard,
            ExternalDataPolicy::PiiFilterStandard,
            PrivacyConfig::default(),
            Arc::new(StaticProcessMonitor { active_window }),
            audit_logger,
        ),
        temp_dir,
    )
}

#[test]
fn resolves_local_providers_by_default() {
    let config = AiProviderConfig::default();
    let adapters =
        resolve_ai_provider_adapters(&config, PiiFilterLevel::Standard, None, None, None)
            .expect("Failed to resolve default configuration");

    assert_eq!(adapters.ocr_source, ProviderSource::Local);
    assert_eq!(adapters.llm_source, ProviderSource::Local);
    assert!(adapters.ocr_fallback_reason.is_none());
    assert!(adapters.llm_fallback_reason.is_none());
    assert!(!adapters.ocr.is_external());
    assert!(!adapters.llm.is_external());
    assert_eq!(adapters.ocr.provider_name(), "local-tesseract");
    assert_eq!(adapters.llm.provider_name(), "local-rule-based");
}

#[test]
fn resolves_remote_providers_when_configured() {
    let config = AiProviderConfig {
        ocr_provider: OcrProviderType::Remote,
        llm_provider: LlmProviderType::Remote,
        ocr_api: Some(secret_bound_remote_endpoint("ocr")),
        llm_api: Some(secret_bound_remote_endpoint("llm")),
        fallback_to_local: false,
        ..AiProviderConfig::default()
    };

    let (privacy_guard, _temp_dir) = make_external_ocr_guard(
        true,
        Some(WindowInfo {
            title: "main.rs".to_string(),
            app_name: "Code".to_string(),
            pid: 7,
            bounds: None,
        }),
        None,
    );
    let adapters = resolve_ai_provider_adapters(
        &config,
        PiiFilterLevel::Standard,
        Some(privacy_guard),
        Some(remote_secret_stores()),
        None,
    )
    .expect("Failed to resolve remote configuration");

    assert_eq!(adapters.ocr_source, ProviderSource::Remote);
    assert_eq!(adapters.llm_source, ProviderSource::Remote);
    assert!(adapters.ocr_fallback_reason.is_none());
    assert!(adapters.llm_fallback_reason.is_none());
    assert!(adapters.ocr.is_external());
    assert!(adapters.llm.is_external());
}

#[test]
fn falls_back_to_local_when_remote_config_missing() {
    let config = AiProviderConfig {
        ocr_provider: OcrProviderType::Remote,
        llm_provider: LlmProviderType::Remote,
        ocr_api: None,
        llm_api: None,
        fallback_to_local: true,
        ..AiProviderConfig::default()
    };

    let adapters =
        resolve_ai_provider_adapters(&config, PiiFilterLevel::Standard, None, None, None)
            .expect("Fallback configuration resolution should not fail");

    assert_eq!(adapters.ocr_source, ProviderSource::LocalFallback);
    assert_eq!(adapters.llm_source, ProviderSource::LocalFallback);
    assert!(adapters
        .ocr_fallback_reason
        .as_deref()
        .is_some_and(|reason| reason.contains("ocr_api")));
    assert!(adapters
        .llm_fallback_reason
        .as_deref()
        .is_some_and(|reason| reason.contains("llm_api")));
    assert!(!adapters.ocr.is_external());
    assert!(!adapters.llm.is_external());
}

#[test]
fn returns_error_when_remote_config_missing_and_fallback_disabled() {
    let config = AiProviderConfig {
        ocr_provider: OcrProviderType::Remote,
        llm_provider: LlmProviderType::Local,
        ocr_api: None,
        llm_api: None,
        fallback_to_local: false,
        ..AiProviderConfig::default()
    };

    match resolve_ai_provider_adapters(&config, PiiFilterLevel::Standard, None, None, None) {
        Ok(_) => panic!("Expected an error"),
        // `ocr_api` being None is a required-field-missing condition; maps to
        // ConfigCode::Missing (config.missing wire code). Previously this was
        // ConfigCode::Invalid but that was semantic drift — "must be configured"
        // is Missing, not Invalid. Fixed in iter-89 ConfigCode audit.
        Err(CoreError::Config {
            code: oneshim_core::error_codes::ConfigCode::Missing,
            message: msg,
        }) => assert!(msg.contains("ocr_api")),
        Err(other) => panic!("Unexpected error type: {other}"),
    }
}

#[test]
fn local_mode_forces_local_adapters_even_if_remote_is_requested() {
    let config = AiProviderConfig {
        access_mode: AiAccessMode::LocalModel,
        ocr_provider: OcrProviderType::Remote,
        llm_provider: LlmProviderType::Remote,
        ocr_api: Some(remote_endpoint()),
        llm_api: Some(remote_endpoint()),
        fallback_to_local: false,
        ..AiProviderConfig::default()
    };

    let adapters =
        resolve_ai_provider_adapters(&config, PiiFilterLevel::Standard, None, None, None)
            .expect("Failed to resolve local mode");
    assert_eq!(adapters.ocr_source, ProviderSource::Local);
    assert_eq!(adapters.llm_source, ProviderSource::Local);
    assert!(adapters.ocr_fallback_reason.is_none());
    assert!(adapters.llm_fallback_reason.is_none());
    assert!(!adapters.ocr.is_external());
    assert!(!adapters.llm.is_external());
}

#[test]
fn cli_subscription_mode_marks_cli_source() {
    let config = AiProviderConfig {
        access_mode: AiAccessMode::ProviderSubscriptionCli,
        ..AiProviderConfig::default()
    };

    let (llm, llm_source, llm_fallback_reason) =
        resolve_cli_subscription_llm_provider_with_detected(
            &config,
            &[ProbedSubprocessCli {
                detected: crate::subprocess_provider::DetectedSubprocessCli {
                    surface_id: "provider_surface.openai.subprocess_cli".to_string(),
                    executable_path: "/tmp/codex".into(),
                },
                auth_status: SubprocessCliAuthStatus::Authenticated,
                auth_detail: Some("cli_authenticated".to_string()),
            }],
        )
        .expect("Failed to resolve CLI mode");

    assert_eq!(llm_source, ProviderSource::CliSubscription);
    assert!(llm_fallback_reason.is_none());
    assert_eq!(llm.provider_name(), "subprocess-codex");
    assert!(llm.is_external());

    let adapters =
        resolve_ai_provider_adapters(&config, PiiFilterLevel::Standard, None, None, None)
            .expect("Failed to resolve CLI mode");
    assert_eq!(adapters.ocr_source, ProviderSource::Local);
    assert!(!adapters.ocr.is_external());
    assert!(matches!(
        adapters.llm_source,
        ProviderSource::CliSubscription | ProviderSource::LocalFallback
    ));
}

#[test]
fn cli_subscription_mode_keeps_direct_remote_ocr_when_configured() {
    let config = AiProviderConfig {
        access_mode: AiAccessMode::ProviderSubscriptionCli,
        ocr_provider: OcrProviderType::Remote,
        ocr_api: Some(secret_bound_remote_endpoint("ocr")),
        fallback_to_local: false,
        ..AiProviderConfig::default()
    };

    let (privacy_guard, _temp_dir) = make_external_ocr_guard(
        true,
        Some(WindowInfo {
            title: "capture.png".to_string(),
            app_name: "Preview".to_string(),
            pid: 11,
            bounds: None,
        }),
        None,
    );

    let (ocr, ocr_source, ocr_fallback_reason) = resolve_cli_subscription_ocr_provider(
        &config,
        PiiFilterLevel::Standard,
        Some(privacy_guard),
        Some(remote_secret_stores()),
        &[],
    )
    .expect("CLI mode should allow direct remote OCR");

    assert_eq!(ocr_source, ProviderSource::Remote);
    assert!(ocr_fallback_reason.is_none());
    assert!(ocr.is_external());
}

#[test]
fn cli_subscription_mode_uses_subprocess_ocr_when_supported() {
    let config = AiProviderConfig {
        access_mode: AiAccessMode::ProviderSubscriptionCli,
        ocr_provider: OcrProviderType::Remote,
        ocr_api: Some(ExternalApiEndpoint {
            endpoint: String::new(),
            api_key: String::new(),
            model: None,
            timeout_secs: 30,
            provider_type: AiProviderType::OpenAi,
            surface_id: Some("provider_surface.openai.subprocess_cli".to_string()),
            credential: None,
        }),
        fallback_to_local: false,
        ..AiProviderConfig::default()
    };

    let (ocr, ocr_source, ocr_fallback_reason) = resolve_cli_subscription_ocr_provider(
        &config,
        PiiFilterLevel::Standard,
        None,
        None,
        &[ProbedSubprocessCli {
            detected: crate::subprocess_provider::DetectedSubprocessCli {
                surface_id: "provider_surface.openai.subprocess_cli".to_string(),
                executable_path: "/tmp/codex".into(),
            },
            auth_status: SubprocessCliAuthStatus::Authenticated,
            auth_detail: Some("cli_authenticated".to_string()),
        }],
    )
    .expect("expected OCR subprocess runtime to resolve");

    assert_eq!(ocr_source, ProviderSource::CliSubscription);
    assert!(ocr_fallback_reason.is_none());
    assert!(ocr.is_external());
    assert_eq!(ocr.provider_name(), "subprocess-codex");
}

#[test]
fn cli_subscription_mode_falls_back_to_local_when_no_supported_cli_runtime_exists() {
    let config = AiProviderConfig {
        access_mode: AiAccessMode::ProviderSubscriptionCli,
        fallback_to_local: true,
        ..AiProviderConfig::default()
    };

    let (llm, llm_source, llm_fallback_reason) =
        resolve_cli_subscription_llm_provider_with_detected(&config, &[])
            .expect("CLI mode should fall back to local LLM");

    assert_eq!(llm_source, ProviderSource::LocalFallback);
    assert!(llm_fallback_reason
        .as_deref()
        .is_some_and(|reason| reason.contains("No supported provider CLI runtime")));
    assert_eq!(llm.provider_name(), "local-rule-based");
    assert!(!llm.is_external());
}

#[test]
fn cli_subscription_mode_prefers_matching_provider_surface() {
    let config = AiProviderConfig {
        access_mode: AiAccessMode::ProviderSubscriptionCli,
        llm_api: Some(ExternalApiEndpoint {
            provider_type: AiProviderType::Anthropic,
            ..remote_endpoint()
        }),
        ..AiProviderConfig::default()
    };

    let (llm, llm_source, llm_fallback_reason) =
        resolve_cli_subscription_llm_provider_with_detected(
            &config,
            &[
                ProbedSubprocessCli {
                    detected: crate::subprocess_provider::DetectedSubprocessCli {
                        surface_id: "provider_surface.openai.subprocess_cli".to_string(),
                        executable_path: "/tmp/codex".into(),
                    },
                    auth_status: SubprocessCliAuthStatus::Authenticated,
                    auth_detail: Some("cli_authenticated".to_string()),
                },
                ProbedSubprocessCli {
                    detected: crate::subprocess_provider::DetectedSubprocessCli {
                        surface_id: "provider_surface.anthropic.subprocess_cli".to_string(),
                        executable_path: "/tmp/claude".into(),
                    },
                    auth_status: SubprocessCliAuthStatus::Authenticated,
                    auth_detail: Some("cli_authenticated".to_string()),
                },
            ],
        )
        .expect("CLI mode should resolve the Anthropic surface");

    assert_eq!(llm_source, ProviderSource::CliSubscription);
    assert!(llm_fallback_reason.is_none());
    assert_eq!(llm.provider_name(), "subprocess-claude-code");
}

#[test]
fn cli_subscription_mode_reports_auth_required_when_matching_cli_is_logged_out() {
    let config = AiProviderConfig {
        access_mode: AiAccessMode::ProviderSubscriptionCli,
        llm_api: Some(ExternalApiEndpoint {
            provider_type: AiProviderType::OpenAi,
            ..remote_endpoint()
        }),
        fallback_to_local: false,
        ..AiProviderConfig::default()
    };

    match resolve_cli_subscription_llm_provider_with_detected(
        &config,
        &[ProbedSubprocessCli {
            detected: crate::subprocess_provider::DetectedSubprocessCli {
                surface_id: "provider_surface.openai.subprocess_cli".to_string(),
                executable_path: "/tmp/codex".into(),
            },
            auth_status: SubprocessCliAuthStatus::Unauthenticated,
            auth_detail: Some("cli_auth_required".to_string()),
        }],
    ) {
        Err(CoreError::Config {
            code: oneshim_core::error_codes::ConfigCode::Invalid,
            message,
        }) => {
            assert!(message.contains("not authenticated"));
            assert!(message.contains("codex"));
        }
        Ok(_) => panic!("Expected an authentication error"),
        Err(other) => panic!("Unexpected error: {other}"),
    }
}

#[test]
fn cli_subscription_mode_accepts_unknown_auth_for_probe_less_surface() {
    let config = AiProviderConfig {
        access_mode: AiAccessMode::ProviderSubscriptionCli,
        llm_api: Some(ExternalApiEndpoint {
            provider_type: AiProviderType::Google,
            ..remote_endpoint()
        }),
        fallback_to_local: false,
        ..AiProviderConfig::default()
    };

    let (llm, llm_source, llm_fallback_reason) =
        resolve_cli_subscription_llm_provider_with_detected(
            &config,
            &[ProbedSubprocessCli {
                detected: crate::subprocess_provider::DetectedSubprocessCli {
                    surface_id: "provider_surface.google.subprocess_cli".to_string(),
                    executable_path: "/tmp/gemini".into(),
                },
                auth_status: SubprocessCliAuthStatus::Unknown,
                auth_detail: Some("auth_status_probe_not_implemented".to_string()),
            }],
        )
        .expect("CLI mode should allow probe-less Gemini runtime");

    assert_eq!(llm_source, ProviderSource::CliSubscription);
    assert!(llm_fallback_reason.is_none());
    assert_eq!(llm.provider_name(), "subprocess-gemini-cli");
}

#[test]
fn provider_api_key_config_reuses_direct_remote_sources() {
    let config = AiProviderConfig {
        access_mode: AiAccessMode::ProviderApiKey,
        ocr_provider: OcrProviderType::Remote,
        llm_provider: LlmProviderType::Remote,
        ocr_api: Some(secret_bound_remote_endpoint("ocr")),
        llm_api: Some(secret_bound_remote_endpoint("llm")),
        fallback_to_local: false,
        ..AiProviderConfig::default()
    };

    let (privacy_guard, _temp_dir) = make_external_ocr_guard(
        true,
        Some(WindowInfo {
            title: "mail".to_string(),
            app_name: "Code".to_string(),
            pid: 9,
            bounds: None,
        }),
        None,
    );
    let adapters = resolve_ai_provider_adapters(
        &config,
        PiiFilterLevel::Standard,
        Some(privacy_guard),
        Some(remote_secret_stores()),
        None,
    )
    .expect("Failed to resolve provider API key config");
    assert_eq!(adapters.ocr_source, ProviderSource::Remote);
    assert_eq!(adapters.llm_source, ProviderSource::Remote);
    assert!(adapters.ocr_fallback_reason.is_none());
    assert!(adapters.llm_fallback_reason.is_none());
    assert!(adapters.ocr.is_external());
    assert!(adapters.llm.is_external());
}

struct FakeExternalOcrProvider {
    responses: Vec<OcrResult>,
}

struct FakeOAuthPort {
    connected: bool,
}

#[async_trait]
impl OAuthPort for FakeOAuthPort {
    async fn start_flow(&self, _provider_id: &str) -> Result<OAuthFlowHandle, CoreError> {
        Ok(OAuthFlowHandle {
            flow_id: "flow-1".to_string(),
            auth_url: "https://example.com/oauth".to_string(),
        })
    }

    async fn flow_status(&self, _flow_id: &str) -> Result<OAuthFlowStatus, CoreError> {
        Ok(OAuthFlowStatus::Completed)
    }

    async fn cancel_flow(&self, _flow_id: &str) -> Result<(), CoreError> {
        Ok(())
    }

    async fn get_access_token(&self, _provider_id: &str) -> Result<Option<String>, CoreError> {
        Ok(self.connected.then(|| "token".to_string()))
    }

    async fn revoke(&self, _provider_id: &str) -> Result<(), CoreError> {
        Ok(())
    }

    async fn connection_status(
        &self,
        provider_id: &str,
    ) -> Result<OAuthConnectionStatus, CoreError> {
        Ok(OAuthConnectionStatus {
            provider_id: provider_id.to_string(),
            connected: self.connected,
            expires_at: None,
            scopes: vec![],
            api_base_url: None,
            has_refresh_token: false,
        })
    }

    async fn refresh_access_token(
        &self,
        _provider_id: &str,
        _min_valid_for_secs: i64,
    ) -> Result<RefreshResult, CoreError> {
        if self.connected {
            Ok(RefreshResult::AlreadyFresh {
                expires_at: chrono::Utc::now().to_rfc3339(),
            })
        } else {
            Ok(RefreshResult::NotAuthenticated)
        }
    }
}

#[async_trait]
impl OcrProvider for FakeExternalOcrProvider {
    async fn extract_elements(
        &self,
        _image: &[u8],
        _image_format: &str,
    ) -> Result<Vec<OcrResult>, CoreError> {
        Ok(self.responses.clone())
    }

    fn provider_name(&self) -> &str {
        "fake-external"
    }

    fn is_external(&self) -> bool {
        true
    }
}

#[test]
fn remote_ocr_requires_runtime_privacy_guard() {
    let config = AiProviderConfig {
        ocr_provider: OcrProviderType::Remote,
        llm_provider: LlmProviderType::Local,
        ocr_api: Some(secret_bound_remote_endpoint("ocr")),
        fallback_to_local: false,
        ..AiProviderConfig::default()
    };

    let result = resolve_ai_provider_adapters(
        &config,
        PiiFilterLevel::Standard,
        None,
        Some(remote_secret_stores()),
        None,
    );
    assert!(
        result.is_err(),
        "Expected remote OCR resolution to require a privacy guard"
    );
    let err = result.err().unwrap();
    assert!(err.to_string().contains("runtime privacy guard"));
}

#[test]
fn oauth_mode_requires_oauth_port() {
    let config = AiProviderConfig {
        access_mode: AiAccessMode::ProviderOAuth,
        llm_provider: LlmProviderType::Remote,
        ..AiProviderConfig::default()
    };

    let result = resolve_ai_provider_adapters(&config, PiiFilterLevel::Standard, None, None, None);
    assert!(
        result.is_err(),
        "ProviderOAuth mode should require an OAuth port"
    );
    let err = result.err().unwrap();
    assert!(err.to_string().contains("OAuth runtime"));
}

#[test]
fn oauth_mode_allows_local_llm_when_no_managed_llm_surface_is_selected() {
    let config = AiProviderConfig {
        access_mode: AiAccessMode::ProviderOAuth,
        llm_provider: LlmProviderType::Local,
        ..AiProviderConfig::default()
    };

    let oauth = Arc::new(FakeOAuthPort { connected: true }) as Arc<dyn OAuthPort>;
    let result =
        resolve_ai_provider_adapters(&config, PiiFilterLevel::Standard, None, None, Some(oauth))
            .expect(
                "ProviderOAuth mode should allow local LLM when no managed LLM surface is selected",
            );
    assert_eq!(result.llm_source, ProviderSource::Local);
}

#[test]
fn oauth_mode_defaults_to_openai_model() {
    let config = AiProviderConfig {
        access_mode: AiAccessMode::ProviderOAuth,
        llm_provider: LlmProviderType::Remote,
        ..AiProviderConfig::default()
    };

    let oauth = Arc::new(FakeOAuthPort { connected: true }) as Arc<dyn OAuthPort>;
    let adapters =
        resolve_ai_provider_adapters(&config, PiiFilterLevel::Standard, None, None, Some(oauth))
            .expect("OAuth mode should resolve when a port is provided");

    assert_eq!(adapters.llm_source, ProviderSource::OAuth);
    assert_eq!(
        adapters.llm.provider_name(),
        helpers::DEFAULT_OPENAI_OAUTH_MODEL
    );
}

#[test]
fn remote_ocr_falls_back_when_selected_managed_ocr_surface_lacks_runtime() {
    let config = AiProviderConfig {
        access_mode: AiAccessMode::ProviderOAuth,
        ocr_provider: OcrProviderType::Remote,
        ocr_api: Some(ExternalApiEndpoint {
            endpoint: String::new(),
            api_key: String::new(),
            model: None,
            timeout_secs: 30,
            provider_type: AiProviderType::OpenAi,
            surface_id: Some("provider_surface.openai.managed_oauth".to_string()),
            credential: None,
        }),
        fallback_to_local: true,
        ..AiProviderConfig::default()
    };

    let (ocr, source, reason) =
        ocr_resolver::resolve_ocr_provider(&config, PiiFilterLevel::Standard, None, None)
            .expect("managed OCR surface should fall back to local when enabled");

    assert_eq!(source, ProviderSource::LocalFallback);
    assert!(reason
        .as_deref()
        .is_some_and(|message| message.contains("managed_oauth")));
    assert!(!ocr.is_external());
}

#[test]
fn oauth_mode_resolves_google_managed_ocr_surface() {
    let config = AiProviderConfig {
        access_mode: AiAccessMode::ProviderOAuth,
        llm_provider: LlmProviderType::Local,
        ocr_provider: OcrProviderType::Remote,
        ocr_api: Some(ExternalApiEndpoint {
            endpoint: "https://vision.googleapis.com/v1/images:annotate".to_string(),
            api_key: String::new(),
            model: None,
            timeout_secs: 30,
            provider_type: AiProviderType::Google,
            surface_id: Some("provider_surface.google.managed_oauth".to_string()),
            credential: None,
        }),
        fallback_to_local: false,
        ..AiProviderConfig::default()
    };

    let oauth = Arc::new(FakeOAuthPort { connected: true }) as Arc<dyn OAuthPort>;
    let (privacy_guard, _tempdir) = make_external_ocr_guard(
        true,
        Some(WindowInfo {
            app_name: "Terminal".to_string(),
            title: "OCR".to_string(),
            pid: 4242,
            bounds: None,
        }),
        None,
    );
    let adapters = resolve_ai_provider_adapters(
        &config,
        PiiFilterLevel::Standard,
        Some(privacy_guard),
        None,
        Some(oauth),
    )
    .expect("Google OCR managed OAuth should resolve when an OAuth port is available");

    assert_eq!(adapters.ocr_source, ProviderSource::OAuth);
    assert!(adapters.ocr.is_external());
}

#[test]
fn resolves_remote_providers_from_secret_binding_with_plaintext_empty() {
    let namespace = "provider/openai/default";
    let key = "api_key";
    let mut snapshot = std::collections::HashMap::new();
    snapshot.insert(
        secret_env_var_name(namespace, key),
        "sk-secret-store".to_string(),
    );
    let secret_store = Arc::new(EnvSecretStore::from_snapshot(snapshot));
    let secret_stores = SecretStoreSet {
        os_secret_store: None,
        file_secret_store: None,
        env_secret_store: Some(secret_store),
        default_backend_kind: CredentialBackendKind::Env,
        fallback_backend_kind: CredentialBackendKind::Unavailable,
    };

    let secret_bound_endpoint = ExternalApiEndpoint {
        endpoint: "https://api.openai.com/v1".to_string(),
        api_key: String::new(),
        model: Some("gpt-5.4".to_string()),
        timeout_secs: 30,
        provider_type: AiProviderType::OpenAi,
        surface_id: None,
        credential: Some(CredentialBinding {
            auth_mode: CredentialAuthMode::ApiKey,
            backend_kind: CredentialBackendKind::Env,
            secret_ref: Some(SecretRef {
                namespace: namespace.to_string(),
                key: key.to_string(),
            }),
            projection_enabled: false,
        }),
    };
    let config = AiProviderConfig {
        ocr_provider: OcrProviderType::Remote,
        llm_provider: LlmProviderType::Remote,
        ocr_api: Some(secret_bound_endpoint.clone()),
        llm_api: Some(secret_bound_endpoint),
        fallback_to_local: false,
        ..AiProviderConfig::default()
    };

    let (privacy_guard, _temp_dir) = make_external_ocr_guard(
        true,
        Some(WindowInfo {
            title: "mail".to_string(),
            app_name: "Code".to_string(),
            pid: 9,
            bounds: None,
        }),
        None,
    );
    let adapters = resolve_ai_provider_adapters(
        &config,
        PiiFilterLevel::Standard,
        Some(privacy_guard),
        Some(secret_stores),
        None,
    )
    .expect("Secret-bound API key configuration should resolve");

    assert_eq!(adapters.ocr_source, ProviderSource::Remote);
    assert_eq!(adapters.llm_source, ProviderSource::Remote);
    assert!(adapters.ocr_fallback_reason.is_none());
    assert!(adapters.llm_fallback_reason.is_none());
    assert!(adapters.ocr.is_external());
    assert!(adapters.llm.is_external());
}

#[tokio::test]
async fn guarded_ocr_provider_filters_invalid_results_when_ratio_is_within_limit() {
    let inner = Arc::new(FakeExternalOcrProvider {
        responses: vec![
            OcrResult {
                text: "save".to_string(),
                x: 10,
                y: 10,
                width: 40,
                height: 20,
                confidence: 0.9,
            },
            OcrResult {
                text: "   ".to_string(),
                x: 12,
                y: 10,
                width: 10,
                height: 20,
                confidence: 0.9,
            },
            OcrResult {
                text: "bad-confidence".to_string(),
                x: 30,
                y: 22,
                width: 20,
                height: 10,
                confidence: 1.5,
            },
        ],
    }) as Arc<dyn OcrProvider>;
    let (privacy_guard, _temp_dir) = make_external_ocr_guard(
        true,
        Some(WindowInfo {
            title: "main.rs".to_string(),
            app_name: "Code".to_string(),
            pid: 11,
            bounds: None,
        }),
        None,
    );
    let guarded = guarded_ocr::GuardedOcrProvider::new(
        inner,
        privacy_guard,
        true,
        OcrValidationConfig {
            enabled: true,
            min_confidence: 0.5,
            max_invalid_ratio: 0.8,
        },
    );

    let results = guarded.extract_elements(b"dummy", "png").await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].text, "save");
}

#[tokio::test]
async fn guarded_ocr_provider_rejects_when_invalid_ratio_exceeds_limit() {
    let inner = Arc::new(FakeExternalOcrProvider {
        responses: vec![
            OcrResult {
                text: "ok".to_string(),
                x: 1,
                y: 1,
                width: 10,
                height: 10,
                confidence: 0.9,
            },
            OcrResult {
                text: "".to_string(),
                x: 1,
                y: 1,
                width: 0,
                height: 0,
                confidence: 0.9,
            },
        ],
    }) as Arc<dyn OcrProvider>;
    let audit_logger = Arc::new(RwLock::new(AuditLogger::new(32, 8)));
    let (privacy_guard, _temp_dir) = make_external_ocr_guard(
        true,
        Some(WindowInfo {
            title: "main.rs".to_string(),
            app_name: "Code".to_string(),
            pid: 12,
            bounds: None,
        }),
        Some(audit_logger.clone()),
    );
    let guarded = guarded_ocr::GuardedOcrProvider::new(
        inner,
        privacy_guard,
        true,
        OcrValidationConfig {
            enabled: true,
            min_confidence: 0.5,
            max_invalid_ratio: 0.2,
        },
    );

    let err = guarded.extract_elements(b"dummy", "png").await.unwrap_err();
    assert!(err.to_string().contains("invalid_ratio"));
}

#[tokio::test]
async fn guarded_ocr_provider_denies_without_ocr_consent_and_audits_it() {
    let inner = Arc::new(FakeExternalOcrProvider {
        responses: vec![OcrResult {
            text: "save".to_string(),
            x: 1,
            y: 1,
            width: 10,
            height: 10,
            confidence: 0.9,
        }],
    }) as Arc<dyn OcrProvider>;
    let audit_logger = Arc::new(RwLock::new(AuditLogger::new(32, 8)));
    let (privacy_guard, _temp_dir) = make_external_ocr_guard(
        false,
        Some(WindowInfo {
            title: "main.rs".to_string(),
            app_name: "Code".to_string(),
            pid: 13,
            bounds: None,
        }),
        Some(audit_logger.clone()),
    );
    let guarded = guarded_ocr::GuardedOcrProvider::new(
        inner,
        privacy_guard,
        false,
        OcrValidationConfig::default(),
    );

    let err = guarded.extract_elements(b"dummy", "png").await.unwrap_err();
    assert!(err.to_string().contains("OCR consent is required"));

    let logger = audit_logger.read().await;
    assert_eq!(logger.pending_count(), 1);
    assert!(logger.recent_entries(1)[0]
        .details
        .as_deref()
        .is_some_and(|details| details.contains("reason=OCR consent is required")));
}

#[tokio::test]
async fn guarded_ocr_provider_denies_sensitive_apps() {
    let inner = Arc::new(FakeExternalOcrProvider {
        responses: vec![OcrResult {
            text: "save".to_string(),
            x: 1,
            y: 1,
            width: 10,
            height: 10,
            confidence: 0.9,
        }],
    }) as Arc<dyn OcrProvider>;
    let (privacy_guard, _temp_dir) = make_external_ocr_guard(
        true,
        Some(WindowInfo {
            title: "Vault".to_string(),
            app_name: "1Password".to_string(),
            pid: 14,
            bounds: None,
        }),
        None,
    );
    let guarded = guarded_ocr::GuardedOcrProvider::new(
        inner,
        privacy_guard,
        false,
        OcrValidationConfig::default(),
    );

    let err = guarded.extract_elements(b"dummy", "png").await.unwrap_err();
    assert!(err.to_string().contains("Blocked sensitive app"));
}
