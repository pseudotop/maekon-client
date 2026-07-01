use serde::{Deserialize, Serialize};

pub const AUTOMATION_TEMPLATE_PACKAGE_TRUST_SCHEMA_VERSION: &str =
    "automation.template_package_trust.v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AutomationTemplatePackageManifest {
    pub schema_version: String,
    pub package_id: String,
    pub version: String,
    pub publisher: TemplatePackagePublisher,
    pub source: TemplatePackageSource,
    pub signature: TemplatePackageSignature,
    pub required_permission_profile: String,
    pub install_permission: TemplatePackagePermissionGate,
    pub execution_permission: TemplatePackagePermissionGate,
    pub allowed_apps: Vec<String>,
    pub allowed_routes: Vec<String>,
    pub allowed_surfaces: Vec<String>,
    pub evidence_policy: TemplatePackageEvidencePolicy,
    pub rollback: TemplatePackageRollbackPolicy,
    pub dry_run_fixture: TemplatePackageDryRunFixture,
    pub real_input_eligibility: TemplatePackageRealInputEligibility,
    pub external_egress: TemplatePackageExternalEgress,
    pub tests: Vec<TemplatePackageValidationTest>,
}

impl AutomationTemplatePackageManifest {
    pub fn requires_install_approval(&self) -> bool {
        self.install_permission.required
    }

    pub fn requires_execution_approval(&self) -> bool {
        self.execution_permission.required
    }

