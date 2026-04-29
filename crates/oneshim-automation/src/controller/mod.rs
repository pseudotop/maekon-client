mod gate;
mod intent;
mod port_impl;
mod preset;
mod types;

pub use types::{
    AutomationAction, AutomationCommand, CommandResult, GuiExecutionResult, MouseButton,
    PlannedIntentResult, WorkflowResult, WorkflowStepResult,
};

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::action_dispatcher::{AutomationActionDispatcher, SandboxActionDispatcher};
use crate::audit::AuditLogger;
use crate::error::AutomationError;
use crate::gui_interaction::{GuiInteractionError, GuiInteractionService};
use crate::intent_planner::IntentPlanner;
use crate::intent_resolver::IntentExecutor;
use crate::policy::PolicyClient;
use gate::CommandExecutionGate;
use oneshim_core::config::SandboxConfig;
use oneshim_core::models::automation::PendingConfirmation;
use oneshim_core::ports::element_finder::ElementFinder;
use oneshim_core::ports::focus_probe::FocusProbe;
use oneshim_core::ports::overlay_driver::OverlayDriver;
use oneshim_core::ports::sandbox::Sandbox;

const GUI_EXECUTE_TIMEOUT_SECS: u64 = 30;
const GUI_ACTION_TIMEOUT_SECS: u64 = 10;

// AutomationController

pub struct AutomationController {
    pub(super) policy_client: Arc<PolicyClient>,
    pub(super) audit_logger: Arc<RwLock<AuditLogger>>,
    pub(super) action_dispatcher: Arc<dyn AutomationActionDispatcher>,
    pub(super) base_sandbox_config: SandboxConfig,
    pub(super) enabled: bool,
    pub(super) intent_executor: Option<Arc<IntentExecutor>>,
    pub(super) intent_planner: Option<Arc<dyn IntentPlanner>>,
    pub(super) scene_finder: Option<Arc<dyn ElementFinder>>,
    pub(super) gui_service: Option<Arc<GuiInteractionService>>,
    /// Health flag: `true` after a successful command, `false` on failure.
    /// Read by the health-check loop. `None` when no caller has wired a flag.
    pub(super) last_command_ok: Option<Arc<AtomicBool>>,
    /// Pending confirmations awaiting user approval via overlay modal.
    /// Key: command_id, Value: (confirmation data, oneshot sender for response).
    #[allow(clippy::type_complexity)]
    pub(super) pending_confirmations: Arc<
        tokio::sync::Mutex<
            HashMap<String, (PendingConfirmation, tokio::sync::oneshot::Sender<bool>)>,
        >,
    >,
    /// Optional callback invoked when a command requires user confirmation.
    /// The frontend (e.g., Tauri overlay) registers this to display a modal.
    #[allow(clippy::type_complexity)]
    pub(super) on_confirmation_needed: Option<Arc<dyn Fn(PendingConfirmation) + Send + Sync>>,
}

impl AutomationController {
    pub fn new(
        policy_client: Arc<PolicyClient>,
        audit_logger: Arc<RwLock<AuditLogger>>,
        sandbox: Arc<dyn Sandbox>,
        sandbox_config: SandboxConfig,
    ) -> Self {
        tracing::info!(
            platform = sandbox.platform(),
            available = sandbox.is_available(),
            "automation controller initialized - sandbox connected"
        );
        Self {
            policy_client,
            audit_logger,
            action_dispatcher: Arc::new(SandboxActionDispatcher::new(sandbox)),
            base_sandbox_config: sandbox_config,
            enabled: false, // disabled by default
            intent_executor: None,
            intent_planner: None,
            scene_finder: None,
            gui_service: None,
            last_command_ok: None,
            pending_confirmations: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            on_confirmation_needed: None,
        }
    }

    /// Attach a shared health flag that is set to `true` on successful command
    /// execution and `false` on failure.
    pub fn with_health_flag(mut self, flag: Arc<AtomicBool>) -> Self {
        self.last_command_ok = Some(flag);
        self
    }

    /// Attach a callback that is invoked when a command needs user confirmation.
    /// The callback should present a UI prompt (e.g., overlay modal) and the
    /// caller later resolves the pending confirmation via `resolve_confirmation`.
    pub fn with_confirmation_callback(
        mut self,
        cb: Arc<dyn Fn(PendingConfirmation) + Send + Sync>,
    ) -> Self {
        self.on_confirmation_needed = Some(cb);
        self
    }

