//! Evaluation logic: `evaluate_model_lifecycle_at_for_surface` and helpers.

use chrono::{DateTime, Utc};

use crate::config::AiProviderType;
use crate::error::CoreError;
use crate::provider_surface::{canonical_provider_surface_id, provider_type_from_vendor_id};

use super::catalog::parse_utc_opt;
use super::messages::{build_block_message, build_warning_message};
use super::types::{ModelLifecycleAction, ModelLifecycleDecision, ModelLifecycleRule};

/// Evaluate a single lifecycle rule against a point-in-time `now`.
///
/// Extracted as `pub(crate)` so tests can exercise all action branches
/// without touching the static catalog.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn evaluate_rule_at(
    provider_type: AiProviderType,
    rule: &ModelLifecycleRule,
    now: DateTime<Utc>,
) -> Result<ModelLifecycleDecision, CoreError> {
    evaluate_rule_at_for_surface(provider_type, None, rule, now)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn evaluate_rule_at_for_surface(
    provider_type: AiProviderType,
    surface_id: Option<&str>,
    rule: &ModelLifecycleRule,
    now: DateTime<Utc>,
) -> Result<ModelLifecycleDecision, CoreError> {
    let trimmed_model = rule.model.trim();

    let warn_at = parse_utc_opt(rule.warn_at.as_deref())?;
    let block_at = parse_utc_opt(rule.block_at.as_deref())?;

    let warn_due = warn_at.is_some_and(|at| now >= at);
    let block_due = block_at.is_some_and(|at| now >= at);

    match rule.action {
        ModelLifecycleAction::WarnOnly => {
            if warn_due {
                Ok(ModelLifecycleDecision::Warn {
                    message: build_warning_message(
                        provider_type,
                        surface_id,
                        trimmed_model,
                        rule,
                        warn_at,
                    ),
                    replacement: rule.replacement.clone(),
                })
            } else {
                Ok(ModelLifecycleDecision::Allowed)
            }
        }
        ModelLifecycleAction::WarnThenBlock => {
            if block_due {
                Ok(ModelLifecycleDecision::Block {
                    message: build_block_message(
                        provider_type,
                        surface_id,
                        trimmed_model,
                        rule,
                        block_at,
                    ),
                    replacement: rule.replacement.clone(),
                })
            } else if warn_due {
                Ok(ModelLifecycleDecision::Warn {
                    message: build_warning_message(
                        provider_type,
                        surface_id,
                        trimmed_model,
                        rule,
                        warn_at,
                    ),
                    replacement: rule.replacement.clone(),
                })
            } else {
                Ok(ModelLifecycleDecision::Allowed)
            }
        }
        ModelLifecycleAction::Block => {
            let should_block = block_at.map(|at| now >= at).unwrap_or(true);
            if should_block {
                Ok(ModelLifecycleDecision::Block {
                    message: build_block_message(
                        provider_type,
                        surface_id,
                        trimmed_model,
                        rule,
                        block_at,
                    ),
                    replacement: rule.replacement.clone(),
                })
            } else {
                Ok(ModelLifecycleDecision::Allowed)
            }
        }
    }
}

/// Check whether a provider + optional surface_id matches a lifecycle rule.
pub(super) fn provider_rule_matches(
    provider_type: AiProviderType,
    surface_id: Option<&str>,
    rule: &ModelLifecycleRule,
) -> bool {
    if let Some(rule_surface_id) = rule.surface_id.as_deref() {
        let Some(expected_surface_id) = canonical_provider_surface_id(rule_surface_id) else {
            return false;
        };
        let Some(actual_surface_id) = surface_id.and_then(canonical_provider_surface_id) else {
            return false;
        };
        return expected_surface_id == actual_surface_id;
    }

    parse_provider_type_label(&rule.provider_type) == Some(provider_type)
}

pub(super) fn parse_provider_type_label(raw: &str) -> Option<AiProviderType> {
    provider_type_from_vendor_id(raw)
}
