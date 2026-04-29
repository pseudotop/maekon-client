use std::path::Path;
use std::sync::Arc;

use oneshim_automation::audit::AuditLogger;
use oneshim_automation::controller::AutomationController;
use oneshim_automation::policy::PolicyClient;
use oneshim_automation::sandbox::create_platform_sandbox;
use oneshim_core::config::{AiAccessMode, AiProviderConfig, AppConfig};
use oneshim_core::ports::skill_loader::SkillLoader;
#[cfg(feature = "server")]
use oneshim_core::ports::{oauth::OAuthPort, secret_store::SecretStoreSet};
use oneshim_monitor::process::ProcessTracker;
use oneshim_storage::frame_storage::FrameFileStorage;
use oneshim_web::AiRuntimeStatus;
use tokio::runtime::Handle;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::automation_runtime::{build_automation_runtime, build_noop_intent_executor};
use crate::provider_adapters::ExternalOcrPrivacyGuard;

#[derive(Clone)]
pub(crate) struct AutomationControllerBuildResult {
    pub(crate) controller: Option<Arc<AutomationController>>,
    pub(crate) ai_runtime_status: Option<AiRuntimeStatus>,
}

pub(crate) struct AutomationControllerBuilder<'a> {
    config: &'a AppConfig,
    data_dir: &'a Path,
    _runtime_handle: &'a Handle,
    audit_logger: Arc<RwLock<AuditLogger>>,
    frame_storage: Option<Arc<FrameFileStorage>>,
    app_handle: Option<tauri::AppHandle>,
    cli_health_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
    #[cfg(feature = "server")]
    provider_secret_stores: Option<SecretStoreSet>,
    #[cfg(feature = "server")]
    oauth_port: Option<Arc<dyn OAuthPort>>,
}

impl<'a> AutomationControllerBuilder<'a> {
    pub(crate) fn new(
        config: &'a AppConfig,
        data_dir: &'a Path,
        runtime_handle: &'a Handle,
        audit_logger: Arc<RwLock<AuditLogger>>,
        frame_storage: Option<Arc<FrameFileStorage>>,
    ) -> Self {
        Self {
            config,
            data_dir,
            _runtime_handle: runtime_handle,
            audit_logger,
            frame_storage,
            app_handle: None,
            cli_health_flag: None,
            #[cfg(feature = "server")]
            provider_secret_stores: None,
            #[cfg(feature = "server")]
            oauth_port: None,
        }
    }

    pub(crate) fn with_cli_health_flag(mut self, flag: Arc<std::sync::atomic::AtomicBool>) -> Self {
        self.cli_health_flag = Some(flag);
        self
    }

    #[cfg(feature = "server")]
    pub(crate) fn with_provider_secret_stores(mut self, secret_stores: SecretStoreSet) -> Self {
        self.provider_secret_stores = Some(secret_stores);
        self
    }

    pub(crate) fn with_app_handle(mut self, handle: tauri::AppHandle) -> Self {
        self.app_handle = Some(handle);
        self
    }

    #[cfg(feature = "server")]
    pub(crate) fn with_oauth_port(mut self, oauth_port: Option<Arc<dyn OAuthPort>>) -> Self {
        self.oauth_port = oauth_port;
        self
    }

