use std::sync::Arc;

use maekon_automation::audit::AuditLogger;
use maekon_core::config::{ExternalDataPolicy, PiiFilterLevel, PrivacyConfig};
use maekon_core::error::CoreError;
use maekon_core::ports::consent_manager::ConsentManagerPort;
use maekon_core::ports::llm_provider::{LlmProvider, ScreenContext};
use maekon_core::ports::monitor::ProcessMonitor;
use maekon_core::ports::ocr_provider::OcrProvider;
use tokio::sync::RwLock;

use maekon_vision::privacy_gateway::{PrivacyGateway, SanitizedImage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Remote and OAuth variants pending provider integration
pub enum ProviderSource {
    Local,
    /// LocalModel arm with a live Ollama backend (loopback, no egress).
    /// Surfaced as "local-ollama" in AiRuntimeStatus and builder logs.
    LocalOllama,
    Remote,
    LocalFallback,
    CliSubscription,
    OAuth,
}

impl ProviderSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::LocalOllama => "local-ollama",
            Self::Remote => "remote",
            Self::LocalFallback => "local-fallback",
            Self::CliSubscription => "cli-subscription",
            Self::OAuth => "oauth",
        }
    }
}

// ── Loopback endpoint classifier ──────────────────────────────────────────────
//
// Used by `resolve_local_model_llm_provider` to decide whether a configured
// Ollama base URL is loopback (no egress gate needed) or an external host
// (must route through `guard_external_llm_provider`).
//
// Reuses `maekon_network::http_client::host_is_loopback` which is already
// fail-closed: unparseable / empty host → returns `false` (treated as external).
#[cfg(feature = "analysis")]
pub(super) fn endpoint_is_loopback(url: &str) -> bool {
    maekon_network::http_client::host_is_loopback(url)
}

#[cfg(not(feature = "analysis"))]
pub(super) fn endpoint_is_loopback(_url: &str) -> bool {
    false
}

