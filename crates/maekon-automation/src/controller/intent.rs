use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use tokio::sync::broadcast;

use crate::controller::gate::{
    audit_action_label, CommandExecutionGate, GUI_SESSION_POLICY_TOKEN, INTENT_HINT_POLICY_TOKEN,
};
use crate::error::AutomationError;
use crate::gui_interaction::{
    GuiConfirmRequest, GuiCreateSessionRequest, GuiCreateSessionResponse, GuiExecutionRequest,
    GuiHighlightRequest, GuiInteractionError,
};
use crate::policy::AuditLevel;
use maekon_core::config::ConfirmationRequirement;
use maekon_core::error::CoreError;
use maekon_core::models::audit::AuditStatus;
use maekon_core::models::automation::{
    AutomationAction, AutomationCommand, CommandOrigin, CommandResult,
};
use maekon_core::models::gui::{GuiExecutionTicket, GuiInteractionSession, GuiSessionEvent};
use maekon_core::models::intent::{AutomationIntent, IntentCommand, IntentResult};
use maekon_core::models::ui_scene::UiScene;
use maekon_core::ports::input_driver::InputDriver;

use super::types::{GuiExecutionResult, PlannedIntentResult};
use super::{AutomationController, GUI_ACTION_TIMEOUT_SECS, GUI_EXECUTE_TIMEOUT_SECS};

struct GatedInputDriver {
    gate: super::gate::CommandExecutionGate,
    /// Underlying real input driver the scoped executor was built from.
    ///
    /// Synthetic-input actions (mouse/keyboard/hotkey) are routed through the gate
    /// and sandbox action dispatcher, but window activation is not a sandboxed
    /// synthetic-input action — it shells out to the platform window manager — so it
    /// is forwarded straight to this driver (see `activate_app`).
    inner: Arc<dyn InputDriver>,
    command_id_prefix: String,
    session_id: String,
    policy_token: String,
    origin: CommandOrigin,
    timeout_ms: Option<u64>,
    next_action_index: AtomicUsize,
}

impl GatedInputDriver {
    fn new(
        gate: super::gate::CommandExecutionGate,
        inner: Arc<dyn InputDriver>,
        command_id_prefix: String,
        session_id: String,
        policy_token: String,
        origin: CommandOrigin,
        timeout_ms: Option<u64>,
    ) -> Self {
        Self {
            gate,
            inner,
            command_id_prefix,
            session_id,
            policy_token,
            origin,
            timeout_ms,
            next_action_index: AtomicUsize::new(0),
        }
    }

    fn build_command(&self, action: AutomationAction) -> AutomationCommand {
        let action_index = self.next_action_index.fetch_add(1, Ordering::Relaxed);
        AutomationCommand {
            command_id: format!("{}:action-{}", self.command_id_prefix, action_index),
            session_id: self.session_id.clone(),
            action,
            timeout_ms: self.timeout_ms,
            policy_token: self.policy_token.clone(),
            origin: self.origin,
        }
    }

    async fn dispatch_action(&self, action: AutomationAction) -> Result<(), AutomationError> {
        let command = self.build_command(action);
        let effective_timeout_ms = self.gate.effective_timeout_ms(&command).await;
        match self.gate.execute(&command).await? {
            CommandResult::Success => Ok(()),
            CommandResult::Failed(message) => Err(AutomationError::Internal(message)),
            CommandResult::Timeout => Err(AutomationError::ExecutionTimeout {
                timeout_ms: effective_timeout_ms.unwrap_or_default(),
            }),
            CommandResult::Denied => Err(AutomationError::PolicyDenied(
                "Intent action denied by policy".to_string(),
            )),
        }
    }
}

#[async_trait]
impl InputDriver for GatedInputDriver {
    async fn mouse_move(&self, x: i32, y: i32) -> Result<(), CoreError> {
        self.dispatch_action(AutomationAction::MouseMove { x, y })
            .await
            .map_err(Into::into)
    }

    async fn mouse_click(&self, button: &str, x: i32, y: i32) -> Result<(), CoreError> {
        self.dispatch_action(AutomationAction::MouseClick {
            button: button.to_string(),
            x,
            y,
        })
        .await
        .map_err(Into::into)
    }

    async fn type_text(&self, text: &str) -> Result<(), CoreError> {
        self.dispatch_action(AutomationAction::KeyType {
            text: text.to_string(),
        })
        .await
        .map_err(Into::into)
    }

    async fn key_press(&self, key: &str) -> Result<(), CoreError> {
        self.dispatch_action(AutomationAction::KeyPress {
            key: key.to_string(),
        })
        .await
        .map_err(Into::into)
    }

