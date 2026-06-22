use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::guarded_ocr;
use super::helpers;
use super::llm_resolver::resolve_cli_subscription_llm_provider_with_detected;
use super::ocr_resolver::resolve_cli_subscription_ocr_provider;
use super::*;
use async_trait::async_trait;
use maekon_automation::audit::AuditLogger;
use maekon_core::config::{
    AiAccessMode, AiProviderConfig, AiProviderType, CredentialAuthMode, CredentialBackendKind,
    CredentialBinding, ExternalApiEndpoint, ExternalDataPolicy, LlmProviderType, OcrProviderType,
    OcrValidationConfig, PiiFilterLevel, PrivacyConfig, SecretRef,
};
use maekon_core::consent::{ConsentManager, ConsentPermissions, ConsentRecord, ConsentStatus};
use maekon_core::error::CoreError;
use maekon_core::models::context::{ProcessInfo, WindowInfo};
use maekon_core::models::event::ProcessDetail;
use maekon_core::models::suggestion::Suggestion;
use maekon_core::ports::analysis_provider::AnalysisProvider;
use maekon_core::ports::consent_manager::ConsentManagerPort;
use maekon_core::ports::llm_provider::{
    InterpretedAction, LlmProvider, ScreenContext, SkillContext,
};
use maekon_core::ports::monitor::ProcessMonitor;
use maekon_core::ports::oauth::{
    OAuthConnectionStatus, OAuthFlowHandle, OAuthFlowStatus, OAuthPort, RefreshResult,
};
use maekon_core::ports::ocr_provider::{OcrProvider, OcrResult};
use maekon_core::ports::secret_store::{
    provider_api_key_secret_ref, secret_env_var_name, SecretStoreSet,
};
use maekon_storage::env_secret_store::EnvSecretStore;
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

struct TrackingConsentManager {
    permissions: ConsentPermissions,
    effective_called: Arc<AtomicBool>,
    deletion_flag: Arc<AtomicBool>,
    erasing: Arc<AtomicBool>,
}

