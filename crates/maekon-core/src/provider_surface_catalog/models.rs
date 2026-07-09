use super::enums::{
    ModelCatalogStrategy, ProviderUnknownModelPolicy, SubprocessAuthProbeMode,
    SubprocessInvocationMode, SurfaceExecutionKind, SurfacePlacementKind, SurfaceStability,
};

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProviderTransportSpec {
    pub method: String,
    pub url: String,
    pub auth_scheme: String,
    pub request_shape: String,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProviderHealthTransportSpec {
    pub method: String,
    pub url: String,
    pub auth_scheme: String,
}

#[derive(Debug, Clone, Copy, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ProviderModelSupportStatus {
    Supported,
    Unsupported,
    Unknown,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, Default, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, Default, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProviderModelCapabilityProfile {
    #[serde(default)]
    pub default_support: String,
    #[serde(default)]
    pub allow_patterns: Vec<String>,
    #[serde(default)]
    pub deny_patterns: Vec<String>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProviderParameterSet {
    pub llm: ProviderParameterProfile,
    pub ocr: ProviderParameterProfile,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProviderParameterProfile {
    #[serde(default)]
    pub supported: Vec<String>,
    #[serde(default)]
    pub unsupported: Vec<String>,
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProviderSurfaceCatalog {
    pub version: u32,
    #[serde(default)]
    pub updated_at: String,
    pub vendors: Vec<ProviderVendorSpec>,
    pub surfaces: Vec<ProviderSurfaceSpec>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProviderVendorProjectionSpec {
    #[serde(default)]
    pub api_key_env_vars: Vec<String>,
    #[serde(default)]
    pub api_key_temp_file_prefix: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProviderSurfaceSpec {
    pub surface_id: String,
    pub vendor_id: String,
    pub provider_type: String,
    pub display_name: String,
    pub execution_kind: SurfaceExecutionKind,
    pub placement_kind: SurfacePlacementKind,
    pub credential_kind: String,
    pub stability: SurfaceStability,
    #[serde(default)]
    pub preferred_for_product_auth: bool,
    #[serde(default)]
    pub related_surface_ids: Vec<String>,
    #[serde(default = "default_model_catalog_strategy")]
    pub catalog_strategy: ModelCatalogStrategy,
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
    pub compatibility: Option<ProviderCliCompatibilitySpec>,
    #[serde(default)]
    pub provisioning: Option<ProviderSurfaceProvisioningSpec>,
    #[serde(default)]
    pub references: Vec<String>,
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProviderSurfaceProvisioningSpec {
    #[serde(default)]
    pub configuration_env_vars: Vec<String>,
    #[serde(default)]
    pub setup_copy_key: Option<String>,
    #[serde(default)]
    pub docs_url: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProviderLlmCapabilities {
    #[serde(default)]
    pub structured_output: bool,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SurfaceDefaultModels {
    #[serde(default)]
    pub llm_models: Vec<String>,
    #[serde(default)]
    pub ocr_models: Vec<String>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProviderKnownModelCapabilities {
    #[serde(default = "default_true")]
    pub llm: bool,
    #[serde(default)]
    pub ocr: bool,
    #[serde(default)]
    pub image_input: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProviderUnknownModelPolicySet {
    #[serde(default = "default_unknown_model_policy")]
    pub llm: ProviderUnknownModelPolicy,
    #[serde(default = "default_unknown_model_policy")]
    pub ocr: ProviderUnknownModelPolicy,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProviderAvailabilityProbeSpec {
    pub method: String,
    pub url: String,
    pub auth_scheme: String,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SubprocessTransportSpec {
    pub tool_id: String,
    #[serde(default)]
    pub executable_candidates: Vec<String>,
    #[serde(default)]
    pub auth_probe_command: Vec<String>,
    pub auth_probe_mode: SubprocessAuthProbeMode,
    pub invocation_mode: SubprocessInvocationMode,
    #[serde(default)]
    pub model_flag: Option<String>,
    #[serde(default)]
    pub json_output_supported: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub oneshot_flags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub session_flags: Vec<String>,
    /// Args that launch the long-lived app-server (e.g. `["app-server"]`), used
    /// only by `codex_app_server` surfaces (E21 #4866). Read by the factory to
    /// build the `CodexAppServerSession` command.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub app_server_args: Vec<String>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProviderCliCompatibilitySpec {
    pub matrix_version: String,
    #[serde(default)]
    pub supported_oses: Vec<String>,
    pub minimum_supported_version: String,
    #[serde(default)]
    pub known_bad_versions: Vec<String>,
    #[serde(default)]
    pub version_probe_command: Vec<String>,
    #[serde(default)]
    pub auth_probe_command: Vec<String>,
    pub invocation_mode: SubprocessInvocationMode,
    pub output_envelope: String,
    pub ocr_support: String,
    pub session_support: String,
    #[serde(default)]
    pub ci_fake_cli_contracts: Vec<String>,
    #[serde(default)]
    pub manual_live_smoke_required: bool,
    #[serde(default)]
    pub notes: Vec<String>,
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

pub(super) fn default_model_catalog_strategy() -> ModelCatalogStrategy {
    ModelCatalogStrategy::None
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
