//! Policy catalog loading, validation, and the `parse_utc_opt` date helper.

use std::sync::OnceLock;

use chrono::{DateTime, Utc};

use crate::error::CoreError;
use crate::provider_surface::canonical_provider_surface_id;

use super::types::ModelLifecyclePolicyCatalog;

const POLICY_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/config/ai_model_lifecycle_policy.json"
));

pub(super) static POLICY_CATALOG: OnceLock<Result<ModelLifecyclePolicyCatalog, String>> =
    OnceLock::new();

pub(super) fn policy_catalog() -> Result<&'static ModelLifecyclePolicyCatalog, CoreError> {
    match POLICY_CATALOG.get_or_init(load_policy_catalog) {
        Ok(catalog) => Ok(catalog),
        Err(message) => Err(CoreError::Internal {
            code: crate::error_codes::InternalCode::Generic,
            message: message.clone(),
        }),
    }
}

fn load_policy_catalog() -> Result<ModelLifecyclePolicyCatalog, String> {
    let catalog = serde_json::from_str::<ModelLifecyclePolicyCatalog>(POLICY_JSON)
        .map_err(|e| format!("Failed to parse model lifecycle policy JSON: {e}"))?;

    validate_policy_catalog(&catalog)
        .map_err(|e| format!("Invalid model lifecycle policy JSON: {e}"))?;

    Ok(catalog)
}

fn validate_policy_catalog(catalog: &ModelLifecyclePolicyCatalog) -> Result<(), String> {
    for rule in &catalog.rules {
        if super::evaluator::parse_provider_type_label(&rule.provider_type).is_none() {
            return Err(format!("unknown provider_type `{}`", rule.provider_type));
        }

        if let Some(surface_id) = rule.surface_id.as_deref() {
            if canonical_provider_surface_id(surface_id).is_none() {
                return Err(format!("unknown surface_id `{surface_id}`"));
            }
        }

        if rule.model.trim().is_empty() {
            return Err("model lifecycle rule has empty `model`".to_string());
        }

        let warn_at = parse_utc_opt(rule.warn_at.as_deref()).map_err(|e| e.to_string())?;
        let block_at = parse_utc_opt(rule.block_at.as_deref()).map_err(|e| e.to_string())?;

        if let (Some(warn), Some(block)) = (warn_at, block_at) {
            if warn > block {
                return Err(format!(
                    "warn_at must be <= block_at for model `{}`",
                    rule.model
                ));
            }
        }
    }

    Ok(())
}

pub(super) fn parse_utc_opt(raw: Option<&str>) -> Result<Option<DateTime<Utc>>, CoreError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    let parsed = DateTime::parse_from_rfc3339(trimmed).map_err(|e| CoreError::Internal {
        code: crate::error_codes::InternalCode::Generic,
        message: format!("Invalid RFC3339 datetime in model lifecycle policy: `{trimmed}` ({e})"),
    })?;

    Ok(Some(parsed.with_timezone(&Utc)))
}