pub(crate) fn ensure_non_external_endpoints_are_loopback(
    provider_name: &str,
    endpoint_urls: Vec<&str>,
) -> Result<(), CoreError> {
    for endpoint in endpoint_urls {
        if !endpoint_is_loopback(endpoint) {
            return Err(CoreError::PolicyDenied {
                code: maekon_core::error_codes::PolicyCode::Denied,
                message: format!(
                    "Provider '{provider_name}' reported non-external, but its endpoint is not loopback; refusing unguarded external egress"
                ),
            });
        }
    }
    Ok(())
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

pub(super) struct SanitizedExternalText {
    pub(super) context_json: String,
    pub(super) system_prompt: String,
}

#[cfg_attr(not(feature = "analysis"), allow(dead_code))]
#[derive(Clone)]
pub struct ExternalOcrPrivacyGuard {
    consent_manager: Arc<dyn ConsentManagerPort>,
    pii_filter_level: PiiFilterLevel,
    external_data_policy: ExternalDataPolicy,
    privacy_config: PrivacyConfig,
    process_monitor: Arc<dyn ProcessMonitor>,
    audit_logger: Option<Arc<RwLock<AuditLogger>>>,
}

#[cfg_attr(not(feature = "analysis"), allow(dead_code))]
impl ExternalOcrPrivacyGuard {
    pub fn new(
        consent_manager: Arc<dyn ConsentManagerPort>,
        pii_filter_level: PiiFilterLevel,
        external_data_policy: ExternalDataPolicy,
        privacy_config: PrivacyConfig,
        process_monitor: Arc<dyn ProcessMonitor>,
        audit_logger: Option<Arc<RwLock<AuditLogger>>>,
    ) -> Self {
        Self {
            consent_manager,
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
                    code: maekon_core::error_codes::PolicyCode::Denied,
                    message,
                });
            }
        };

        let gateway = PrivacyGateway::new(
            self.consent_manager.clone(),
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
                    // Do not write the raw window title to the audit log: it can carry
                    // document/page names (e.g. "2024_TaxReturn.xlsx") that are NOT
                    // PII-pattern-matched and so survive the Strict audit sanitizer.
                    // Record only a length marker for forensics.
                    &format!(
                        "provider={provider_name} app={} title_chars={} redacted_regions={} metadata_stripped={}",
                        active_window.app_name,
                        active_window.title.chars().count(),
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
                        "provider={provider_name} app={} title_chars={} reason={}",
                        active_window.app_name,
                        active_window.title.chars().count(),
                        err
                    ),
                )
                .await;
                Err(CoreError::PolicyDenied {
                    code: maekon_core::error_codes::PolicyCode::Denied,
                    message: format!("External OCR blocked: {err}"),
                })
            }
        }
    }

    pub(super) async fn prepare_screen_context_for_external_llm(
        &self,
        screen_context: &ScreenContext,
        provider_name: &str,
    ) -> Result<ScreenContext, CoreError> {
        let active_window = self
            .ensure_external_llm_allowed(provider_name, "screen")
            .await?;

        let filter_level = self.external_llm_filter_level();
        let sanitized = ScreenContext {
            visible_texts: screen_context
                .visible_texts
                .iter()
                .map(|text| maekon_vision::privacy::sanitize_title_with_level(text, filter_level))
                .collect(),
            active_app: screen_context.active_app.clone(),
            active_window_title: maekon_vision::privacy::sanitize_title_with_level(
                &screen_context.active_window_title,
                filter_level,
            ),
            layout_description: screen_context.layout_description.as_deref().map(|layout| {
                maekon_vision::privacy::sanitize_title_with_level(layout, filter_level)
            }),
        };

        self.log_event(
            "privacy.external_llm.allowed",
            &format!(
                "provider={provider_name} app={} text_items={} has_layout={}",
                active_window.app_name,
                sanitized.visible_texts.len(),
                sanitized.layout_description.is_some()
            ),
        )
        .await;
        Ok(sanitized)
    }

    pub(super) async fn prepare_text_for_external_llm(
        &self,
        context_json: &str,
        system_prompt: &str,
        provider_name: &str,
        purpose: &str,
    ) -> Result<SanitizedExternalText, CoreError> {
        let active_window = self
            .ensure_external_llm_allowed(provider_name, purpose)
            .await?;
        let filter_level = self.external_llm_filter_level();
        let context_json =
            maekon_vision::privacy::sanitize_title_with_level(context_json, filter_level);
        let system_prompt =
            maekon_vision::privacy::sanitize_title_with_level(system_prompt, filter_level);

        self.log_event(
            "privacy.external_llm.allowed",
            &format!(
                "provider={provider_name} purpose={purpose} app={} context_chars={} prompt_chars={}",
                self.audit_safe_fragment(&active_window.app_name),
                context_json.len(),
                system_prompt.len()
            ),
        )
        .await;

        Ok(SanitizedExternalText {
            context_json,
            system_prompt,
        })
    }

    async fn ensure_external_llm_allowed(
        &self,
        provider_name: &str,
        purpose: &str,
    ) -> Result<maekon_core::models::context::WindowInfo, CoreError> {
        let active_window = match self.process_monitor.get_active_window().await? {
            Some(window) => window,
            None => {
                self.log_event(
                    "privacy.external_llm.denied",
                    &format!("provider={provider_name} purpose={purpose} reason=no_active_window"),
                )
                .await;
                return Err(CoreError::PolicyDenied {
                    code: maekon_core::error_codes::PolicyCode::Denied,
                    message: "External LLM blocked: active window context unavailable".to_string(),
                });
            }
        };

        if !self
            .consent_manager
            .effective_permissions()
            .full_text_extraction
        {
            self.log_event(
                "privacy.external_llm.denied",
                &format!(
                    "provider={provider_name} purpose={purpose} reason=full_text_extraction_missing"
                ),
            )
            .await;
            return Err(CoreError::PolicyDenied {
                code: maekon_core::error_codes::PolicyCode::Denied,
                message: "External LLM blocked: full text extraction consent is required"
                    .to_string(),
            });
        }

        if maekon_vision::privacy::is_sensitive_app(&active_window.app_name) {
            self.log_event(
                "privacy.external_llm.denied",
                &format!(
                    "provider={provider_name} purpose={purpose} app={} reason=sensitive_app",
                    self.audit_safe_fragment(&active_window.app_name),
                ),
            )
            .await;
            return Err(CoreError::PolicyDenied {
                code: maekon_core::error_codes::PolicyCode::Denied,
                message: format!(
                    "External LLM blocked: sensitive app {}",
                    self.audit_safe_fragment(&active_window.app_name)
                ),
            });
        }

        if maekon_vision::privacy::should_exclude(
            &active_window.app_name,
            &active_window.title,
            &self.privacy_config.excluded_apps,
            &self.privacy_config.excluded_app_patterns,
            &self.privacy_config.excluded_title_patterns,
            self.privacy_config.auto_exclude_sensitive,
        ) {
            self.log_event(
                "privacy.external_llm.denied",
                &format!(
                    "provider={provider_name} purpose={purpose} app={} reason=excluded_by_policy",
                    self.audit_safe_fragment(&active_window.app_name),
                ),
            )
            .await;
            return Err(CoreError::PolicyDenied {
                code: maekon_core::error_codes::PolicyCode::Denied,
                message: "External LLM blocked: active surface excluded by policy".to_string(),
            });
        }

        Ok(active_window)
    }

    fn external_llm_filter_level(&self) -> PiiFilterLevel {
        self.external_data_policy
            .effective_egress_pii_level(self.pii_filter_level)
    }

    fn audit_safe_fragment(&self, text: &str) -> String {
        maekon_vision::privacy::sanitize_title_with_level(text, PiiFilterLevel::Strict)
    }

    async fn log_event(&self, action_type: &str, details: &str) {
        let Some(audit_logger) = self.audit_logger.as_ref() else {
            return;
        };

        let mut logger = audit_logger.write().await;
        logger.log_event(action_type, "runtime-ai-egress", details);
    }
}

