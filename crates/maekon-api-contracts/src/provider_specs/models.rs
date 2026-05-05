use crate::ai_providers::{
    ProviderModelCapabilityRules, ProviderModelCatalogTransportSpec, ProviderParameterSet,
    ProviderTransportSpec,
};

use super::enums::ProviderUnknownModelPolicy;

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct ProviderSurfaceCatalog {
    pub version: u32,
    #[serde(default)]
    pub updated_at: String,
    pub vendors: Vec<ProviderVendorSpec>,
    pub surfaces: Vec<ProviderSurfaceSpec>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct ProviderVendorSpec {
    pub vendor_id: String,
    pub provider_type: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub display_name: String,
    #[serde(default)]
    pub projection: Option<ProviderVendorProjectionSpec>,
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct ProviderVendorProjectionSpec {
    #[serde(default)]
    pub api_key_env_vars: Vec<String>,
    #[serde(default)]
    pub api_key_temp_file_prefix: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct ProviderSurfaceSpec {
    pub surface_id: String,
    pub vendor_id: String,
    pub provider_type: String,
    pub display_name: String,
    pub execution_kind: String,
    pub placement_kind: String,
    pub credential_kind: String,
    pub stability: String,
    #[serde(default)]
    pub preferred_for_product_auth: bool,
    #[serde(default)]
    pub related_surface_ids: Vec<String>,
    #[serde(default)]
    pub catalog_strategy: String,
    pub supports: ProviderSurfaceSupports,
    #[serde(default)]
    pub llm_capabilities: ProviderLlmCapabilities,
    #[serde(default)]
    pub ocr_capabilities: ProviderOcrCapabilities,
    pub default_models: SurfaceDefaultModels,
    #[serde(default)]
    pub capability_rules: ProviderModelCapabilityRules,
    pub parameter_profiles: ProviderParameterSet,
    #[serde(default)]
    pub unknown_model_policy: ProviderUnknownModelPolicySet,
    #[serde(default)]
    pub known_models: Vec<ProviderKnownModelSpec>,
    #[serde(default)]
    pub llm_transport: Option<ProviderTransportSpec>,
    #[serde(default)]
    pub ocr_transport: Option<ProviderTransportSpec>,
    #[serde(default)]
    pub model_catalog_transport: Option<ProviderModelCatalogTransportSpec>,
    #[serde(default)]
    pub availability_probe: Option<ProviderAvailabilityProbeSpec>,
    #[serde(default)]
    pub subprocess_transport: Option<SubprocessTransportSpec>,
    #[serde(default)]
    pub provisioning: Option<ProviderSurfaceProvisioningSpec>,
    #[serde(default)]
    pub references: Vec<String>,
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct ProviderSurfaceProvisioningSpec {
    #[serde(default)]
    pub configuration_env_vars: Vec<String>,
    #[serde(default)]
    pub setup_copy_key: Option<String>,
    #[serde(default)]
    pub docs_url: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct ProviderSurfaceSupports {
    #[serde(default)]
    pub llm: bool,
    #[serde(default)]
    pub ocr: bool,
    #[serde(default)]
    pub model_catalog: bool,
    #[serde(default)]
    pub context_bridge: bool,
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct ProviderLlmCapabilities {
    #[serde(default)]
    pub structured_output: bool,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct ProviderOcrCapabilities {
    #[serde(default = "default_ocr_strategy")]
    pub strategy: String,
    #[serde(default)]
    pub supports_geometry: bool,
    #[serde(default)]
    pub supports_confidence: bool,
    #[serde(default)]
    pub requires_image_input_model: bool,
    #[serde(default)]
    pub requires_structured_output_model: bool,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct SurfaceDefaultModels {
    #[serde(default)]
    pub llm_models: Vec<String>,
    #[serde(default)]
    pub ocr_models: Vec<String>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct ProviderKnownModelSpec {
    pub id: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub id_prefixes: Vec<String>,
    pub capabilities: ProviderKnownModelCapabilities,
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct ProviderKnownModelCapabilities {
    #[serde(default = "default_true")]
    pub llm: bool,
    #[serde(default)]
    pub ocr: bool,
    #[serde(default)]
    pub image_input: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct ProviderUnknownModelPolicySet {
    #[serde(default = "default_unknown_model_policy")]
    pub llm: ProviderUnknownModelPolicy,
    #[serde(default = "default_unknown_model_policy")]
    pub ocr: ProviderUnknownModelPolicy,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct ProviderAvailabilityProbeSpec {
    pub method: String,
    pub url: String,
    pub auth_scheme: String,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct SubprocessTransportSpec {
    pub tool_id: String,
    #[serde(default)]
    pub executable_candidates: Vec<String>,
    #[serde(default)]
    pub auth_probe_command: Vec<String>,
    pub auth_probe_mode: String,
    pub invocation_mode: String,
    #[serde(default)]
    pub model_flag: Option<String>,
    #[serde(default)]
    pub json_output_supported: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub oneshot_flags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub session_flags: Vec<String>,
}

fn default_true() -> bool {
    true
}

pub(super) fn default_unknown_model_policy() -> ProviderUnknownModelPolicy {
    ProviderUnknownModelPolicy::Warn
}

pub(super) fn default_ocr_strategy() -> String {
    "none".to_string()
}

impl Default for ProviderUnknownModelPolicySet {
    fn default() -> Self {
        Self {
            llm: default_unknown_model_policy(),
            ocr: default_unknown_model_policy(),
        }
    }
}

impl Default for ProviderOcrCapabilities {
    fn default() -> Self {
        Self {
            strategy: default_ocr_strategy(),
            supports_geometry: false,
            supports_confidence: false,
            requires_image_input_model: false,
            requires_structured_output_model: false,
        }
    }
}
