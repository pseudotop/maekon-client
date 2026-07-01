use std::collections::BTreeSet;

use maekon_core::config::DEFAULT_MIN_LLM_CONFIDENCE;
use maekon_core::models::gui::{
    GUI_INTERACTION_SCHEMA_VERSION, GUI_PERMISSION_EVIDENCE_POLICY_SCHEMA_VERSION,
    GUI_REAL_INPUT_EXECUTION_SCHEMA_VERSION, GUI_TICKET_SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};

pub const TEMPLATE_DRAFT_SCHEMA_VERSION: &str = "automation.template_draft.v1";
pub const TEMPLATE_PACKAGE_SCHEMA_VERSION: &str = "automation.template_package.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateWorkflowKind {
    LocalDashboard,
    OsNeutralPreset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateReviewState {
    RequiresUserReview,
    Reviewed,
    Signed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateProofRequirement {
    ObservableStateChange,
    DryRunOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateExecutionModeClass {
    DryRunOnly,
    RealInputGated,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedWorkflowObservation {
    pub evidence_id: String,
    pub schema_version: String,
    pub artifact_type: String,
    pub action_kind: String,
    pub stable_target_id: String,
    pub raw_label: Option<String>,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateStepDraft {
    pub step_id: String,
    pub action_kind: String,
    pub stable_target_id: String,
    pub redacted_label: Option<String>,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateDraft {
    pub schema_version: String,
    pub template_id: String,
    pub workflow_kind: TemplateWorkflowKind,
    pub steps: Vec<TemplateStepDraft>,
    pub required_permissions: Vec<String>,
    pub evidence_policy: String,
    pub execution_contracts: Vec<String>,
    pub execution_mode: TemplateExecutionModeClass,
    pub source_evidence_ids: Vec<String>,
    pub unsafe_findings: Vec<String>,
    pub review_state: TemplateReviewState,
    pub min_llm_confidence_floor: f64,
}

impl TemplateDraft {
    pub fn passes_llm_confidence_floor(&self, confidence: f64) -> bool {
        confidence >= self.min_llm_confidence_floor
    }

    pub fn can_auto_execute(&self) -> bool {
        false
    }

    pub fn package_for_review(
        &self,
        publisher: TemplatePublisher,
        rollback_policy: TemplateRollbackPolicy,
        test_fixture: TemplateTestFixture,
    ) -> TemplatePackage {
        TemplatePackage {
            version: TEMPLATE_PACKAGE_SCHEMA_VERSION.to_string(),
            template_id: self.template_id.clone(),
            workflow_kind: self.workflow_kind,
            steps: self.steps.clone(),
            required_permissions: self.required_permissions.clone(),
            evidence_policy: self.evidence_policy.clone(),
            execution_contracts: self.execution_contracts.clone(),
            execution_mode: self.execution_mode,
            source_evidence_ids: self.source_evidence_ids.clone(),
            unsafe_findings: self.unsafe_findings.clone(),
            review_state: TemplateReviewState::RequiresUserReview,
            min_llm_confidence_floor: self.min_llm_confidence_floor,
            execution_gates: TemplateExecutionGates::default(),
            publisher,
            rollback_policy,
            test_fixture,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplatePublisher {
    pub publisher_id: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateRollbackPolicy {
    pub expires_after_days: u32,
    pub rollback_strategy: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateTestFixture {
    pub fixture_id: String,
    pub proof_required: TemplateProofRequirement,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateExecutionGates {
    pub policy_validation_required: bool,
    pub audit_logging_required: bool,
    pub user_confirmation_required: bool,
    pub signed_ticket_required: bool,
}

impl Default for TemplateExecutionGates {
    fn default() -> Self {
        Self {
            policy_validation_required: true,
            audit_logging_required: true,
            user_confirmation_required: true,
            signed_ticket_required: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplatePackage {
    pub version: String,
    pub template_id: String,
    pub workflow_kind: TemplateWorkflowKind,
    pub steps: Vec<TemplateStepDraft>,
    pub required_permissions: Vec<String>,
    pub evidence_policy: String,
    pub execution_contracts: Vec<String>,
    pub execution_mode: TemplateExecutionModeClass,
    pub source_evidence_ids: Vec<String>,
    pub unsafe_findings: Vec<String>,
    pub review_state: TemplateReviewState,
    pub min_llm_confidence_floor: f64,
    pub execution_gates: TemplateExecutionGates,
    pub publisher: TemplatePublisher,
    pub rollback_policy: TemplateRollbackPolicy,
    pub test_fixture: TemplateTestFixture,
}

impl TemplatePackage {
    pub fn accepts_command_acceptance_only_proof(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone)]
pub struct RecordToTemplateAuthor {
    min_llm_confidence_floor: f64,
}

impl RecordToTemplateAuthor {
    pub fn draft_from_observations(
        &self,
        template_id: impl Into<String>,
        workflow_kind: TemplateWorkflowKind,
        observations: Vec<RecordedWorkflowObservation>,
    ) -> Result<TemplateDraft, String> {
        if observations.is_empty() {
            return Err("template draft requires at least one observation".to_string());
        }

        let mut source_evidence_ids = BTreeSet::new();
        let mut unsafe_findings = BTreeSet::new();
        let mut steps = Vec::with_capacity(observations.len());

        for (index, observation) in observations.into_iter().enumerate() {
            if observation.artifact_type == "broad_screenshot" {
                return Err("broad_screenshot artifact is rejected for template drafts".to_string());
            }
            validate_safe_identifier("evidence_id", &observation.evidence_id)?;
            validate_safe_identifier("schema_version", &observation.schema_version)?;
            validate_safe_identifier("artifact_type", &observation.artifact_type)?;
            validate_safe_identifier("action_kind", &observation.action_kind)?;
            validate_safe_identifier("stable_target_id", &observation.stable_target_id)?;
            if observation.raw_label.is_some() {
                unsafe_findings.insert("field.raw_label.omitted".to_string());
            }

            source_evidence_ids.insert(observation.evidence_id);
            steps.push(TemplateStepDraft {
                step_id: format!("step-{:03}", index + 1),
                action_kind: observation.action_kind,
                stable_target_id: observation.stable_target_id,
                redacted_label: observation
                    .raw_label
                    .as_ref()
                    .map(|_| "[omitted:raw-label]".to_string()),
                confidence: observation.confidence,
            });
        }

        Ok(TemplateDraft {
            schema_version: TEMPLATE_DRAFT_SCHEMA_VERSION.to_string(),
            template_id: template_id.into(),
            workflow_kind,
            steps,
            required_permissions: required_permissions_for(workflow_kind),
            evidence_policy: GUI_PERMISSION_EVIDENCE_POLICY_SCHEMA_VERSION.to_string(),
            execution_contracts: vec![
                GUI_INTERACTION_SCHEMA_VERSION.to_string(),
                GUI_TICKET_SCHEMA_VERSION.to_string(),
                GUI_REAL_INPUT_EXECUTION_SCHEMA_VERSION.to_string(),
            ],
            execution_mode: execution_mode_for(workflow_kind),
            source_evidence_ids: source_evidence_ids.into_iter().collect(),
            unsafe_findings: unsafe_findings.into_iter().collect(),
            review_state: TemplateReviewState::RequiresUserReview,
            min_llm_confidence_floor: self.min_llm_confidence_floor,
        })
    }
}

impl Default for RecordToTemplateAuthor {
    fn default() -> Self {
        Self {
            min_llm_confidence_floor: DEFAULT_MIN_LLM_CONFIDENCE,
        }
    }
}

pub fn sample_local_dashboard_workflow() -> TemplateDraft {
    RecordToTemplateAuthor::default()
        .draft_from_observations(
            "sample-local-dashboard-settings",
            TemplateWorkflowKind::LocalDashboard,
            vec![RecordedWorkflowObservation {
                evidence_id: "sample-dashboard-rendered-state".to_string(),
                schema_version: "automation.web.rendered_state.v1".to_string(),
                artifact_type: "evidence_manifest".to_string(),
                action_kind: "navigate".to_string(),
                stable_target_id: "settings.general".to_string(),
                raw_label: None,
                confidence: 1.0,
            }],
        )
        .expect("built-in dashboard sample must be valid")
}

pub fn sample_os_neutral_preset_workflow() -> TemplateDraft {
    RecordToTemplateAuthor::default()
        .draft_from_observations(
            "sample-os-neutral-copy",
            TemplateWorkflowKind::OsNeutralPreset,
            vec![RecordedWorkflowObservation {
                evidence_id: "sample-preset-hotkey".to_string(),
                schema_version: "automation.gui.event.v1".to_string(),
                artifact_type: "gui_session_event".to_string(),
                action_kind: "hotkey.copy".to_string(),
                stable_target_id: "keyboard.copy".to_string(),
                raw_label: None,
                confidence: 1.0,
            }],
        )
        .expect("built-in OS-neutral sample must be valid")
}

fn execution_mode_for(workflow_kind: TemplateWorkflowKind) -> TemplateExecutionModeClass {
    match workflow_kind {
        TemplateWorkflowKind::LocalDashboard => TemplateExecutionModeClass::DryRunOnly,
        TemplateWorkflowKind::OsNeutralPreset => TemplateExecutionModeClass::RealInputGated,
    }
}

fn required_permissions_for(workflow_kind: TemplateWorkflowKind) -> Vec<String> {
    match workflow_kind {
        TemplateWorkflowKind::LocalDashboard => vec![
            "local_service_reachability".to_string(),
            "privacy_policy".to_string(),
            "audit".to_string(),
        ],
        TemplateWorkflowKind::OsNeutralPreset => vec![
            "automation_input_control".to_string(),
            "privacy_policy".to_string(),
            "audit".to_string(),
        ],
    }
}

fn validate_safe_identifier(field: &str, value: &str) -> Result<(), String> {
    let is_safe_shape = !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | ':'));
    let lower = value.to_ascii_lowercase();
    let looks_sensitive = value.contains('@')
        || value.contains('/')
        || value.contains('\\')
        || lower.contains("token")
        || lower.contains("secret")
        || lower.contains("password")
        || lower.contains("api_key")
        || lower.contains("apikey");

    if is_safe_shape && !looks_sensitive {
        Ok(())
    } else {
        Err(format!("{field} must be a stable opaque identifier"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn safe_observation() -> RecordedWorkflowObservation {
        RecordedWorkflowObservation {
            evidence_id: "gui-session-001".to_string(),
            schema_version: "automation.gui.event.v1".to_string(),
            artifact_type: "gui_session_event".to_string(),
            action_kind: "click".to_string(),
            stable_target_id: "settings.general".to_string(),
            raw_label: Some("Jane Doe payroll settings".to_string()),
            confidence: 0.91,
        }
    }

    #[test]
    fn draft_omits_raw_labels_and_keeps_only_safe_evidence_refs() {
        let draft = RecordToTemplateAuthor::default()
            .draft_from_observations(
                "template-dashboard-settings",
                TemplateWorkflowKind::LocalDashboard,
                vec![safe_observation()],
            )
            .expect("safe local observation should draft");

        assert_eq!(
            draft.steps[0].redacted_label.as_deref(),
            Some("[omitted:raw-label]")
        );
        assert_eq!(draft.source_evidence_ids, vec!["gui-session-001"]);
        assert!(draft
            .unsafe_findings
            .contains(&"field.raw_label.omitted".to_string()));

        let serialized = serde_json::to_string(&draft).expect("must serialize");
        assert!(!serialized.contains("Jane Doe"));
        assert!(!serialized.contains("payroll"));
    }

    #[test]
    fn draft_rejects_broad_screenshot_artifacts() {
        let mut observation = safe_observation();
        observation.artifact_type = "broad_screenshot".to_string();

        let err = RecordToTemplateAuthor::default()
            .draft_from_observations(
                "template-unsafe",
                TemplateWorkflowKind::LocalDashboard,
                vec![observation],
            )
            .expect_err("broad screenshots must not be accepted into a template draft");

        assert!(err.contains("broad_screenshot"));
    }

    #[test]
    fn draft_rejects_sensitive_values_in_stable_identifier_fields() {
        for (field, value) in [
            ("evidence_id", "alice@example.com"),
            ("schema_version", "file:///Users/alice/schema.json"),
            ("artifact_type", "secret_token_snapshot"),
            ("action_kind", "click/password-reset"),
            ("stable_target_id", "customer/alice@example.com"),
        ] {
            let mut observation = safe_observation();
            match field {
                "evidence_id" => observation.evidence_id = value.to_string(),
                "schema_version" => observation.schema_version = value.to_string(),
                "artifact_type" => observation.artifact_type = value.to_string(),
                "action_kind" => observation.action_kind = value.to_string(),
                "stable_target_id" => observation.stable_target_id = value.to_string(),
                _ => unreachable!(),
            }

            let err = RecordToTemplateAuthor::default()
                .draft_from_observations(
                    "template-unsafe-id",
                    TemplateWorkflowKind::LocalDashboard,
                    vec![observation],
                )
                .expect_err("sensitive stable identifiers must fail closed");

            assert!(
                err.contains(field),
                "{field} should identify the rejected field"
            );
        }
    }

    #[test]
    fn package_requires_policy_audit_confirmation_fixture_and_rollback_metadata() {
        let draft = RecordToTemplateAuthor::default()
            .draft_from_observations(
                "template-dashboard-settings",
                TemplateWorkflowKind::LocalDashboard,
                vec![safe_observation()],
            )
            .expect("safe local observation should draft");

        let package = draft.package_for_review(
            TemplatePublisher {
                publisher_id: "local-user".to_string(),
                source: "recorded-local-session".to_string(),
            },
            TemplateRollbackPolicy {
                expires_after_days: 30,
                rollback_strategy: "disable_template".to_string(),
            },
            TemplateTestFixture {
                fixture_id: "settings-general-rendered-state".to_string(),
                proof_required: TemplateProofRequirement::ObservableStateChange,
            },
        );

        assert_eq!(package.version, "automation.template_package.v1");
        assert_eq!(
            package.review_state,
            TemplateReviewState::RequiresUserReview
        );
        assert_eq!(
            package.evidence_policy,
            "automation.gui.permission_evidence.v1"
        );
        assert!(package.execution_gates.policy_validation_required);
        assert!(package.execution_gates.audit_logging_required);
        assert!(package.execution_gates.user_confirmation_required);
        assert_eq!(
            package.execution_mode,
            TemplateExecutionModeClass::DryRunOnly
        );
        assert_eq!(
            package.test_fixture.proof_required,
            TemplateProofRequirement::ObservableStateChange
        );
        assert_eq!(package.rollback_policy.expires_after_days, 30);
        assert!(!package.accepts_command_acceptance_only_proof());
    }

    #[test]
    fn low_confidence_llm_plan_never_auto_executes_below_floor() {
        let draft = RecordToTemplateAuthor::default()
            .draft_from_observations(
                "template-dashboard-settings",
                TemplateWorkflowKind::LocalDashboard,
                vec![safe_observation()],
            )
            .expect("safe local observation should draft");

        assert!(!draft.passes_llm_confidence_floor(0.3));
        assert!(draft.passes_llm_confidence_floor(0.91));
        assert!(!draft.can_auto_execute());
    }

    #[test]
    fn built_in_examples_cover_dashboard_and_os_neutral_workflows() {
        let dashboard = sample_local_dashboard_workflow();
        let os_neutral = sample_os_neutral_preset_workflow();

        assert_eq!(
            dashboard.workflow_kind,
            TemplateWorkflowKind::LocalDashboard
        );
        assert_eq!(
            os_neutral.workflow_kind,
            TemplateWorkflowKind::OsNeutralPreset
        );
        assert_eq!(
            dashboard.execution_mode,
            TemplateExecutionModeClass::DryRunOnly
        );
        assert_eq!(
            os_neutral.execution_mode,
            TemplateExecutionModeClass::RealInputGated
        );
        assert!(dashboard
            .required_permissions
            .contains(&"local_service_reachability".to_string()));
        assert!(os_neutral
            .required_permissions
            .contains(&"automation_input_control".to_string()));
        assert!(serde_json::to_string(&dashboard)
            .expect("must serialize")
            .contains("automation.gui.ticket.v1"));
    }
}