    pub(crate) fn build(self) -> AutomationControllerBuildResult {
        if !self.config.automation.enabled {
            return AutomationControllerBuildResult {
                controller: None,
                ai_runtime_status: None,
            };
        }

        let process_monitor = Arc::new(ProcessTracker::new());
        let external_ocr_privacy_guard = ExternalOcrPrivacyGuard::new(
            self.data_dir.join("consent.json"),
            self.config.privacy.pii_filter_level,
            self.config.ai_provider.external_data_policy,
            self.config.privacy.clone(),
            process_monitor.clone(),
            Some(self.audit_logger.clone()),
        );
        let skill_loader = discover_skill_loader();

        #[cfg(feature = "server")]
        let runtime = preflight_provider_oauth_connection(
            self._runtime_handle,
            &self.config.ai_provider,
            self.oauth_port.clone(),
        )
        .and_then(|validated_oauth_port| {
            build_automation_runtime(
                &self.config.ai_provider,
                self.config.privacy.pii_filter_level,
                self.frame_storage.clone(),
                Some(external_ocr_privacy_guard.clone()),
                skill_loader.clone(),
                self.provider_secret_stores.clone(),
                validated_oauth_port,
            )
        });

        #[cfg(not(feature = "server"))]
        let runtime = build_automation_runtime(
            &self.config.ai_provider,
            self.config.privacy.pii_filter_level,
            self.frame_storage.clone(),
            Some(external_ocr_privacy_guard),
            skill_loader,
            None,
        );

        match runtime {
            Ok(runtime) => AutomationControllerBuildResult {
                ai_runtime_status: Some(AiRuntimeStatus {
                    ocr_source: runtime.ocr_source.as_str().to_string(),
                    llm_source: runtime.llm_source.as_str().to_string(),
                    ocr_fallback_reason: runtime.ocr_fallback_reason.clone(),
                    llm_fallback_reason: runtime.llm_fallback_reason.clone(),
                }),
                controller: Some(Arc::new(build_controller_from_runtime(
                    self.config,
                    self.audit_logger,
                    process_monitor,
                    runtime,
                    self.app_handle,
                    self.cli_health_flag.clone(),
                ))),
            },
            Err(err) => {
                if should_fallback_to_noop(&self.config.ai_provider) {
                    let fallback_reason = err.to_string();
                    warn!(error = %err, "AI provider fallback to NoOp executor");
                    return AutomationControllerBuildResult {
                        ai_runtime_status: Some(AiRuntimeStatus {
                            ocr_source: "local-fallback".to_string(),
                            llm_source: "local-fallback".to_string(),
                            ocr_fallback_reason: Some(fallback_reason.clone()),
                            llm_fallback_reason: Some(fallback_reason),
                        }),
                        controller: Some(Arc::new(build_noop_controller(
                            self.config,
                            self.audit_logger,
                        ))),
                    };
                }

                let ai_runtime_status =
                    if self.config.ai_provider.access_mode == AiAccessMode::ProviderOAuth {
                        Some(oauth_runtime_error_status(
                            &self.config.ai_provider,
                            err.to_string(),
                        ))
                    } else {
                        None
                    };
                error!(error = %err, "AI provider failed, automation disabled");
                AutomationControllerBuildResult {
                    controller: None,
                    ai_runtime_status,
                }
            }
        }
    }
}

fn discover_skill_loader() -> Option<Arc<dyn SkillLoader>> {
    let mut roots = Vec::new();
    if let Some(home) = directories::BaseDirs::new() {
        roots.push(home.home_dir().to_path_buf());
    }
    let loader = crate::skill_loader::FileSkillLoader::new(roots);
    if loader.list_skills().is_empty() {
        None
    } else {
        Some(Arc::new(loader))
    }
}

fn build_controller_from_runtime(
    config: &AppConfig,
    audit_logger: Arc<RwLock<AuditLogger>>,
    process_monitor: Arc<ProcessTracker>,
    runtime: crate::automation_runtime::AutomationRuntime,
    app_handle: Option<tauri::AppHandle>,
    cli_health_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
) -> AutomationController {
    // Clone handle early so we can wire the confirmation callback after
    // the overlay driver consumes the original.
    let confirmation_handle = app_handle.clone();
    info!(
        access_mode = ?runtime.access_mode,
        ocr = runtime.ocr_provider_name,
        llm = runtime.llm_provider_name,
        "AI provider adapters resolved"
    );
    let policy_client = Arc::new(PolicyClient::new());
    let sandbox = create_platform_sandbox(&config.automation.sandbox);
    let mut controller = if let Some(flag) = cli_health_flag {
        AutomationController::new(
            policy_client,
            audit_logger,
            sandbox,
            config.automation.sandbox.clone(),
        )
        .with_health_flag(flag)
    } else {
        AutomationController::new(
            policy_client,
            audit_logger,
            sandbox,
            config.automation.sandbox.clone(),
        )
    };
    controller.set_enabled(true);
    controller.set_scene_finder(runtime.element_finder.clone());
    controller.set_intent_executor(runtime.intent_executor);
    controller.set_intent_planner(runtime.intent_planner);

    let focus_probe: Arc<dyn oneshim_core::ports::focus_probe::FocusProbe> = Arc::new(
        crate::focus_probe_adapter::ProcessMonitorFocusProbe::new(process_monitor),
    );

    // Use MagicOverlayDriver (Tauri WebView) when AppHandle is available,
    // fall back to PlatformOverlayDriver (script-based) otherwise.
    let overlay_driver: Arc<dyn oneshim_core::ports::overlay_driver::OverlayDriver> =
        if let Some(handle) = app_handle {
            info!("GUI overlay: using MagicOverlayDriver (Tauri WebView)");
            Arc::new(crate::magic_overlay_driver::MagicOverlayDriver::new(handle))
        } else {
            info!("GUI overlay: using PlatformOverlayDriver (script-based fallback)");
            crate::platform_overlay::create_platform_overlay_driver()
        };

    let hmac_secret = std::env::var("ONESHIM_GUI_TICKET_HMAC_SECRET").ok();
    if let Err(error) =
        controller.configure_gui_interaction(focus_probe, overlay_driver, hmac_secret)
    {
        warn!(error = %error, "GUI interaction setup failed (non-fatal)");
    }

    // Wire confirmation callback to emit Tauri event for overlay modal.
    if let Some(handle) = confirmation_handle {
        let handle_clone = handle.clone();
        controller = controller.with_confirmation_callback(Arc::new(move |confirmation| {
            use tauri::Emitter;
            let _ = handle_clone.emit("automation:confirm-request", &confirmation);
        }));
    }

    controller
}