    async fn key_release(&self, key: &str) -> Result<(), CoreError> {
        self.dispatch_action(AutomationAction::KeyRelease {
            key: key.to_string(),
        })
        .await
        .map_err(Into::into)
    }

    async fn hotkey(&self, keys: &[String]) -> Result<(), CoreError> {
        self.dispatch_action(AutomationAction::Hotkey {
            keys: keys.to_vec(),
        })
        .await
        .map_err(Into::into)
    }

    async fn activate_app(&self, app_name: &str) -> Result<bool, CoreError> {
        // Window activation is NOT a sandboxed synthetic-input action (it shells out
        // to the platform window manager via the real driver), so — unlike the
        // mouse/keyboard/hotkey methods above — it is forwarded directly to the
        // underlying real driver instead of through the gate's action dispatcher.
        //
        // Without this override, GatedInputDriver would inherit the InputDriver port
        // default `Ok(false)`, silently dropping window activation on every
        // trusted-internal path (run_workflow / execute_intent_hint / gui_execute /
        // sentinel-token execute_intent). That made the activation a no-op AND let
        // subsequent `stop_on_failure` steps synthesize input against the wrong,
        // un-switched window (MAEKON-AUTO-1 / #7070). The trusted-internal
        // confirmation/consent check already ran at the intent layer
        // (enforce_intent_confirmation_policy) before this executor was built, and
        // the intent outcome is audited there, so no additional gating is required
        // here.
        self.inner.activate_app(app_name).await
    }

    fn platform(&self) -> &str {
        "gated"
    }
}

impl AutomationController {
    async fn enforce_intent_confirmation_policy(
        &self,
        command_id: &str,
        process_name: &str,
        intent: &AutomationIntent,
    ) -> Result<(), AutomationError> {
        match self.confirmation_policy {
            ConfirmationRequirement::Block => Err(AutomationError::PolicyBlocked),
            ConfirmationRequirement::Confirm => {
                let action_label = audit_intent_label(intent);
                let approved = self
                    .request_confirmation(command_id, process_name, &[action_label], "CONFIRM")
                    .await?;
                if approved {
                    Ok(())
                } else {
                    Err(AutomationError::UserDenied)
                }
            }
            ConfirmationRequirement::Auto => Ok(()),
        }
    }

    pub(super) fn scoped_intent_executor(
        &self,
        cmd: &IntentCommand,
    ) -> Result<crate::intent_resolver::IntentExecutor, AutomationError> {
        let template = self.require_intent_executor()?;
        if !CommandExecutionGate::is_trusted_internal(cmd.origin, &cmd.policy_token) {
            // Intent-scoped external policy tokens do not have a stable low-level action scope,
            // so retain the template executor until intent-native policy tokens exist.
            return Ok(template.with_overrides(None, cmd.config.clone()));
        }

        let input_driver: Arc<dyn InputDriver> = Arc::new(GatedInputDriver::new(
            self.command_execution_gate(),
            // Forward non-sandboxed window activation to the template's real driver
            // (see GatedInputDriver::activate_app); synthetic input still routes
            // through the gate.
            template.input_driver(),
            cmd.command_id.clone(),
            cmd.session_id.clone(),
            cmd.policy_token.clone(),
            cmd.origin,
            cmd.timeout_ms,
        ));
        Ok(template.with_overrides(Some(input_driver), cmd.config.clone()))
    }

    pub async fn execute_intent(
        &self,
        cmd: &IntentCommand,
    ) -> Result<IntentResult, AutomationError> {
        self.ensure_enabled()?;
        let start = Instant::now();

        {
            let mut logger = self.audit_logger.write().await;
            logger.log_start_if(
                AuditLevel::Basic,
                &cmd.command_id,
                &cmd.session_id,
                &audit_intent_label(&cmd.intent),
            );
        }

        if let Err(err) = self
            .enforce_intent_confirmation_policy(&cmd.command_id, "intent", &cmd.intent)
            .await
        {
            let elapsed_ms = start.elapsed().as_millis() as u64;
            let mut logger = self.audit_logger.write().await;
            logger.log_with_status_and_time(
                AuditLevel::Basic,
                &cmd.command_id,
                &cmd.session_id,
                "denied",
                AuditStatus::Denied,
                &format!(
                    "{}, error={}, elapsed={}ms",
                    audit_intent_label(&cmd.intent),
                    err,
                    elapsed_ms
                ),
                elapsed_ms,
            );
            return Err(err);
        }

        let executor = self.scoped_intent_executor(cmd)?;
        let result = executor.execute(&cmd.intent).await?;
        let elapsed_ms = start.elapsed().as_millis() as u64;

        {
            let mut logger = self.audit_logger.write().await;
            // Record the real outcome status (review4 A1).
            let (action_type, status) = if result.success {
                ("complete", AuditStatus::Completed)
            } else {
                ("failed", AuditStatus::Failed)
            };
            logger.log_with_status_and_time(
                AuditLevel::Basic,
                &cmd.command_id,
                &cmd.session_id,
                action_type,
                status,
                &format!("success={}, elapsed={}ms", result.success, elapsed_ms),
                elapsed_ms,
            );
        }

        Ok(result)
    }

