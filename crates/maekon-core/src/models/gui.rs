use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::models::context::WindowBounds;
use crate::models::intent::ElementBounds;
use crate::models::ui_scene::{UiScene, UiSceneElement};

pub const GUI_INTERACTION_SCHEMA_VERSION: &str = "automation.gui.v2";
pub const GUI_SESSION_EVENT_SCHEMA_VERSION: &str = "automation.gui.event.v1";
pub const GUI_TICKET_SCHEMA_VERSION: &str = "automation.gui.ticket.v1";
pub const GUI_READINESS_SCHEMA_VERSION: &str = "automation.gui.readiness.v1";
pub const GUI_SESSION_ACCEPTANCE_MATRIX_SCHEMA_VERSION: &str =
    "automation.gui.acceptance_matrix.v1";
pub const GUI_PERMISSION_EVIDENCE_POLICY_SCHEMA_VERSION: &str =
    "automation.gui.permission_evidence.v1";
pub const GUI_BENCHMARK_HARNESS_SCHEMA_VERSION: &str = "automation.gui.benchmark_harness.v1";
pub const GUI_REAL_INPUT_EXECUTION_SCHEMA_VERSION: &str = "automation.gui.real_input_execution.v1";
pub const GUI_BENCHMARK_REPORT_SCHEMA_VERSION: &str = "automation.gui.benchmark_report.v1";

fn default_gui_interaction_schema_version() -> String {
    GUI_INTERACTION_SCHEMA_VERSION.to_string()
}

fn default_gui_event_schema_version() -> String {
    GUI_SESSION_EVENT_SCHEMA_VERSION.to_string()
}