// ── ConversationContentGuard (E21 #4882/#4883/#4869) ──────────────────────────
//
// Implements the narrow chat-path guard port using the same fail-closed consent
// gate and PII filter as the screen/text external-LLM paths. Lives in types.rs
// (not guarded_conversation.rs) so it can reach the private gate helpers.

/// Sanitize the human-readable text fields of an attachment before it egresses
/// to an external provider (E21 #4869). Paths, window titles, app names, and
/// skill display names can all carry PII (usernames in paths, document names in
/// titles), so they are run through the same level-aware filter as chat content.
///
/// Inline binary/text `data` blobs are stripped before external provider
/// egress. The title-level masker is only safe for metadata; preserving raw
/// attachment bodies would bypass the consented text/path sanitization surface.
pub(super) fn sanitize_attachment(
    attachment: &maekon_core::models::ai_session::Attachment,
    level: PiiFilterLevel,
) -> maekon_core::models::ai_session::Attachment {
    use maekon_core::models::ai_session::Attachment;
    fn mask(text: &str, level: PiiFilterLevel) -> String {
        maekon_vision::privacy::sanitize_title_with_level(text, level)
    }
    match attachment {
        Attachment::Image {
            mime,
            path,
            data: _,
        } => Attachment::Image {
            mime: mime.clone(),
            path: path.as_ref().map(|p| mask(p, level)),
            data: None,
        },
        Attachment::File {
            path,
            mime,
            data: _,
        } => Attachment::File {
            path: mask(path, level),
            mime: mime.clone(),
            data: None,
        },
        Attachment::Directory { path } => Attachment::Directory {
            path: mask(path, level),
        },
        Attachment::Skill {
            skill_id,
            display_name,
        } => Attachment::Skill {
            skill_id: skill_id.clone(),
            display_name: display_name.as_ref().map(|d| mask(d, level)),
        },
        Attachment::AppReference {
            app_name,
            window_title,
        } => Attachment::AppReference {
            app_name: mask(app_name, level),
            window_title: window_title.as_ref().map(|w| mask(w, level)),
        },
    }
}

#[async_trait::async_trait]
impl super::guarded_conversation::ConversationContentGuard for ExternalOcrPrivacyGuard {
    async fn sanitize_outbound(
        &self,
        message: &maekon_core::models::ai_session::SessionMessage,
    ) -> Result<maekon_core::models::ai_session::SessionMessage, CoreError> {
        // Fail-closed: consent / active-window / sensitive-app / exclusion gate.
        let active_window = self
            .ensure_external_llm_allowed("conversation", "chat")
            .await?;

        let level = self.external_llm_filter_level();
        let mut sanitized = message.clone();
        sanitized.content =
            maekon_vision::privacy::sanitize_title_with_level(&message.content, level);
        if let Some(context) = sanitized.context.as_mut() {
            if let Some(app) = context.active_app.as_deref() {
                context.active_app = Some(maekon_vision::privacy::sanitize_title_with_level(
                    app, level,
                ));
            }
            if let Some(regime) = context.regime.as_deref() {
                context.regime = Some(maekon_vision::privacy::sanitize_title_with_level(
                    regime, level,
                ));
            }
        }

        // #4869: outbound is not a single prompt — attachments (paths/titles/
        // names) egress too, so each one is sanitized at the same filter level.
        sanitized.attachments = message
            .attachments
            .iter()
            .map(|attachment| sanitize_attachment(attachment, level))
            .collect();

        self.log_event(
            "privacy.external_llm.allowed",
            &format!(
                "provider=conversation purpose=chat app={} content_chars={} attachments={}",
                self.audit_safe_fragment(&active_window.app_name),
                sanitized.content.len(),
                sanitized.attachments.len()
            ),
        )
        .await;

        Ok(sanitized)
    }
}