    pub fn execution_requires_separate_approval(&self) -> bool {
        self.execution_permission.required
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TemplatePackagePublisher {
    pub publisher_id: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TemplatePackageSource {
    pub kind: TemplatePackageSourceKind,
    pub location: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TemplatePackageSourceKind {
    LocalFile,
    CuratedInternalRegistry,
    PublicMarketplace,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TemplatePackageSignature {
    pub status: TemplatePackageSignatureStatus,
    pub key_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TemplatePackageSignatureStatus {
    Unsigned,
    Signed,
    Verified,
    Invalid,
}

impl TemplatePackageSignatureStatus {
    fn as_audit_str(self) -> &'static str {
        match self {
            Self::Unsigned => "unsigned",
            Self::Signed => "signed",
            Self::Verified => "verified",
            Self::Invalid => "invalid",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TemplatePackagePermissionGate {
    pub required: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TemplatePackageEvidencePolicy {
    pub allowed_artifacts: Vec<String>,
    pub allow_raw_labels: bool,
    pub allow_broad_screenshot: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TemplatePackageRollbackPolicy {
    pub expires_after_days: u32,
    pub rollback_strategy: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TemplatePackageDryRunFixture {
    pub fixture_id: String,
    pub available: bool,
    pub requires_real_input: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TemplatePackageRealInputEligibility {
    pub eligible: bool,
    pub requires_execution_approval: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TemplatePackageExternalEgress {
    pub required: bool,
    pub allowed_domains: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TemplatePackageValidationTest {
    pub test_id: String,
    pub dry_run_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TemplatePackageDryRunReport {
    pub package_id: String,
    pub dry_run_ready: bool,
    pub real_input_used: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplatePackageTrustError {
    code: &'static str,
    message: String,
    fail_closed: bool,
}

impl TemplatePackageTrustError {
    fn fail_closed(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            fail_closed: true,
        }
    }

    pub fn is_fail_closed(&self) -> bool {
        self.fail_closed
    }

    pub fn user_visible_message(&self) -> &str {
        &self.message
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TemplatePackageTrustValidator;

impl TemplatePackageTrustValidator {
    pub fn validate_install(
        manifest: &AutomationTemplatePackageManifest,
    ) -> Result<(), TemplatePackageTrustError> {
        if manifest.schema_version != AUTOMATION_TEMPLATE_PACKAGE_TRUST_SCHEMA_VERSION {
            return Err(TemplatePackageTrustError::fail_closed(
                "unsupported_schema_version",
                "Template package schema version is not supported.",
            ));
        }

        if manifest.source.kind == TemplatePackageSourceKind::PublicMarketplace {
            return Err(TemplatePackageTrustError::fail_closed(
                "public_marketplace_out_of_scope",
                "Template package public marketplace installs are out of scope.",
            ));
        }

        if manifest.signature.status != TemplatePackageSignatureStatus::Verified {
            return Err(TemplatePackageTrustError::fail_closed(
                "unverified_signature",
                "Template package signature must be verified before install.",
            ));
        }

        if manifest.evidence_policy.allow_raw_labels
            || manifest.evidence_policy.allow_broad_screenshot
            || manifest
                .evidence_policy
                .allowed_artifacts
                .iter()
                .any(|artifact| artifact == "broad_screenshot")
        {
            return Err(TemplatePackageTrustError::fail_closed(
                "unsafe_evidence_policy",
                "Template package evidence policy requests unsafe raw evidence.",
            ));
        }

        if manifest.external_egress.required && manifest.external_egress.allowed_domains.is_empty()
        {
            return Err(TemplatePackageTrustError::fail_closed(
                "egress_without_allowlist",
                "Template package external egress requires an allowlist.",
            ));
        }

        Ok(())
    }

    pub fn validate_dry_run(
        manifest: &AutomationTemplatePackageManifest,
    ) -> Result<TemplatePackageDryRunReport, TemplatePackageTrustError> {
        Self::validate_install(manifest)?;

        if !manifest.dry_run_fixture.available || manifest.dry_run_fixture.requires_real_input {
            return Err(TemplatePackageTrustError::fail_closed(
                "dry_run_fixture_unavailable",
                "Template package dry-run fixture is unavailable without real input.",
            ));
        }

        if manifest.tests.iter().any(|test| !test.dry_run_only) {
            return Err(TemplatePackageTrustError::fail_closed(
                "non_dry_run_test",
                "Template package validation tests must be dry-run-only at install time.",
            ));
        }

        Ok(TemplatePackageDryRunReport {
            package_id: manifest.package_id.clone(),
            dry_run_ready: true,
            real_input_used: false,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TemplatePackageAuditEntry {
    pub package_id: String,
    pub package_version: String,
    pub run_id: String,
    pub signature_status: TemplatePackageSignatureStatus,
    pub install_approval_required: bool,
    pub execution_approval_required: bool,
    pub external_egress_required: bool,
}

impl TemplatePackageAuditEntry {
    pub fn execution_started(
        manifest: &AutomationTemplatePackageManifest,
        run_id: impl Into<String>,
    ) -> Self {
        Self {
            package_id: manifest.package_id.clone(),
            package_version: manifest.version.clone(),
            run_id: run_id.into(),
            signature_status: manifest.signature.status,
            install_approval_required: manifest.install_permission.required,
            execution_approval_required: manifest.execution_permission.required,
            external_egress_required: manifest.external_egress.required,
        }
    }

    pub fn to_audit_details(&self) -> String {
        format!(
            "package_id={} package_version={} run_id={} signature_status={} install_approval_required={} execution_approval_required={} external_egress_required={}",
            self.package_id,
            self.package_version,
            self.run_id,
            self.signature_status.as_audit_str(),
            self.install_approval_required,
            self.execution_approval_required,
            self.external_egress_required,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn safe_local_manifest() -> AutomationTemplatePackageManifest {
        AutomationTemplatePackageManifest {
            schema_version: AUTOMATION_TEMPLATE_PACKAGE_TRUST_SCHEMA_VERSION.to_string(),
            package_id: "pkg.local.settings-review".to_string(),
            version: "0.1.0".to_string(),
            publisher: TemplatePackagePublisher {
                publisher_id: "maekon-internal".to_string(),
                display_name: "Maekon Internal".to_string(),
            },
            source: TemplatePackageSource {
                kind: TemplatePackageSourceKind::LocalFile,
                location: "file:///templates/settings-review.json".to_string(),
            },
            signature: TemplatePackageSignature {
                status: TemplatePackageSignatureStatus::Verified,
                key_id: Some("maekon-local-test-key".to_string()),
            },
            required_permission_profile: "standard".to_string(),
            install_permission: TemplatePackagePermissionGate {
                required: true,
                reason: "install adds reusable workflow metadata".to_string(),
            },
            execution_permission: TemplatePackagePermissionGate {
                required: true,
                reason: "execution can inject real input".to_string(),
            },
            allowed_apps: vec!["maekon".to_string()],
            allowed_routes: vec!["/settings/general".to_string()],
            allowed_surfaces: vec!["local_dashboard".to_string()],
            evidence_policy: TemplatePackageEvidencePolicy {
                allowed_artifacts: vec![
                    "redacted_dom_snapshot".to_string(),
                    "evidence_manifest".to_string(),
                ],
                allow_raw_labels: false,
                allow_broad_screenshot: false,
            },
            rollback: TemplatePackageRollbackPolicy {
                expires_after_days: 30,
                rollback_strategy: "disable_package".to_string(),
            },
            dry_run_fixture: TemplatePackageDryRunFixture {
                fixture_id: "fixture.settings.review".to_string(),
                available: true,
                requires_real_input: false,
            },
            real_input_eligibility: TemplatePackageRealInputEligibility {
                eligible: true,
                requires_execution_approval: true,
            },
            external_egress: TemplatePackageExternalEgress {
                required: false,
                allowed_domains: Vec::new(),
            },
            tests: vec![TemplatePackageValidationTest {
                test_id: "dry-run-settings-review".to_string(),
                dry_run_only: true,
            }],
        }
    }

    #[test]
    fn manifest_distinguishes_install_permission_from_execution_permission() {
        let mut manifest = safe_local_manifest();
        manifest.install_permission.required = false;
        manifest.execution_permission.required = true;

        assert!(!manifest.requires_install_approval());
        assert!(manifest.requires_execution_approval());
        assert!(manifest.execution_requires_separate_approval());
    }

    #[test]
    fn broad_screenshot_or_raw_label_policy_fails_closed() {
        let mut manifest = safe_local_manifest();
        manifest
            .evidence_policy
            .allowed_artifacts
            .push("broad_screenshot".to_string());
        manifest.evidence_policy.allow_raw_labels = true;

        let err = TemplatePackageTrustValidator::validate_install(&manifest)
            .expect_err("unsafe evidence policy must fail closed");

        assert!(err.is_fail_closed());
        assert!(err.user_visible_message().contains("evidence"));
    }

    #[test]
    fn execution_audit_entry_contains_package_identity_and_signature_status() {
        let manifest = safe_local_manifest();

        let audit =
            TemplatePackageAuditEntry::execution_started(&manifest, "run-123").to_audit_details();

        assert!(audit.contains("package_id=pkg.local.settings-review"));
        assert!(audit.contains("package_version=0.1.0"));
        assert!(audit.contains("signature_status=verified"));
        assert!(!audit.contains("maekon-local-test-key"));
    }

    #[test]
    fn dry_run_validation_does_not_require_real_input() {
        let manifest = safe_local_manifest();

        let report = TemplatePackageTrustValidator::validate_dry_run(&manifest)
            .expect("dry-run fixture must validate without real input");

        assert_eq!(report.package_id, manifest.package_id);
        assert!(report.dry_run_ready);
        assert!(!report.real_input_used);
    }

    #[test]
    fn public_marketplace_source_is_out_of_scope_and_fail_closed() {
        let mut manifest = safe_local_manifest();
        manifest.source.kind = TemplatePackageSourceKind::PublicMarketplace;
        manifest.source.location = "https://marketplace.example.invalid/pkg".to_string();

        let err = TemplatePackageTrustValidator::validate_install(&manifest)
            .expect_err("public marketplace must be out of scope");

        assert!(err.is_fail_closed());
        assert!(err.user_visible_message().contains("public marketplace"));
    }

    #[test]
    fn unsigned_or_unverified_signature_fails_closed() {
        for status in [
            TemplatePackageSignatureStatus::Unsigned,
            TemplatePackageSignatureStatus::Signed,
            TemplatePackageSignatureStatus::Invalid,
        ] {
            let mut manifest = safe_local_manifest();
            manifest.signature.status = status;

            let err = TemplatePackageTrustValidator::validate_install(&manifest)
                .expect_err("unverified signature states must fail closed");

            assert!(err.is_fail_closed());
            assert!(err.user_visible_message().contains("verified"));
        }
    }
}
