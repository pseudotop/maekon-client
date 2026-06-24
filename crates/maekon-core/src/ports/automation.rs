//! Automation controller port — defines the contract for executing
//! commands, intents, workflows, and GUI interaction sessions.
//! Implemented by `AutomationController` in `maekon-automation`.

use async_trait::async_trait;
use tokio::sync::broadcast;

use crate::error::{CoreError, GuiInteractionError};
use crate::models::automation::{
    AutomationCommand, CommandResult, ExecutionPolicyDto, GuiExecutionResult, PendingConfirmation,
    PlannedIntentResult, WorkflowResult,
};
use crate::models::gui::{
    GuiConfirmRequest, GuiCreateSessionRequest, GuiCreateSessionResponse, GuiExecutionRequest,
    GuiExecutionTicket, GuiHighlightRequest, GuiInteractionSession, GuiSessionEvent,
};
use crate::models::intent::{IntentCommand, IntentResult, WorkflowPreset};
use crate::models::ui_scene::UiScene;

/// Automation execution port — the automation controller interface used by
/// maekon-web handlers.
///
/// This trait abstracts the public API of
/// `maekon-automation::AutomationController`. maekon-web accesses automation
/// functionality through this port rather than depending on maekon-automation
/// directly. (ADR-001 §7)
///
/// # Errors
/// - `CoreError::PolicyDenied` (wire: `policy.denied`) when PolicyClient
///   blocks the command. `UserDenied`/`PolicyBlocked` both collapse
///   here (iter-88 dedup).
/// - `CoreError::Config` with `ConfigCode::Missing` (wire: `config.missing`)
///   when required dependencies aren't wired (IntentExecutor/
///   IntentPlanner/Scene analyzer — iter-100).
/// - `CoreError::SandboxInit` (wire: `sandbox.init_failed`) for sandbox
///   setup failures (seccomp, Landlock, Job Object — iter-88 re-routed
///   from SandboxExecution). `CoreError::SandboxExecution` for
///   runtime sandbox enforcement.
/// - `CoreError::ElementNotFound` (wire: `ui.element_missing`) for click
///   targets; `CoreError::InvalidArguments` (wire:
///   `validation.invalid_arguments`) for bad hotkey/input specs.
/// - `GuiInteractionError` on the GUI-interaction subset of this port;
///   see `crates/maekon-automation/src/gui_interaction/helpers.rs`'s
///   `map_core_error` for the CoreError→GuiInteractionError mapping
///   (iter-91 expanded to 6 semantic routes).
#[async_trait]
pub trait AutomationPort: Send + Sync {
    // ── Core automation ──

    /// Execute a low-level command (policy validation + sandbox).
    ///
    /// POLICY_TOKEN_VALIDATION_RESERVED_FOR_EXTERNAL_PRODUCERS: this is the
    /// direct signed policy-token path. Current production callers should prefer
    /// `execute_intent`, `execute_intent_hint`, `run_workflow`, or GUI ticket
    /// execution unless they also provide an explicit signed-token issuance and
    /// command-scope validation plan.
    async fn execute_command(&self, cmd: &AutomationCommand) -> Result<CommandResult, CoreError>;

    /// Execute an intent (intent specified directly).
    async fn execute_intent(&self, cmd: &IntentCommand) -> Result<IntentResult, CoreError>;

    /// Execute an intent from a natural-language hint (IntentPlanner → IntentExecutor).
    async fn execute_intent_hint(
        &self,
        command_id: &str,
        session_id: &str,
        intent_hint: &str,
    ) -> Result<PlannedIntentResult, CoreError>;

    /// Run a workflow preset.
    async fn run_workflow(&self, preset: &WorkflowPreset) -> Result<WorkflowResult, CoreError>;

    // ── Scene analysis ──

    /// Analyze the on-screen scene (current focus or a specific app).
    async fn analyze_scene(
        &self,
        app_name: Option<&str>,
        screen_id: Option<&str>,
    ) -> Result<UiScene, CoreError>;

    /// Analyze a scene from image data.
    async fn analyze_scene_from_image(
        &self,
        image_data: Vec<u8>,
        image_format: String,
        app_name: Option<&str>,
        screen_id: Option<&str>,
    ) -> Result<UiScene, CoreError>;

    // ── GUI interaction ──

    /// Create a GUI interaction session.
    async fn gui_create_session(
        &self,
        req: GuiCreateSessionRequest,
    ) -> Result<GuiCreateSessionResponse, GuiInteractionError>;

    /// Get a GUI session.
    async fn gui_get_session(
        &self,
        session_id: &str,
        capability_token: &str,
    ) -> Result<GuiInteractionSession, GuiInteractionError>;

    /// Highlight GUI candidates.
    async fn gui_highlight_session(
        &self,
        session_id: &str,
        capability_token: &str,
        req: GuiHighlightRequest,
    ) -> Result<GuiInteractionSession, GuiInteractionError>;

    /// Confirm a GUI candidate → issue an execution ticket.
    async fn gui_confirm_candidate(
        &self,
        session_id: &str,
        capability_token: &str,
        req: GuiConfirmRequest,
    ) -> Result<GuiExecutionTicket, GuiInteractionError>;

    /// Execute a GUI action (ticket-based).
    async fn gui_execute(
        &self,
        session_id: &str,
        capability_token: &str,
        req: GuiExecutionRequest,
    ) -> Result<GuiExecutionResult, GuiInteractionError>;

    /// Cancel a GUI session.
    async fn gui_cancel_session(
        &self,
        session_id: &str,
        capability_token: &str,
    ) -> Result<GuiInteractionSession, GuiInteractionError>;

    /// Subscribe to GUI session events.
    async fn gui_subscribe_events(
        &self,
        session_id: &str,
        capability_token: &str,
    ) -> Result<broadcast::Receiver<GuiSessionEvent>, GuiInteractionError>;

    // ── Confirmation flow ──

    /// List automation confirmations awaiting user approval.
    async fn list_pending_confirmations(&self) -> Result<Vec<PendingConfirmation>, CoreError>;

    /// Submit the user's response to an automation confirmation (approve/deny).
    /// `nonce` is the single-use token issued when the confirmation was created;
    /// a mismatch results in denial.
    async fn submit_confirmation(
        &self,
        command_id: &str,
        nonce: &str,
        approved: bool,
    ) -> Result<(), CoreError>;

    // ── Policy CRUD ──

    /// List execution policies.
    async fn list_execution_policies(&self) -> Result<Vec<ExecutionPolicyDto>, CoreError>;

    /// Add or replace an execution policy (replaces an existing one with the same `policy_id`).
    async fn add_execution_policy(
        &self,
        policy: ExecutionPolicyDto,
    ) -> Result<ExecutionPolicyDto, CoreError>;

    /// Remove an execution policy.
    async fn remove_execution_policy(&self, policy_id: &str) -> Result<bool, CoreError>;
}
