//! Privacy-safe AI capability readiness contract (#11735).
//!
//! This module deliberately accepts normalized booleans and enums rather than
//! provider configuration objects. Consequently a serialized readiness
//! snapshot cannot contain prompts, captured text, credentials, account IDs,
//! endpoints, model paths, or any other user-controlled string.

use serde::{Deserialize, Serialize};

use crate::config::AiAccessMode;

pub const AI_READINESS_CONTRACT_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AiCapabilityId {
    #[serde(rename = "chat.subprocess")]
    ChatSubprocess,
    #[serde(rename = "chat.http_api")]
    ChatHttpApi,
    #[serde(rename = "chat.local_llm")]
    ChatLocalLlm,
    #[serde(rename = "ocr.capture")]
    OcrCapture,
    #[serde(rename = "ocr.suggestion_analysis")]
    OcrSuggestionAnalysis,
    #[serde(rename = "segment_summary")]
    SegmentSummary,
    #[serde(rename = "daily_narrative")]
    DailyNarrative,
}

impl AiCapabilityId {
    pub const ALL: [Self; 7] = [
        Self::ChatSubprocess,
        Self::ChatHttpApi,
        Self::ChatLocalLlm,
        Self::OcrCapture,
        Self::OcrSuggestionAnalysis,
        Self::SegmentSummary,
        Self::DailyNarrative,
    ];