    /// Create a pending confirmation for a command and wait for the user to
    /// approve or deny it. Returns `Ok(true)` if approved, `Ok(false)` on
    /// denial or timeout (30 s).
    pub(super) async fn request_confirmation(
        &self,
        cmd_id: &str,
        process_name: &str,
        args: &[String],
        audit_level: &str,
    ) -> Result<bool, AutomationError> {
        let nonce = uuid::Uuid::new_v4().to_string();
        let confirmation = PendingConfirmation {
            command_id: cmd_id.to_string(),
            nonce: nonce.clone(),
            process_name: process_name.to_string(),
            args: args.to_vec(),
            audit_level: audit_level.to_string(),
            requested_at: chrono::Utc::now(),
        };

        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending_confirmations
            .lock()
            .await
            .insert(cmd_id.to_string(), (confirmation.clone(), tx));

        if let Some(ref cb) = self.on_confirmation_needed {
            cb(confirmation);
        }

        match tokio::time::timeout(std::time::Duration::from_secs(30), rx).await {
            Ok(Ok(approved)) => Ok(approved),
            _ => {
                self.pending_confirmations.lock().await.remove(cmd_id);
                Ok(false) // timeout or channel error -> denied
            }
        }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn set_intent_executor(&mut self, executor: Arc<IntentExecutor>) {
        self.intent_executor = Some(executor);
    }

    pub fn set_intent_planner(&mut self, planner: Arc<dyn IntentPlanner>) {
        self.intent_planner = Some(planner);
    }

    pub fn set_scene_finder(&mut self, finder: Arc<dyn ElementFinder>) {
        self.scene_finder = Some(finder);
    }

    pub fn scene_finder(&self) -> Option<&Arc<dyn ElementFinder>> {
        self.scene_finder.as_ref()
    }

    pub fn set_action_dispatcher(&mut self, dispatcher: Arc<dyn AutomationActionDispatcher>) {
        self.action_dispatcher = dispatcher;
    }

    pub fn configure_gui_interaction(
        &mut self,
        focus_probe: Arc<dyn FocusProbe>,
        overlay_driver: Arc<dyn OverlayDriver>,
        hmac_secret: Option<String>,
    ) -> Result<(), AutomationError> {
        let scene_finder = self
            .scene_finder
            .as_ref()
            .ok_or_else(|| {
                // Iter-100: "not configured" = Missing semantic. Route via
                // AutomationError::Core to emit ConfigCode::Missing wire code
                // (config.missing) instead of internal.generic.
                AutomationError::Core(oneshim_core::error::CoreError::Config {
                    code: oneshim_core::error_codes::ConfigCode::Missing,
                    message: "Scene analyzer is not configured".to_string(),
                })
            })?
            .clone();

        let service = Arc::new(GuiInteractionService::new(
            scene_finder,
            focus_probe,
            overlay_driver,
            hmac_secret,
        ));
        service.ensure_cleanup_task();
        self.gui_service = Some(service);
        Ok(())
    }

    pub(super) fn ensure_enabled(&self) -> Result<(), AutomationError> {
        if self.enabled {
            Ok(())
        } else {
            Err(AutomationError::PolicyDenied(
                "자동화가 비active화 state입니다".to_string(),
            ))
        }
    }

    pub(super) fn require_intent_executor(&self) -> Result<&Arc<IntentExecutor>, AutomationError> {
        self.intent_executor.as_ref().ok_or_else(|| {
            // Iter-100: "not configured" = Missing semantic.
            AutomationError::Core(oneshim_core::error::CoreError::Config {
                code: oneshim_core::error_codes::ConfigCode::Missing,
                message: "IntentExecutor is not configured".to_string(),
            })
        })
    }

    pub(super) fn require_intent_planner(
        &self,
    ) -> Result<&Arc<dyn IntentPlanner>, AutomationError> {
        self.intent_planner.as_ref().ok_or_else(|| {
            // Iter-100: "not configured" = Missing semantic.
            AutomationError::Core(oneshim_core::error::CoreError::Config {
                code: oneshim_core::error_codes::ConfigCode::Missing,
                message: "IntentPlanner is not configured".to_string(),
            })
        })
    }

    pub(super) fn require_scene_finder(&self) -> Result<&Arc<dyn ElementFinder>, AutomationError> {
        self.scene_finder.as_ref().ok_or_else(|| {
            // Iter-100: "not configured" = Missing semantic.
            AutomationError::Core(oneshim_core::error::CoreError::Config {
                code: oneshim_core::error_codes::ConfigCode::Missing,
                message: "Scene analyzer is not configured".to_string(),
            })
        })
    }

    /// Returns a reference to the GUI interaction service, if configured.
    pub fn gui_service(&self) -> Option<&Arc<GuiInteractionService>> {
        self.gui_service.as_ref()
    }

    pub(super) fn require_gui_service(
        &self,
    ) -> Result<&Arc<GuiInteractionService>, GuiInteractionError> {
        self.gui_service
            .as_ref()
            .ok_or_else(|| GuiInteractionError::Unavailable {
                code: oneshim_core::error_codes::GuiCode::Unavailable,
                message: "GUI interaction service is not configured".to_string(),
            })
    }

    fn command_execution_gate(&self) -> CommandExecutionGate {
        CommandExecutionGate::new(
            self.policy_client.clone(),
            self.audit_logger.clone(),
            self.action_dispatcher.clone(),
            self.base_sandbox_config.clone(),
        )
    }
}

#[cfg(test)]
mod tests;
