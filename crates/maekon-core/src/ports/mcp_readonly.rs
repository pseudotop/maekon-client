use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpTransportChoice {
    StdioLocalOnly,
    StreamableHttpDeferred,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpCapabilityKind {
    Resource,
    ReadOnlyTool,
    WriteTool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpCapability {
    pub name: String,
    pub kind: McpCapabilityKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpFieldPrivacyClass {
    SummaryOrStableId,
    RawDesktopContent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpFieldMapping {
    pub field: String,
    pub source_contract: Option<String>,
    pub proposed_contract: Option<String>,
    pub privacy_class: McpFieldPrivacyClass,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpReadOnlyResource {
    pub resource_id: String,
    pub stable_ids_only: bool,
    pub fields: Vec<McpFieldMapping>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadOnlyMcpSurfaceSpec {
    pub transport: McpTransportChoice,
    pub streamable_http_deferred_reason: Option<String>,
    pub local_auth_required: bool,
    pub consent_required: bool,
    pub consent_scope: String,
    pub capabilities: Vec<McpCapability>,
    pub resources: Vec<McpReadOnlyResource>,
}

impl ReadOnlyMcpSurfaceSpec {
    pub fn first_implementation() -> Self {
        let resource_ids = [
            "maekon.timeline.summary",
            "maekon.readiness.snapshot",
            "maekon.audit.summary",
            "maekon.automation.pending_approvals",
            "maekon.evidence.manifest",
            "maekon.release_decision.status",
        ];

        Self {
            transport: McpTransportChoice::StdioLocalOnly,
            streamable_http_deferred_reason: Some(
                "Streamable HTTP waits for local auth, consent, and raw-data gates.".to_string(),
            ),
            local_auth_required: true,
            consent_required: true,
            consent_scope: "external_agent_context_read".to_string(),
            capabilities: resource_ids
                .iter()
                .map(|resource_id| McpCapability {
                    name: (*resource_id).to_string(),
                    kind: McpCapabilityKind::Resource,
                })
                .collect(),
            resources: vec![
                mcp_resource(
                    "maekon.timeline.summary",
                    "maekon.timeline.summary.v1",
                    true,
                    &["summary_window_id", "segment_ids", "activity_counts"],
                ),
                mcp_resource(
                    "maekon.readiness.snapshot",
                    "automation.gui.readiness.v1",
                    false,
                    &["readiness_state", "capability_codes", "diagnostic_codes"],
                ),
                mcp_resource(
                    "maekon.audit.summary",
                    "maekon.audit.summary.v1",
                    true,
                    &["audit_window_id", "status_counts", "policy_event_codes"],
                ),
                mcp_resource(
                    "maekon.automation.pending_approvals",
                    "automation.remote.observe_approve.v1",
                    false,
                    &["request_ids", "matched_policy_ids", "expiry_states"],
                ),
                mcp_resource(
                    "maekon.evidence.manifest",
                    "automation.gui.permission_evidence.v1",
                    false,
                    &["artifact_ids", "artifact_kinds", "redaction_status"],
                ),
                mcp_resource(
                    "maekon.release_decision.status",
                    "automation.gui.benchmark_report.v1.release_decision",
                    false,
                    &["release_state", "covered_issue_ids", "evidence_ids"],
                ),
            ],
        }
    }

    pub fn allows_action_execution(&self) -> bool {
        self.capabilities
            .iter()
            .any(|capability| capability.kind == McpCapabilityKind::WriteTool)
    }

    pub fn exposed_resource_ids(&self) -> Vec<&str> {
        self.resources
            .iter()
            .map(|resource| resource.resource_id.as_str())
            .collect()
    }

    pub fn validate_policy(&self) -> Result<(), McpReadOnlyPolicyError> {
        if self.transport != McpTransportChoice::StdioLocalOnly {
            return Err(McpReadOnlyPolicyError::fail_closed(
                "transport_not_allowed",
                "read-only MCP first implementation must stay stdio local-only.",
            ));
        }

        if !self.local_auth_required || !self.consent_required || self.consent_scope.is_empty() {
            return Err(McpReadOnlyPolicyError::fail_closed(
                "missing_auth_or_consent",
                "read-only MCP surface requires explicit local auth and consent gates.",
            ));
        }

        if self.allows_action_execution() {
            return Err(McpReadOnlyPolicyError::fail_closed(
                "write_tool_not_allowed",
                "read-only MCP surface cannot expose write tools or action execution.",
            ));
        }

        for resource in &self.resources {
            if !resource.stable_ids_only {
                return Err(McpReadOnlyPolicyError::fail_closed(
                    "unstable_resource",
                    "read-only MCP resources must expose summaries and stable ids only.",
                ));
            }

            for field in &resource.fields {
                if field.source_contract.is_none() && field.proposed_contract.is_none() {
                    return Err(McpReadOnlyPolicyError::fail_closed(
                        "unmapped_field",
                        "read-only MCP fields must map to existing or proposed sanitized contracts.",
                    ));
                }

                if field.privacy_class == McpFieldPrivacyClass::RawDesktopContent
                    || contains_forbidden_raw_field_name(&field.field)
                {
                    return Err(McpReadOnlyPolicyError::fail_closed(
                        "raw_content_not_allowed",
                        "read-only MCP surface cannot expose raw desktop content.",
                    ));
                }
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpReadOnlyPolicyError {
    code: &'static str,
    message: String,
    fail_closed: bool,
}

impl McpReadOnlyPolicyError {
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

fn mcp_resource(
    resource_id: &str,
    contract: &str,
    proposed: bool,
    fields: &[&str],
) -> McpReadOnlyResource {
    McpReadOnlyResource {
        resource_id: resource_id.to_string(),
        stable_ids_only: true,
        fields: fields
            .iter()
            .map(|field| McpFieldMapping {
                field: (*field).to_string(),
                source_contract: (!proposed).then(|| contract.to_string()),
                proposed_contract: proposed.then(|| contract.to_string()),
                privacy_class: McpFieldPrivacyClass::SummaryOrStableId,
            })
            .collect(),
    }
}

fn contains_forbidden_raw_field_name(field: &str) -> bool {
    let normalized = field.to_ascii_lowercase();
    [
        "raw",
        "screenshot",
        "ocr_text",
        "window_title",
        "typed_text",
        "file_path",
        "label",
    ]
    .iter()
    .any(|forbidden| normalized.contains(forbidden))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_implementation_chooses_stdio_local_only() {
        let spec = ReadOnlyMcpSurfaceSpec::first_implementation();

        assert_eq!(spec.transport, McpTransportChoice::StdioLocalOnly);
        assert!(spec.streamable_http_deferred_reason.is_some());
        assert!(!spec.allows_action_execution());
    }

    #[test]
    fn every_resource_maps_to_contract_and_stable_summary_fields() {
        let spec = ReadOnlyMcpSurfaceSpec::first_implementation();

        for resource in &spec.resources {
            assert!(
                resource.stable_ids_only,
                "{} must expose stable ids and summaries only",
                resource.resource_id
            );
            assert!(
                resource
                    .fields
                    .iter()
                    .all(|field| field.source_contract.is_some()
                        || field.proposed_contract.is_some()),
                "{} has unmapped fields",
                resource.resource_id
            );
            assert!(
                resource
                    .fields
                    .iter()
                    .all(|field| field.privacy_class == McpFieldPrivacyClass::SummaryOrStableId),
                "{} must not expose raw desktop content",
                resource.resource_id
            );
        }
    }

    #[test]
    fn policy_rejects_write_tools_action_execution_and_raw_content() {
        let mut spec = ReadOnlyMcpSurfaceSpec::first_implementation();
        spec.capabilities.push(McpCapability {
            name: "maekon.automation.execute".to_string(),
            kind: McpCapabilityKind::WriteTool,
        });
        spec.resources[0].fields.push(McpFieldMapping {
            field: "raw_window_title".to_string(),
            source_contract: None,
            proposed_contract: Some("proposed.raw.debug".to_string()),
            privacy_class: McpFieldPrivacyClass::RawDesktopContent,
        });

        let err = spec
            .validate_policy()
            .expect_err("write tools and raw content must fail closed");

        assert!(err.is_fail_closed());
        assert!(err.user_visible_message().contains("read-only"));
    }

    #[test]
    fn policy_rejects_streamable_http_until_auth_and_raw_data_gates_exist() {
        let mut spec = ReadOnlyMcpSurfaceSpec::first_implementation();
        spec.transport = McpTransportChoice::StreamableHttpDeferred;

        let err = spec
            .validate_policy()
            .expect_err("streamable HTTP must remain deferred for the first implementation");

        assert!(err.is_fail_closed());
        assert!(err.user_visible_message().contains("stdio"));
    }

    #[test]
    fn local_auth_and_consent_requirements_are_explicit() {
        let spec = ReadOnlyMcpSurfaceSpec::first_implementation();

        assert!(spec.local_auth_required);
        assert!(spec.consent_required);
        assert_eq!(spec.consent_scope, "external_agent_context_read");
        spec.validate_policy()
            .expect("read-only MCP surface policy should be valid");
    }

    #[test]
    fn exposes_only_expected_read_only_resource_ids() {
        let spec = ReadOnlyMcpSurfaceSpec::first_implementation();
        let mut ids = spec.exposed_resource_ids();
        ids.sort_unstable();

        assert_eq!(
            ids,
            vec![
                "maekon.audit.summary",
                "maekon.automation.pending_approvals",
                "maekon.evidence.manifest",
                "maekon.readiness.snapshot",
                "maekon.release_decision.status",
                "maekon.timeline.summary",
            ]
        );
    }
}
