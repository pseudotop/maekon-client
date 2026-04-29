use crate::config::{AiAccessMode, AiProviderType};
use serde::Deserialize;
use std::sync::OnceLock;

const PROVIDER_SURFACE_CATALOG_JSON: &str =
    include_str!("../../../specs/providers/provider-surface-catalog.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderSurfaceTransport {
    DirectApi,
    ManagedOAuth,
    SubprocessCli,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSurfaceSpec {
    pub id: String,
    pub vendor_id: String,
    pub provider_type: AiProviderType,
    pub transport: ProviderSurfaceTransport,
    pub supports_llm: bool,
    pub supports_ocr: bool,
    pub uses_no_auth: bool,
    pub preferred_for_product_auth: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderVendorProjection {
    pub vendor_id: String,
    pub provider_type: AiProviderType,
    pub api_key_env_vars: Vec<String>,
    pub api_key_temp_file_prefix: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderVendorInfo {
    pub vendor_id: String,
    pub provider_type: AiProviderType,
    pub aliases: Vec<String>,
    pub display_name: String,
}

#[derive(Debug, Deserialize)]
struct ProviderSurfaceCatalogDocument {
    vendors: Vec<ProviderSurfaceCatalogVendor>,
    surfaces: Vec<ProviderSurfaceCatalogSurface>,
}

#[derive(Debug, Deserialize)]
struct ProviderSurfaceCatalogVendor {
    vendor_id: String,
    provider_type: String,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    projection: Option<ProviderSurfaceCatalogVendorProjection>,
}

#[derive(Debug, Deserialize)]
struct ProviderSurfaceCatalogVendorProjection {
    #[serde(default)]
    api_key_env_vars: Vec<String>,
    #[serde(default)]
    api_key_temp_file_prefix: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProviderSurfaceCatalogSurface {
    surface_id: String,
    vendor_id: String,
    provider_type: String,
    execution_kind: String,
    #[serde(default)]
    preferred_for_product_auth: bool,
    supports: ProviderSurfaceCatalogSupports,
    #[serde(default)]
    llm_transport: Option<ProviderSurfaceCatalogTransport>,
    #[serde(default)]
    ocr_transport: Option<ProviderSurfaceCatalogTransport>,
}

#[derive(Debug, Deserialize)]
struct ProviderSurfaceCatalogSupports {
    llm: bool,
    ocr: bool,
}

#[derive(Debug, Deserialize)]
struct ProviderSurfaceCatalogTransport {
    auth_scheme: String,
}

static KNOWN_PROVIDER_SURFACES: OnceLock<Result<Vec<ProviderSurfaceSpec>, String>> =
    OnceLock::new();
static KNOWN_PROVIDER_VENDOR_PROJECTIONS: OnceLock<Result<Vec<ProviderVendorProjection>, String>> =
    OnceLock::new();
static KNOWN_PROVIDER_VENDORS: OnceLock<Result<Vec<ProviderVendorInfo>, String>> = OnceLock::new();

pub fn canonical_provider_surface_id(raw: &str) -> Option<&'static str> {
    provider_surface_spec(raw).map(|spec| spec.id.as_str())
}

pub fn provider_surface_spec(raw: &str) -> Option<&'static ProviderSurfaceSpec> {
    let normalized = raw.trim().to_ascii_lowercase();
    provider_surface_specs()?
        .iter()
        .find(|spec| spec.id.eq_ignore_ascii_case(&normalized))
}

pub fn provider_surface_supports_llm(raw: &str) -> bool {
    provider_surface_spec(raw).is_some_and(|spec| spec.supports_llm)
}

pub fn provider_surface_supports_ocr(raw: &str) -> bool {
    provider_surface_spec(raw).is_some_and(|spec| spec.supports_ocr)
}

pub fn provider_surface_uses_no_auth(raw: &str) -> bool {
    provider_surface_spec(raw).is_some_and(|spec| spec.uses_no_auth)
}

pub fn provider_projection_for_type(
    provider_type: AiProviderType,
) -> Option<&'static ProviderVendorProjection> {
    provider_vendor_projections()?
        .iter()
        .find(|projection| projection.provider_type == provider_type)
}

pub fn provider_vendor_id(provider_type: AiProviderType) -> Option<&'static str> {
    provider_vendors()?
        .iter()
        .find(|vendor| vendor.provider_type == provider_type)
        .map(|vendor| vendor.vendor_id.as_str())
}

pub fn provider_vendor_id_or_default(provider_type: AiProviderType) -> &'static str {
    provider_vendor_id(provider_type).unwrap_or_else(|| fallback_provider_vendor_id(provider_type))
}

pub fn provider_display_name(provider_type: AiProviderType) -> Option<&'static str> {
    provider_vendors()?
        .iter()
        .find(|vendor| vendor.provider_type == provider_type)
        .map(|vendor| vendor.display_name.as_str())
}

