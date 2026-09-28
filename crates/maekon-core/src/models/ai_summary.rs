use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Coarse provider boundary recorded with generated summaries.
///
/// This intentionally excludes provider names, model IDs, endpoints, account
/// identifiers, and prompts. The value is safe to persist and is sufficient
/// for the UI to distinguish device-loopback, subscription CLI, and external
/// API processing without making an unsupported "on-device" claim.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiSummaryProviderClass {
    Loopback,
    Subprocess,
    ExternalApi,
    #[default]
    Unknown,
}

/// Stable, privacy-safe reason why an AI summary is unavailable.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiSummaryFailureReason {
    PipelineDisabled,
    BelowMinimumDuration,
    ProviderUnavailable,
    ProviderFailed,
    InvalidResponse,
    CapacityLimited,
    #[default]
    NotGenerated,
}

/// Persisted presentation metadata for one AI-generated summary artifact.
///
/// `text` is already PII-filtered by the summarizer. No raw prompt, endpoint,
/// model, credential, or provider response is retained here.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiSummaryArtifact {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_class: Option<AiSummaryProviderClass>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<AiSummaryFailureReason>,
}

impl AiSummaryArtifact {
    pub fn generated(
        text: String,
        provider_class: AiSummaryProviderClass,
        generated_at: DateTime<Utc>,
    ) -> Self {
        Self {
            text: Some(text),
            provider_class: Some(provider_class),
            generated_at: Some(generated_at),
            failure_reason: None,
        }
    }

    pub fn unavailable(
        provider_class: Option<AiSummaryProviderClass>,
        reason: AiSummaryFailureReason,
    ) -> Self {
        Self {
            text: None,
            provider_class,
            generated_at: None,
            failure_reason: Some(reason),
        }
    }

    pub fn is_generated(&self) -> bool {
        self.text
            .as_ref()
            .is_some_and(|text| !text.trim().is_empty())
            && self.generated_at.is_some()
            && self.failure_reason.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_roundtrip_never_needs_provider_details() {
        let artifact = AiSummaryArtifact::generated(
            "Focused on the settings flow.".to_string(),
            AiSummaryProviderClass::Subprocess,
            Utc::now(),
        );
        let json = serde_json::to_string(&artifact).unwrap();
        assert!(json.contains("subprocess"));
        assert!(!json.contains("endpoint"));
        assert!(!json.contains("prompt"));
        assert!(serde_json::from_str::<AiSummaryArtifact>(&json)
            .unwrap()
            .is_generated());
    }

    #[test]
    fn legacy_empty_object_is_safe_not_generated() {
        let artifact: AiSummaryArtifact = serde_json::from_str("{}").unwrap();
        assert!(!artifact.is_generated());
        assert_eq!(artifact.failure_reason, None);
    }
}
