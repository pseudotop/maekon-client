use std::path::PathBuf;
use std::sync::Arc;

use oneshim_automation::audit::AuditLogger;
use oneshim_core::config::{ExternalDataPolicy, PiiFilterLevel, PrivacyConfig};
use oneshim_core::error::CoreError;
use oneshim_core::ports::llm_provider::LlmProvider;
use oneshim_core::ports::monitor::ProcessMonitor;
use oneshim_core::ports::ocr_provider::OcrProvider;
use tokio::sync::RwLock;

use oneshim_core::consent::ConsentManager;
use oneshim_vision::privacy_gateway::{PrivacyGateway, SanitizedImage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Remote and OAuth variants pending provider integration
pub enum ProviderSource {
    Local,
    Remote,
    LocalFallback,
    CliSubscription,
    OAuth,
}

impl ProviderSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote => "remote",
            Self::LocalFallback => "local-fallback",
            Self::CliSubscription => "cli-subscription",
            Self::OAuth => "oauth",
        }
    }
}

pub struct AiProviderAdapters {
    pub ocr: Arc<dyn OcrProvider>,
    pub llm: Arc<dyn LlmProvider>,
    pub ocr_source: ProviderSource,
    pub llm_source: ProviderSource,
    pub ocr_fallback_reason: Option<String>,
    pub llm_fallback_reason: Option<String>,
}

pub(super) type OcrProviderResolution =
    Result<(Arc<dyn OcrProvider>, ProviderSource, Option<String>), CoreError>;
pub(super) type LlmProviderResolution =
    Result<(Arc<dyn LlmProvider>, ProviderSource, Option<String>), CoreError>;

#[cfg_attr(not(feature = "server"), allow(dead_code))]
#[derive(Clone)]
pub struct ExternalOcrPrivacyGuard {
    consent_path: PathBuf,
    pii_filter_level: PiiFilterLevel,
    external_data_policy: ExternalDataPolicy,
    privacy_config: PrivacyConfig,
    process_monitor: Arc<dyn ProcessMonitor>,
    audit_logger: Option<Arc<RwLock<AuditLogger>>>,
}

#[cfg_attr(not(feature = "server"), allow(dead_code))]
impl ExternalOcrPrivacyGuard {
    pub fn new(
        consent_path: PathBuf,
        pii_filter_level: PiiFilterLevel,
        external_data_policy: ExternalDataPolicy,
        privacy_config: PrivacyConfig,
        process_monitor: Arc<dyn ProcessMonitor>,
        audit_logger: Option<Arc<RwLock<AuditLogger>>>,
    ) -> Self {
        Self {
            consent_path,
            pii_filter_level,
            external_data_policy,
            privacy_config,
            process_monitor,
            audit_logger,
        }
    }

    pub(super) async fn prepare_image_for_external(
        &self,
        image_data: &[u8],
        provider_name: &str,
        allow_unredacted_external_ocr: bool,
    ) -> Result<SanitizedImage, CoreError> {
        let active_window = match self.process_monitor.get_active_window().await? {
            Some(window) => window,
            None => {
                let message = "External OCR blocked: active window context unavailable".to_string();
                self.log_event(
                    "privacy.external_ocr.denied",
                    &format!("provider={provider_name} reason=no_active_window"),
                )
                .await;
                return Err(CoreError::PolicyDenied {
                    code: oneshim_core::error_codes::PolicyCode::Denied,
                    message,
                });
            }
        };

        let gateway = PrivacyGateway::new(
            Arc::new(ConsentManager::new(self.consent_path.clone())),
            self.pii_filter_level,
            self.external_data_policy,
            self.privacy_config.clone(),
        );

        match gateway
            .prepare_image_for_external_with_override(
                image_data,
                &active_window.app_name,
                &active_window.title,
                allow_unredacted_external_ocr,
            )
            .await
        {
            Ok(sanitized) => {
                self.log_event(
                    "privacy.external_ocr.allowed",
                    &format!(
                        "provider={provider_name} app={} title={} redacted_regions={} metadata_stripped={}",
                        active_window.app_name,
                        active_window.title,
                        sanitized.redacted_regions,
                        sanitized.metadata_stripped
                    ),
                )
                .await;
                Ok(sanitized)
            }
            Err(err) => {
                self.log_event(
                    "privacy.external_ocr.denied",
                    &format!(
                        "provider={provider_name} app={} title={} reason={}",
                        active_window.app_name, active_window.title, err
                    ),
                )
                .await;
                Err(CoreError::PolicyDenied {
                    code: oneshim_core::error_codes::PolicyCode::Denied,
                    message: format!("External OCR blocked: {err}"),
                })
            }
        }
    }

    async fn log_event(&self, action_type: &str, details: &str) {
        let Some(audit_logger) = self.audit_logger.as_ref() else {
            return;
        };

        let mut logger = audit_logger.write().await;
        logger.log_event(action_type, "runtime-ocr", details);
    }
}