pub fn provider_display_name_or_default(provider_type: AiProviderType) -> &'static str {
    provider_display_name(provider_type)
        .unwrap_or_else(|| fallback_provider_display_name(provider_type))
}

pub fn provider_type_from_vendor_id(raw: &str) -> Option<AiProviderType> {
    let normalized = raw.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return None;
    }

    provider_vendors()?
        .iter()
        .find(|vendor| {
            vendor.vendor_id.eq_ignore_ascii_case(&normalized)
                || vendor
                    .aliases
                    .iter()
                    .any(|alias| alias.eq_ignore_ascii_case(&normalized))
        })
        .map(|vendor| vendor.provider_type)
}

pub fn default_provider_surface_id(
    provider_type: AiProviderType,
    access_mode: AiAccessMode,
) -> Option<&'static str> {
    let expected_transport = match access_mode.normalized_for_ai_surfaces() {
        AiAccessMode::ProviderOAuth => ProviderSurfaceTransport::ManagedOAuth,
        AiAccessMode::ProviderSubscriptionCli => ProviderSurfaceTransport::SubprocessCli,
        AiAccessMode::ProviderApiKey | AiAccessMode::LocalModel => {
            ProviderSurfaceTransport::DirectApi
        }
    };

    provider_surface_specs()?
        .iter()
        .filter(|spec| spec.provider_type == provider_type && spec.transport == expected_transport)
        .max_by_key(|spec| spec.preferred_for_product_auth)
        .map(|spec| spec.id.as_str())
}

fn provider_surface_specs() -> Option<&'static [ProviderSurfaceSpec]> {
    match KNOWN_PROVIDER_SURFACES.get_or_init(load_provider_surface_specs) {
        Ok(specs) => Some(specs.as_slice()),
        Err(error) => {
            tracing::warn!(
                error = %error,
                "Failed to load provider surface catalog inside oneshim-core."
            );
            None
        }
    }
}

fn load_provider_surface_specs() -> Result<Vec<ProviderSurfaceSpec>, String> {
    let catalog =
        serde_json::from_str::<ProviderSurfaceCatalogDocument>(PROVIDER_SURFACE_CATALOG_JSON)
            .map_err(|error| format!("Failed to parse provider surface catalog JSON: {error}"))?;

    catalog
        .surfaces
        .into_iter()
        .map(|surface| {
            Ok(ProviderSurfaceSpec {
                id: surface.surface_id,
                vendor_id: surface.vendor_id,
                provider_type: parse_provider_type(&surface.provider_type)?,
                transport: parse_transport(&surface.execution_kind)?,
                supports_llm: surface.supports.llm,
                supports_ocr: surface.supports.ocr,
                uses_no_auth: transport_uses_no_auth(&surface.llm_transport)
                    || transport_uses_no_auth(&surface.ocr_transport),
                preferred_for_product_auth: surface.preferred_for_product_auth,
            })
        })
        .collect()
}

fn provider_vendor_projections() -> Option<&'static [ProviderVendorProjection]> {
    match KNOWN_PROVIDER_VENDOR_PROJECTIONS.get_or_init(load_provider_vendor_projections) {
        Ok(projections) => Some(projections.as_slice()),
        Err(error) => {
            tracing::warn!(
                error = %error,
                "Failed to load provider vendor projection metadata inside oneshim-core."
            );
            None
        }
    }
}

fn provider_vendors() -> Option<&'static [ProviderVendorInfo]> {
    match KNOWN_PROVIDER_VENDORS.get_or_init(load_provider_vendors) {
        Ok(vendors) => Some(vendors.as_slice()),
        Err(error) => {
            tracing::warn!(
                error = %error,
                "Failed to load provider vendor metadata inside oneshim-core."
            );
            None
        }
    }
}

fn load_provider_vendors() -> Result<Vec<ProviderVendorInfo>, String> {
    let catalog =
        serde_json::from_str::<ProviderSurfaceCatalogDocument>(PROVIDER_SURFACE_CATALOG_JSON)
            .map_err(|error| format!("Failed to parse provider surface catalog JSON: {error}"))?;

    catalog
        .vendors
        .into_iter()
        .map(|vendor| {
            Ok(ProviderVendorInfo {
                vendor_id: vendor.vendor_id,
                provider_type: parse_provider_type(&vendor.provider_type)?,
                aliases: vendor.aliases,
                display_name: vendor.display_name,
            })
        })
        .collect()
}

fn load_provider_vendor_projections() -> Result<Vec<ProviderVendorProjection>, String> {
    let catalog =
        serde_json::from_str::<ProviderSurfaceCatalogDocument>(PROVIDER_SURFACE_CATALOG_JSON)
            .map_err(|error| format!("Failed to parse provider surface catalog JSON: {error}"))?;

    catalog
        .vendors
        .into_iter()
        .filter_map(|vendor| {
            let projection = vendor.projection?;
            let prefix = projection
                .api_key_temp_file_prefix
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())?
                .to_string();
            let env_vars = projection
                .api_key_env_vars
                .into_iter()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>();
            Some(
                parse_provider_type(&vendor.provider_type).map(|provider_type| {
                    ProviderVendorProjection {
                        vendor_id: vendor.vendor_id,
                        provider_type,
                        api_key_env_vars: env_vars,
                        api_key_temp_file_prefix: prefix,
                    }
                }),
            )
        })
        .collect::<Result<Vec<_>, _>>()
}