    pub const fn wire_id(self) -> &'static str {
        match self {
            Self::ChatSubprocess => "chat.subprocess",
            Self::ChatHttpApi => "chat.http_api",
            Self::ChatLocalLlm => "chat.local_llm",
            Self::OcrCapture => "ocr.capture",
            Self::OcrSuggestionAnalysis => "ocr.suggestion_analysis",
            Self::SegmentSummary => "segment_summary",
            Self::DailyNarrative => "daily_narrative",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiReadinessStatus {
    Ready,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiReadinessReasonCode {
    Ready,
    CompiledCapabilityMissing,
    RuntimeFlagDisabled,
    ConsentRequired,
    AccessModeMismatch,
    EndpointOrProfileRequired,
    ProviderNotDetected,
    ProviderAuthRequired,
    ProviderAuthUnverified,
    ProviderInvocationUnavailable,
    ProviderInvocationUnverified,
    ModelUnavailable,
    ModelAvailabilityUnverified,
    HotRewireRequired,
    RestartRequired,
    PrivacyGateUnavailable,
    EgressGateUnavailable,
    BudgetGateUnavailable,
    AuditGateUnavailable,
}

impl AiReadinessReasonCode {
    pub const fn wire_code(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::CompiledCapabilityMissing => "compiled_capability_missing",
            Self::RuntimeFlagDisabled => "runtime_flag_disabled",
            Self::ConsentRequired => "consent_required",
            Self::AccessModeMismatch => "access_mode_mismatch",
            Self::EndpointOrProfileRequired => "endpoint_or_profile_required",
            Self::ProviderNotDetected => "provider_not_detected",
            Self::ProviderAuthRequired => "provider_auth_required",
            Self::ProviderAuthUnverified => "provider_auth_unverified",
            Self::ProviderInvocationUnavailable => "provider_invocation_unavailable",
            Self::ProviderInvocationUnverified => "provider_invocation_unverified",
            Self::ModelUnavailable => "model_unavailable",
            Self::ModelAvailabilityUnverified => "model_availability_unverified",
            Self::HotRewireRequired => "hot_rewire_required",
            Self::RestartRequired => "restart_required",
            Self::PrivacyGateUnavailable => "privacy_gate_unavailable",
            Self::EgressGateUnavailable => "egress_gate_unavailable",
            Self::BudgetGateUnavailable => "budget_gate_unavailable",
            Self::AuditGateUnavailable => "audit_gate_unavailable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiReadinessAction {
    None,
    OpenAiSettings,
    OpenPrivacyConsent,
    EnableFeature,
    InstallProvider,
    AuthenticateProvider,
    VerifyProviderInvocation,
    SelectModel,
    ApplyHotRewire,
    RestartApp,
    ReviewPrivacy,
    ReviewEgress,
    ReviewBudget,
    ReviewAudit,
}

impl AiReadinessAction {
    pub const fn copy_key(self) -> &'static str {
        match self {
            Self::None => "aiReadiness.action.none",
            Self::OpenAiSettings => "aiReadiness.action.openAiSettings",
            Self::OpenPrivacyConsent => "aiReadiness.action.openPrivacyConsent",
            Self::EnableFeature => "aiReadiness.action.enableFeature",
            Self::InstallProvider => "aiReadiness.action.installProvider",
            Self::AuthenticateProvider => "aiReadiness.action.authenticateProvider",
            Self::VerifyProviderInvocation => "aiReadiness.action.verifyProviderInvocation",
            Self::SelectModel => "aiReadiness.action.selectModel",
            Self::ApplyHotRewire => "aiReadiness.action.applyHotRewire",
            Self::RestartApp => "aiReadiness.action.restartApp",
            Self::ReviewPrivacy => "aiReadiness.action.reviewPrivacy",
            Self::ReviewEgress => "aiReadiness.action.reviewEgress",
            Self::ReviewBudget => "aiReadiness.action.reviewBudget",
            Self::ReviewAudit => "aiReadiness.action.reviewAudit",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiProviderDetection {
    NotRequired,
    NotDetected,
    Detected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiProviderAuthReadiness {
    NotRequired,
    Required,
    Unverified,
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiProviderInvocationReadiness {
    NotRequired,
    Unavailable,
    Unverified,
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiModelAvailability {
    NotRequired,
    Unavailable,
    Unverified,
    Available,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiRuntimeApplyRequirement {
    RuntimeApplied,
    HotRewire,
    Restart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiConsentField {
    OcrProcessing,
    ActivityPatternLearning,
    FullTextExtraction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiConsentReadiness {
    pub field: AiConsentField,
    pub granted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiInvocationGuardState {
    /// The readiness contract does not grant this authority. The real request
    /// path must still evaluate the corresponding gate for every invocation.
    EnforcedAtInvocation,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiReadinessDimensions {
    pub compiled_capability: bool,
    pub selected_access_mode: AiAccessMode,
    pub access_mode_compatible: bool,
    pub endpoint_or_profile_configured: bool,
    pub provider_detection: AiProviderDetection,
    pub provider_auth: AiProviderAuthReadiness,
    pub provider_invocation: AiProviderInvocationReadiness,
    pub model_availability: AiModelAvailability,
    pub runtime_flag_enabled: bool,
    pub consent: Vec<AiConsentReadiness>,
    pub apply_requirement: AiRuntimeApplyRequirement,
    pub apply_pending: bool,
    pub privacy_gate: AiInvocationGuardState,
    pub egress_gate: AiInvocationGuardState,
    pub budget_gate: AiInvocationGuardState,
    pub audit_gate: AiInvocationGuardState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiCapabilityReadiness {
    pub capability_id: AiCapabilityId,
    pub status: AiReadinessStatus,
    pub reason_code: AiReadinessReasonCode,
    pub action: AiReadinessAction,
    pub action_copy_key: String,
    pub dimensions: AiReadinessDimensions,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiReadinessSnapshot {
    pub contract_version: u16,
    pub capabilities: Vec<AiCapabilityReadiness>,
}

impl AiReadinessSnapshot {
    pub fn new(capabilities: Vec<AiCapabilityReadiness>) -> Self {
        Self {
            contract_version: AI_READINESS_CONTRACT_VERSION,
            capabilities,
        }
    }

    pub fn find(&self, capability_id: AiCapabilityId) -> Option<&AiCapabilityReadiness> {
        self.capabilities
            .iter()
            .find(|item| item.capability_id == capability_id)
    }
}

pub fn evaluate_ai_readiness(
    capability_id: AiCapabilityId,
    dimensions: AiReadinessDimensions,
) -> AiCapabilityReadiness {
    let reason_code = primary_blocker(&dimensions);
    let status = if reason_code == AiReadinessReasonCode::Ready {
        AiReadinessStatus::Ready
    } else {
        AiReadinessStatus::Blocked
    };
    let action = action_for_reason(reason_code);

    AiCapabilityReadiness {
        capability_id,
        status,
        reason_code,
        action,
        action_copy_key: action.copy_key().to_string(),
        dimensions,
    }
}

fn primary_blocker(dimensions: &AiReadinessDimensions) -> AiReadinessReasonCode {
    if !dimensions.compiled_capability {
        return AiReadinessReasonCode::CompiledCapabilityMissing;
    }
    if !dimensions.runtime_flag_enabled {
        return AiReadinessReasonCode::RuntimeFlagDisabled;
    }
    if dimensions.consent.iter().any(|consent| !consent.granted) {
        return AiReadinessReasonCode::ConsentRequired;
    }
    // Mode mismatch intentionally precedes endpoint/profile absence. A ready
    // CLI selected under provider_api_key is actionable as a mode mismatch,
    // not a misleading generic empty-configuration state (#11735 AC5).
    if !dimensions.access_mode_compatible {
        return AiReadinessReasonCode::AccessModeMismatch;
    }
    if !dimensions.endpoint_or_profile_configured {
        return AiReadinessReasonCode::EndpointOrProfileRequired;
    }
    if dimensions.provider_detection == AiProviderDetection::NotDetected {
        return AiReadinessReasonCode::ProviderNotDetected;
    }
    match dimensions.provider_auth {
        AiProviderAuthReadiness::Required => return AiReadinessReasonCode::ProviderAuthRequired,
        AiProviderAuthReadiness::Unverified => {
            return AiReadinessReasonCode::ProviderAuthUnverified;
        }
        AiProviderAuthReadiness::NotRequired | AiProviderAuthReadiness::Ready => {}
    }
    match dimensions.provider_invocation {
        AiProviderInvocationReadiness::Unavailable => {
            return AiReadinessReasonCode::ProviderInvocationUnavailable;
        }
        AiProviderInvocationReadiness::Unverified => {
            return AiReadinessReasonCode::ProviderInvocationUnverified;
        }
        AiProviderInvocationReadiness::NotRequired | AiProviderInvocationReadiness::Ready => {}
    }
    match dimensions.model_availability {
        AiModelAvailability::Unavailable => return AiReadinessReasonCode::ModelUnavailable,
        AiModelAvailability::Unverified => {
            return AiReadinessReasonCode::ModelAvailabilityUnverified;
        }
        AiModelAvailability::NotRequired | AiModelAvailability::Available => {}
    }
    if dimensions.apply_pending {
        return match dimensions.apply_requirement {
            AiRuntimeApplyRequirement::RuntimeApplied => AiReadinessReasonCode::HotRewireRequired,
            AiRuntimeApplyRequirement::HotRewire => AiReadinessReasonCode::HotRewireRequired,
            AiRuntimeApplyRequirement::Restart => AiReadinessReasonCode::RestartRequired,
        };
    }
    for (state, reason) in [
        (
            dimensions.privacy_gate,
            AiReadinessReasonCode::PrivacyGateUnavailable,
        ),
        (
            dimensions.egress_gate,
            AiReadinessReasonCode::EgressGateUnavailable,
        ),
        (
            dimensions.budget_gate,
            AiReadinessReasonCode::BudgetGateUnavailable,
        ),
        (
            dimensions.audit_gate,
            AiReadinessReasonCode::AuditGateUnavailable,
        ),
    ] {
        if state == AiInvocationGuardState::Unavailable {
            return reason;
        }
    }
    AiReadinessReasonCode::Ready
}

const fn action_for_reason(reason: AiReadinessReasonCode) -> AiReadinessAction {
    match reason {
        AiReadinessReasonCode::Ready => AiReadinessAction::None,
        // A compile-time omission cannot be repaired by mutating runtime
        // settings. Keep the reason explicit without offering a dead action.
        AiReadinessReasonCode::CompiledCapabilityMissing => AiReadinessAction::None,
        AiReadinessReasonCode::RuntimeFlagDisabled => AiReadinessAction::EnableFeature,
        AiReadinessReasonCode::ConsentRequired => AiReadinessAction::OpenPrivacyConsent,
        AiReadinessReasonCode::AccessModeMismatch
        | AiReadinessReasonCode::EndpointOrProfileRequired => AiReadinessAction::OpenAiSettings,
        AiReadinessReasonCode::ProviderNotDetected => AiReadinessAction::InstallProvider,
        AiReadinessReasonCode::ProviderAuthRequired
        | AiReadinessReasonCode::ProviderAuthUnverified => AiReadinessAction::AuthenticateProvider,
        AiReadinessReasonCode::ProviderInvocationUnavailable
        | AiReadinessReasonCode::ProviderInvocationUnverified => {
            AiReadinessAction::VerifyProviderInvocation
        }
        AiReadinessReasonCode::ModelUnavailable
        | AiReadinessReasonCode::ModelAvailabilityUnverified => AiReadinessAction::SelectModel,
        AiReadinessReasonCode::HotRewireRequired => AiReadinessAction::ApplyHotRewire,
        AiReadinessReasonCode::RestartRequired => AiReadinessAction::RestartApp,
        AiReadinessReasonCode::PrivacyGateUnavailable => AiReadinessAction::ReviewPrivacy,
        AiReadinessReasonCode::EgressGateUnavailable => AiReadinessAction::ReviewEgress,
        AiReadinessReasonCode::BudgetGateUnavailable => AiReadinessAction::ReviewBudget,
        AiReadinessReasonCode::AuditGateUnavailable => AiReadinessAction::ReviewAudit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready_dimensions() -> AiReadinessDimensions {
        AiReadinessDimensions {
            compiled_capability: true,
            selected_access_mode: AiAccessMode::ProviderSubscriptionCli,
            access_mode_compatible: true,
            endpoint_or_profile_configured: true,
            provider_detection: AiProviderDetection::Detected,
            provider_auth: AiProviderAuthReadiness::Ready,
            provider_invocation: AiProviderInvocationReadiness::Ready,
            model_availability: AiModelAvailability::NotRequired,
            runtime_flag_enabled: true,
            consent: vec![AiConsentReadiness {
                field: AiConsentField::OcrProcessing,
                granted: true,
            }],
            apply_requirement: AiRuntimeApplyRequirement::RuntimeApplied,
            apply_pending: false,
            privacy_gate: AiInvocationGuardState::EnforcedAtInvocation,
            egress_gate: AiInvocationGuardState::EnforcedAtInvocation,
            budget_gate: AiInvocationGuardState::EnforcedAtInvocation,
            audit_gate: AiInvocationGuardState::EnforcedAtInvocation,
        }
    }

    #[test]
    fn positive_control_is_ready_without_granting_invocation_authority() {
        let result =
            evaluate_ai_readiness(AiCapabilityId::OcrSuggestionAnalysis, ready_dimensions());
        assert_eq!(result.status, AiReadinessStatus::Ready);
        assert_eq!(result.reason_code, AiReadinessReasonCode::Ready);
        assert_eq!(
            result.dimensions.egress_gate,
            AiInvocationGuardState::EnforcedAtInvocation
        );
    }

    #[test]
    fn every_readiness_dimension_has_a_red_mutation_control() {
        struct ReadinessMutation {
            expected: AiReadinessReasonCode,
            mutate: fn(&mut AiReadinessDimensions),
        }

        let mutations = [
            ReadinessMutation {
                expected: AiReadinessReasonCode::CompiledCapabilityMissing,
                mutate: |d| d.compiled_capability = false,
            },
            ReadinessMutation {
                expected: AiReadinessReasonCode::RuntimeFlagDisabled,
                mutate: |d| d.runtime_flag_enabled = false,
            },
            ReadinessMutation {
                expected: AiReadinessReasonCode::ConsentRequired,
                mutate: |d| d.consent[0].granted = false,
            },
            ReadinessMutation {
                expected: AiReadinessReasonCode::AccessModeMismatch,
                mutate: |d| d.access_mode_compatible = false,
            },
            ReadinessMutation {
                expected: AiReadinessReasonCode::EndpointOrProfileRequired,
                mutate: |d| d.endpoint_or_profile_configured = false,
            },
            ReadinessMutation {
                expected: AiReadinessReasonCode::ProviderNotDetected,
                mutate: |d| d.provider_detection = AiProviderDetection::NotDetected,
            },
            ReadinessMutation {
                expected: AiReadinessReasonCode::ProviderAuthRequired,
                mutate: |d| d.provider_auth = AiProviderAuthReadiness::Required,
            },
            ReadinessMutation {
                expected: AiReadinessReasonCode::ProviderAuthUnverified,
                mutate: |d| d.provider_auth = AiProviderAuthReadiness::Unverified,
            },
            ReadinessMutation {
                expected: AiReadinessReasonCode::ProviderInvocationUnavailable,
                mutate: |d| d.provider_invocation = AiProviderInvocationReadiness::Unavailable,
            },
            ReadinessMutation {
                expected: AiReadinessReasonCode::ProviderInvocationUnverified,
                mutate: |d| d.provider_invocation = AiProviderInvocationReadiness::Unverified,
            },
            ReadinessMutation {
                expected: AiReadinessReasonCode::ModelUnavailable,
                mutate: |d| d.model_availability = AiModelAvailability::Unavailable,
            },
            ReadinessMutation {
                expected: AiReadinessReasonCode::ModelAvailabilityUnverified,
                mutate: |d| d.model_availability = AiModelAvailability::Unverified,
            },
            ReadinessMutation {
                expected: AiReadinessReasonCode::HotRewireRequired,
                mutate: |d| {
                    d.apply_requirement = AiRuntimeApplyRequirement::HotRewire;
                    d.apply_pending = true;
                },
            },
            ReadinessMutation {
                expected: AiReadinessReasonCode::RestartRequired,
                mutate: |d| {
                    d.apply_requirement = AiRuntimeApplyRequirement::Restart;
                    d.apply_pending = true;
                },
            },
            ReadinessMutation {
                expected: AiReadinessReasonCode::PrivacyGateUnavailable,
                mutate: |d| d.privacy_gate = AiInvocationGuardState::Unavailable,
            },
            ReadinessMutation {
                expected: AiReadinessReasonCode::EgressGateUnavailable,
                mutate: |d| d.egress_gate = AiInvocationGuardState::Unavailable,
            },
            ReadinessMutation {
                expected: AiReadinessReasonCode::BudgetGateUnavailable,
                mutate: |d| d.budget_gate = AiInvocationGuardState::Unavailable,
            },
            ReadinessMutation {
                expected: AiReadinessReasonCode::AuditGateUnavailable,
                mutate: |d| d.audit_gate = AiInvocationGuardState::Unavailable,
            },
        ];

        for ReadinessMutation { expected, mutate } in mutations {
            let mut dimensions = ready_dimensions();
            mutate(&mut dimensions);
            let result = evaluate_ai_readiness(AiCapabilityId::OcrSuggestionAnalysis, dimensions);
            assert_eq!(result.status, AiReadinessStatus::Blocked);
            assert_eq!(result.reason_code, expected);
        }
    }

    #[test]
    fn provider_api_key_with_ready_cli_reports_mode_mismatch_before_empty_endpoint() {
        let mut dimensions = ready_dimensions();
        dimensions.selected_access_mode = AiAccessMode::ProviderApiKey;
        dimensions.access_mode_compatible = false;
        dimensions.endpoint_or_profile_configured = false;
        dimensions.provider_invocation = AiProviderInvocationReadiness::Ready;

        let result = evaluate_ai_readiness(AiCapabilityId::ChatSubprocess, dimensions);
        assert_eq!(
            result.reason_code,
            AiReadinessReasonCode::AccessModeMismatch
        );
    }

    #[test]
    fn compiled_capability_missing_does_not_offer_a_dead_runtime_action() {
        let mut dimensions = ready_dimensions();
        dimensions.compiled_capability = false;

        let result = evaluate_ai_readiness(AiCapabilityId::ChatSubprocess, dimensions);
        assert_eq!(
            result.reason_code,
            AiReadinessReasonCode::CompiledCapabilityMissing
        );
        assert_eq!(result.action, AiReadinessAction::None);
    }

    #[test]
    fn auth_ready_without_invocation_ready_is_not_usable() {
        let mut dimensions = ready_dimensions();
        dimensions.provider_invocation = AiProviderInvocationReadiness::Unverified;

        let result = evaluate_ai_readiness(AiCapabilityId::ChatSubprocess, dimensions);
        assert_eq!(result.status, AiReadinessStatus::Blocked);
        assert_eq!(
            result.reason_code,
            AiReadinessReasonCode::ProviderInvocationUnverified
        );
    }

    #[test]
    fn serialized_contract_cannot_carry_sensitive_user_strings() {
        let snapshot = AiReadinessSnapshot::new(vec![evaluate_ai_readiness(
            AiCapabilityId::ChatSubprocess,
            ready_dimensions(),
        )]);
        let json = serde_json::to_string(&snapshot).expect("readiness snapshot serialization");

        for forbidden in [
            "prompt",
            "ocr_text",
            "window_text",
            "token",
            "account_id",
            "path",
        ] {
            assert!(
                !json.contains(forbidden),
                "unexpected sensitive field: {forbidden}"
            );
        }
    }

    #[test]
    fn every_capability_has_the_stable_wire_id() {
        let expected = [
            "chat.subprocess",
            "chat.http_api",
            "chat.local_llm",
            "ocr.capture",
            "ocr.suggestion_analysis",
            "segment_summary",
            "daily_narrative",
        ];

        assert_eq!(AiCapabilityId::ALL.map(AiCapabilityId::wire_id), expected);
    }

    #[test]
    fn every_reason_has_the_stable_wire_code() {
        let cases = [
            (AiReadinessReasonCode::Ready, "ready"),
            (
                AiReadinessReasonCode::CompiledCapabilityMissing,
                "compiled_capability_missing",
            ),
            (
                AiReadinessReasonCode::RuntimeFlagDisabled,
                "runtime_flag_disabled",
            ),
            (AiReadinessReasonCode::ConsentRequired, "consent_required"),
            (
                AiReadinessReasonCode::AccessModeMismatch,
                "access_mode_mismatch",
            ),
            (
                AiReadinessReasonCode::EndpointOrProfileRequired,
                "endpoint_or_profile_required",
            ),
            (
                AiReadinessReasonCode::ProviderNotDetected,
                "provider_not_detected",
            ),
            (
                AiReadinessReasonCode::ProviderAuthRequired,
                "provider_auth_required",
            ),
            (
                AiReadinessReasonCode::ProviderAuthUnverified,
                "provider_auth_unverified",
            ),
            (
                AiReadinessReasonCode::ProviderInvocationUnavailable,
                "provider_invocation_unavailable",
            ),
            (
                AiReadinessReasonCode::ProviderInvocationUnverified,
                "provider_invocation_unverified",
            ),
            (AiReadinessReasonCode::ModelUnavailable, "model_unavailable"),
            (
                AiReadinessReasonCode::ModelAvailabilityUnverified,
                "model_availability_unverified",
            ),
            (
                AiReadinessReasonCode::HotRewireRequired,
                "hot_rewire_required",
            ),
            (AiReadinessReasonCode::RestartRequired, "restart_required"),
            (
                AiReadinessReasonCode::PrivacyGateUnavailable,
                "privacy_gate_unavailable",
            ),
            (
                AiReadinessReasonCode::EgressGateUnavailable,
                "egress_gate_unavailable",
            ),
            (
                AiReadinessReasonCode::BudgetGateUnavailable,
                "budget_gate_unavailable",
            ),
            (
                AiReadinessReasonCode::AuditGateUnavailable,
                "audit_gate_unavailable",
            ),
        ];

        for (reason, expected) in cases {
            assert_eq!(reason.wire_code(), expected);
        }
    }

    #[test]
    fn every_action_has_the_stable_copy_key() {
        let cases = [
            (AiReadinessAction::None, "aiReadiness.action.none"),
            (
                AiReadinessAction::OpenAiSettings,
                "aiReadiness.action.openAiSettings",
            ),
            (
                AiReadinessAction::OpenPrivacyConsent,
                "aiReadiness.action.openPrivacyConsent",
            ),
            (
                AiReadinessAction::EnableFeature,
                "aiReadiness.action.enableFeature",
            ),
            (
                AiReadinessAction::InstallProvider,
                "aiReadiness.action.installProvider",
            ),
            (
                AiReadinessAction::AuthenticateProvider,
                "aiReadiness.action.authenticateProvider",
            ),
            (
                AiReadinessAction::VerifyProviderInvocation,
                "aiReadiness.action.verifyProviderInvocation",
            ),
            (
                AiReadinessAction::SelectModel,
                "aiReadiness.action.selectModel",
            ),
            (
                AiReadinessAction::ApplyHotRewire,
                "aiReadiness.action.applyHotRewire",
            ),
            (
                AiReadinessAction::RestartApp,
                "aiReadiness.action.restartApp",
            ),
            (
                AiReadinessAction::ReviewPrivacy,
                "aiReadiness.action.reviewPrivacy",
            ),
            (
                AiReadinessAction::ReviewEgress,
                "aiReadiness.action.reviewEgress",
            ),
            (
                AiReadinessAction::ReviewBudget,
                "aiReadiness.action.reviewBudget",
            ),
            (
                AiReadinessAction::ReviewAudit,
                "aiReadiness.action.reviewAudit",
            ),
        ];

        for (action, expected) in cases {
            assert_eq!(action.copy_key(), expected);
        }
    }

    #[test]
    fn snapshot_find_matches_only_the_requested_capability() {
        let snapshot = AiReadinessSnapshot::new(vec![
            evaluate_ai_readiness(AiCapabilityId::ChatSubprocess, ready_dimensions()),
            evaluate_ai_readiness(AiCapabilityId::DailyNarrative, ready_dimensions()),
        ]);

        assert_eq!(
            snapshot
                .find(AiCapabilityId::DailyNarrative)
                .map(|item| item.capability_id),
            Some(AiCapabilityId::DailyNarrative)
        );
        assert!(snapshot.find(AiCapabilityId::OcrCapture).is_none());
    }
}