    pub async fn execute_intent_hint(
        &self,
        command_id: &str,
        session_id: &str,
        intent_hint: &str,
    ) -> Result<PlannedIntentResult, AutomationError> {
        self.ensure_enabled()?;
        let planner = self.require_intent_planner()?;

        {
            let mut logger = self.audit_logger.write().await;
            logger.log_start_if(
                AuditLevel::Basic,
                command_id,
                session_id,
                // Payload-free: the raw NL hint can carry secrets (review4 A12).
                &format!("intent_hint_len={}", intent_hint.chars().count()),
            );
        }

        let start = Instant::now();
        let planned_intent = planner.plan(intent_hint).await?;

        if let Err(err) = self
            .enforce_intent_confirmation_policy(command_id, "intent-hint", &planned_intent)
            .await
        {
            let elapsed_ms = start.elapsed().as_millis() as u64;
            let mut logger = self.audit_logger.write().await;
            logger.log_with_status_and_time(
                AuditLevel::Basic,
                command_id,
                session_id,
                "denied",
                AuditStatus::Denied,
                &format!(
                    "{}, error={}, elapsed={}ms",
                    audit_intent_label(&planned_intent),
                    err,
                    elapsed_ms
                ),
                elapsed_ms,
            );
            return Err(err);
        }

        let intent_command = IntentCommand {
            command_id: command_id.to_string(),
            session_id: session_id.to_string(),
            intent: planned_intent.clone(),
            config: None,
            timeout_ms: None,
            policy_token: INTENT_HINT_POLICY_TOKEN.to_string(),
            origin: CommandOrigin::Internal,
        };
        let executor = self.scoped_intent_executor(&intent_command)?;
        let result = executor.execute(&planned_intent).await?;
        let elapsed_ms = start.elapsed().as_millis() as u64;

        {
            let mut logger = self.audit_logger.write().await;
            // Mask the planned intent via audit_intent_label (text_len form) and
            // record the real status (review4 A1/A12): the raw Debug of an
            // AutomationIntent embeds free-form TypeIntoElement/KeyType text.
            let (action_type, status) = if result.success {
                ("complete", AuditStatus::Completed)
            } else {
                ("failed", AuditStatus::Failed)
            };
            logger.log_with_status_and_time(
                AuditLevel::Basic,
                command_id,
                session_id,
                action_type,
                status,
                &format!(
                    "{}, success={}, elapsed={}ms",
                    audit_intent_label(&planned_intent),
                    result.success,
                    elapsed_ms
                ),
                elapsed_ms,
            );
        }

        Ok(PlannedIntentResult {
            planned_intent,
            result,
        })
    }

    pub async fn analyze_scene(
        &self,
        app_name: Option<&str>,
        screen_id: Option<&str>,
    ) -> Result<UiScene, CoreError> {
        self.ensure_enabled()?;
        let finder = self.require_scene_finder()?;
        finder.analyze_scene(app_name, screen_id).await
    }

    pub async fn analyze_scene_from_image(
        &self,
        image_data: Vec<u8>,
        image_format: String,
        app_name: Option<&str>,
        screen_id: Option<&str>,
    ) -> Result<UiScene, CoreError> {
        self.ensure_enabled()?;
        let finder = self.require_scene_finder()?;
        finder
            .analyze_scene_from_image(image_data, image_format, app_name, screen_id)
            .await
    }

    pub async fn gui_create_session(
        &self,
        req: GuiCreateSessionRequest,
    ) -> Result<GuiCreateSessionResponse, GuiInteractionError> {
        self.ensure_enabled()
            .map_err(|e| GuiInteractionError::Unavailable {
                code: maekon_core::error_codes::GuiCode::Unavailable,
                message: e.to_string(),
            })?;
        let service = self.require_gui_service()?;
        service.create_session(req).await
    }