fn parse_provider_type(raw: &str) -> Result<AiProviderType, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "anthropic" => Ok(AiProviderType::Anthropic),
        "openai" => Ok(AiProviderType::OpenAi),
        "google" => Ok(AiProviderType::Google),
        "ollama" => Ok(AiProviderType::Ollama),
        "bedrock" => Ok(AiProviderType::Bedrock),
        "copilot" => Ok(AiProviderType::Copilot),
        "generic" => Ok(AiProviderType::Generic),
        other => Err(format!(
            "Unsupported provider_type '{other}' in provider surface catalog."
        )),
    }
}

fn parse_transport(raw: &str) -> Result<ProviderSurfaceTransport, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "direct_http" => Ok(ProviderSurfaceTransport::DirectApi),
        "managed_http" => Ok(ProviderSurfaceTransport::ManagedOAuth),
        "subprocess_cli" => Ok(ProviderSurfaceTransport::SubprocessCli),
        other => Err(format!(
            "Unsupported execution_kind '{other}' in provider surface catalog."
        )),
    }
}

fn fallback_provider_vendor_id(provider_type: AiProviderType) -> &'static str {
    match provider_type {
        AiProviderType::Anthropic => "anthropic",
        AiProviderType::OpenAi => "openai",
        AiProviderType::Google => "google",
        AiProviderType::Ollama => "ollama",
        AiProviderType::Bedrock => "bedrock",
        AiProviderType::Copilot => "copilot",
        AiProviderType::Generic => "generic",
    }
}

fn fallback_provider_display_name(provider_type: AiProviderType) -> &'static str {
    match provider_type {
        AiProviderType::Anthropic => "Anthropic",
        AiProviderType::OpenAi => "OpenAI",
        AiProviderType::Google => "Google",
        AiProviderType::Ollama => "Ollama",
        AiProviderType::Bedrock => "Amazon Bedrock",
        AiProviderType::Copilot => "GitHub Copilot",
        AiProviderType::Generic => "Generic",
    }
}

fn transport_uses_no_auth(transport: &Option<ProviderSurfaceCatalogTransport>) -> bool {
    transport
        .as_ref()
        .is_some_and(|transport| transport.auth_scheme.eq_ignore_ascii_case("none"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalizes_known_surface_ids() {
        assert_eq!(
            canonical_provider_surface_id("provider_surface.openai.subprocess_cli"),
            Some("provider_surface.openai.subprocess_cli")
        );
        assert_eq!(
            canonical_provider_surface_id("PROVIDER_SURFACE.OPENAI.DIRECT_API"),
            Some("provider_surface.openai.direct_api")
        );
    }

    #[test]
    fn derives_defaults_for_access_mode() {
        assert_eq!(
            default_provider_surface_id(AiProviderType::OpenAi, AiAccessMode::ProviderOAuth),
            Some("provider_surface.openai.managed_oauth")
        );
        assert_eq!(
            default_provider_surface_id(
                AiProviderType::Anthropic,
                AiAccessMode::ProviderSubscriptionCli
            ),
            Some("provider_surface.anthropic.subprocess_cli")
        );
        assert_eq!(
            default_provider_surface_id(AiProviderType::Generic, AiAccessMode::ProviderApiKey),
            Some("provider_surface.generic.direct_api")
        );
    }

    #[test]
    fn resolves_no_auth_from_catalog_transports() {
        assert!(provider_surface_uses_no_auth(
            "provider_surface.ollama.local_http"
        ));
        assert!(!provider_surface_uses_no_auth(
            "provider_surface.openai.direct_api"
        ));
    }

    #[test]
    fn resolves_projection_metadata_from_vendor_catalog() {
        let projection =
            provider_projection_for_type(AiProviderType::OpenAi).expect("projection should exist");
        assert_eq!(projection.vendor_id, "openai");
        assert_eq!(
            projection.api_key_env_vars,
            vec!["OPENAI_API_KEY".to_string()]
        );
        assert_eq!(projection.api_key_temp_file_prefix, "openai");
    }

    #[test]
    fn resolves_vendor_ids_and_aliases_from_catalog() {
        assert_eq!(provider_vendor_id(AiProviderType::Google), Some("google"));
        assert_eq!(
            provider_type_from_vendor_id("gemini"),
            Some(AiProviderType::Google)
        );
        assert_eq!(
            provider_type_from_vendor_id("open_ai"),
            Some(AiProviderType::OpenAi)
        );
    }
}
