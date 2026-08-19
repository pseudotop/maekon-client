//! GUI session acceptance-spec models — the fixed catalog of acceptance test
//! cases (`GuiSessionAcceptanceMatrix`) that every session-flow adapter must
//! satisfy, plus the coverage validation enforcing that catalog's invariants.
//!
//! Split from `models/gui.rs` (issue #7721 F4). Pure move — no behavior change.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::readiness::{GuiInputExecutionMode, GuiReadinessPlatform};

pub const GUI_SESSION_ACCEPTANCE_MATRIX_SCHEMA_VERSION: &str =
    "automation.gui.acceptance_matrix.v1";

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gui_session_acceptance_matrix_fixture_covers_required_cases() {
        let matrix: GuiSessionAcceptanceMatrix = serde_json::from_str(include_str!(
            "../../../../../docs/contracts/gui-session-acceptance-matrix.v1.json"
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
}