    pub async fn gui_get_session(
        &self,
        session_id: &str,
        capability_token: &str,
    ) -> Result<GuiInteractionSession, GuiInteractionError> {
        self.ensure_enabled()
            .map_err(|e| GuiInteractionError::Unavailable {
                code: maekon_core::error_codes::GuiCode::Unavailable,
                message: e.to_string(),
            })?;
        let service = self.require_gui_service()?;
        service.get_session(session_id, capability_token).await
    }

    pub async fn gui_highlight_session(
        &self,
        session_id: &str,
        capability_token: &str,
        req: GuiHighlightRequest,
    ) -> Result<GuiInteractionSession, GuiInteractionError> {
        self.ensure_enabled()
            .map_err(|e| GuiInteractionError::Unavailable {
                code: maekon_core::error_codes::GuiCode::Unavailable,
                message: e.to_string(),
            })?;
        let service = self.require_gui_service()?;
        service
            .highlight_session(session_id, capability_token, req)
            .await
    }

    pub async fn gui_confirm_candidate(
        &self,
        session_id: &str,
        capability_token: &str,
        req: GuiConfirmRequest,
    ) -> Result<GuiExecutionTicket, GuiInteractionError> {
        self.ensure_enabled()
            .map_err(|e| GuiInteractionError::Unavailable {
                code: maekon_core::error_codes::GuiCode::Unavailable,
                message: e.to_string(),
            })?;
        let service = self.require_gui_service()?;
        service
            .confirm_candidate(session_id, capability_token, req)
            .await
    }

    pub async fn gui_execute(
        &self,
        session_id: &str,
        capability_token: &str,
        req: GuiExecutionRequest,
    ) -> Result<GuiExecutionResult, GuiInteractionError> {
        self.ensure_enabled()
            .map_err(|e| GuiInteractionError::Unavailable {
                code: maekon_core::error_codes::GuiCode::Unavailable,
                message: e.to_string(),
            })?;
        let service = self.require_gui_service()?;
        let plan = service
            .prepare_execution(session_id, capability_token, req)
            .await?;

        let total_deadline = std::time::Duration::from_secs(GUI_EXECUTE_TIMEOUT_SECS);
        let action_timeout = std::time::Duration::from_secs(GUI_ACTION_TIMEOUT_SECS);
        let execution_start = Instant::now();

        let total_steps = plan.actions.len();
        tracing::info!(
            session_id,
            command_id = %plan.command_id,
            total_steps,
            timeout_secs = GUI_EXECUTE_TIMEOUT_SECS,
            "GUI execution started"
        );

        let actions_result = tokio::time::timeout(total_deadline, async {
            let mut last_result = IntentResult {
                success: false,
                element: None,
                verification: None,
                retry_count: 0,
                elapsed_ms: 0,
                error: Some("No executable actions in GUI plan".to_string()),
            };
            let mut execution_error: Option<String> = None;
            let mut steps_completed: usize = 0;

            for (index, action) in plan.actions.iter().enumerate() {
                let command_id = if index == 0 {
                    plan.command_id.clone()
                } else {
                    format!("{}:stage-{index}", plan.command_id)
                };

                let intent_command = IntentCommand {
                    command_id,
                    session_id: plan.session_id.clone(),
                    intent: AutomationIntent::Raw(action.clone()),
                    config: None,
                    timeout_ms: None,
                    policy_token: GUI_SESSION_POLICY_TOKEN.to_string(),
                    origin: CommandOrigin::Internal,
                };

                match tokio::time::timeout(action_timeout, self.execute_intent(&intent_command))
                    .await
                {
                    Ok(Ok(result)) => {
                        last_result = result;
                        if last_result.success {
                            steps_completed += 1;
                        } else {
                            tracing::warn!(
                                stage = index,
                                error = last_result.error.as_deref().unwrap_or("unknown"),
                                "GUI action failed at stage"
                            );
                            break;
                        }
                    }
                    Ok(Err(err)) => {
                        tracing::warn!(stage = index, error = %err, "GUI action error at stage");
                        execution_error = Some(err.to_string());
                        break;
                    }
                    Err(_elapsed) => {
                        tracing::warn!(
                            stage = index,
                            timeout_secs = GUI_ACTION_TIMEOUT_SECS,
                            "GUI action timed out at stage"
                        );
                        execution_error = Some(format!(
                            "GUI action timed out after {}s (stage {})",
                            GUI_ACTION_TIMEOUT_SECS, index
                        ));
                        break;
                    }
                }
            }

            (last_result, execution_error, steps_completed)
        })
        .await;

        let (last_result, execution_error, steps_completed) = match actions_result {
            Ok(inner) => inner,
            Err(_elapsed) => {
                let detail = format!("GUI execution timed out after {GUI_EXECUTE_TIMEOUT_SECS}s");
                tracing::error!(
                    session_id,
                    timeout_secs = GUI_EXECUTE_TIMEOUT_SECS,
                    elapsed_ms = execution_start.elapsed().as_millis() as u64,
                    "GUI total execution timeout exceeded"
                );
                let _ = service
                    .complete_execution(session_id, false, Some(detail.clone()), 0, total_steps)
                    .await;
                return Err(GuiInteractionError::Internal {
                    code: maekon_core::error_codes::GuiCode::InternalError,
                    message: detail,
                });
            }
        };

        let succeeded = execution_error.is_none() && last_result.success;
        let detail = execution_error.clone().or_else(|| {
            if !last_result.success {
                if steps_completed > 0 {
                    Some(format!(
                        "Partial execution: {}/{} steps completed. {}",
                        steps_completed,
                        total_steps,
                        last_result.error.as_deref().unwrap_or("action failed")
                    ))
                } else {
                    last_result.error.clone()
                }
            } else {
                None
            }
        });
        let outcome = service
            .complete_execution(
                session_id,
                succeeded,
                detail.clone(),
                steps_completed,
                total_steps,
            )
            .await?;

        tracing::info!(
            session_id,
            succeeded,
            steps_completed,
            total_steps,
            elapsed_ms = execution_start.elapsed().as_millis() as u64,
            "GUI execution finished"
        );

        if let Some(err) = execution_error {
            return Err(GuiInteractionError::Internal {
                code: maekon_core::error_codes::GuiCode::InternalError,
                message: err,
            });
        }

        Ok(GuiExecutionResult {
            command_id: plan.command_id,
            ticket: plan.ticket,
            result: last_result,
            outcome,
        })
    }

