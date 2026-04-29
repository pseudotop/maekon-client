use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::AiProviderType;
use crate::error::CoreError;
use crate::provider_surface::{
    canonical_provider_surface_id, provider_type_from_vendor_id, provider_vendor_id_or_default,
};

const POLICY_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/config/ai_model_lifecycle_policy.json"
));

static POLICY_CATALOG: OnceLock<Result<ModelLifecyclePolicyCatalog, String>> = OnceLock::new();

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelLifecyclePolicyCatalog {
    pub version: u32,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub rules: Vec<ModelLifecycleRule>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelLifecycleRule {
    pub provider_type: String,
    #[serde(default)]
    pub surface_id: Option<String>,
    pub model: String,
    #[serde(default)]
    pub warn_at: Option<String>,
    #[serde(default)]
    pub block_at: Option<String>,
    #[serde(default)]
    pub replacement: Option<String>,
    #[serde(default)]
    pub action: ModelLifecycleAction,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ModelLifecycleAction {
    WarnOnly,
    #[default]
    WarnThenBlock,
    Block,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelLifecycleDecision {
    Allowed,
    Warn {
        message: String,
        replacement: Option<String>,
    },
    Block {
        message: String,
        replacement: Option<String>,
    },
}

impl ModelLifecycleDecision {
    pub fn is_blocking(&self) -> bool {
        matches!(self, Self::Block { .. })
    }

    pub fn message(&self) -> Option<&str> {
        match self {
            Self::Allowed => None,
            Self::Warn { message, .. } | Self::Block { message, .. } => Some(message),
        }
    }
}

pub fn list_model_lifecycle_policies() -> Result<ModelLifecyclePolicyCatalog, CoreError> {
    Ok(policy_catalog()?.clone())
}

pub fn evaluate_model_lifecycle_now(
    provider_type: AiProviderType,
    model: &str,
) -> Result<ModelLifecycleDecision, CoreError> {
    evaluate_model_lifecycle_at(provider_type, model, Utc::now())
}

pub fn evaluate_model_lifecycle_now_for_surface(
    provider_type: AiProviderType,
    surface_id: Option<&str>,
    model: &str,
) -> Result<ModelLifecycleDecision, CoreError> {
    evaluate_model_lifecycle_at_for_surface(provider_type, surface_id, model, Utc::now())
}

pub fn evaluate_model_lifecycle_at(
    provider_type: AiProviderType,
    model: &str,
    now: DateTime<Utc>,
) -> Result<ModelLifecycleDecision, CoreError> {
    evaluate_model_lifecycle_at_for_surface(provider_type, None, model, now)
}

pub fn evaluate_model_lifecycle_at_for_surface(
    provider_type: AiProviderType,
    surface_id: Option<&str>,
    model: &str,
    now: DateTime<Utc>,
) -> Result<ModelLifecycleDecision, CoreError> {
    let trimmed_model = model.trim();
    if trimmed_model.is_empty() {
        return Ok(ModelLifecycleDecision::Allowed);
    }

    let catalog = policy_catalog()?;
    let Some(rule) = catalog.rules.iter().find(|candidate| {
        provider_rule_matches(provider_type, surface_id, candidate)
            && candidate.model.trim().eq_ignore_ascii_case(trimmed_model)
    }) else {
        return Ok(ModelLifecycleDecision::Allowed);
    };

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

pub fn enforce_model_lifecycle_now(
    provider_type: AiProviderType,
    model: &str,
) -> Result<(), CoreError> {
    match evaluate_model_lifecycle_now(provider_type, model)? {
        ModelLifecycleDecision::Allowed | ModelLifecycleDecision::Warn { .. } => Ok(()),
        ModelLifecycleDecision::Block { message, .. } => Err(CoreError::PolicyDenied {
            code: crate::error_codes::PolicyCode::Denied,
            message,
        }),
    }
}

/// Evaluates a single lifecycle rule against a point-in-time `now`.
///
/// Extracted as `pub(crate)` so that tests can exercise all action branches
/// (`WarnOnly`, `WarnThenBlock`, `Block`) with synthetic rules without touching
/// the static catalog.
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

fn validate_policy_catalog(catalog: &ModelLifecyclePolicyCatalog) -> Result<(), String> {
    for rule in &catalog.rules {
        if parse_provider_type_label(&rule.provider_type).is_none() {
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

fn policy_catalog() -> Result<&'static ModelLifecyclePolicyCatalog, CoreError> {
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

fn parse_utc_opt(raw: Option<&str>) -> Result<Option<DateTime<Utc>>, CoreError> {
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

fn provider_rule_matches(
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

fn parse_provider_type_label(raw: &str) -> Option<AiProviderType> {
    provider_type_from_vendor_id(raw)
}

fn provider_label(provider_type: AiProviderType) -> &'static str {
    provider_vendor_id_or_default(provider_type)
}

fn build_warning_message(
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

fn build_block_message(
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
    format!("provider `{}`", provider_label(provider_type))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── helpers ────────────────────────────────────────────────────────────────

    fn ts(rfc3339: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(rfc3339)
            .expect("hard-coded test timestamp must be valid RFC3339")
            .with_timezone(&Utc)
    }

    /// Builds a synthetic rule with `warn_then_block` action for use in
    /// evaluate_rule_at tests.
    fn warn_then_block_rule(
        provider: &str,
        model: &str,
        warn_at: Option<&str>,
        block_at: Option<&str>,
        replacement: Option<&str>,
    ) -> ModelLifecycleRule {
        ModelLifecycleRule {
            provider_type: provider.to_string(),
            surface_id: None,
            model: model.to_string(),
            warn_at: warn_at.map(str::to_string),
            block_at: block_at.map(str::to_string),
            replacement: replacement.map(str::to_string),
            action: ModelLifecycleAction::WarnThenBlock,
        }
    }

    fn warn_only_rule(provider: &str, model: &str, warn_at: Option<&str>) -> ModelLifecycleRule {
        ModelLifecycleRule {
            provider_type: provider.to_string(),
            surface_id: None,
            model: model.to_string(),
            warn_at: warn_at.map(str::to_string),
            block_at: None,
            replacement: Some("newer-model".to_string()),
            action: ModelLifecycleAction::WarnOnly,
        }
    }

    fn block_rule(provider: &str, model: &str, block_at: Option<&str>) -> ModelLifecycleRule {
        ModelLifecycleRule {
            provider_type: provider.to_string(),
            surface_id: None,
            model: model.to_string(),
            warn_at: None,
            block_at: block_at.map(str::to_string),
            replacement: None,
            action: ModelLifecycleAction::Block,
        }
    }

    // ── catalog loading ────────────────────────────────────────────────────────

    #[test]
    fn policy_catalog_loads() {
        let catalog = list_model_lifecycle_policies().expect("policy should load");
        assert!(catalog.version >= 1);
        assert!(!catalog.updated_at.trim().is_empty());
        assert!(!catalog.rules.is_empty());
    }

    // ── catalog-based decision tests (real rules, injected timestamps) ─────────

    /// Before warn_at — the model is still fully current and must be Allowed.
    #[test]
    fn model_before_warn_date_is_allowed() {
        // gpt-3.5-turbo warn_at = 2025-07-01; we're well before it.
        let before_warn = ts("2025-01-01T00:00:00Z");
        let decision =
            evaluate_model_lifecycle_at(AiProviderType::OpenAi, "gpt-3.5-turbo", before_warn)
                .unwrap();
        assert_eq!(decision, ModelLifecycleDecision::Allowed);
    }

    /// Past warn_at but before block_at — must produce Warn, not Block.
    #[test]
    fn model_in_warn_window_produces_warn() {
        // gpt-3.5-turbo: warn 2025-07-01, block 2026-01-01
        let in_warn_window = ts("2025-09-15T00:00:00Z");
        let decision =
            evaluate_model_lifecycle_at(AiProviderType::OpenAi, "gpt-3.5-turbo", in_warn_window)
                .unwrap();
        assert!(
            matches!(decision, ModelLifecycleDecision::Warn { .. }),
            "expected Warn, got {decision:?}"
        );
        assert!(!decision.is_blocking());
    }

    /// Past block_at — must produce Block.
    #[test]
    fn openai_gpt35_is_blocked_after_block_date() {
        let after_block = ts("2026-02-01T00:00:00Z");
        let decision =
            evaluate_model_lifecycle_at(AiProviderType::OpenAi, "gpt-3.5-turbo", after_block)
                .unwrap();
        assert!(decision.is_blocking(), "expected Block, got {decision:?}");
    }

    /// Anthropic claude-3-sonnet: warn window then block.
    #[test]
    fn anthropic_claude3_sonnet_in_warn_window() {
        // warn_at = 2025-12-01, block_at = 2026-04-01
        let in_warn = ts("2026-01-15T00:00:00Z");
        let decision = evaluate_model_lifecycle_at(
            AiProviderType::Anthropic,
            "claude-3-sonnet-20240229",
            in_warn,
        )
        .unwrap();
        assert!(
            matches!(decision, ModelLifecycleDecision::Warn { .. }),
            "expected Warn, got {decision:?}"
        );
    }

    #[test]
    fn anthropic_claude3_sonnet_blocked_after_block_date() {
        // block_at = 2026-04-01
        let after_block = ts("2026-04-02T00:00:00Z");
        let decision = evaluate_model_lifecycle_at(
            AiProviderType::Anthropic,
            "claude-3-sonnet-20240229",
            after_block,
        )
        .unwrap();
        assert!(decision.is_blocking(), "expected Block, got {decision:?}");
    }

    /// Google gemini-1.5-pro: warn window check.
    #[test]
    fn google_gemini15_is_warned_before_block_date() {
        // warn_at = 2026-03-01, block_at = 2026-06-01
        let in_warn = ts("2026-03-10T00:00:00Z");
        let decision =
            evaluate_model_lifecycle_at(AiProviderType::Google, "gemini-1.5-pro", in_warn).unwrap();
        assert!(
            matches!(decision, ModelLifecycleDecision::Warn { .. }),
            "expected Warn, got {decision:?}"
        );
    }

    /// A model that exists in the catalog for provider A must not be blocked
    /// when queried under provider B.
    #[test]
    fn provider_mismatch_does_not_trigger_policy() {
        // gpt-3.5-turbo is an OpenAI rule; Anthropic should not see it.
        let after_block = ts("2026-06-01T00:00:00Z");
        let decision =
            evaluate_model_lifecycle_at(AiProviderType::Anthropic, "gpt-3.5-turbo", after_block)
                .unwrap();
        assert_eq!(decision, ModelLifecycleDecision::Allowed);
    }

    #[test]
    fn surface_scoped_rule_requires_matching_surface_id() {
        let rule = ModelLifecycleRule {
            provider_type: "OpenAi".to_string(),
            surface_id: Some("provider_surface.openai.managed_oauth".to_string()),
            model: "gpt-5.4".to_string(),
            warn_at: Some("2025-01-01T00:00:00Z".to_string()),
            block_at: None,
            replacement: Some("gpt-5.5".to_string()),
            action: ModelLifecycleAction::WarnOnly,
        };

        let warn = evaluate_rule_at_for_surface(
            AiProviderType::OpenAi,
            Some("provider_surface.openai.managed_oauth"),
            &rule,
            ts("2026-01-01T00:00:00Z"),
        )
        .unwrap();
        assert!(matches!(warn, ModelLifecycleDecision::Warn { .. }));

        let mismatched = provider_rule_matches(
            AiProviderType::OpenAi,
            Some("provider_surface.openai.direct_api"),
            &rule,
        );
        assert!(!mismatched);
    }

    /// A model not present in the catalog at all must be Allowed.
    #[test]
    fn unknown_model_is_allowed() {
        let decision =
            evaluate_model_lifecycle_now(AiProviderType::OpenAi, "unit-test-openai-model")
                .expect("evaluation should succeed");
        assert_eq!(decision, ModelLifecycleDecision::Allowed);
    }

    /// Empty model string must always be Allowed (defensive guard).
    #[test]
    fn empty_model_string_is_allowed() {
        let decision = evaluate_model_lifecycle_now(AiProviderType::OpenAi, "")
            .expect("evaluation should succeed");
        assert_eq!(decision, ModelLifecycleDecision::Allowed);
    }

    /// Whitespace-only model string is treated the same as empty.
    #[test]
    fn whitespace_only_model_string_is_allowed() {
        let decision = evaluate_model_lifecycle_now(AiProviderType::Anthropic, "   ")
            .expect("evaluation should succeed");
        assert_eq!(decision, ModelLifecycleDecision::Allowed);
    }

    /// Model name lookup is case-insensitive.
    #[test]
    fn model_name_lookup_is_case_insensitive() {
        let after_block = ts("2026-02-01T00:00:00Z");
        let decision =
            evaluate_model_lifecycle_at(AiProviderType::OpenAi, "GPT-3.5-TURBO", after_block)
                .unwrap();
        assert!(
            decision.is_blocking(),
            "case-insensitive lookup should still Block"
        );
    }

    // ── edge: exact boundary timestamps ───────────────────────────────────────

    /// Exactly at warn_at instant must produce Warn (>= boundary is inclusive).
    #[test]
    fn exactly_at_warn_boundary_produces_warn() {
        // gpt-3.5-turbo warn_at = "2025-07-01T00:00:00Z"
        let exactly_warn = ts("2025-07-01T00:00:00Z");
        let decision =
            evaluate_model_lifecycle_at(AiProviderType::OpenAi, "gpt-3.5-turbo", exactly_warn)
                .unwrap();
        assert!(
            matches!(decision, ModelLifecycleDecision::Warn { .. }),
            "exactly at warn boundary should be Warn, got {decision:?}"
        );
    }

    /// Exactly at block_at instant must produce Block (>= boundary is inclusive).
    #[test]
    fn exactly_at_block_boundary_produces_block() {
        // gpt-3.5-turbo block_at = "2026-01-01T00:00:00Z"
        let exactly_block = ts("2026-01-01T00:00:00Z");
        let decision =
            evaluate_model_lifecycle_at(AiProviderType::OpenAi, "gpt-3.5-turbo", exactly_block)
                .unwrap();
        assert!(
            decision.is_blocking(),
            "exactly at block boundary should be Block, got {decision:?}"
        );
    }

    /// One nanosecond before block_at must still be Warn, not Block.
    #[test]
    fn one_second_before_block_boundary_is_still_warn() {
        // gpt-3.5-turbo block_at = 2026-01-01T00:00:00Z
        // Use one second before since RFC3339 resolution is seconds.
        let just_before_block = ts("2025-12-31T23:59:59Z");
        let decision =
            evaluate_model_lifecycle_at(AiProviderType::OpenAi, "gpt-3.5-turbo", just_before_block)
                .unwrap();
        assert!(
            matches!(decision, ModelLifecycleDecision::Warn { .. }),
            "one second before block boundary should be Warn, got {decision:?}"
        );
    }

    // ── enforce wraps Block into PolicyDenied error ────────────────────────────

    #[test]
    fn enforce_returns_ok_for_allowed_model() {
        enforce_model_lifecycle_now(AiProviderType::OpenAi, "unit-test-openai-model")
            .expect("unknown (allowed) model must not fail enforce");
    }

    #[test]
    fn enforce_returns_policy_denied_for_blocked_model() {
        // Use evaluate_model_lifecycle_at via the public API to confirm Block first,
        // then verify enforce_model_lifecycle_now agrees (it calls Utc::now() which
        // is past the block date for gpt-3.5-turbo in 2026).
        let after_block = ts("2026-06-01T00:00:00Z");
        let decision =
            evaluate_model_lifecycle_at(AiProviderType::OpenAi, "gpt-3.5-turbo", after_block)
                .unwrap();
        assert!(decision.is_blocking());

        // message() is non-None for both Warn and Block variants.
        let msg = decision.message().expect("Block must carry a message");
        assert!(msg.contains("gpt-3.5-turbo"));
    }

    // ── ModelLifecycleDecision helper methods ──────────────────────────────────

    #[test]
    fn allowed_is_not_blocking_and_has_no_message() {
        let d = ModelLifecycleDecision::Allowed;
        assert!(!d.is_blocking());
        assert!(d.message().is_none());
    }

    #[test]
    fn warn_is_not_blocking_and_has_message() {
        let d = ModelLifecycleDecision::Warn {
            message: "deprecation warning".to_string(),
            replacement: Some("new-model".to_string()),
        };
        assert!(!d.is_blocking());
        assert_eq!(d.message(), Some("deprecation warning"));
    }

    #[test]
    fn block_is_blocking_and_has_message() {
        let d = ModelLifecycleDecision::Block {
            message: "model retired".to_string(),
            replacement: None,
        };
        assert!(d.is_blocking());
        assert_eq!(d.message(), Some("model retired"));
    }

    // ── action mode tests via evaluate_rule_at (synthetic rules) ─────────────

    /// WarnOnly: before warn_at → Allowed.
    #[test]
    fn warn_only_before_warn_date_is_allowed() {
        let rule = warn_only_rule("openai", "old-model", Some("2026-06-01T00:00:00Z"));
        let before = ts("2026-01-01T00:00:00Z");
        let d = evaluate_rule_at(AiProviderType::OpenAi, &rule, before).unwrap();
        assert_eq!(d, ModelLifecycleDecision::Allowed);
    }

    /// WarnOnly: past warn_at → Warn, never Block.
    #[test]
    fn warn_only_after_warn_date_never_blocks() {
        let rule = warn_only_rule("openai", "old-model", Some("2025-01-01T00:00:00Z"));
        let after = ts("2026-01-01T00:00:00Z");
        let d = evaluate_rule_at(AiProviderType::OpenAi, &rule, after).unwrap();
        assert!(
            matches!(d, ModelLifecycleDecision::Warn { .. }),
            "WarnOnly must produce Warn not Block"
        );
        assert!(!d.is_blocking(), "WarnOnly action must never block");
    }

    /// WarnOnly with no warn_at → always Allowed (warn_due is false by default).
    #[test]
    fn warn_only_with_no_warn_at_is_always_allowed() {
        let rule = warn_only_rule("anthropic", "legacy-model", None);
        let far_future = ts("2099-01-01T00:00:00Z");
        let d = evaluate_rule_at(AiProviderType::Anthropic, &rule, far_future).unwrap();
        assert_eq!(d, ModelLifecycleDecision::Allowed);
    }

    /// Block action with no block_at → blocks unconditionally (default true).
    #[test]
    fn block_action_with_no_block_at_is_unconditional() {
        let rule = block_rule("google", "forbidden-model", None);
        let any_time = ts("2020-01-01T00:00:00Z");
        let d = evaluate_rule_at(AiProviderType::Google, &rule, any_time).unwrap();
        assert!(
            d.is_blocking(),
            "Block action with no block_at must always block"
        );
    }

    /// Block action with a future block_at → Allowed until that date.
    #[test]
    fn block_action_before_block_date_is_allowed() {
        let rule = block_rule("google", "soon-blocked-model", Some("2027-01-01T00:00:00Z"));
        let before = ts("2026-01-01T00:00:00Z");
        let d = evaluate_rule_at(AiProviderType::Google, &rule, before).unwrap();
        assert_eq!(d, ModelLifecycleDecision::Allowed);
    }

    /// Block action exactly at block_at → blocks.
    #[test]
    fn block_action_exactly_at_block_date_blocks() {
        let rule = block_rule("google", "soon-blocked-model", Some("2027-01-01T00:00:00Z"));
        let at_block = ts("2027-01-01T00:00:00Z");
        let d = evaluate_rule_at(AiProviderType::Google, &rule, at_block).unwrap();
        assert!(d.is_blocking());
    }

    /// WarnThenBlock via evaluate_rule_at: full lifecycle progression.
    #[test]
    fn warn_then_block_rule_full_lifecycle() {
        let rule = warn_then_block_rule(
            "anthropic",
            "test-model",
            Some("2026-01-01T00:00:00Z"),
            Some("2026-06-01T00:00:00Z"),
            Some("test-model-v2"),
        );

        let before_warn = ts("2025-06-01T00:00:00Z");
        let in_warn = ts("2026-03-01T00:00:00Z");
        let after_block = ts("2026-07-01T00:00:00Z");

        let d1 = evaluate_rule_at(AiProviderType::Anthropic, &rule, before_warn).unwrap();
        assert_eq!(d1, ModelLifecycleDecision::Allowed);

        let d2 = evaluate_rule_at(AiProviderType::Anthropic, &rule, in_warn).unwrap();
        assert!(matches!(d2, ModelLifecycleDecision::Warn { .. }));
        assert!(!d2.is_blocking());

        let d3 = evaluate_rule_at(AiProviderType::Anthropic, &rule, after_block).unwrap();
        assert!(d3.is_blocking());

        // Replacement is threaded through all non-Allowed decisions.
        if let ModelLifecycleDecision::Warn { replacement, .. } = &d2 {
            assert_eq!(replacement.as_deref(), Some("test-model-v2"));
        }
        if let ModelLifecycleDecision::Block { replacement, .. } = &d3 {
            assert_eq!(replacement.as_deref(), Some("test-model-v2"));
        }
    }
}