fn build_noop_controller(
    config: &AppConfig,
    audit_logger: Arc<RwLock<AuditLogger>>,
) -> AutomationController {
    let policy_client = Arc::new(PolicyClient::new());
    let sandbox = create_platform_sandbox(&config.automation.sandbox);
    let mut controller = AutomationController::new(
        policy_client,
        audit_logger,
        sandbox,
        config.automation.sandbox.clone(),
    );
    controller.set_enabled(true);
    controller.set_intent_executor(build_noop_intent_executor());
    controller
}

#[cfg(feature = "server")]
fn preflight_provider_oauth_connection(
    handle: &Handle,
    ai_config: &oneshim_core::config::AiProviderConfig,
    oauth_port: Option<Arc<dyn OAuthPort>>,
) -> std::result::Result<Option<Arc<dyn OAuthPort>>, oneshim_core::error::CoreError> {
    if ai_config.access_mode != AiAccessMode::ProviderOAuth {
        return Ok(oauth_port);
    }

    let selected_provider_ids =
        crate::oauth_provider_registry::selected_managed_oauth_provider_ids(ai_config)?;
    if selected_provider_ids.is_empty() {
        return Ok(oauth_port);
    }

    let oauth = oauth_port.ok_or_else(|| oneshim_core::error::CoreError::Config {
        code: oneshim_core::error_codes::ConfigCode::Invalid,
        message: "ProviderOAuth mode requires an available OS secret store".to_string(),
    })?;

    for provider_id in selected_provider_ids {
        let status = handle
            .block_on(oauth.connection_status(&provider_id))
            .map_err(|e| oneshim_core::error::CoreError::Config {
                code: oneshim_core::error_codes::ConfigCode::Invalid,
                message: e.to_string(),
            })?;

        if !status.connected && !status.has_refresh_token {
            return Err(oneshim_core::error::CoreError::Config { code: oneshim_core::error_codes::ConfigCode::Invalid, message: format!(
                "ProviderOAuth mode requires an active OAuth connection or a refresh token for provider '{provider_id}'."
            ) });
        }
    }

    Ok(Some(oauth))
}

fn oauth_runtime_error_status(ai_config: &AiProviderConfig, reason: String) -> AiRuntimeStatus {
    let ocr_source = match ai_config.ocr_provider {
        oneshim_core::config::OcrProviderType::Remote => "remote",
        oneshim_core::config::OcrProviderType::Local => "local",
    };

    AiRuntimeStatus {
        ocr_source: ocr_source.to_string(),
        llm_source: "oauth".to_string(),
        ocr_fallback_reason: Some(reason.clone()),
        llm_fallback_reason: Some(reason),
    }
}

fn should_fallback_to_noop(ai_config: &AiProviderConfig) -> bool {
    ai_config.fallback_to_local && ai_config.access_mode != AiAccessMode::ProviderOAuth
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_oauth_never_uses_noop_fallback() {
        let config = AiProviderConfig {
            access_mode: AiAccessMode::ProviderOAuth,
            fallback_to_local: true,
            ..AiProviderConfig::default()
        };

        assert!(!should_fallback_to_noop(&config));
    }

    #[test]
    fn oauth_runtime_error_status_reports_oauth_source() {
        let config = AiProviderConfig {
            access_mode: AiAccessMode::ProviderOAuth,
            ..AiProviderConfig::default()
        };

        let status = oauth_runtime_error_status(&config, "not authenticated".to_string());
        assert_eq!(status.ocr_source, "local");
        assert_eq!(status.llm_source, "oauth");
        assert_eq!(
            status.llm_fallback_reason.as_deref(),
            Some("not authenticated")
        );
    }
}