impl TrackingConsentManager {
    fn new(permissions: ConsentPermissions, effective_called: Arc<AtomicBool>) -> Self {
        Self {
            permissions,
            effective_called,
            deletion_flag: Arc::new(AtomicBool::new(false)),
            erasing: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl ConsentManagerPort for TrackingConsentManager {
    fn check_consent(&self) -> ConsentStatus {
        ConsentStatus::Valid
    }

    fn current_consent(&self) -> Option<ConsentRecord> {
        None
    }

    fn effective_permissions(&self) -> ConsentPermissions {
        self.effective_called.store(true, Ordering::SeqCst);
        self.permissions.clone()
    }

    fn status_and_permissions(&self) -> (ConsentStatus, ConsentPermissions) {
        (ConsentStatus::Valid, self.permissions.clone())
    }

    fn grant_consent(
        &self,
        _permissions: ConsentPermissions,
        _data_retention_days: u32,
    ) -> Result<(), CoreError> {
        Ok(())
    }

    fn revoke_consent(&self) -> Result<(), CoreError> {
        Ok(())
    }

    fn has_pending_deletion(&self) -> bool {
        false
    }

    fn pending_erasure_id(&self) -> Option<String> {
        None
    }

    fn clear_pending_deletion(&self) {}

    fn deletion_flag(&self) -> Arc<AtomicBool> {
        self.deletion_flag.clone()
    }

    fn erasing(&self) -> Arc<AtomicBool> {
        self.erasing.clone()
    }
}

fn write_consent_permissions(path: &std::path::Path, permissions: ConsentPermissions) {
    let consent_manager = ConsentManager::new(path.to_path_buf());
    consent_manager
        .grant_consent(permissions, 30)
        .expect("Failed to write consent");
}

fn make_external_privacy_guard_with_permissions(
    permissions: Option<ConsentPermissions>,
    active_window: Option<WindowInfo>,
    audit_logger: Option<Arc<RwLock<AuditLogger>>>,
) -> (ExternalOcrPrivacyGuard, TempDir) {
    make_external_privacy_guard_with_options(
        permissions,
        active_window,
        audit_logger,
        PiiFilterLevel::Standard,
        ExternalDataPolicy::PiiFilterStandard,
    )
}

fn make_external_privacy_guard_with_options(
    permissions: Option<ConsentPermissions>,
    active_window: Option<WindowInfo>,
    audit_logger: Option<Arc<RwLock<AuditLogger>>>,
    pii_filter_level: PiiFilterLevel,
    external_data_policy: ExternalDataPolicy,
) -> (ExternalOcrPrivacyGuard, TempDir) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let consent_path = temp_dir.path().join("consent.json");
    if let Some(permissions) = permissions {
        write_consent_permissions(&consent_path, permissions);
    }
    let consent_manager: Arc<dyn ConsentManagerPort> = Arc::new(ConsentManager::new(consent_path));

    (
        ExternalOcrPrivacyGuard::new(
            consent_manager,
            pii_filter_level,
            external_data_policy,
            PrivacyConfig::default(),
            Arc::new(StaticProcessMonitor { active_window }),
            audit_logger,
        ),
        temp_dir,
    )
}

fn make_external_ocr_guard(
    ocr_permitted: bool,
    active_window: Option<WindowInfo>,
    audit_logger: Option<Arc<RwLock<AuditLogger>>>,
) -> (ExternalOcrPrivacyGuard, TempDir) {
    let permissions = ocr_permitted.then_some(ConsentPermissions {
        ocr_processing: true,
        screen_capture: true,
        ..Default::default()
    });

    make_external_privacy_guard_with_permissions(permissions, active_window, audit_logger)
}

fn make_external_llm_guard() -> (ExternalOcrPrivacyGuard, TempDir) {
    make_external_privacy_guard_with_permissions(
        Some(ConsentPermissions {
            ocr_processing: true,
            screen_capture: true,
            full_text_extraction: true,
            ..Default::default()
        }),
        Some(WindowInfo {
            title: "main.rs".to_string(),
            app_name: "Code".to_string(),
            app_bundle_id: None,
            pid: 8,
            bounds: None,
        }),
        None,
    )
}

#[tokio::test]
async fn external_llm_guard_uses_injected_consent_manager() {
    let effective_called = Arc::new(AtomicBool::new(false));
    let consent_manager: Arc<dyn ConsentManagerPort> = Arc::new(TrackingConsentManager::new(
        ConsentPermissions {
            full_text_extraction: true,
            ..Default::default()
        },
        effective_called.clone(),
    ));
    let guard = ExternalOcrPrivacyGuard::new(
        consent_manager,
        PiiFilterLevel::Standard,
        ExternalDataPolicy::PiiFilterStandard,
        PrivacyConfig::default(),
        Arc::new(StaticProcessMonitor {
            active_window: Some(WindowInfo {
                title: "main.rs".to_string(),
                app_name: "Code".to_string(),
                app_bundle_id: None,
                pid: 8,
                bounds: None,
            }),
        }),
        None,
    );

    let context = ScreenContext {
        visible_texts: vec!["hello".to_string()],
        active_app: "Code".to_string(),
        active_window_title: "main.rs".to_string(),
        layout_description: None,
    };

    let sanitized = guard
        .prepare_screen_context_for_external_llm(&context, "remote-test")
        .await
        .expect("injected consent manager should permit external LLM");

    assert!(effective_called.load(Ordering::SeqCst));
    assert_eq!(sanitized.visible_texts, vec!["hello".to_string()]);
}

#[test]
fn resolves_local_providers_by_default() {
    // Default config: access_mode=ProviderApiKey (not LocalModel), llm_provider=Local.
    // FU-3 (#5741): this path now routes through resolve_local_model_llm_provider
    // — loopback Ollama primary with the rule matcher as runtime fallback
    // (LocalOllama), Ok-degrading to the bare rule matcher (Local) only when
    // provider construction fails.
    let config = AiProviderConfig::default();
    let adapters = resolve_ai_provider_adapters(
        &config,
        PiiFilterLevel::Standard,
        None,
        None,
        None,
        maekon_network::CircuitBreakerRegistry::new(),
        None, // #5734: llm_call_health — not exercised in these tests
    )
    .expect("Failed to resolve default configuration");

    assert_eq!(adapters.ocr_source, ProviderSource::Local);
    assert!(adapters.ocr_fallback_reason.is_none());
    assert!(!adapters.ocr.is_external());
    // Loopback Ollama composite and the bare rule matcher are both non-external.
    assert!(!adapters.llm.is_external());
    assert!(
        matches!(
            adapters.ocr.provider_name(),
            // #5857: Windows resolves to native OCR (windows-media-ocr).
            "local-tesseract" | "macos-vision" | "windows-media-ocr"
        ),
        "unexpected local OCR provider: {}",
        adapters.ocr.provider_name()
    );
    // Per-branch exact expectations: the happy path resolves the loopback
    // Ollama composite (provider_name = the catalog model id, no fallback
    // reason); the Ok-degrade path keeps the rule matcher and records why.
    match adapters.llm_source {
        ProviderSource::LocalOllama => {
            assert_ne!(adapters.llm.provider_name(), "local-rule-based");
            assert!(adapters.llm_fallback_reason.is_none());
        }
        ProviderSource::Local => {
            assert_eq!(adapters.llm.provider_name(), "local-rule-based");
            assert!(adapters.llm_fallback_reason.is_some());
        }
        other => panic!("unexpected llm_source: {other:?}"),
    }
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
            app_bundle_id: None,
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
        maekon_network::CircuitBreakerRegistry::new(),
        None, // #5734: llm_call_health — not exercised in these tests
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

    let adapters = resolve_ai_provider_adapters(
        &config,
        PiiFilterLevel::Standard,
        None,
        None,
        None,
        maekon_network::CircuitBreakerRegistry::new(),
        None, // #5734: llm_call_health — not exercised in these tests
    )
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

    match resolve_ai_provider_adapters(
        &config,
        PiiFilterLevel::Standard,
        None,
        None,
        None,
        maekon_network::CircuitBreakerRegistry::new(),
        None, // #5734: llm_call_health — not exercised in these tests
    ) {
        Ok(_) => panic!("Expected an error"),
        // `ocr_api` being None is a required-field-missing condition; maps to
        // ConfigCode::Missing (config.missing wire code). Previously this was
        // ConfigCode::Invalid but that was semantic drift — "must be configured"
        // is Missing, not Invalid. Fixed in iter-89 ConfigCode audit.
        Err(CoreError::Config {
            code: maekon_core::error_codes::ConfigCode::Missing,
            message: msg,
        }) => assert!(msg.contains("ocr_api")),
        Err(other) => panic!("Unexpected error type: {other}"),
    }
}

#[test]
fn local_mode_forces_local_adapters_even_if_remote_is_requested() {
    // C3: LocalModel arm now routes LLM through resolve_local_model_llm_provider.
    // OCR stays local; LLM becomes LocalOllama (catalog default endpoint =
    // http://localhost:11434/v1/responses, which is loopback).
    // ocr_provider/llm_provider fields are ignored in LocalModel arm.
    let config = AiProviderConfig {
        access_mode: AiAccessMode::LocalModel,
        ocr_provider: OcrProviderType::Remote,
        llm_provider: LlmProviderType::Remote,
        ocr_api: Some(remote_endpoint()),
        llm_api: Some(remote_endpoint()),
        fallback_to_local: false,
        ..AiProviderConfig::default()
    };

    let adapters = resolve_ai_provider_adapters(
        &config,
        PiiFilterLevel::Standard,
        None,
        None,
        None,
        maekon_network::CircuitBreakerRegistry::new(),
        None, // #5734: llm_call_health — not exercised in these tests
    )
    .expect("LocalModel arm must never return Err (ok-degrade invariant)");

    // OCR always local.
    assert_eq!(adapters.ocr_source, ProviderSource::Local);
    assert!(adapters.ocr_fallback_reason.is_none());
    assert!(!adapters.ocr.is_external());

    // LLM: LocalOllama (loopback catalog default) or Local (analysis feature off /
    // construction failure — ok-degrade). Either is acceptable; never Remote.
    assert!(
        matches!(
            adapters.llm_source,
            ProviderSource::LocalOllama | ProviderSource::Local
        ),
        "expected LocalOllama or Local, got {:?}",
        adapters.llm_source
    );
    // Composite wraps Ollama primary; is_external = false for loopback.
    assert!(!adapters.llm.is_external());
}

#[test]
fn cli_subscription_mode_marks_cli_source() {
    let config = AiProviderConfig {
        access_mode: AiAccessMode::ProviderSubscriptionCli,
        ..AiProviderConfig::default()
    };
    let (privacy_guard, _temp_dir) = make_external_llm_guard();

    let (llm, llm_source, llm_fallback_reason) =
        resolve_cli_subscription_llm_provider_with_detected(
            &config,
            Some(privacy_guard.clone()),
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

    let adapters = resolve_ai_provider_adapters(
        &config,
        PiiFilterLevel::Standard,
        Some(privacy_guard),
        None,
        None,
        maekon_network::CircuitBreakerRegistry::new(),
        None, // #5734: llm_call_health — not exercised in these tests
    )
    .expect("Failed to resolve CLI mode");
    assert_eq!(adapters.ocr_source, ProviderSource::Local);
    assert!(!adapters.ocr.is_external());
    assert!(matches!(
        adapters.llm_source,
        ProviderSource::CliSubscription | ProviderSource::LocalFallback
    ));
}

#[tokio::test]
async fn cli_subscription_llm_provider_requires_privacy_guard_before_screen_context_egress() {
    let config = AiProviderConfig {
        access_mode: AiAccessMode::ProviderSubscriptionCli,
        ..AiProviderConfig::default()
    };
    let (privacy_guard, _temp_dir) = make_external_privacy_guard_with_permissions(
        Some(ConsentPermissions {
            ocr_processing: true,
            screen_capture: true,
            full_text_extraction: false,
            ..Default::default()
        }),
        Some(WindowInfo {
            title: "Inbox user@example.com".to_string(),
            app_name: "Code".to_string(),
            app_bundle_id: None,
            pid: 23,
            bounds: None,
        }),
        None,
    );

    let (llm, llm_source, llm_fallback_reason) =
        resolve_cli_subscription_llm_provider_with_detected(
            &config,
            Some(privacy_guard),
            &[ProbedSubprocessCli {
                detected: crate::subprocess_provider::DetectedSubprocessCli {
                    surface_id: "provider_surface.openai.subprocess_cli".to_string(),
                    executable_path: "/tmp/codex".into(),
                },
                auth_status: SubprocessCliAuthStatus::Authenticated,
                auth_detail: Some("cli_authenticated".to_string()),
            }],
        )
        .expect("Failed to resolve guarded CLI LLM mode");

    assert_eq!(llm_source, ProviderSource::CliSubscription);
    assert!(llm_fallback_reason.is_none());

    let err = llm
        .interpret_intent(
            &ScreenContext {
                visible_texts: vec!["email user@example.com".to_string()],
                active_app: "Code".to_string(),
                active_window_title: "Inbox user@example.com".to_string(),
                layout_description: Some("Contact user@example.com".to_string()),
            },
            "click archive",
        )
        .await
        .unwrap_err();

    assert!(err.to_string().contains("full text extraction consent"));
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
            app_bundle_id: None,
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
        maekon_network::CircuitBreakerRegistry::new(),
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
    let (privacy_guard, _temp_dir) = make_external_ocr_guard(
        true,
        Some(WindowInfo {
            title: "capture.png".to_string(),
            app_name: "Preview".to_string(),
            app_bundle_id: None,
            pid: 12,
            bounds: None,
        }),
        None,
    );

    let (ocr, ocr_source, ocr_fallback_reason) = resolve_cli_subscription_ocr_provider(
        &config,
        PiiFilterLevel::Standard,
        Some(privacy_guard),
        None,
        &[ProbedSubprocessCli {
            detected: crate::subprocess_provider::DetectedSubprocessCli {
                surface_id: "provider_surface.openai.subprocess_cli".to_string(),
                executable_path: "/tmp/codex".into(),
            },
            auth_status: SubprocessCliAuthStatus::Authenticated,
            auth_detail: Some("cli_authenticated".to_string()),
        }],
        maekon_network::CircuitBreakerRegistry::new(),
    )
    .expect("expected OCR subprocess runtime to resolve");

    assert_eq!(ocr_source, ProviderSource::CliSubscription);
    assert!(ocr_fallback_reason.is_none());
    assert!(ocr.is_external());
    assert_eq!(ocr.provider_name(), "subprocess-codex");
}

#[test]
fn cli_subscription_subprocess_ocr_requires_privacy_guard() {
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

    let err = match resolve_cli_subscription_ocr_provider(
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
        maekon_network::CircuitBreakerRegistry::new(),
    ) {
        Ok(_) => panic!("expected CLI OCR resolution to fail without a privacy guard"),
        Err(err) => err,
    };

    assert_eq!(err.code(), "config.missing");
    assert!(err.to_string().contains("privacy guard"));
}

#[test]
fn cli_subscription_mode_falls_back_to_local_when_no_supported_cli_runtime_exists() {
    let config = AiProviderConfig {
        access_mode: AiAccessMode::ProviderSubscriptionCli,
        fallback_to_local: true,
        ..AiProviderConfig::default()
    };

    let (llm, llm_source, llm_fallback_reason) =
        resolve_cli_subscription_llm_provider_with_detected(&config, None, &[])
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
    let (privacy_guard, _temp_dir) = make_external_llm_guard();

    let (llm, llm_source, llm_fallback_reason) =
        resolve_cli_subscription_llm_provider_with_detected(
            &config,
            Some(privacy_guard),
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
        None,
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
            code: maekon_core::error_codes::ConfigCode::Invalid,
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
    let (privacy_guard, _temp_dir) = make_external_llm_guard();

    let (llm, llm_source, llm_fallback_reason) =
        resolve_cli_subscription_llm_provider_with_detected(
            &config,
            Some(privacy_guard),
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
fn cli_subscription_mode_reports_antigravity_as_manual_gui_only() {
    let config = AiProviderConfig {
        access_mode: AiAccessMode::ProviderSubscriptionCli,
        llm_api: Some(ExternalApiEndpoint {
            provider_type: AiProviderType::Google,
            ..remote_endpoint()
        }),
        fallback_to_local: false,
        ..AiProviderConfig::default()
    };

    match resolve_cli_subscription_llm_provider_with_detected(
        &config,
        None,
        &[ProbedSubprocessCli {
            detected: crate::subprocess_provider::DetectedSubprocessCli {
                surface_id: "provider_surface.google.antigravity_cli".to_string(),
                executable_path: "/tmp/antigravity".into(),
            },
            auth_status: SubprocessCliAuthStatus::Unknown,
            auth_detail: Some("auth_status_probe_not_implemented".to_string()),
        }],
    ) {
        Err(CoreError::Config {
            code: maekon_core::error_codes::ConfigCode::Invalid,
            message,
        }) => {
            assert!(message.contains("antigravity"));
            assert!(message.contains("manual/GUI-only"));
            assert!(message.contains("headless"));
        }
        Ok(_) => panic!("Expected an unsupported Antigravity CLI surface error"),
        Err(other) => panic!("Unexpected error: {other}"),
    }
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
            app_bundle_id: None,
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
        maekon_network::CircuitBreakerRegistry::new(),
        None, // #5734: llm_call_health — not exercised in these tests
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

struct FakeExternalLlmProvider {
    seen_context: Arc<std::sync::Mutex<Option<ScreenContext>>>,
}

struct FakeExternalAnalysisProvider {
    seen_analyze: Arc<std::sync::Mutex<Option<(String, String)>>>,
    seen_summary: Arc<std::sync::Mutex<Option<(String, String)>>>,
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

#[async_trait]
impl LlmProvider for FakeExternalLlmProvider {
    async fn interpret_intent(
        &self,
        screen_context: &ScreenContext,
        _intent_hint: &str,
    ) -> Result<InterpretedAction, CoreError> {
        *self.seen_context.lock().unwrap() = Some(screen_context.clone());
        Ok(InterpretedAction {
            target_text: Some("Save".to_string()),
            target_role: Some("button".to_string()),
            action_type: "click".to_string(),
            confidence: 0.8,
        })
    }

    async fn interpret_intent_with_skills(
        &self,
        screen_context: &ScreenContext,
        intent_hint: &str,
        _skill_ctx: &SkillContext,
    ) -> Result<InterpretedAction, CoreError> {
        self.interpret_intent(screen_context, intent_hint).await
    }

    fn provider_name(&self) -> &str {
        "fake-external-llm"
    }

    fn is_external(&self) -> bool {
        true
    }
}

#[async_trait]
impl AnalysisProvider for FakeExternalAnalysisProvider {
    async fn analyze(
        &self,
        context_json: &str,
        system_prompt: &str,
    ) -> Result<Vec<Suggestion>, CoreError> {
        *self.seen_analyze.lock().unwrap() =
            Some((context_json.to_string(), system_prompt.to_string()));
        Ok(vec![])
    }

    async fn summarize_text(
        &self,
        context_json: &str,
        system_prompt: &str,
    ) -> Result<String, CoreError> {
        *self.seen_summary.lock().unwrap() =
            Some((context_json.to_string(), system_prompt.to_string()));
        Ok("summary".to_string())
    }

    fn provider_name(&self) -> &str {
        "fake-external-analysis"
    }
}

#[tokio::test]
async fn guarded_analysis_provider_denies_without_full_text_consent_and_audits_it() {
    let seen_analyze = Arc::new(std::sync::Mutex::new(None));
    let seen_summary = Arc::new(std::sync::Mutex::new(None));
    let inner = Arc::new(FakeExternalAnalysisProvider {
        seen_analyze: seen_analyze.clone(),
        seen_summary,
    }) as Arc<dyn AnalysisProvider>;
    let audit_logger = Arc::new(RwLock::new(AuditLogger::new(32, 8)));
    let (privacy_guard, _temp_dir) = make_external_privacy_guard_with_permissions(
        Some(ConsentPermissions {
            ocr_processing: true,
            screen_capture: true,
            full_text_extraction: false,
            ..Default::default()
        }),
        Some(WindowInfo {
            title: "Quarterly payroll user@example.com".to_string(),
            app_name: "Code".to_string(),
            app_bundle_id: None,
            pid: 41,
            bounds: None,
        }),
        Some(audit_logger.clone()),
    );
    let guarded = super::guarded_analysis::GuardedAnalysisProvider::new(inner, privacy_guard);

    let err = guarded
        .analyze(
            r#"{"visible_text":"email user@example.com"}"#,
            "summarize the current screen",
        )
        .await
        .unwrap_err();

    assert!(err.to_string().contains("full text extraction consent"));
    assert!(seen_analyze.lock().unwrap().is_none());

    let logger = audit_logger.read().await;
    assert_eq!(logger.pending_count(), 1);
    assert!(logger.recent_entries(1)[0]
        .details
        .as_deref()
        .is_some_and(|details| details.contains("purpose=analysis")));
    assert!(logger.recent_entries(1)[0]
        .details
        .as_deref()
        .is_some_and(|details| details.contains("reason=full_text_extraction_missing")));
}

#[tokio::test]
async fn guarded_analysis_provider_sanitizes_context_and_prompt_before_external_call() {
    let seen_analyze = Arc::new(std::sync::Mutex::new(None));
    let seen_summary = Arc::new(std::sync::Mutex::new(None));
    let inner = Arc::new(FakeExternalAnalysisProvider {
        seen_analyze: seen_analyze.clone(),
        seen_summary: seen_summary.clone(),
    }) as Arc<dyn AnalysisProvider>;
    let (privacy_guard, _temp_dir) = make_external_privacy_guard_with_permissions(
        Some(ConsentPermissions {
            ocr_processing: true,
            screen_capture: true,
            full_text_extraction: true,
            ..Default::default()
        }),
        Some(WindowInfo {
            title: "Inbox user@example.com".to_string(),
            app_name: "Code".to_string(),
            app_bundle_id: None,
            pid: 42,
            bounds: None,
        }),
        None,
    );
    let guarded = super::guarded_analysis::GuardedAnalysisProvider::new(inner, privacy_guard);

    guarded
        .analyze(
            r#"{"visible_text":"email user@example.com","token":"sk-test-secret"}"#,
            "explain email user@example.com without leaking it",
        )
        .await
        .expect("sanitized analysis context should reach inner provider");
    guarded
        .summarize_text(
            r#"{"visible_text":"phone 010-1234-5678"}"#,
            "summarize phone 010-1234-5678",
        )
        .await
        .expect("sanitized summary context should reach inner provider");

    let (analyze_context, analyze_prompt) = seen_analyze
        .lock()
        .unwrap()
        .clone()
        .expect("inner analyze should receive sanitized input");
    assert!(!analyze_context.contains("user@example.com"));
    assert!(!analyze_context.contains("sk-test-secret"));
    assert!(!analyze_prompt.contains("user@example.com"));

    let (summary_context, summary_prompt) = seen_summary
        .lock()
        .unwrap()
        .clone()
        .expect("inner summarize should receive sanitized input");
    assert!(!summary_context.contains("010-1234-5678"));
    assert!(!summary_prompt.contains("010-1234-5678"));
}

#[tokio::test]
async fn guarded_llm_provider_denies_without_full_text_consent_and_audits_it() {
    let seen_context = Arc::new(std::sync::Mutex::new(None));
    let inner = Arc::new(FakeExternalLlmProvider {
        seen_context: seen_context.clone(),
    }) as Arc<dyn LlmProvider>;
    let audit_logger = Arc::new(RwLock::new(AuditLogger::new(32, 8)));
    let (privacy_guard, _temp_dir) = make_external_privacy_guard_with_permissions(
        Some(ConsentPermissions {
            ocr_processing: true,
            screen_capture: true,
            full_text_extraction: false,
            ..Default::default()
        }),
        Some(WindowInfo {
            title: "Account 123-45-6789".to_string(),
            app_name: "Code".to_string(),
            app_bundle_id: None,
            pid: 21,
            bounds: None,
        }),
        Some(audit_logger.clone()),
    );
    let guarded = super::guarded_llm::GuardedLlmProvider::new(inner, privacy_guard);

    let err = guarded
        .interpret_intent(
            &ScreenContext {
                visible_texts: vec!["email user@example.com".to_string()],
                active_app: "Code".to_string(),
                active_window_title: "Account 123-45-6789".to_string(),
                layout_description: Some("form with account number".to_string()),
            },
            "click save",
        )
        .await
        .unwrap_err();

    assert!(err.to_string().contains("full text extraction consent"));
    assert!(seen_context.lock().unwrap().is_none());

    let logger = audit_logger.read().await;
    assert_eq!(logger.pending_count(), 1);
    assert!(logger.recent_entries(1)[0]
        .details
        .as_deref()
        .is_some_and(|details| details.contains("reason=full_text_extraction_missing")));
}

#[tokio::test]
async fn guarded_llm_provider_sanitizes_screen_context_before_external_call() {
    let seen_context = Arc::new(std::sync::Mutex::new(None));
    let inner = Arc::new(FakeExternalLlmProvider {
        seen_context: seen_context.clone(),
    }) as Arc<dyn LlmProvider>;
    let (privacy_guard, _temp_dir) = make_external_privacy_guard_with_permissions(
        Some(ConsentPermissions {
            ocr_processing: true,
            screen_capture: true,
            full_text_extraction: true,
            ..Default::default()
        }),
        Some(WindowInfo {
            title: "Inbox user@example.com".to_string(),
            app_name: "Code".to_string(),
            app_bundle_id: None,
            pid: 22,
            bounds: None,
        }),
        None,
    );
    let guarded = super::guarded_llm::GuardedLlmProvider::new(inner, privacy_guard);

    guarded
        .interpret_intent(
            &ScreenContext {
                visible_texts: vec!["email user@example.com".to_string()],
                active_app: "Code".to_string(),
                active_window_title: "Inbox user@example.com".to_string(),
                layout_description: Some("Contact user@example.com".to_string()),
            },
            "click save",
        )
        .await
        .expect("sanitized context should reach inner provider");

    let sent = seen_context
        .lock()
        .unwrap()
        .clone()
        .expect("inner provider should receive sanitized context");
    assert_eq!(sent.active_app, "Code");
    assert!(!sent.active_window_title.contains("user@example.com"));
    assert!(!sent.visible_texts[0].contains("user@example.com"));
    assert!(!sent
        .layout_description
        .as_deref()
        .unwrap_or_default()
        .contains("user@example.com"));
}

#[tokio::test]
async fn guarded_llm_provider_floors_allow_filtered_off_before_external_call() {
    let seen_context = Arc::new(std::sync::Mutex::new(None));
    let inner = Arc::new(FakeExternalLlmProvider {
        seen_context: seen_context.clone(),
    }) as Arc<dyn LlmProvider>;
    let (privacy_guard, _temp_dir) = make_external_privacy_guard_with_options(
        Some(ConsentPermissions {
            ocr_processing: true,
            screen_capture: true,
            full_text_extraction: true,
            ..Default::default()
        }),
        Some(WindowInfo {
            title: "Inbox user@example.com".to_string(),
            app_name: "Code".to_string(),
            app_bundle_id: None,
            pid: 22,
            bounds: None,
        }),
        None,
        PiiFilterLevel::Off,
        ExternalDataPolicy::AllowFiltered,
    );
    let guarded = super::guarded_llm::GuardedLlmProvider::new(inner, privacy_guard);

    guarded
        .interpret_intent(
            &ScreenContext {
                visible_texts: vec!["email user@example.com".to_string()],
                active_app: "Code".to_string(),
                active_window_title: "Inbox user@example.com".to_string(),
                layout_description: Some("Contact user@example.com".to_string()),
            },
            "click save",
        )
        .await
        .expect("AllowFiltered + Off must floor to a masking level before egress");

    let sent = seen_context
        .lock()
        .unwrap()
        .clone()
        .expect("inner provider should receive sanitized context");
    assert!(
        !sent.visible_texts[0].contains("user@example.com"),
        "AllowFiltered + Off must not egress raw visible text: {:?}",
        sent.visible_texts
    );
    assert!(
        !sent.active_window_title.contains("user@example.com"),
        "AllowFiltered + Off must not egress raw window titles: {}",
        sent.active_window_title
    );
    assert!(
        !sent
            .layout_description
            .as_deref()
            .unwrap_or_default()
            .contains("user@example.com"),
        "AllowFiltered + Off must not egress raw layout text: {:?}",
        sent.layout_description
    );
}

#[tokio::test]
async fn guarded_llm_provider_pins_strict_and_standard_policy_before_external_call() {
    for policy in [
        ExternalDataPolicy::PiiFilterStrict,
        ExternalDataPolicy::PiiFilterStandard,
    ] {
        let seen_context = Arc::new(std::sync::Mutex::new(None));
        let inner = Arc::new(FakeExternalLlmProvider {
            seen_context: seen_context.clone(),
        }) as Arc<dyn LlmProvider>;
        let (privacy_guard, _temp_dir) = make_external_privacy_guard_with_options(
            Some(ConsentPermissions {
                ocr_processing: true,
                screen_capture: true,
                full_text_extraction: true,
                ..Default::default()
            }),
            Some(WindowInfo {
                title: "Inbox user@example.com".to_string(),
                app_name: "Code".to_string(),
                app_bundle_id: None,
                pid: 23,
                bounds: None,
            }),
            None,
            PiiFilterLevel::Off,
            policy,
        );
        let guarded = super::guarded_llm::GuardedLlmProvider::new(inner, privacy_guard);

        guarded
            .interpret_intent(
                &ScreenContext {
                    visible_texts: vec!["email user@example.com".to_string()],
                    active_app: "Code".to_string(),
                    active_window_title: "Inbox user@example.com".to_string(),
                    layout_description: Some("Contact user@example.com".to_string()),
                },
                "click save",
            )
            .await
            .unwrap_or_else(|err| panic!("{policy:?} should allow sanitized context: {err}"));

        let sent = seen_context
            .lock()
            .unwrap()
            .clone()
            .expect("inner provider should receive sanitized context");
        assert!(
            !sent.visible_texts[0].contains("user@example.com"),
            "{policy:?} must pin filtering even when configured level is Off"
        );
    }
}

#[tokio::test]
async fn guarded_llm_provider_denies_sensitive_apps_before_external_call() {
    let seen_context = Arc::new(std::sync::Mutex::new(None));
    let inner = Arc::new(FakeExternalLlmProvider {
        seen_context: seen_context.clone(),
    }) as Arc<dyn LlmProvider>;
    let (privacy_guard, _temp_dir) = make_external_privacy_guard_with_permissions(
        Some(ConsentPermissions {
            ocr_processing: true,
            screen_capture: true,
            full_text_extraction: true,
            ..Default::default()
        }),
        Some(WindowInfo {
            title: "Vault".to_string(),
            app_name: "1Password".to_string(),
            app_bundle_id: None,
            pid: 24,
            bounds: None,
        }),
        None,
    );
    let guarded = super::guarded_llm::GuardedLlmProvider::new(inner, privacy_guard);

    let err = guarded
        .interpret_intent(
            &ScreenContext {
                visible_texts: vec!["password".to_string()],
                active_app: "1Password".to_string(),
                active_window_title: "Vault".to_string(),
                layout_description: None,
            },
            "read password",
        )
        .await
        .unwrap_err();

    assert!(err.to_string().contains("sensitive app"));
    assert!(seen_context.lock().unwrap().is_none());
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
        maekon_network::CircuitBreakerRegistry::new(),
        None, // #5734: llm_call_health — not exercised in these tests
    );
    let err = result
        .err()
        .expect("expected Err: remote OCR requires a privacy guard");
    assert!(err.to_string().contains("runtime privacy guard"));
}

#[test]
fn oauth_mode_requires_oauth_port() {
    let config = AiProviderConfig {
        access_mode: AiAccessMode::ProviderOAuth,
        llm_provider: LlmProviderType::Remote,
        ..AiProviderConfig::default()
    };

    let result = resolve_ai_provider_adapters(
        &config,
        PiiFilterLevel::Standard,
        None,
        None,
        None,
        maekon_network::CircuitBreakerRegistry::new(),
        None, // #5734: llm_call_health — not exercised in these tests
    );
    let err = result
        .err()
        .expect("expected Err: ProviderOAuth mode requires an OAuth port");
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
    let result = resolve_ai_provider_adapters(
        &config,
        PiiFilterLevel::Standard,
        None,
        None,
        Some(oauth),
        maekon_network::CircuitBreakerRegistry::new(),
        None, // #5734: llm_call_health — not exercised in these tests
    )
    .expect("ProviderOAuth mode should allow local LLM when no managed LLM surface is selected");
    // FU-3 (#5741): the explicit Local LLM choice now resolves the loopback
    // Ollama composite (LocalOllama), Ok-degrading to the bare rule matcher
    // (Local) only when provider construction fails.
    assert!(
        matches!(
            result.llm_source,
            ProviderSource::LocalOllama | ProviderSource::Local
        ),
        "unexpected llm_source: {:?}",
        result.llm_source
    );
}

#[test]
fn oauth_mode_defaults_to_openai_model() {
    let config = AiProviderConfig {
        access_mode: AiAccessMode::ProviderOAuth,
        llm_provider: LlmProviderType::Remote,
        ..AiProviderConfig::default()
    };

    let oauth = Arc::new(FakeOAuthPort { connected: true }) as Arc<dyn OAuthPort>;
    let (privacy_guard, _temp_dir) = make_external_privacy_guard_with_permissions(
        Some(ConsentPermissions {
            ocr_processing: true,
            screen_capture: true,
            full_text_extraction: true,
            ..Default::default()
        }),
        Some(WindowInfo {
            title: "OAuth LLM".to_string(),
            app_name: "Code".to_string(),
            app_bundle_id: None,
            pid: 33,
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
        maekon_network::CircuitBreakerRegistry::new(),
        None, // #5734: llm_call_health — not exercised in these tests
    )
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

    let (ocr, source, reason) = ocr_resolver::resolve_ocr_provider(
        &config,
        PiiFilterLevel::Standard,
        None,
        None,
        maekon_network::CircuitBreakerRegistry::new(),
    )
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
            app_bundle_id: None,
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
        maekon_network::CircuitBreakerRegistry::new(),
        None, // #5734: llm_call_health — not exercised in these tests
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
            app_bundle_id: None,
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
        maekon_network::CircuitBreakerRegistry::new(),
        None, // #5734: llm_call_health — not exercised in these tests
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
            app_bundle_id: None,
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
            app_bundle_id: None,
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
            app_bundle_id: None,
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
            app_bundle_id: None,
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

// ── E21 #4882/#4883: ExternalOcrPrivacyGuard as ConversationContentGuard ──

fn chat_message(content: &str) -> maekon_core::models::ai_session::SessionMessage {
    maekon_core::models::ai_session::SessionMessage {
        role: maekon_core::models::ai_session::MessageRole::User,
        content: content.to_string(),
        attachments: vec![],
        tools: None,
        context: None,
        response_format: None,
    }
}

#[tokio::test]
async fn sanitize_outbound_masks_pii_in_chat_content() {
    use super::guarded_conversation::ConversationContentGuard;

    let (guard, _temp_dir) = make_external_privacy_guard_with_permissions(
        Some(ConsentPermissions {
            ocr_processing: true,
            screen_capture: true,
            full_text_extraction: true,
            ..Default::default()
        }),
        Some(WindowInfo {
            title: "Editor".to_string(),
            app_name: "Code".to_string(),
            app_bundle_id: None,
            pid: 7,
            bounds: None,
        }),
        None,
    );

    let sanitized = guard
        .sanitize_outbound(&chat_message("please email user@example.com about it"))
        .await
        .expect("benign window + consent should allow sanitized transmission");

    assert!(
        !sanitized.content.contains("user@example.com"),
        "chat content PII must be masked before external transmission, got: {}",
        sanitized.content
    );
}

#[tokio::test]
async fn sanitize_outbound_fails_closed_without_active_window() {
    use super::guarded_conversation::ConversationContentGuard;

    // No active window → the shared external-LLM gate denies (fail-closed).
    let (guard, _temp_dir) = make_external_privacy_guard_with_permissions(
        Some(ConsentPermissions {
            ocr_processing: true,
            screen_capture: true,
            full_text_extraction: true,
            ..Default::default()
        }),
        None,
        None,
    );

    let result = guard.sanitize_outbound(&chat_message("hello")).await;

    assert!(
        matches!(
            result.unwrap_err(),
            maekon_core::error::CoreError::PolicyDenied { .. }
        ),
        "guard must fail closed with PolicyDenied when the active-window/consent gate denies"
    );
}

// ── E21 #4869: outbound is more than a single prompt (attachments + levels) ───

#[tokio::test]
async fn sanitize_outbound_masks_pii_in_attachments() {
    use super::guarded_conversation::ConversationContentGuard;
    use maekon_core::models::ai_session::Attachment;

    let (guard, _temp_dir) = make_external_privacy_guard_with_permissions(
        Some(ConsentPermissions {
            ocr_processing: true,
            screen_capture: true,
            full_text_extraction: true,
            ..Default::default()
        }),
        Some(WindowInfo {
            title: "Editor".to_string(),
            app_name: "Code".to_string(),
            app_bundle_id: None,
            pid: 7,
            bounds: None,
        }),
        None,
    );

    let mut message = chat_message("see attached");
    message.attachments = vec![
        Attachment::File {
            path: "/Users/jane.doe/finances.xlsx".to_string(),
            mime: Some("application/vnd.ms-excel".to_string()),
            data: None,
        },
        Attachment::AppReference {
            app_name: "Mail".to_string(),
            window_title: Some("Inbox jane.doe@example.com".to_string()),
        },
    ];

    let sanitized = guard
        .sanitize_outbound(&message)
        .await
        .expect("benign window + consent should allow sanitized transmission");

    // Attachments must be sanitized too — the outbound surface is not a single
    // prompt. Paths and window titles carry PII (usernames, email addresses).
    assert_eq!(
        sanitized.attachments.len(),
        2,
        "attachments preserved in count"
    );
    let serialized = serde_json::to_string(&sanitized.attachments).unwrap();
    assert!(
        !serialized.contains("jane.doe@example.com"),
        "attachment window-title PII must be masked, got: {serialized}"
    );
    assert!(
        !serialized.contains("/Users/jane.doe"),
        "attachment file-path PII must be masked, got: {serialized}"
    );
}

#[test]
fn sanitize_attachment_respects_filter_level() {
    use super::types::sanitize_attachment;
    use maekon_core::config::PiiFilterLevel;
    use maekon_core::models::ai_session::Attachment;

    let attachment = Attachment::File {
        path: "/Users/jane.doe/secret.txt".to_string(),
        mime: None,
        data: None,
    };

    // Off → passthrough (no masking).
    match sanitize_attachment(&attachment, PiiFilterLevel::Off) {
        Attachment::File { path, .. } => {
            assert_eq!(
                path, "/Users/jane.doe/secret.txt",
                "Off level must not mask"
            );
        }
        other => panic!("variant changed: {other:?}"),
    }

    // Strict → masked (the original PII path must not survive).
    match sanitize_attachment(&attachment, PiiFilterLevel::Strict) {
        Attachment::File { path, .. } => {
            assert_ne!(
                path, "/Users/jane.doe/secret.txt",
                "Strict level must mask the file path"
            );
        }
        other => panic!("variant changed: {other:?}"),
    }
}

// ── C3: LocalModel Ollama resolver tests ──────────────────────────────────────

#[test]
fn local_model_arm_resolves_ollama_source() {
    // Default config with LocalModel — catalog default (localhost:11434) is
    // loopback, so llm_source must be LocalOllama.
    let config = AiProviderConfig {
        access_mode: AiAccessMode::LocalModel,
        ..AiProviderConfig::default()
    };
    let adapters = resolve_ai_provider_adapters(
        &config,
        PiiFilterLevel::Standard,
        None,
        None,
        None,
        maekon_network::CircuitBreakerRegistry::new(),
        None, // #5734: llm_call_health — not exercised in these tests
    )
    .expect("LocalModel arm must not return Err");

    assert_eq!(
        adapters.llm_source,
        ProviderSource::LocalOllama,
        "catalog default endpoint is loopback → LocalOllama expected"
    );
    assert!(adapters.llm_fallback_reason.is_none());
    // is_external=false for loopback composite.
    assert!(!adapters.llm.is_external());
    // OCR still local.
    assert_eq!(adapters.ocr_source, ProviderSource::Local);
}

#[test]
fn local_model_arm_never_returns_err_with_guard_present() {
    // Decision 1 invariant: guard Some → LocalModel arm is infallible.
    let config = AiProviderConfig {
        access_mode: AiAccessMode::LocalModel,
        ..AiProviderConfig::default()
    };
    let (guard, _temp) = make_external_llm_guard();
    let result = resolve_ai_provider_adapters(
        &config,
        PiiFilterLevel::Standard,
        Some(guard),
        None,
        None,
        maekon_network::CircuitBreakerRegistry::new(),
        None, // #5734: llm_call_health — not exercised in these tests
    );
    result.expect("LocalModel arm must not return Err when guard is present");
}

#[test]
fn local_model_arm_never_returns_err_without_guard() {
    // Decision 1 invariant: guard None → still Ok-degrade (loopback skips guard).
    let config = AiProviderConfig {
        access_mode: AiAccessMode::LocalModel,
        ..AiProviderConfig::default()
    };
    let result = resolve_ai_provider_adapters(
        &config,
        PiiFilterLevel::Standard,
        None,
        None,
        None,
        maekon_network::CircuitBreakerRegistry::new(),
        None, // #5734: llm_call_health — not exercised in these tests
    );
    result.expect("LocalModel arm must not return Err (ok-degrade)");
}

#[test]
fn local_model_arm_llm_is_not_external_for_catalog_default() {
    // Catalog default: http://localhost:11434/v1/responses — loopback.
    // FallbackLlmProvider.is_external() must be false (primary loopback + fallback local).
    let config = AiProviderConfig {
        access_mode: AiAccessMode::LocalModel,
        ..AiProviderConfig::default()
    };
    let adapters = resolve_ai_provider_adapters(
        &config,
        PiiFilterLevel::Standard,
        None,
        None,
        None,
        maekon_network::CircuitBreakerRegistry::new(),
        None, // #5734: llm_call_health — not exercised in these tests
    )
    .expect("LocalModel arm must not return Err");
    assert!(
        !adapters.llm.is_external(),
        "loopback Ollama must not be classified as external"
    );
}

#[test]
fn provider_source_local_ollama_as_str() {
    assert_eq!(ProviderSource::LocalOllama.as_str(), "local-ollama");
}

#[test]
fn resolve_local_model_llm_provider_direct_loopback() {
    use super::llm_resolver::resolve_local_model_llm_provider;
    // Direct call to resolver with catalog default config.
    let config = AiProviderConfig {
        access_mode: AiAccessMode::LocalModel,
        ..AiProviderConfig::default()
    };
    let (llm, source, reason) = resolve_local_model_llm_provider(
        &config,
        None,
        maekon_network::CircuitBreakerRegistry::new(),
        None, // Decision 2: no health flag in unit tests
    )
    .expect("resolver must not return Err");
    assert_eq!(source, ProviderSource::LocalOllama);
    assert!(reason.is_none());
    assert!(!llm.is_external());
}

#[test]
fn resolve_local_model_llm_provider_not_analysis_twin() {
    // This test verifies the not(analysis) twin behaves correctly.
    // Since we ARE under feature = "analysis", we test the analysis path;
    // the not(analysis) twin is separately compile-tested by `cargo check
    // -p maekon-app --no-default-features`.
    //
    // Under analysis: catalog default → LocalOllama.
    use super::llm_resolver::resolve_local_model_llm_provider;
    let config = AiProviderConfig::default();
    let (_, source, _) = resolve_local_model_llm_provider(
        &config,
        None,
        maekon_network::CircuitBreakerRegistry::new(),
        None, // Decision 2: no health flag in unit tests
    )
    .expect("must not err");
    // Under analysis feature: LocalOllama (catalog loopback).
    assert_eq!(source, ProviderSource::LocalOllama);
}

// ── C3: Behavioral mock HTTP + circuit-breaker + HITL invariant tests ─────────

/// C3 behavioral test: mock `/v1/responses` on a loopback OS-assigned port
/// returns a valid OpenAI Responses API body → `interpret_intent` returns a
/// parsed `InterpretedAction`.
///
/// Uses a fresh `CircuitBreakerRegistry` per test (ADR-075 P-4 / regression
/// note: shared module-global state causes flakes across test threads).
/// Does NOT contact `localhost:11434` — all traffic goes to the mock server.
#[cfg(feature = "analysis")]
#[tokio::test]
async fn behavioral_mock_http_ollama_intent_succeeds() {
    use maekon_core::config::{AiProviderType, ExternalApiEndpoint};
    use maekon_core::ports::llm_provider::{LlmProvider, ScreenContext};
    use maekon_network::ai_llm_client::RemoteLlmProvider;

    // Spin up a mock server on an OS-assigned port (never :11434).
    let mut server = mockito::Server::new_async().await;
    // Fake /v1/responses returns an OpenAI Responses API shape with JSON action.
    let _mock = server
        .mock("POST", "/v1/responses")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"output_text": "{\"target_text\": \"Save\", \"target_role\": \"button\", \"action_type\": \"click\", \"confidence\": 0.92}"}"#)
        .create_async()
        .await;

    // Build a RemoteLlmProvider targeting the mock server with Ollama surface.
    let endpoint = ExternalApiEndpoint {
        endpoint: format!("{}/v1/responses", server.url()),
        api_key: String::new(),
        model: Some("qwen3:8b".to_string()),
        timeout_secs: 5,
        provider_type: AiProviderType::Ollama,
        surface_id: Some("provider_surface.ollama.local_http".to_string()),
        credential: None,
    };
    let provider = RemoteLlmProvider::new(&endpoint, maekon_network::CircuitBreakerRegistry::new())
        .expect("provider init")
        .with_token_budget(2048); // Decision 4 seam exercised

    let ctx = ScreenContext {
        visible_texts: vec!["File".to_string(), "Save".to_string()],
        active_app: "Code".to_string(),
        active_window_title: "main.rs".to_string(),
        layout_description: None,
    };
    let action = provider
        .interpret_intent(&ctx, "click save button")
        .await
        .expect("mock server returned valid JSON action");

    assert_eq!(action.action_type, "click");
    assert_eq!(action.target_text.as_deref(), Some("Save"));
    assert_eq!(action.target_role.as_deref(), Some("button"));
    assert!((action.confidence - 0.92).abs() < f64::EPSILON);
}

/// C3 behavioral test: when the primary Ollama provider gets a connection-refused
/// response, `FallbackLlmProvider` catches the error and routes to the rule
/// matcher, which succeeds.
///
/// Uses `FallbackLlmProvider` directly (not via the full resolver) to avoid
/// requiring a live mock server — just a deliberately wrong port so
/// ECONNREFUSED fires instantly.
#[cfg(feature = "analysis")]
#[tokio::test]
async fn behavioral_connection_refused_falls_back_to_rule_matcher() {
    use std::sync::Arc;

    use maekon_automation::local_llm::LocalLlmProvider;
    use maekon_core::config::{AiProviderType, ExternalApiEndpoint};
    use maekon_core::ports::llm_provider::{LlmProvider, ScreenContext};
    use maekon_network::ai_llm_client::RemoteLlmProvider;

    use super::fallback_llm::{FallbackLlmProvider, LoopbackLlmProvider};

    // Port 1 is always refused on macOS/Linux (privileged, never bound by user).
    let refused_endpoint = ExternalApiEndpoint {
        endpoint: "http://127.0.0.1:1/v1/responses".to_string(),
        api_key: String::new(),
        model: Some("qwen3:8b".to_string()),
        timeout_secs: 2, // short timeout for fast test
        provider_type: AiProviderType::Ollama,
        surface_id: Some("provider_surface.ollama.local_http".to_string()),
        credential: None,
    };
    // Fresh registry — isolated from any module-global breaker state.
    let registry = maekon_network::CircuitBreakerRegistry::new();
    let remote =
        RemoteLlmProvider::new(&refused_endpoint, registry).expect("provider construction ok");
    // Wrap in LoopbackLlmProvider (is_external=false), matching the resolver path.
    let primary: Arc<dyn LlmProvider> = Arc::new(LoopbackLlmProvider::new(Arc::new(remote)));
    let fallback: Arc<dyn LlmProvider> = Arc::new(LocalLlmProvider::new());
    let composite = FallbackLlmProvider::new(primary, fallback);

    let ctx = ScreenContext {
        visible_texts: vec!["click".to_string()],
        active_app: "Code".to_string(),
        active_window_title: "main.rs".to_string(),
        layout_description: None,
    };
    // Connection-refused → primary fails → FallbackLlmProvider routes to
    // LocalLlmProvider (rule matcher) → plan succeeds.
    let action = composite
        .interpret_intent(&ctx, "click something")
        .await
        .expect("fallback to rule matcher must succeed on connection-refused primary");

    // LocalLlmProvider returns an action (action_type may vary by keyword match).
    // We only assert it didn't return Err — the composite absorbed the primary error.
    assert!(
        !action.action_type.is_empty(),
        "rule-matcher fallback must return a non-empty action_type"
    );
    assert!(
        !composite.is_external(),
        "loopback composite must not be external"
    );
}

/// C3 circuit-breaker test: after the failure threshold is reached, subsequent
/// calls fast-fail with `CircuitOpen` without hitting the server, and
/// `FallbackLlmProvider` degrades to the rule matcher immediately.
///
/// Fresh `CircuitBreakerRegistry` per test to avoid cross-test state pollution
/// (ADR-075 P-4 / serial_test precedent in maekon-network).
#[cfg(feature = "analysis")]
#[tokio::test]
async fn behavioral_breaker_open_falls_back_to_rule_matcher() {
    use std::sync::Arc;

    use maekon_automation::local_llm::LocalLlmProvider;
    use maekon_core::config::{AiProviderType, ExternalApiEndpoint};
    use maekon_core::ports::llm_provider::{LlmProvider, ScreenContext};
    use maekon_network::ai_llm_client::RemoteLlmProvider;

    use super::fallback_llm::{FallbackLlmProvider, LoopbackLlmProvider};

    let mut server = mockito::Server::new_async().await;
    // Return 503 to trip the breaker (classify_for_breaker: 5xx=Failure).
    let _mock = server
        .mock("POST", "/v1/responses")
        .with_status(503)
        .with_body("down")
        .create_async()
        .await;

    // Configure a fast breaker: threshold=3, cooldown=100ms.
    let registry = maekon_network::CircuitBreakerRegistry::new();
    let key = maekon_network::resilience::endpoint_authority(&server.url()).unwrap();
    let _ = registry.get_with_config(
        &key,
        maekon_network::circuit_breaker::CircuitBreakerConfig {
            failure_threshold: 3,
            initial_cooldown: std::time::Duration::from_millis(100),
            max_cooldown: std::time::Duration::from_millis(500),
            half_open_probes: 1,
        },
    );

    let endpoint = ExternalApiEndpoint {
        endpoint: format!("{}/v1/responses", server.url()),
        api_key: String::new(),
        model: Some("qwen3:8b".to_string()),
        timeout_secs: 5,
        provider_type: AiProviderType::Ollama,
        surface_id: Some("provider_surface.ollama.local_http".to_string()),
        credential: None,
    };
    let remote = RemoteLlmProvider::new(&endpoint, registry).expect("provider init");
    let primary: Arc<dyn LlmProvider> = Arc::new(LoopbackLlmProvider::new(Arc::new(remote)));
    let fallback: Arc<dyn LlmProvider> = Arc::new(LocalLlmProvider::new());
    let composite = FallbackLlmProvider::new(primary, fallback);

    let ctx = ScreenContext {
        visible_texts: vec!["click".to_string()],
        active_app: "App".to_string(),
        active_window_title: "Win".to_string(),
        layout_description: None,
    };

    // Trip the breaker (threshold=3).
    for _ in 0..3 {
        let _ = composite.interpret_intent(&ctx, "click").await;
    }

    // After threshold: CircuitOpen fast-fail → FallbackLlmProvider routes to
    // rule matcher.  The rule matcher itself succeeds (no server hit).
    let action = composite
        .interpret_intent(&ctx, "click")
        .await
        .expect("after breaker opens, fallback to rule matcher must succeed");

    assert!(
        !action.action_type.is_empty(),
        "rule-matcher fallback must return a non-empty action_type after breaker opens"
    );
}

// ── HITL invariant regression pin ─────────────────────────────────────────────
//
// Decision 7: execute_intent_hint runs under INTENT_HINT_POLICY_TOKEN, which
// resolves default_strict_config and skips PolicyClient.validate_command.
// C3 changes only plan quality (LLM vs rule matcher), not authority.
//
// This test pins the current semantics: the LocalModel arm must not introduce
// any new confirmation paths or widen execution authority.

#[test]
fn local_model_arm_ocr_source_unchanged_by_c3() {
    // OCR is immutable under LocalModel arm — always best_local_ocr_provider().
    let config = AiProviderConfig {
        access_mode: AiAccessMode::LocalModel,
        // Even if remote OCR is configured, LocalModel ignores it.
        ocr_provider: OcrProviderType::Remote,
        ocr_api: Some(remote_endpoint()),
        ..AiProviderConfig::default()
    };
    let adapters = resolve_ai_provider_adapters(
        &config,
        PiiFilterLevel::Standard,
        None,
        None,
        None,
        maekon_network::CircuitBreakerRegistry::new(),
        None, // #5734: llm_call_health — not exercised in these tests
    )
    .expect("LocalModel arm must not return Err");
    assert_eq!(adapters.ocr_source, ProviderSource::Local);
    assert!(!adapters.ocr.is_external());
    assert!(adapters.ocr_fallback_reason.is_none());
}
