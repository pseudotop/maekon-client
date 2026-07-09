//! GUI benchmark harness + report models — the shared execution-plane
//! benchmark case catalog (`GuiBenchmarkHarnessCatalog`), individual results,
//! threshold policy, and the aggregate benchmark report + its validation.
//!
//! Split from `models/gui.rs` (issue #7721 F4). Pure move — no behavior change.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::permission_policy::{GuiEvidenceArtifactKind, GuiEvidenceAudience};
use super::readiness::{
    GuiCapabilityKind, GuiCapabilityMatrix, GuiExecutionVerificationMode, GuiInputExecutionMode,
    GuiReadinessPlatform,
};

pub const GUI_BENCHMARK_HARNESS_SCHEMA_VERSION: &str = "automation.gui.benchmark_harness.v1";
pub const GUI_BENCHMARK_REPORT_SCHEMA_VERSION: &str = "automation.gui.benchmark_report.v1";

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

    #[test]
    fn gui_benchmark_harness_fixture_covers_shared_execution_plane() {
        let catalog: GuiBenchmarkHarnessCatalog = serde_json::from_str(include_str!(
            "../../../../../docs/contracts/gui-benchmark-harness.v1.json"
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
            "../../../../../docs/contracts/gui-benchmark-harness.v1.json"
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
    fn gui_benchmark_report_fixture_covers_thresholds_and_locations() {
        let catalog: GuiBenchmarkHarnessCatalog = serde_json::from_str(include_str!(
            "../../../../../docs/contracts/gui-benchmark-harness.v1.json"
        ))
        .unwrap();
        let report: GuiBenchmarkReport = serde_json::from_str(include_str!(
            "../../../../../docs/contracts/gui-benchmark-report.v1.json"
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
            "../../../../../docs/contracts/gui-benchmark-harness.v1.json"
        ))
        .unwrap();
        let mut report: GuiBenchmarkReport = serde_json::from_str(include_str!(
            "../../../../../docs/contracts/gui-benchmark-report.v1.json"
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