fn default_gui_ticket_schema_version() -> String {
    GUI_TICKET_SCHEMA_VERSION.to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FocusSnapshot {
    pub app_name: String,
    pub window_title: String,
    pub pid: u32,
    pub bounds: Option<WindowBounds>,
    pub captured_at: DateTime<Utc>,
    pub focus_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionBinding {
    pub focus_hash: String,
    pub app_name: Option<String>,
    pub pid: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FocusValidation {
    pub valid: bool,
    pub reason: Option<String>,
    pub current_focus: Option<FocusSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HighlightTarget {
    pub candidate_id: String,
    pub bbox_abs: ElementBounds,
    pub color: String,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HighlightRequest {
    pub session_id: String,
    pub scene_id: String,
    pub targets: Vec<HighlightTarget>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HighlightHandle {
    pub handle_id: String,
    pub rendered_at: DateTime<Utc>,
    pub target_count: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiActionType {
    Click,
    DoubleClick,
    RightClick,
    TypeText,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiActionRequest {
    pub action_type: GuiActionType,
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiCandidate {
    pub element: UiSceneElement,
    pub ranking_reason: Option<String>,
    pub eligible: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiSessionState {
    Proposed,
    Highlighted,
    Confirmed,
    Executing,
    Executed,
    Cancelled,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiExecutionTicket {
    #[serde(default = "default_gui_ticket_schema_version")]
    pub schema_version: String,
    pub ticket_id: String,
    pub session_id: String,
    pub scene_id: String,
    pub element_id: String,
    pub action_hash: String,
    pub focus_hash: String,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub nonce: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiSessionEvent {
    #[serde(default = "default_gui_event_schema_version")]
    pub schema_version: String,
    pub event_type: String,
    pub session_id: String,
    pub state: GuiSessionState,
    pub emitted_at: DateTime<Utc>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiInteractionSession {
    #[serde(default = "default_gui_interaction_schema_version")]
    pub schema_version: String,
    pub session_id: String,
    pub state: GuiSessionState,
    pub scene: UiScene,
    pub focus: FocusSnapshot,
    pub candidates: Vec<GuiCandidate>,
    pub selected_element_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

// ── GUI Interaction request/response types ──
// Previously lived in maekon-automation::gui_interaction, but moved to
// maekon-core to abstract over AutomationPort (ADR-001 §7).

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiCreateSessionRequest {
    pub app_name: Option<String>,
    pub screen_id: Option<String>,
    pub min_confidence: Option<f64>,
    pub max_candidates: Option<usize>,
    pub session_ttl_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiCreateSessionResponse {
    pub session: GuiInteractionSession,
    pub capability_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiHighlightRequest {
    pub candidate_ids: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiConfirmRequest {
    pub candidate_id: String,
    pub action: GuiActionRequest,
    pub ticket_ttl_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiExecutionRequest {
    pub ticket: GuiExecutionTicket,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiExecutionOutcome {
    pub session: GuiInteractionSession,
    pub succeeded: bool,
    pub detail: Option<String>,
    pub steps_completed: usize,
    pub total_steps: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiReadinessPlatform {
    Macos,
    Windows,
    Linux,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiCapabilityState {
    Unavailable,
    Denied,
    Degraded,
    Available,
    Unsupported,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiCapabilityKind {
    ScreenVisibility,
    AccessibilityExtraction,
    OcrFallback,
    Overlay,
    InputExecution,
    Permissions,
    SandboxSupport,
    Audit,
    PrivacyPolicy,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiInputExecutionMode {
    Noop,
    DryRunWorker,
    SandboxedRealInput,
    DirectRealInput,
    Unsupported,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiInputExecutionModeReason {
    AutomationDisabled,
    ControllerMissing,
    GuiServiceMissing,
    HmacSecretMissing,
    PermissionDenied,
    PolicyDenied,
    OperatorConfiguredNoop,
    SandboxWorkerDryRun,
    SandboxWorkerRealInput,
    DirectNativeInput,
    UnsupportedPlatform,
    VerificationUnavailable,
    Unknown,
}

fn default_gui_input_execution_reason() -> GuiInputExecutionModeReason {
    GuiInputExecutionModeReason::Unknown
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiExecutionVerificationMode {
    None,
    CommandAccepted,
    ObservableStateChange,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiSessionConstraint {
    ForegroundOnly,
    BackgroundAllowed,
    LockedSessionAllowed,
    LockedSessionUnsupported,
    InteractiveSessionRequired,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiBenchmarkDecision {
    Run,
    Skip,
    Fail,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GuiCapabilityMatrix {
    pub screen_visibility: GuiCapabilityState,
    pub accessibility_extraction: GuiCapabilityState,
    pub ocr_fallback: GuiCapabilityState,
    pub overlay: GuiCapabilityState,
    pub input_execution: GuiCapabilityState,
    pub permissions: GuiCapabilityState,
    pub sandbox_support: GuiCapabilityState,
    pub audit: GuiCapabilityState,
    pub privacy_policy: GuiCapabilityState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GuiReadinessDiagnostic {
    pub code: String,
    pub capability: GuiCapabilityKind,
    pub state: GuiCapabilityState,
    pub display_label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remediation_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GuiReadinessSnapshot {
    pub schema_version: String,
    pub platform: GuiReadinessPlatform,
    pub captured_at: DateTime<Utc>,
    pub automation_enabled: bool,
    pub controller_built: bool,
    pub gui_service_configured: bool,
    pub hmac_secret_present: bool,
    pub input_execution_mode: GuiInputExecutionMode,
    #[serde(default = "default_gui_input_execution_reason")]
    pub input_execution_reason: GuiInputExecutionModeReason,
    pub execution_verification_mode: GuiExecutionVerificationMode,
    pub session_constraints: Vec<GuiSessionConstraint>,
    pub capabilities: GuiCapabilityMatrix,
    pub diagnostics: Vec<GuiReadinessDiagnostic>,
}

impl GuiReadinessSnapshot {
    pub fn benchmark_decision(&self) -> GuiBenchmarkDecision {
        if !self.automation_enabled
            || !self.controller_built
            || !self.gui_service_configured
            || !self.hmac_secret_present
            || self.capabilities.permissions == GuiCapabilityState::Denied
            || self.capabilities.screen_visibility == GuiCapabilityState::Denied
            || self.capabilities.privacy_policy == GuiCapabilityState::Denied
        {
            return GuiBenchmarkDecision::Fail;
        }

        if matches!(
            self.input_execution_mode,
            GuiInputExecutionMode::Noop
                | GuiInputExecutionMode::DryRunWorker
                | GuiInputExecutionMode::Unsupported
                | GuiInputExecutionMode::Unknown
        ) || self.execution_verification_mode
            != GuiExecutionVerificationMode::ObservableStateChange
            || matches!(
                self.capabilities.input_execution,
                GuiCapabilityState::Unavailable | GuiCapabilityState::Unsupported
            )
        {
            return GuiBenchmarkDecision::Skip;
        }

        GuiBenchmarkDecision::Run
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiSandboxWorkerExecutionKind {
    DryRunLogging,
    ExecutesRealInput,
    DelegatesToNativeInjector,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GuiSandboxWorkerExecutionContract {
    pub worker_id: String,
    pub execution_kind: GuiSandboxWorkerExecutionKind,
    pub input_execution_mode: GuiInputExecutionMode,
    pub verification_mode: GuiExecutionVerificationMode,
    pub requires_policy_approval: bool,
    pub emits_audit: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GuiRealInputExecutionContract {
    pub schema_version: String,
    pub sandbox_worker_contracts: Vec<GuiSandboxWorkerExecutionContract>,
}

impl GuiRealInputExecutionContract {
    pub fn validate_contract_coverage(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema_version != GUI_REAL_INPUT_EXECUTION_SCHEMA_VERSION {
            errors.push(format!(
                "schema_version must be {GUI_REAL_INPUT_EXECUTION_SCHEMA_VERSION}"
            ));
        }

        for required in [
            GuiSandboxWorkerExecutionKind::DryRunLogging,
            GuiSandboxWorkerExecutionKind::ExecutesRealInput,
            GuiSandboxWorkerExecutionKind::DelegatesToNativeInjector,
            GuiSandboxWorkerExecutionKind::Unsupported,
        ] {
            if !self
                .sandbox_worker_contracts
                .iter()
                .any(|contract| contract.execution_kind == required)
            {
                errors.push(format!("missing sandbox worker contract {required:?}"));
            }
        }

        for contract in &self.sandbox_worker_contracts {
            if contract.worker_id.trim().is_empty() {
                errors.push("sandbox worker contract must include worker_id".to_string());
            }
            match contract.execution_kind {
                GuiSandboxWorkerExecutionKind::DryRunLogging => {
                    if contract.input_execution_mode != GuiInputExecutionMode::DryRunWorker {
                        errors.push(format!(
                            "{} dry-run worker must report dry_run_worker mode",
                            contract.worker_id
                        ));
                    }
                    if contract.verification_mode
                        == GuiExecutionVerificationMode::ObservableStateChange
                    {
                        errors.push(format!(
                            "{} dry-run worker must not claim observable state change",
                            contract.worker_id
                        ));
                    }
                }
                GuiSandboxWorkerExecutionKind::ExecutesRealInput
                | GuiSandboxWorkerExecutionKind::DelegatesToNativeInjector => {
                    if contract.input_execution_mode != GuiInputExecutionMode::SandboxedRealInput {
                        errors.push(format!(
                            "{} real sandbox worker must report sandboxed_real_input mode",
                            contract.worker_id
                        ));
                    }
                    if contract.verification_mode
                        != GuiExecutionVerificationMode::ObservableStateChange
                    {
                        errors.push(format!(
                            "{} real sandbox worker must prove observable state change",
                            contract.worker_id
                        ));
                    }
                    if !contract.requires_policy_approval || !contract.emits_audit {
                        errors.push(format!(
                            "{} real sandbox worker must require policy approval and audit",
                            contract.worker_id
                        ));
                    }
                }
                GuiSandboxWorkerExecutionKind::Unsupported => {
                    if contract.input_execution_mode != GuiInputExecutionMode::Unsupported {
                        errors.push(format!(
                            "{} unsupported worker must report unsupported mode",
                            contract.worker_id
                        ));
                    }
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiSessionAcceptanceStage {
    Propose,
    Highlight,
    Confirm,
    Execute,
    Verify,
    Audit,
    Timeout,
    Cancel,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiSessionAcceptanceEvidenceKind {
    HttpResponse,
    SessionEvent,
    AuditRecord,
    OverlayGeometry,
    FocusSnapshot,
    TicketValidation,
    BeforeAfterState,
    ReadinessSnapshot,
    PolicyDecision,
    CapabilitySnapshot,
    PrivacySafeDiagnostic,
    TimingSpan,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiSessionFailureClass {
    PolicyDenied,
    PermissionDenied,
    CapabilityUnsupported,
    CapabilityDegraded,
    FocusDrift,
    StaleScene,
    StaleBoundingBox,
    CoordinateDrift,
    TicketInvalid,
    Timeout,
    AdapterUnavailable,
    ExecutionUnsupported,
    VerificationMissing,
    LegacyPath,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiSessionBenchmarkDefault {
    Included,
    Excluded,
    Conditional,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GuiSessionAcceptanceCase {
    pub case_id: String,
    pub title: String,
    pub stage: GuiSessionAcceptanceStage,
    pub platforms: Vec<GuiReadinessPlatform>,
    pub prerequisites: Vec<String>,
    pub action: String,
    pub expected_result: String,
    pub evidence: Vec<GuiSessionAcceptanceEvidenceKind>,
    pub failure_classification: Option<GuiSessionFailureClass>,
    pub benchmark_default: GuiSessionBenchmarkDefault,
    pub input_execution_mode: Option<GuiInputExecutionMode>,
    pub high_risk_action: bool,
    pub legacy_direct_execution: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GuiSessionAcceptanceMatrix {
    pub schema_version: String,
    pub cases: Vec<GuiSessionAcceptanceCase>,
}

impl GuiSessionAcceptanceMatrix {
    pub fn required_case_ids() -> &'static [&'static str] {
        &[
            "SESSION-PROPOSE-HAPPY",
            "HIGHLIGHT-OVERLAY-HIDPI",
            "HIGHLIGHT-OVERLAY-MULTI-MONITOR",
            "HIGHLIGHT-OVERLAY-NEGATIVE-ORIGIN",
            "HIGHLIGHT-PRIMARY-MONITOR-FALLBACK",
            "CONFIRM-FOCUS-REVALIDATION",
            "EXEC-SANDBOXED-REAL-INPUT",
            "EXEC-DIRECT-REAL-INPUT-HIGH-RISK",
            "EXEC-DRY-RUN-WORKER",
            "EXEC-NOOP",
            "EXEC-UNSUPPORTED",
            "VERIFY-BEFORE-AFTER-STATE",
            "AUDIT-SESSION-EVIDENCE",
            "TIMEOUT-SESSION-EXPIRES",
            "CANCEL-SESSION",
            "NEGATIVE-STALE-SCENE",
            "NEGATIVE-STALE-BOUNDS",
            "NEGATIVE-COORDINATE-DRIFT",
            "NEGATIVE-FOCUS-WINDOW-MISMATCH",
            "NEGATIVE-TICKET-CAPABILITY-FAILURE",
            "LEGACY-DIRECT-EXECUTION-SEPARATED",
        ]
    }

    pub fn validate_contract_coverage(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema_version != GUI_SESSION_ACCEPTANCE_MATRIX_SCHEMA_VERSION {
            errors.push(format!(
                "schema_version must be {GUI_SESSION_ACCEPTANCE_MATRIX_SCHEMA_VERSION}"
            ));
        }

        let mut case_ids = BTreeSet::new();
        let mut execution_modes = Vec::new();
        for case in &self.cases {
            if case.case_id.trim().is_empty() {
                errors.push("case_id must not be empty".to_string());
            } else if !case_ids.insert(case.case_id.as_str()) {
                errors.push(format!("duplicate case_id {}", case.case_id));
            }
            if case.title.trim().is_empty() {
                errors.push(format!("{} title must not be empty", case.case_id));
            }
            if case.platforms.is_empty() {
                errors.push(format!("{} must list at least one platform", case.case_id));
            }
            if case.prerequisites.is_empty() {
                errors.push(format!(
                    "{} must list at least one prerequisite",
                    case.case_id
                ));
            }
            if case.action.trim().is_empty() || case.expected_result.trim().is_empty() {
                errors.push(format!(
                    "{} must include action and expected_result",
                    case.case_id
                ));
            }
            if case.evidence.is_empty() {
                errors.push(format!(
                    "{} must list at least one evidence kind",
                    case.case_id
                ));
            }
            if case.high_risk_action
                && case.benchmark_default != GuiSessionBenchmarkDefault::Excluded
            {
                errors.push(format!(
                    "{} high-risk action must be excluded from default benchmark runs",
                    case.case_id
                ));
            }
            if case.legacy_direct_execution
                && case.benchmark_default == GuiSessionBenchmarkDefault::Included
            {
                errors.push(format!(
                    "{} legacy direct execution must not be a default benchmark case",
                    case.case_id
                ));
            }
            if case.stage == GuiSessionAcceptanceStage::Execute {
                if let Some(mode) = case.input_execution_mode {
                    if !execution_modes.contains(&mode) {
                        execution_modes.push(mode);
                    }
                }
                if matches!(
                    case.input_execution_mode,
                    Some(
                        GuiInputExecutionMode::SandboxedRealInput
                            | GuiInputExecutionMode::DirectRealInput
                    )
                ) && !case
                    .evidence
                    .contains(&GuiSessionAcceptanceEvidenceKind::BeforeAfterState)
                {
                    errors.push(format!(
                        "{} real input execution must require before/after state evidence",
                        case.case_id
                    ));
                }
            }
        }

        for required in Self::required_case_ids() {
            if !case_ids.contains(required) {
                errors.push(format!("missing required acceptance case {required}"));
            }
        }

        for mode in [
            GuiInputExecutionMode::Noop,
            GuiInputExecutionMode::DryRunWorker,
            GuiInputExecutionMode::SandboxedRealInput,
            GuiInputExecutionMode::DirectRealInput,
            GuiInputExecutionMode::Unsupported,
        ] {
            if !execution_modes.contains(&mode) {
                errors.push(format!("missing execute case for input mode {mode:?}"));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiPermissionKind {
    ScreenCapture,
    Accessibility,
    AutomationInputControl,
    Notifications,
    LocalServiceReachability,
    OcrCapability,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiPermissionRequirement {
    Required,
    Recommended,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiPermissionRemediationActionKind {
    OpenSystemSettings,
    EnableService,
    ConfigureLocalService,
    InstallCapability,
    RunDiagnostic,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GuiPlatformPermissionName {
    pub platform: GuiReadinessPlatform,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GuiPermissionRemediationRule {
    pub permission: GuiPermissionKind,
    pub requirement: GuiPermissionRequirement,
    pub platform_names: Vec<GuiPlatformPermissionName>,
    pub action_kind: GuiPermissionRemediationActionKind,
    pub remediation_key: String,
    pub safe_display_label: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiEvidenceArtifactKind {
    BroadScreenshot,
    CroppedRegion,
    TextMetadata,
    AuditExcerpt,
    LogExcerpt,
    WorkerLog,
    GuiSessionEvent,
    BenchmarkReport,
    RawAccessibilityLabel,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiEvidenceAudience {
    LocalOnly,
    AuditOnly,
    ShareableBenchmarkArtifact,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiEvidenceRedactionRule {
    MaskedSceneLabels,
    GeometryOnly,
    DiagnosticCodesOnly,
    MetadataOnly,
    RawAllowedLocalOnly,
    Prohibited,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiEvidenceOverrideRequirement {
    NecessityJustification,
    RetentionPolicy,
    DeletionPlan,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GuiEvidenceArtifactPolicyRule {
    pub artifact_kind: GuiEvidenceArtifactKind,
    pub default_audience: GuiEvidenceAudience,
    pub shareable_by_default: bool,
    pub requires_opt_in_override: bool,
    #[serde(default)]
    pub opt_in_override_requirements: Vec<GuiEvidenceOverrideRequirement>,
    pub raw_ui_allowed: bool,
    pub redaction_rule: GuiEvidenceRedactionRule,
    pub retention_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GuiEvidenceReviewRequest {
    pub artifact_kind: GuiEvidenceArtifactKind,
    pub requested_audience: GuiEvidenceAudience,
    pub opt_in_override: bool,
    pub override_requirement_confirmations: Vec<GuiEvidenceOverrideRequirement>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GuiEvidenceReviewDecision {
    pub accepted: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GuiPermissionEvidencePolicy {
    pub schema_version: String,
    pub permission_rules: Vec<GuiPermissionRemediationRule>,
    pub evidence_rules: Vec<GuiEvidenceArtifactPolicyRule>,
}

impl GuiPermissionEvidencePolicy {
    pub fn required_permission_kinds() -> &'static [GuiPermissionKind] {
        &[
            GuiPermissionKind::ScreenCapture,
            GuiPermissionKind::Accessibility,
            GuiPermissionKind::AutomationInputControl,
            GuiPermissionKind::Notifications,
            GuiPermissionKind::LocalServiceReachability,
            GuiPermissionKind::OcrCapability,
        ]
    }

    pub fn required_evidence_artifact_kinds() -> &'static [GuiEvidenceArtifactKind] {
        &[
            GuiEvidenceArtifactKind::BroadScreenshot,
            GuiEvidenceArtifactKind::CroppedRegion,
            GuiEvidenceArtifactKind::TextMetadata,
            GuiEvidenceArtifactKind::AuditExcerpt,
            GuiEvidenceArtifactKind::LogExcerpt,
            GuiEvidenceArtifactKind::WorkerLog,
            GuiEvidenceArtifactKind::GuiSessionEvent,
            GuiEvidenceArtifactKind::BenchmarkReport,
            GuiEvidenceArtifactKind::RawAccessibilityLabel,
        ]
    }

    pub fn required_override_requirements() -> &'static [GuiEvidenceOverrideRequirement] {
        &[
            GuiEvidenceOverrideRequirement::NecessityJustification,
            GuiEvidenceOverrideRequirement::RetentionPolicy,
            GuiEvidenceOverrideRequirement::DeletionPlan,
        ]
    }

    pub fn validate_contract_coverage(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema_version != GUI_PERMISSION_EVIDENCE_POLICY_SCHEMA_VERSION {
            errors.push(format!(
                "schema_version must be {GUI_PERMISSION_EVIDENCE_POLICY_SCHEMA_VERSION}"
            ));
        }

        let permissions = self
            .permission_rules
            .iter()
            .map(|rule| rule.permission)
            .collect::<Vec<_>>();
        for required in Self::required_permission_kinds() {
            if !permissions.contains(required) {
                errors.push(format!("missing permission rule {required:?}"));
            }
        }

        for rule in &self.permission_rules {
            if rule.platform_names.is_empty() {
                errors.push(format!("{:?} must include platform names", rule.permission));
            }
            for platform in [
                GuiReadinessPlatform::Macos,
                GuiReadinessPlatform::Windows,
                GuiReadinessPlatform::Linux,
            ] {
                if !rule
                    .platform_names
                    .iter()
                    .any(|name| name.platform == platform && !name.name.trim().is_empty())
                {
                    errors.push(format!(
                        "{:?} must include {:?} permission name",
                        rule.permission, platform
                    ));
                }
            }
            if rule.remediation_key.trim().is_empty() || rule.safe_display_label.trim().is_empty() {
                errors.push(format!(
                    "{:?} must include remediation_key and safe_display_label",
                    rule.permission
                ));
            }
        }

        let artifact_kinds = self
            .evidence_rules
            .iter()
            .map(|rule| rule.artifact_kind)
            .collect::<Vec<_>>();
        for required in Self::required_evidence_artifact_kinds() {
            if !artifact_kinds.contains(required) {
                errors.push(format!("missing evidence rule {required:?}"));
            }
        }

        for rule in &self.evidence_rules {
            if rule.retention_key.trim().is_empty() {
                errors.push(format!(
                    "{:?} must include retention_key",
                    rule.artifact_kind
                ));
            }
            if matches!(
                rule.artifact_kind,
                GuiEvidenceArtifactKind::AuditExcerpt
                    | GuiEvidenceArtifactKind::LogExcerpt
                    | GuiEvidenceArtifactKind::WorkerLog
                    | GuiEvidenceArtifactKind::GuiSessionEvent
                    | GuiEvidenceArtifactKind::BenchmarkReport
            ) && rule.raw_ui_allowed
            {
                errors.push(format!(
                    "{:?} must not allow raw UI payloads by default",
                    rule.artifact_kind
                ));
            }
            if rule.artifact_kind == GuiEvidenceArtifactKind::BroadScreenshot
                && (!rule.requires_opt_in_override || rule.shareable_by_default)
            {
                errors.push(
                    "broad screenshots must require opt-in and must not be shareable by default"
                        .to_string(),
                );
            }
            if rule.artifact_kind == GuiEvidenceArtifactKind::RawAccessibilityLabel
                && rule.default_audience != GuiEvidenceAudience::LocalOnly
            {
                errors.push("raw accessibility labels must be local-only".to_string());
            }
            if rule.requires_opt_in_override {
                for required in Self::required_override_requirements() {
                    if !rule.opt_in_override_requirements.contains(required) {
                        errors.push(format!(
                            "{:?} override must require {required:?}",
                            rule.artifact_kind
                        ));
                    }
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    pub fn review_artifact(&self, request: GuiEvidenceReviewRequest) -> GuiEvidenceReviewDecision {
        let Some(rule) = self
            .evidence_rules
            .iter()
            .find(|rule| rule.artifact_kind == request.artifact_kind)
        else {
            return GuiEvidenceReviewDecision {
                accepted: false,
                reason: Some("unknown evidence artifact kind".to_string()),
            };
        };

        if request.requested_audience == GuiEvidenceAudience::ShareableBenchmarkArtifact
            && !rule.shareable_by_default
            && rule.requires_opt_in_override
            && !request.opt_in_override
        {
            return GuiEvidenceReviewDecision {
                accepted: false,
                reason: Some("artifact requires explicit opt-in override".to_string()),
            };
        }

        if request.requested_audience == GuiEvidenceAudience::ShareableBenchmarkArtifact
            && !rule.shareable_by_default
            && rule.requires_opt_in_override
            && request.opt_in_override
        {
            let missing_requirement =
                rule.opt_in_override_requirements
                    .iter()
                    .find(|requirement| {
                        !request
                            .override_requirement_confirmations
                            .contains(requirement)
                    });
            if let Some(requirement) = missing_requirement {
                return GuiEvidenceReviewDecision {
                    accepted: false,
                    reason: Some(format!("override is missing requirement {requirement:?}")),
                };
            }
        }

        let audience_override = request.requested_audience
            == GuiEvidenceAudience::ShareableBenchmarkArtifact
            && rule.requires_opt_in_override
            && request.opt_in_override;
        if !audience_override
            && rule.default_audience == GuiEvidenceAudience::LocalOnly
            && request.requested_audience != GuiEvidenceAudience::LocalOnly
        {
            return GuiEvidenceReviewDecision {
                accepted: false,
                reason: Some("artifact is local-only and cannot be shared".to_string()),
            };
        }

        if request.requested_audience == GuiEvidenceAudience::ShareableBenchmarkArtifact
            && !rule.shareable_by_default
            && !rule.requires_opt_in_override
        {
            return GuiEvidenceReviewDecision {
                accepted: false,
                reason: Some("artifact is not shareable by policy".to_string()),
            };
        }

        if request.requested_audience == GuiEvidenceAudience::ShareableBenchmarkArtifact
            && rule.raw_ui_allowed
        {
            return GuiEvidenceReviewDecision {
                accepted: false,
                reason: Some("shareable artifacts cannot contain raw UI payloads".to_string()),
            };
        }

        GuiEvidenceReviewDecision {
            accepted: true,
            reason: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiBenchmarkPortKind {
    FocusProbe,
    ElementFinder,
    OverlayDriver,
    InputDriver,
    GuiSessionFlow,
    Launcher,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum GuiBenchmarkStage {
    LauncherReadiness,
    Focus,
    SceneExtraction,
    CandidateExtraction,
    OverlayLifecycle,
    InputAction,
    Verification,
    Audit,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiBenchmarkOutcome {
    Pass,
    Fail,
    Skip,
    Degraded,
    Blocked,
    Unsupported,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiBenchmarkFailureMode {
    PermissionDenied,
    CapabilityUnavailable,
    AdapterError,
    LauncherUnavailable,
    EvidenceMissing,
    VerificationMissing,
    PrivacyPolicyDenied,
    UnsupportedPlatform,
    Timeout,
    EmptyEvidence,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiBenchmarkResultField {
    Outcome,
    LatencyMs,
    Confidence,
    FailureMode,
    EvidencePath,
    PrivacyStatus,
    InputExecutionMode,
    VerificationMode,
    LauncherPlatform,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiBenchmarkPrivacyStatus {
    Safe,
    Redacted,
    LocalOnly,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GuiBenchmarkCase {
    pub case_id: String,
    pub title: String,
    pub port_kind: GuiBenchmarkPortKind,
    pub stage: GuiBenchmarkStage,
    pub platforms: Vec<GuiReadinessPlatform>,
    pub required_capabilities: Vec<GuiCapabilityKind>,
    pub input_execution_modes: Vec<GuiInputExecutionMode>,
    pub verification_modes: Vec<GuiExecutionVerificationMode>,
    pub expected_evidence: Vec<GuiEvidenceArtifactKind>,
    pub privacy_audience: GuiEvidenceAudience,
    pub requires_observable_state_change: bool,
    pub result_fields: Vec<GuiBenchmarkResultField>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GuiBenchmarkHarnessCatalog {
    pub schema_version: String,
    pub cases: Vec<GuiBenchmarkCase>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiBenchmarkResult {
    pub case_id: String,
    pub outcome: GuiBenchmarkOutcome,
    pub latency_ms: Option<u64>,
    pub confidence: Option<f64>,
    pub failure_mode: Option<GuiBenchmarkFailureMode>,
    pub evidence_paths: Vec<String>,
    pub evidence_artifacts: Vec<GuiEvidenceArtifactKind>,
    pub privacy_status: GuiBenchmarkPrivacyStatus,
    pub input_execution_mode: GuiInputExecutionMode,
    pub verification_mode: GuiExecutionVerificationMode,
    pub launcher_platform: GuiReadinessPlatform,
    pub adapter_name: String,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiBenchmarkReportSource {
    Criterion,
    OsInteractive,
    ManualReview,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiBenchmarkReportLocation {
    LocalJson,
    CiArtifact,
    ProjectIssueSummary,
    ManualReviewBundle,
    CriterionSummary,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiBenchmarkMetricKind {
    LatencyP95Ms,
    SuccessRateBasisPoints,
    SkipRateBasisPoints,
    BlockedRateBasisPoints,
    EvidenceFreshnessSeconds,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiBenchmarkThresholdComparator {
    LessThanOrEqual,
    GreaterThanOrEqual,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiBenchmarkThresholdSeverity {
    Advisory,
    Blocking,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiBenchmarkThresholdDecision {
    Pass,
    AdvisoryRegression,
    BlockingFailure,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GuiBenchmarkThresholdRule {
    pub metric: GuiBenchmarkMetricKind,
    pub comparator: GuiBenchmarkThresholdComparator,
    pub value: u64,
    pub severity: GuiBenchmarkThresholdSeverity,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GuiBenchmarkThresholdEvaluation {
    pub metric: GuiBenchmarkMetricKind,
    pub observed_value: u64,
    pub rule: GuiBenchmarkThresholdRule,
    pub decision: GuiBenchmarkThresholdDecision,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiBenchmarkReportedResult {
    pub result: GuiBenchmarkResult,
    pub evidence_fresh: bool,
    pub sidecar_present: bool,
    pub hmac_secret_present: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GuiBenchmarkPlatformSummary {
    pub platform: GuiReadinessPlatform,
    pub launcher_platform: GuiReadinessPlatform,
    pub sidecar_present: bool,
    pub hmac_secret_present: bool,
    pub capability_snapshot: GuiCapabilityMatrix,
    pub input_execution_mode: GuiInputExecutionMode,
    pub verification_mode: GuiExecutionVerificationMode,
    pub result_count: u64,
    pub pass_count: u64,
    pub fail_count: u64,
    pub skip_count: u64,
    pub blocked_count: u64,
    pub degraded_count: u64,
    pub unsupported_count: u64,
    pub stale_evidence_count: u64,
    pub privacy_statuses: Vec<GuiBenchmarkPrivacyStatus>,
    pub caveats: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiBenchmarkReport {
    pub schema_version: String,
    pub report_id: String,
    pub generated_at: DateTime<Utc>,
    pub source: GuiBenchmarkReportSource,
    pub report_locations: Vec<GuiBenchmarkReportLocation>,
    pub results: Vec<GuiBenchmarkReportedResult>,
    pub platform_summaries: Vec<GuiBenchmarkPlatformSummary>,
    pub threshold_policy: Vec<GuiBenchmarkThresholdRule>,
    pub threshold_evaluations: Vec<GuiBenchmarkThresholdEvaluation>,
}

impl GuiBenchmarkReport {
    pub fn validate_report(&self, catalog: &GuiBenchmarkHarnessCatalog) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema_version != GUI_BENCHMARK_REPORT_SCHEMA_VERSION {
            errors.push(format!(
                "schema_version must be {GUI_BENCHMARK_REPORT_SCHEMA_VERSION}"
            ));
        }
        if self.report_id.trim().is_empty() {
            errors.push("report_id must not be empty".to_string());
        }
        if self.report_locations.is_empty() {
            errors.push("report_locations must not be empty".to_string());
        }
        if self.results.is_empty() {
            errors.push("result stream must not be empty".to_string());
        }
        if self.platform_summaries.is_empty() {
            errors.push("platform_summaries must not be empty".to_string());
        }
        if !self
            .threshold_policy
            .iter()
            .any(|rule| rule.severity == GuiBenchmarkThresholdSeverity::Advisory)
        {
            errors.push("threshold_policy must include advisory thresholds".to_string());
        }
        if !self
            .threshold_policy
            .iter()
            .any(|rule| rule.severity == GuiBenchmarkThresholdSeverity::Blocking)
        {
            errors.push("threshold_policy must include blocking thresholds".to_string());
        }

        for reported in &self.results {
            if let Err(result_errors) = catalog.validate_result(&reported.result) {
                errors.extend(result_errors);
            }

            if reported.result.outcome == GuiBenchmarkOutcome::Pass && !reported.evidence_fresh {
                errors.push(format!(
                    "{} stale evidence cannot pass",
                    reported.result.case_id
                ));
            }
            if reported.result.outcome == GuiBenchmarkOutcome::Pass
                && matches!(
                    reported.result.input_execution_mode,
                    GuiInputExecutionMode::Noop | GuiInputExecutionMode::DryRunWorker
                )
            {
                errors.push(format!(
                    "{} dispatch-only or dry-run evidence cannot pass execution",
                    reported.result.case_id
                ));
            }
            if reported.result.outcome == GuiBenchmarkOutcome::Pass
                && reported.result.verification_mode
                    == GuiExecutionVerificationMode::CommandAccepted
                && reported.result.input_execution_mode != GuiInputExecutionMode::DirectRealInput
                && reported.result.input_execution_mode != GuiInputExecutionMode::SandboxedRealInput
            {
                errors.push(format!(
                    "{} command accepted is not pass evidence",
                    reported.result.case_id
                ));
            }
            if reported.result.outcome == GuiBenchmarkOutcome::Pass
                && reported.result.evidence_paths.is_empty()
            {
                errors.push(format!(
                    "{} missing artifacts cannot pass",
                    reported.result.case_id
                ));
            }
        }

        for summary in &self.platform_summaries {
            let total = summary.pass_count
                + summary.fail_count
                + summary.skip_count
                + summary.blocked_count
                + summary.degraded_count
                + summary.unsupported_count;
            if total != summary.result_count {
                errors.push(format!(
                    "{:?} platform summary counts must equal result_count",
                    summary.platform
                ));
            }
            if summary.launcher_platform == GuiReadinessPlatform::Unknown {
                errors.push(format!(
                    "{:?} platform summary must include launcher_platform",
                    summary.platform
                ));
            }
            if summary.privacy_statuses.is_empty() {
                errors.push(format!(
                    "{:?} platform summary must include privacy_statuses",
                    summary.platform
                ));
            }
            if (summary.skip_count
                + summary.blocked_count
                + summary.degraded_count
                + summary.unsupported_count)
                > 0
                && summary.caveats.is_empty()
            {
                errors.push(format!(
                    "{:?} platform caveats must explain non-pass outcomes",
                    summary.platform
                ));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

impl GuiBenchmarkHarnessCatalog {
    pub fn required_stages() -> &'static [GuiBenchmarkStage] {
        &[
            GuiBenchmarkStage::LauncherReadiness,
            GuiBenchmarkStage::Focus,
            GuiBenchmarkStage::SceneExtraction,
            GuiBenchmarkStage::CandidateExtraction,
            GuiBenchmarkStage::OverlayLifecycle,
            GuiBenchmarkStage::InputAction,
            GuiBenchmarkStage::Verification,
            GuiBenchmarkStage::Audit,
        ]
    }

    pub fn required_result_fields() -> &'static [GuiBenchmarkResultField] {
        &[
            GuiBenchmarkResultField::Outcome,
            GuiBenchmarkResultField::LatencyMs,
            GuiBenchmarkResultField::Confidence,
            GuiBenchmarkResultField::FailureMode,
            GuiBenchmarkResultField::EvidencePath,
            GuiBenchmarkResultField::PrivacyStatus,
            GuiBenchmarkResultField::InputExecutionMode,
            GuiBenchmarkResultField::VerificationMode,
            GuiBenchmarkResultField::LauncherPlatform,
        ]
    }

    pub fn supported_outcomes() -> &'static [GuiBenchmarkOutcome] {
        &[
            GuiBenchmarkOutcome::Pass,
            GuiBenchmarkOutcome::Fail,
            GuiBenchmarkOutcome::Skip,
            GuiBenchmarkOutcome::Degraded,
            GuiBenchmarkOutcome::Blocked,
            GuiBenchmarkOutcome::Unsupported,
        ]
    }

    pub fn validate_contract_coverage(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema_version != GUI_BENCHMARK_HARNESS_SCHEMA_VERSION {
            errors.push(format!(
                "schema_version must be {GUI_BENCHMARK_HARNESS_SCHEMA_VERSION}"
            ));
        }

        let case_ids = self
            .cases
            .iter()
            .map(|case| case.case_id.as_str())
            .collect::<Vec<_>>();
        let unique_case_ids = case_ids.iter().copied().collect::<BTreeSet<_>>();
        if unique_case_ids.len() != case_ids.len() {
            errors.push("case_id values must be unique".to_string());
        }

        let stages = self
            .cases
            .iter()
            .map(|case| case.stage)
            .collect::<BTreeSet<_>>();
        for required in Self::required_stages() {
            if !stages.contains(required) {
                errors.push(format!("missing benchmark stage {required:?}"));
            }
        }

        for case in &self.cases {
            if case.case_id.trim().is_empty() || case.title.trim().is_empty() {
                errors.push("benchmark cases must include case_id and title".to_string());
            }
            if case.platforms.is_empty() {
                errors.push(format!("{} must include target platforms", case.case_id));
            }
            if case.expected_evidence.is_empty() {
                errors.push(format!("{} must include expected evidence", case.case_id));
            }
            for required in Self::required_result_fields() {
                if !case.result_fields.contains(required) {
                    errors.push(format!(
                        "{} result schema must include {required:?}",
                        case.case_id
                    ));
                }
            }
            if case.stage == GuiBenchmarkStage::LauncherReadiness
                && case.port_kind != GuiBenchmarkPortKind::Launcher
            {
                errors.push(format!(
                    "{} launcher readiness must use launcher port kind",
                    case.case_id
                ));
            }
            if case.stage == GuiBenchmarkStage::InputAction {
                if !case.requires_observable_state_change {
                    errors.push(format!(
                        "{} input action must require observable state change",
                        case.case_id
                    ));
                }
                if case.input_execution_modes.is_empty() || case.verification_modes.is_empty() {
                    errors.push(format!(
                        "{} input action must declare execution and verification modes",
                        case.case_id
                    ));
                }
            }
            if case.privacy_audience == GuiEvidenceAudience::ShareableBenchmarkArtifact
                && case
                    .expected_evidence
                    .contains(&GuiEvidenceArtifactKind::BroadScreenshot)
            {
                errors.push(format!(
                    "{} shareable cases must not require broad screenshots",
                    case.case_id
                ));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    pub fn validate_result(&self, result: &GuiBenchmarkResult) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        let Some(case) = self
            .cases
            .iter()
            .find(|case| case.case_id == result.case_id)
        else {
            return Err(vec![format!("unknown benchmark case {}", result.case_id)]);
        };

        if result.adapter_name.trim().is_empty() {
            errors.push(format!(
                "{} result must include adapter_name",
                result.case_id
            ));
        }

        if result.outcome == GuiBenchmarkOutcome::Pass {
            if result.evidence_paths.is_empty() || result.evidence_artifacts.is_empty() {
                errors.push(format!(
                    "{} pass result must include evidence",
                    result.case_id
                ));
            }
            if result.privacy_status == GuiBenchmarkPrivacyStatus::Rejected {
                errors.push(format!(
                    "{} pass result cannot use rejected privacy status",
                    result.case_id
                ));
            }
            if case.requires_observable_state_change {
                if result.verification_mode != GuiExecutionVerificationMode::ObservableStateChange {
                    errors.push(format!(
                        "{} pass result must prove observable state change",
                        result.case_id
                    ));
                }
                if matches!(
                    result.input_execution_mode,
                    GuiInputExecutionMode::Noop
                        | GuiInputExecutionMode::DryRunWorker
                        | GuiInputExecutionMode::Unsupported
                        | GuiInputExecutionMode::Unknown
                ) {
                    errors.push(format!(
                        "{} observable state change requires real input execution mode",
                        result.case_id
                    ));
                }
            }
        } else if result.failure_mode.is_none() {
            errors.push(format!(
                "{} non-pass result must include failure_mode",
                result.case_id
            ));
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::intent::ElementBounds;
    use crate::models::ui_scene::{NormalizedBounds, UiSceneElement};

    #[test]
    fn gui_ticket_serde_roundtrip() {
        let ticket = GuiExecutionTicket {
            schema_version: GUI_TICKET_SCHEMA_VERSION.to_string(),
            ticket_id: "ticket-1".to_string(),
            session_id: "session-1".to_string(),
            scene_id: "scene-1".to_string(),
            element_id: "el-1".to_string(),
            action_hash: "abc".to_string(),
            focus_hash: "focus".to_string(),
            issued_at: Utc::now(),
            expires_at: Utc::now(),
            nonce: "nonce".to_string(),
            signature: "sig".to_string(),
        };

        let json = serde_json::to_string(&ticket).unwrap();
        let parsed: GuiExecutionTicket = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.ticket_id, "ticket-1");
        assert_eq!(parsed.schema_version, GUI_TICKET_SCHEMA_VERSION);
    }

    #[test]
    fn gui_candidate_wraps_scene_element() {
        let candidate = GuiCandidate {
            element: UiSceneElement {
                element_id: "el-1".to_string(),
                bbox_abs: ElementBounds {
                    x: 10,
                    y: 20,
                    width: 100,
                    height: 30,
                },
                bbox_norm: NormalizedBounds::new(0.0, 0.0, 1.0, 1.0),
                label: "Save".to_string(),
                role: Some("button".to_string()),
                intent: None,
                state: None,
                confidence: 0.9,
                text_masked: Some("Save".to_string()),
                parent_id: None,
            },
            ranking_reason: Some("confidence=0.90".to_string()),
            eligible: true,
        };

        assert_eq!(candidate.element.label, "Save");
        assert!(candidate.eligible);
    }

    #[test]
    fn gui_readiness_snapshot_serializes_os_neutral_contract() {
        let snapshot = GuiReadinessSnapshot {
            schema_version: GUI_READINESS_SCHEMA_VERSION.to_string(),
            platform: GuiReadinessPlatform::Windows,
            captured_at: Utc::now(),
            automation_enabled: true,
            controller_built: true,
            gui_service_configured: true,
            hmac_secret_present: true,
            input_execution_mode: GuiInputExecutionMode::SandboxedRealInput,
            input_execution_reason: GuiInputExecutionModeReason::SandboxWorkerRealInput,
            execution_verification_mode: GuiExecutionVerificationMode::ObservableStateChange,
            session_constraints: vec![GuiSessionConstraint::ForegroundOnly],
            capabilities: GuiCapabilityMatrix {
                screen_visibility: GuiCapabilityState::Available,
                accessibility_extraction: GuiCapabilityState::Degraded,
                ocr_fallback: GuiCapabilityState::Available,
                overlay: GuiCapabilityState::Available,
                input_execution: GuiCapabilityState::Available,
                permissions: GuiCapabilityState::Available,
                sandbox_support: GuiCapabilityState::Available,
                audit: GuiCapabilityState::Available,
                privacy_policy: GuiCapabilityState::Available,
            },
            diagnostics: vec![GuiReadinessDiagnostic {
                code: "uia_cache_partial".to_string(),
                capability: GuiCapabilityKind::AccessibilityExtraction,
                state: GuiCapabilityState::Degraded,
                display_label: "UIA cache coverage partial".to_string(),
                remediation_key: Some("windows_uia_cache".to_string()),
            }],
        };

        let value = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(value["platform"], "windows");
        assert_eq!(value["input_execution_mode"], "sandboxed_real_input");
        assert_eq!(value["input_execution_reason"], "sandbox_worker_real_input");
        assert_eq!(
            value["execution_verification_mode"],
            "observable_state_change"
        );
        assert_eq!(value["session_constraints"][0], "foreground_only");
        assert_eq!(
            value["capabilities"]["accessibility_extraction"],
            "degraded"
        );
        assert_eq!(
            value["diagnostics"][0]["capability"],
            "accessibility_extraction"
        );
        assert!(value["diagnostics"][0].get("raw_window_title").is_none());
    }

    #[test]
    fn gui_readiness_snapshot_can_express_macos_background_constraints() {
        let snapshot = GuiReadinessSnapshot {
            schema_version: GUI_READINESS_SCHEMA_VERSION.to_string(),
            platform: GuiReadinessPlatform::Macos,
            captured_at: Utc::now(),
            automation_enabled: false,
            controller_built: false,
            gui_service_configured: false,
            hmac_secret_present: false,
            input_execution_mode: GuiInputExecutionMode::Noop,
            input_execution_reason: GuiInputExecutionModeReason::AutomationDisabled,
            execution_verification_mode: GuiExecutionVerificationMode::None,
            session_constraints: vec![
                GuiSessionConstraint::BackgroundAllowed,
                GuiSessionConstraint::LockedSessionUnsupported,
            ],
            capabilities: GuiCapabilityMatrix {
                screen_visibility: GuiCapabilityState::Denied,
                accessibility_extraction: GuiCapabilityState::Unavailable,
                ocr_fallback: GuiCapabilityState::Unsupported,
                overlay: GuiCapabilityState::Unavailable,
                input_execution: GuiCapabilityState::Unavailable,
                permissions: GuiCapabilityState::Denied,
                sandbox_support: GuiCapabilityState::Unsupported,
                audit: GuiCapabilityState::Available,
                privacy_policy: GuiCapabilityState::Available,
            },
            diagnostics: vec![],
        };

        assert_eq!(snapshot.benchmark_decision(), GuiBenchmarkDecision::Fail);
        assert!(snapshot
            .session_constraints
            .contains(&GuiSessionConstraint::BackgroundAllowed));
        assert!(snapshot
            .session_constraints
            .contains(&GuiSessionConstraint::LockedSessionUnsupported));
    }

    #[test]
    fn gui_readiness_snapshot_skips_benchmark_when_real_input_is_unsupported() {
        let snapshot = GuiReadinessSnapshot {
            schema_version: GUI_READINESS_SCHEMA_VERSION.to_string(),
            platform: GuiReadinessPlatform::Linux,
            captured_at: Utc::now(),
            automation_enabled: true,
            controller_built: true,
            gui_service_configured: true,
            hmac_secret_present: true,
            input_execution_mode: GuiInputExecutionMode::Unsupported,
            input_execution_reason: GuiInputExecutionModeReason::UnsupportedPlatform,
            execution_verification_mode: GuiExecutionVerificationMode::None,
            session_constraints: vec![GuiSessionConstraint::InteractiveSessionRequired],
            capabilities: GuiCapabilityMatrix {
                screen_visibility: GuiCapabilityState::Available,
                accessibility_extraction: GuiCapabilityState::Available,
                ocr_fallback: GuiCapabilityState::Available,
                overlay: GuiCapabilityState::Available,
                input_execution: GuiCapabilityState::Unsupported,
                permissions: GuiCapabilityState::Available,
                sandbox_support: GuiCapabilityState::Unsupported,
                audit: GuiCapabilityState::Available,
                privacy_policy: GuiCapabilityState::Available,
            },
            diagnostics: vec![],
        };

        assert_eq!(snapshot.benchmark_decision(), GuiBenchmarkDecision::Skip);
    }

    #[test]
    fn gui_session_acceptance_matrix_fixture_covers_required_cases() {
        let matrix: GuiSessionAcceptanceMatrix = serde_json::from_str(include_str!(
            "../../../../docs/contracts/gui-session-acceptance-matrix.v1.json"
        ))
        .unwrap();

        matrix.validate_contract_coverage().unwrap();

        let case_ids = matrix
            .cases
            .iter()
            .map(|case| case.case_id.as_str())
            .collect::<BTreeSet<_>>();
        for required in GuiSessionAcceptanceMatrix::required_case_ids() {
            assert!(case_ids.contains(required), "missing case {required}");
        }

        let windows_execute = matrix
            .cases
            .iter()
            .find(|case| case.case_id == "EXEC-SANDBOXED-REAL-INPUT")
            .unwrap();
        assert!(windows_execute
            .platforms
            .contains(&GuiReadinessPlatform::Windows));
        assert_eq!(
            windows_execute.input_execution_mode,
            Some(GuiInputExecutionMode::SandboxedRealInput)
        );
        assert!(windows_execute
            .evidence
            .contains(&GuiSessionAcceptanceEvidenceKind::BeforeAfterState));
    }

    #[test]
    fn gui_session_acceptance_matrix_rejects_high_risk_default_case() {
        let matrix = GuiSessionAcceptanceMatrix {
            schema_version: GUI_SESSION_ACCEPTANCE_MATRIX_SCHEMA_VERSION.to_string(),
            cases: vec![GuiSessionAcceptanceCase {
                case_id: "EXEC-HIGH-RISK-DEFAULT".to_string(),
                title: "High-risk execute should not be default".to_string(),
                stage: GuiSessionAcceptanceStage::Execute,
                platforms: vec![GuiReadinessPlatform::Windows],
                prerequisites: vec!["policy consent granted".to_string()],
                action: "execute high-risk typed input".to_string(),
                expected_result: "must be excluded from default benchmark".to_string(),
                evidence: vec![GuiSessionAcceptanceEvidenceKind::PolicyDecision],
                failure_classification: Some(GuiSessionFailureClass::PolicyDenied),
                benchmark_default: GuiSessionBenchmarkDefault::Included,
                input_execution_mode: Some(GuiInputExecutionMode::DirectRealInput),
                high_risk_action: true,
                legacy_direct_execution: false,
            }],
        };

        let errors = matrix.validate_contract_coverage().unwrap_err();
        assert!(errors
            .iter()
            .any(|error| error.contains("high-risk action must be excluded")));
    }

    #[test]
    fn gui_permission_evidence_policy_fixture_covers_required_rules() {
        let policy: GuiPermissionEvidencePolicy = serde_json::from_str(include_str!(
            "../../../../docs/contracts/gui-permission-evidence-policy.v1.json"
        ))
        .unwrap();

        policy.validate_contract_coverage().unwrap();

        let permission_kinds = policy
            .permission_rules
            .iter()
            .map(|rule| rule.permission)
            .collect::<Vec<_>>();
        for required in GuiPermissionEvidencePolicy::required_permission_kinds() {
            assert!(permission_kinds.contains(required), "missing {required:?}");
        }

        let broad_screenshot = policy
            .evidence_rules
            .iter()
            .find(|rule| rule.artifact_kind == GuiEvidenceArtifactKind::BroadScreenshot)
            .unwrap();
        assert!(broad_screenshot.requires_opt_in_override);
        assert!(!broad_screenshot.shareable_by_default);
        for required in GuiPermissionEvidencePolicy::required_override_requirements() {
            assert!(
                broad_screenshot
                    .opt_in_override_requirements
                    .contains(required),
                "missing override requirement {required:?}"
            );
        }

        let raw_label = policy
            .evidence_rules
            .iter()
            .find(|rule| rule.artifact_kind == GuiEvidenceArtifactKind::RawAccessibilityLabel)
            .unwrap();
        assert_eq!(raw_label.default_audience, GuiEvidenceAudience::LocalOnly);
        assert!(raw_label.raw_ui_allowed);
    }

    #[test]
    fn gui_permission_evidence_policy_rejects_unsafe_artifacts_by_default() {
        let policy: GuiPermissionEvidencePolicy = serde_json::from_str(include_str!(
            "../../../../docs/contracts/gui-permission-evidence-policy.v1.json"
        ))
        .unwrap();

        let broad_screenshot = policy.review_artifact(GuiEvidenceReviewRequest {
            artifact_kind: GuiEvidenceArtifactKind::BroadScreenshot,
            requested_audience: GuiEvidenceAudience::ShareableBenchmarkArtifact,
            opt_in_override: false,
            override_requirement_confirmations: Vec::new(),
        });
        assert!(!broad_screenshot.accepted);
        assert!(broad_screenshot
            .reason
            .unwrap()
            .contains("explicit opt-in override"));

        let incomplete_override = policy.review_artifact(GuiEvidenceReviewRequest {
            artifact_kind: GuiEvidenceArtifactKind::BroadScreenshot,
            requested_audience: GuiEvidenceAudience::ShareableBenchmarkArtifact,
            opt_in_override: true,
            override_requirement_confirmations: vec![
                GuiEvidenceOverrideRequirement::NecessityJustification,
            ],
        });
        assert!(!incomplete_override.accepted);
        assert!(incomplete_override
            .reason
            .unwrap()
            .contains("override is missing requirement"));

        let approved_broad_screenshot = policy.review_artifact(GuiEvidenceReviewRequest {
            artifact_kind: GuiEvidenceArtifactKind::BroadScreenshot,
            requested_audience: GuiEvidenceAudience::ShareableBenchmarkArtifact,
            opt_in_override: true,
            override_requirement_confirmations:
                GuiPermissionEvidencePolicy::required_override_requirements().to_vec(),
        });
        assert!(approved_broad_screenshot.accepted);

        let raw_label = policy.review_artifact(GuiEvidenceReviewRequest {
            artifact_kind: GuiEvidenceArtifactKind::RawAccessibilityLabel,
            requested_audience: GuiEvidenceAudience::ShareableBenchmarkArtifact,
            opt_in_override: true,
            override_requirement_confirmations:
                GuiPermissionEvidencePolicy::required_override_requirements().to_vec(),
        });
        assert!(!raw_label.accepted);
        assert!(raw_label.reason.unwrap().contains("local-only"));
    }

    #[test]
    fn gui_benchmark_harness_fixture_covers_shared_execution_plane() {
        let catalog: GuiBenchmarkHarnessCatalog = serde_json::from_str(include_str!(
            "../../../../docs/contracts/gui-benchmark-harness.v1.json"
        ))
        .unwrap();

        catalog.validate_contract_coverage().unwrap();

        for required in GuiBenchmarkHarnessCatalog::required_stages() {
            assert!(
                catalog.cases.iter().any(|case| case.stage == *required),
                "missing benchmark stage {required:?}"
            );
        }

        let launcher_case = catalog
            .cases
            .iter()
            .find(|case| case.stage == GuiBenchmarkStage::LauncherReadiness)
            .unwrap();
        assert_eq!(launcher_case.port_kind, GuiBenchmarkPortKind::Launcher);
        assert!(launcher_case
            .platforms
            .contains(&GuiReadinessPlatform::Windows));

        let input_case = catalog
            .cases
            .iter()
            .find(|case| case.stage == GuiBenchmarkStage::InputAction)
            .unwrap();
        assert!(input_case.requires_observable_state_change);
        assert!(input_case
            .result_fields
            .contains(&GuiBenchmarkResultField::InputExecutionMode));
        assert!(input_case
            .result_fields
            .contains(&GuiBenchmarkResultField::VerificationMode));
        assert!(input_case
            .result_fields
            .contains(&GuiBenchmarkResultField::LauncherPlatform));
    }

    #[test]
    fn gui_benchmark_harness_rejects_empty_or_unproven_pass_results() {
        let catalog: GuiBenchmarkHarnessCatalog = serde_json::from_str(include_str!(
            "../../../../docs/contracts/gui-benchmark-harness.v1.json"
        ))
        .unwrap();

        let empty_evidence_pass = GuiBenchmarkResult {
            case_id: "INPUT-ACTION-OBSERVABLE-STATE".to_string(),
            outcome: GuiBenchmarkOutcome::Pass,
            latency_ms: Some(25),
            confidence: Some(0.91),
            failure_mode: None,
            evidence_paths: Vec::new(),
            evidence_artifacts: Vec::new(),
            privacy_status: GuiBenchmarkPrivacyStatus::Redacted,
            input_execution_mode: GuiInputExecutionMode::DirectRealInput,
            verification_mode: GuiExecutionVerificationMode::ObservableStateChange,
            launcher_platform: GuiReadinessPlatform::Windows,
            adapter_name: "windows-uia".to_string(),
            message: None,
        };
        let errors = catalog.validate_result(&empty_evidence_pass).unwrap_err();
        assert!(errors
            .iter()
            .any(|error| error.contains("pass result must include evidence")));

        let dry_run_pass = GuiBenchmarkResult {
            case_id: "INPUT-ACTION-OBSERVABLE-STATE".to_string(),
            outcome: GuiBenchmarkOutcome::Pass,
            latency_ms: Some(10),
            confidence: Some(0.8),
            failure_mode: None,
            evidence_paths: vec!["artifact://state-summary".to_string()],
            evidence_artifacts: vec![GuiEvidenceArtifactKind::BenchmarkReport],
            privacy_status: GuiBenchmarkPrivacyStatus::Redacted,
            input_execution_mode: GuiInputExecutionMode::DryRunWorker,
            verification_mode: GuiExecutionVerificationMode::CommandAccepted,
            launcher_platform: GuiReadinessPlatform::Windows,
            adapter_name: "dry-run-worker".to_string(),
            message: None,
        };
        let errors = catalog.validate_result(&dry_run_pass).unwrap_err();
        assert!(errors
            .iter()
            .any(|error| error.contains("observable state change")));
    }

    #[test]
    fn gui_real_input_execution_contract_fixture_distinguishes_dispatch_modes() {
        let contract: GuiRealInputExecutionContract = serde_json::from_str(include_str!(
            "../../../../docs/contracts/gui-real-input-execution.v1.json"
        ))
        .unwrap();

        contract.validate_contract_coverage().unwrap();

        let dry_run = contract
            .sandbox_worker_contracts
            .iter()
            .find(|contract| {
                contract.execution_kind == GuiSandboxWorkerExecutionKind::DryRunLogging
            })
            .unwrap();
        assert_eq!(
            dry_run.input_execution_mode,
            GuiInputExecutionMode::DryRunWorker
        );
        assert_eq!(
            dry_run.verification_mode,
            GuiExecutionVerificationMode::CommandAccepted
        );

        let delegated_real_input = contract
            .sandbox_worker_contracts
            .iter()
            .find(|contract| {
                contract.execution_kind == GuiSandboxWorkerExecutionKind::DelegatesToNativeInjector
            })
            .unwrap();
        assert_eq!(
            delegated_real_input.input_execution_mode,
            GuiInputExecutionMode::SandboxedRealInput
        );
        assert_eq!(
            delegated_real_input.verification_mode,
            GuiExecutionVerificationMode::ObservableStateChange
        );
    }

    #[test]
    fn gui_readiness_skips_command_accepted_real_input_without_state_change() {
        let snapshot = GuiReadinessSnapshot {
            schema_version: GUI_READINESS_SCHEMA_VERSION.to_string(),
            platform: GuiReadinessPlatform::Windows,
            captured_at: Utc::now(),
            automation_enabled: true,
            controller_built: true,
            gui_service_configured: true,
            hmac_secret_present: true,
            input_execution_mode: GuiInputExecutionMode::DirectRealInput,
            input_execution_reason: GuiInputExecutionModeReason::DirectNativeInput,
            execution_verification_mode: GuiExecutionVerificationMode::CommandAccepted,
            session_constraints: vec![GuiSessionConstraint::InteractiveSessionRequired],
            capabilities: GuiCapabilityMatrix {
                screen_visibility: GuiCapabilityState::Available,
                accessibility_extraction: GuiCapabilityState::Available,
                ocr_fallback: GuiCapabilityState::Degraded,
                overlay: GuiCapabilityState::Available,
                input_execution: GuiCapabilityState::Available,
                permissions: GuiCapabilityState::Available,
                sandbox_support: GuiCapabilityState::Available,
                audit: GuiCapabilityState::Available,
                privacy_policy: GuiCapabilityState::Available,
            },
            diagnostics: Vec::new(),
        };

        assert_eq!(snapshot.benchmark_decision(), GuiBenchmarkDecision::Skip);
    }

    #[test]
    fn gui_benchmark_report_fixture_covers_thresholds_and_locations() {
        let catalog: GuiBenchmarkHarnessCatalog = serde_json::from_str(include_str!(
            "../../../../docs/contracts/gui-benchmark-harness.v1.json"
        ))
        .unwrap();
        let report: GuiBenchmarkReport = serde_json::from_str(include_str!(
            "../../../../docs/contracts/gui-benchmark-report.v1.json"
        ))
        .unwrap();

        report.validate_report(&catalog).unwrap();

        assert!(report
            .report_locations
            .contains(&GuiBenchmarkReportLocation::ProjectIssueSummary));
        assert!(report
            .report_locations
            .contains(&GuiBenchmarkReportLocation::CiArtifact));
        assert!(report
            .threshold_policy
            .iter()
            .any(|rule| rule.severity == GuiBenchmarkThresholdSeverity::Advisory));
        assert!(report
            .threshold_policy
            .iter()
            .any(|rule| rule.severity == GuiBenchmarkThresholdSeverity::Blocking));

        let windows = report
            .platform_summaries
            .iter()
            .find(|summary| summary.platform == GuiReadinessPlatform::Windows)
            .unwrap();
        assert_eq!(windows.launcher_platform, GuiReadinessPlatform::Windows);
        assert!(windows.sidecar_present);
        assert!(windows.hmac_secret_present);
    }

    #[test]
    fn gui_benchmark_report_rejects_empty_stale_or_dispatch_only_pass() {
        let catalog: GuiBenchmarkHarnessCatalog = serde_json::from_str(include_str!(
            "../../../../docs/contracts/gui-benchmark-harness.v1.json"
        ))
        .unwrap();
        let mut report: GuiBenchmarkReport = serde_json::from_str(include_str!(
            "../../../../docs/contracts/gui-benchmark-report.v1.json"
        ))
        .unwrap();

        let mut empty_report = report.clone();
        empty_report.results.clear();
        let errors = empty_report.validate_report(&catalog).unwrap_err();
        assert!(errors
            .iter()
            .any(|error| error.contains("result stream must not be empty")));

        report.results[0].evidence_fresh = false;
        let errors = report.validate_report(&catalog).unwrap_err();
        assert!(errors
            .iter()
            .any(|error| error.contains("stale evidence cannot pass")));

        let mut dispatch_only = report.clone();
        dispatch_only.results[0].evidence_fresh = true;
        dispatch_only.results[0].result.input_execution_mode = GuiInputExecutionMode::DryRunWorker;
        dispatch_only.results[0].result.verification_mode =
            GuiExecutionVerificationMode::CommandAccepted;
        let errors = dispatch_only.validate_report(&catalog).unwrap_err();
        assert!(errors
            .iter()
            .any(|error| error.contains("dispatch-only or dry-run evidence")));
    }
}
