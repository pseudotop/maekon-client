use oneshim_api_contracts::ai_providers::{ProviderDiscoveredModel, ProviderModelSupportStatus};
use oneshim_api_contracts::provider_specs::{
    model_capability_status_for_surface, ModelCatalogResponseShape, SurfaceModelCapabilityKind,
};
use oneshim_core::config::AiProviderType;
use serde_json::Value;

use crate::error::ApiError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedModelRecord {
    pub(crate) id: String,
    pub(crate) display_name: Option<String>,
}

pub(crate) fn parse_models(
    shape: ModelCatalogResponseShape,
    body: &str,
) -> Result<Vec<ParsedModelRecord>, ApiError> {
    let value: Value = serde_json::from_str(body)
        .map_err(|e| ApiError::BadRequest(format!("Invalid model catalog response JSON: {e}")))?;

    match shape {
        ModelCatalogResponseShape::GoogleModels => parse_google_models(&value),
        ModelCatalogResponseShape::StandardDataOrModels => parse_standard_models(&value),
    }
}

pub(crate) fn parse_google_models(value: &Value) -> Result<Vec<ParsedModelRecord>, ApiError> {
    let Some(entries) = value.get("models").and_then(|m| m.as_array()) else {
        return Err(ApiError::BadRequest(
            "Google model catalog response missing `models`.".to_string(),
        ));
    };

    let mut generation_models = Vec::new();
    let mut fallback_models = Vec::new();
    for entry in entries {
        let raw_name = entry
            .get("name")
            .and_then(|v| v.as_str())
            .or_else(|| entry.get("displayName").and_then(|v| v.as_str()))
            .unwrap_or("")
            .trim();
        if raw_name.is_empty() {
            continue;
        }
        let normalized = raw_name
            .strip_prefix("models/")
            .unwrap_or(raw_name)
            .to_string();
        let display_name = entry
            .get("displayName")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
        let record = ParsedModelRecord {
            id: normalized.clone(),
            display_name,
        };
        fallback_models.push(record.clone());

        let supports_generation = entry
            .get("supportedGenerationMethods")
            .and_then(|v| v.as_array())
            .map(|methods| {
                methods
                    .iter()
                    .filter_map(|m| m.as_str())
                    .any(|method| method.eq_ignore_ascii_case("generateContent"))
            })
            .unwrap_or(false);
        if supports_generation {
            generation_models.push(record);
        }
    }

    if !generation_models.is_empty() {
        return Ok(generation_models);
    }
    Ok(fallback_models)
}

pub(crate) fn parse_standard_models(value: &Value) -> Result<Vec<ParsedModelRecord>, ApiError> {
    let entries = value
        .get("data")
        .and_then(|d| d.as_array())
        .or_else(|| value.get("models").and_then(|d| d.as_array()))
        .ok_or_else(|| {
            ApiError::BadRequest(
                "Model catalog response missing `data` (or `models`) array.".to_string(),
            )
        })?;

    let models = entries
        .iter()
        .filter_map(|entry| {
            let id = entry
                .get("id")
                .and_then(|v| v.as_str())
                .or_else(|| entry.get("name").and_then(|v| v.as_str()))
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(ToString::to_string)?;
            let display_name = entry
                .get("display_name")
                .and_then(|v| v.as_str())
                .or_else(|| entry.get("displayName").and_then(|v| v.as_str()))
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(ToString::to_string);
            Some(ParsedModelRecord { id, display_name })
        })
        .collect::<Vec<_>>();

    Ok(models)
}

pub(crate) fn build_model_details(
    provider_type: AiProviderType,
    surface_id: Option<&str>,
    models: &[ParsedModelRecord],
) -> Result<Vec<ProviderDiscoveredModel>, ApiError> {
    let Some(surface_id) = surface_id else {
        return Ok(Vec::new());
    };

    models
        .iter()
        .map(|model| {
            let known = oneshim_api_contracts::provider_specs::known_model_spec_for_surface(
                surface_id, &model.id,
            )
            .map_err(ApiError::Internal)?;
            let llm_support = Some(
                model_capability_status_for_surface(
                    surface_id,
                    SurfaceModelCapabilityKind::Llm,
                    &model.id,
                )
                .map_err(ApiError::Internal)?,
            );
            let ocr_support = Some(
                model_capability_status_for_surface(
                    surface_id,
                    SurfaceModelCapabilityKind::Ocr,
                    &model.id,
                )
                .map_err(ApiError::Internal)?,
            );
            let image_input_support = {
                let resolved = model_capability_status_for_surface(
                    surface_id,
                    SurfaceModelCapabilityKind::ImageInput,
                    &model.id,
                )
                .map_err(ApiError::Internal)?;
                Some(
                    if provider_type == AiProviderType::Google
                        && resolved == ProviderModelSupportStatus::Unknown
                        && llm_support == Some(ProviderModelSupportStatus::Supported)
                    {
                        ProviderModelSupportStatus::Supported
                    } else {
                        resolved
                    },
                )
            };
            let structured_output_support = Some(
                model_capability_status_for_surface(
                    surface_id,
                    SurfaceModelCapabilityKind::StructuredOutput,
                    &model.id,
                )
                .map_err(ApiError::Internal)?,
            );
            let capability_source = if known.is_some() {
                Some("known_model_catalog".to_string())
            } else if [
                llm_support,
                ocr_support,
                image_input_support,
                structured_output_support,
            ]
            .into_iter()
            .flatten()
            .any(|status| status != ProviderModelSupportStatus::Unknown)
            {
                Some("capability_rules".to_string())
            } else {
                Some("surface_unknown".to_string())
            };

            Ok(ProviderDiscoveredModel {
                id: model.id.clone(),
                display_name: model.display_name.clone(),
                llm_support,
                supports_ocr: ocr_support
                    .map(|status| status == ProviderModelSupportStatus::Supported),
                ocr_support,
                image_input_support,
                structured_output_support,
                capability_source,
            })
        })
        .collect()
}
