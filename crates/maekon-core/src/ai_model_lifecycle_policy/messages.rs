//! Warning and block message builders for lifecycle decisions.

use chrono::{DateTime, Utc};

use crate::config::AiProviderType;
use crate::provider_surface::{canonical_provider_surface_id, provider_vendor_id_or_default};

use super::types::ModelLifecycleRule;

pub(super) fn build_warning_message(
    provider_type: AiProviderType,
    surface_id: Option<&str>,
    model: &str,
    rule: &ModelLifecycleRule,
    warn_at: Option<DateTime<Utc>>,
) -> String {
    let replacement = rule
        .replacement
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let replacement_msg = replacement
        .map(|value| format!(" Use `{value}` instead."))
        .unwrap_or_default();

    if let Some(warn_at) = warn_at {
        format!(
            "Model `{model}` for {} is in deprecation window since {}.{}",
            lifecycle_subject_label(provider_type, surface_id, rule),
            warn_at.to_rfc3339(),
            replacement_msg,
        )
    } else {
        format!(
            "Model `{model}` for {} is deprecated.{}",
            lifecycle_subject_label(provider_type, surface_id, rule),
            replacement_msg,
        )
    }
}

pub(super) fn build_block_message(
    provider_type: AiProviderType,
    surface_id: Option<&str>,
    model: &str,
    rule: &ModelLifecycleRule,
    block_at: Option<DateTime<Utc>>,
) -> String {
    let replacement = rule
        .replacement
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let replacement_msg = replacement
        .map(|value| format!(" Use `{value}` instead."))
        .unwrap_or_default();

    if let Some(block_at) = block_at {
        format!(
            "Model `{model}` for {} is retired as of {}.{}",
            lifecycle_subject_label(provider_type, surface_id, rule),
            block_at.to_rfc3339(),
            replacement_msg,
        )
    } else {
        format!(
            "Model `{model}` for {} is blocked by lifecycle policy.{}",
            lifecycle_subject_label(provider_type, surface_id, rule),
            replacement_msg,
        )
    }
}

fn lifecycle_subject_label(
    provider_type: AiProviderType,
    surface_id: Option<&str>,
    rule: &ModelLifecycleRule,
) -> String {
    if let Some(rule_surface_id) = rule.surface_id.as_deref() {
        if let Some(canonical) = canonical_provider_surface_id(rule_surface_id) {
            return format!("provider surface `{canonical}`");
        }
    }
    if let Some(surface_id) = surface_id.and_then(canonical_provider_surface_id) {
        return format!("provider surface `{surface_id}`");
    }
    format!(
        "provider `{}`",
        provider_vendor_id_or_default(provider_type)
    )
}
