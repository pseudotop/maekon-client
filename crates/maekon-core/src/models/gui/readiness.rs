//! GUI readiness/capability reporting models — per-platform capability
//! matrix, readiness snapshot, and the OS-neutral input execution /
//! verification mode taxonomy shared by the rest of the GUI domain.
//!
//! Split from `models/gui.rs` (issue #7721 F4). Pure move — no behavior change.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const GUI_READINESS_SCHEMA_VERSION: &str = "automation.gui.readiness.v1";

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
