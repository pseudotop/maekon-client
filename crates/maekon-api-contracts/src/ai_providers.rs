use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProviderTransportSpec {
    pub method: String,
    pub url: String,
    pub auth_scheme: String,
    pub request_shape: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProviderModelCatalogTransportSpec {
    pub method: String,
    pub url: String,
    pub auth_scheme: String,
    pub response_shape: String,
    #[serde(default = "default_true")]
    pub llm_supported: bool,
    #[serde(default = "default_true")]
    pub ocr_supported: bool,
    #[serde(default)]
    pub ocr_notice: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProviderHealthTransportSpec {
    pub method: String,
    pub url: String,
    pub auth_scheme: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderModelSupportStatus {
    Supported,
    Unsupported,
    Unknown,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default, PartialEq, Eq)]
pub struct ProviderModelCapabilityRules {
    #[serde(default)]
    pub llm: ProviderModelCapabilityProfile,
    #[serde(default)]
    pub ocr: ProviderModelCapabilityProfile,
    #[serde(default)]
    pub image_input: ProviderModelCapabilityProfile,
    #[serde(default)]
    pub structured_output: ProviderModelCapabilityProfile,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default, PartialEq, Eq)]
pub struct ProviderModelCapabilityProfile {
    #[serde(default)]
    pub default_support: String,
    #[serde(default)]
    pub allow_patterns: Vec<String>,
    #[serde(default)]
    pub deny_patterns: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProviderParameterSet {
    pub llm: ProviderParameterProfile,
    pub ocr: ProviderParameterProfile,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProviderParameterProfile {
    #[serde(default)]
    pub supported: Vec<String>,
    #[serde(default)]
    pub unsupported: Vec<String>,
    #[serde(default)]
    pub notes: Vec<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize, Serialize)]
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

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
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