    pub async fn gui_cancel_session(
        &self,
        session_id: &str,
        capability_token: &str,
    ) -> Result<GuiInteractionSession, GuiInteractionError> {
        self.ensure_enabled()
            .map_err(|e| GuiInteractionError::Unavailable {
                code: maekon_core::error_codes::GuiCode::Unavailable,
                message: e.to_string(),
            })?;
        let service = self.require_gui_service()?;
        service.cancel_session(session_id, capability_token).await
    }

    pub async fn gui_subscribe_events(
        &self,
        session_id: &str,
        capability_token: &str,
    ) -> Result<broadcast::Receiver<GuiSessionEvent>, GuiInteractionError> {
        self.ensure_enabled()
            .map_err(|e| GuiInteractionError::Unavailable {
                code: maekon_core::error_codes::GuiCode::Unavailable,
                message: e.to_string(),
            })?;
        let service = self.require_gui_service()?;
        service
            .subscribe_session(session_id, capability_token)
            .await
    }
}

pub(super) fn audit_intent_label(intent: &AutomationIntent) -> String {
    match intent {
        AutomationIntent::ClickElement {
            text,
            role,
            app_name,
            button,
        } => format!(
            "ClickElement {{ text_present={}, role_present={}, app_present={}, button={} }}",
            text.as_ref().is_some_and(|value| !value.trim().is_empty()),
            role.as_ref().is_some_and(|value| !value.trim().is_empty()),
            app_name
                .as_ref()
                .is_some_and(|value| !value.trim().is_empty()),
            button
        ),
        AutomationIntent::TypeIntoElement {
            element_text,
            role,
            text,
        } => format!(
            "TypeIntoElement {{ element_text_present={}, role_present={}, text_len={} }}",
            element_text
                .as_ref()
                .is_some_and(|value| !value.trim().is_empty()),
            role.as_ref().is_some_and(|value| !value.trim().is_empty()),
            text.chars().count()
        ),
        AutomationIntent::ExecuteHotkey { keys } => {
            format!("ExecuteHotkey {{ keys={} }}", keys.join("+"))
        }
        AutomationIntent::WaitForText { text, timeout_ms } => format!(
            "WaitForText {{ text_present={}, timeout_ms={} }}",
            !text.trim().is_empty(),
            timeout_ms
        ),
        AutomationIntent::ActivateApp { app_name } => format!(
            "ActivateApp {{ app_present={} }}",
            !app_name.trim().is_empty()
        ),
        AutomationIntent::Raw(action) => format!("Raw({})", audit_action_label(action)),
    }
}
