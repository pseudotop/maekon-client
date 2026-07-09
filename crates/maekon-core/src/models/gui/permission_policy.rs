//! GUI permission remediation + evidence-artifact policy models — the
//! catalog tying each OS permission to its per-platform remediation action,
//! and the evidence-artifact sharing/redaction policy plus review workflow.
//!
//! Split from `models/gui.rs` (issue #7721 F4). Pure move — no behavior change.

use serde::{Deserialize, Serialize};

use super::readiness::GuiReadinessPlatform;

pub const GUI_PERMISSION_EVIDENCE_POLICY_SCHEMA_VERSION: &str =
    "automation.gui.permission_evidence.v1";

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gui_permission_evidence_policy_fixture_covers_required_rules() {
        let policy: GuiPermissionEvidencePolicy = serde_json::from_str(include_str!(
            "../../../../../docs/contracts/gui-permission-evidence-policy.v1.json"
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
            "../../../../../docs/contracts/gui-permission-evidence-policy.v1.json"
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
}
