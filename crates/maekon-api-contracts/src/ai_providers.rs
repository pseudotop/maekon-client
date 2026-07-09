use serde::{Deserialize, Serialize};

pub use maekon_core::provider_surface_catalog::{
    ProviderHealthTransportSpec, ProviderModelCapabilityProfile, ProviderModelCapabilityRules,
    ProviderModelCatalogTransportSpec, ProviderModelSupportStatus, ProviderParameterProfile,
    ProviderParameterSet, ProviderTransportSpec,
};

// NOTE: Debug is hand-written (not derived) to mask `api_key` (#5639). This is
// a BYOK secret; a derived Debug would emit it verbatim under any `{:?}`, so a
// single error-path `?req` would leak the key to the file/OTel log sink.
#[derive(Deserialize, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProviderModelsRequest {
    pub provider_type: String,
    pub api_key: String,
    pub endpoint: Option<String>,
    #[serde(default)]
    pub surface: Option<String>,
    #[serde(default)]
    pub surface_id: Option<String>,
    #[serde(default)]
    pub use_saved_secret: bool,
}

impl std::fmt::Debug for ProviderModelsRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderModelsRequest")
            .field("provider_type", &self.provider_type)
            .field("api_key", &"[REDACTED]")
            .field("endpoint", &self.endpoint)
            .field("surface", &self.surface)
            .field("surface_id", &self.surface_id)
            .field("use_saved_secret", &self.use_saved_secret)
            .finish()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProviderDiscoveredModel {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_support: Option<ProviderModelSupportStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_ocr: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ocr_support: Option<ProviderModelSupportStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_input_support: Option<ProviderModelSupportStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_output_support: Option<ProviderModelSupportStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability_source: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProviderModelsResponse {
    pub models: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_details: Vec<ProviderDiscoveredModel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_models_request_debug_redacts_api_key() {
        let req = ProviderModelsRequest {
            provider_type: "openai".to_string(),
            api_key: "sk-secret-byok-key-value".to_string(),
            endpoint: None,
            surface: None,
            surface_id: None,
            use_saved_secret: false,
        };
        let rendered = format!("{req:?}");
        assert!(
            !rendered.contains("sk-secret-byok-key-value"),
            "Debug must not leak the BYOK api_key: {rendered}"
        );
        assert!(
            rendered.contains("[REDACTED]"),
            "api_key must render as [REDACTED]: {rendered}"
        );
        // Non-secret fields must still be visible for diagnostics.
        assert!(rendered.contains("openai"));
    }

    #[test]
    fn round_trip_provider_model_support_status() {
        for status in [
            ProviderModelSupportStatus::Supported,
            ProviderModelSupportStatus::Unsupported,
            ProviderModelSupportStatus::Unknown,
        ] {
            let json = serde_json::to_string(&status).unwrap();
            let decoded: ProviderModelSupportStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(status, decoded);
        }
    }

    #[test]
    fn round_trip_provider_model_capability_profile() {
        let original = ProviderModelCapabilityProfile {
            default_support: "supported".to_string(),
            allow_patterns: vec!["gpt-4*".to_string(), "claude-*".to_string()],
            deny_patterns: vec!["*-instruct".to_string()],
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ProviderModelCapabilityProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn round_trip_provider_model_capability_rules() {
        let original = ProviderModelCapabilityRules {
            llm: ProviderModelCapabilityProfile {
                default_support: "supported".to_string(),
                allow_patterns: vec!["gpt-4o*".to_string()],
                deny_patterns: vec![],
            },
            ocr: ProviderModelCapabilityProfile {
                default_support: "unsupported".to_string(),
                allow_patterns: vec!["gpt-4-vision*".to_string()],
                deny_patterns: vec![],
            },
            image_input: ProviderModelCapabilityProfile::default(),
            structured_output: ProviderModelCapabilityProfile::default(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ProviderModelCapabilityRules = serde_json::from_str(&json).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn round_trip_provider_discovered_model_minimal() {
        let original = ProviderDiscoveredModel {
            id: "gpt-4o".to_string(),
            display_name: Some("GPT-4o".to_string()),
            llm_support: Some(ProviderModelSupportStatus::Supported),
            supports_ocr: Some(true),
            ocr_support: Some(ProviderModelSupportStatus::Supported),
            image_input_support: Some(ProviderModelSupportStatus::Supported),
            structured_output_support: Some(ProviderModelSupportStatus::Supported),
            capability_source: Some("rules".to_string()),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ProviderDiscoveredModel = serde_json::from_str(&json).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn provider_discovered_model_optional_fields_skipped_when_none() {
        let original = ProviderDiscoveredModel {
            id: "unknown-model".to_string(),
            display_name: None,
            llm_support: None,
            supports_ocr: None,
            ocr_support: None,
            image_input_support: None,
            structured_output_support: None,
            capability_source: None,
        };
        let json = serde_json::to_string(&original).unwrap();
        assert!(!json.contains("display_name"));
        assert!(!json.contains("llm_support"));
        let decoded: ProviderDiscoveredModel = serde_json::from_str(&json).unwrap();
        assert_eq!(original, decoded);
    }
}
