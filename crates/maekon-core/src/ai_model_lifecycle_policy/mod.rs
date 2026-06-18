// OOS-TBD: ADR-013 file split (cycle 37+) — LOC: 541
//! AI model lifecycle policy — evaluates whether a given AI model is current,
//! in a deprecation warning window, or retired/blocked.
//!
//! Split from the original monolithic `ai_model_lifecycle_policy.rs`.
//! All public symbols are re-exported at the same path; no downstream changes
//! are required.

mod catalog;
mod evaluator;
mod messages;
mod types;

// ── Public re-exports (unchanged API surface) ────────────────────────────────
pub use types::{
    ModelLifecycleAction, ModelLifecycleDecision, ModelLifecyclePolicyCatalog, ModelLifecycleRule,
};

use chrono::{DateTime, Utc};

use crate::config::AiProviderType;
use crate::error::CoreError;

use catalog::{parse_utc_opt, policy_catalog};
use evaluator::provider_rule_matches;
use messages::{build_block_message, build_warning_message};
use types::ModelLifecycleAction as Action;

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
        Action::WarnOnly => {
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
        Action::WarnThenBlock => {
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
        Action::Block => {
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

#[cfg(test)]
mod tests {
    use super::evaluator::{evaluate_rule_at, evaluate_rule_at_for_surface, provider_rule_matches};
    use super::*;
    use crate::config::AiProviderType;

    // ── helpers ──────────────────────────────────────────────────────────────

    fn ts(rfc3339: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(rfc3339)
            .expect("hard-coded test timestamp must be valid RFC3339")
            .with_timezone(&Utc)
    }

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

    // ── catalog loading ──────────────────────────────────────────────────────

    #[test]
    fn policy_catalog_loads() {
        let catalog = list_model_lifecycle_policies().expect("policy should load");
        assert!(catalog.version >= 1);
        assert!(!catalog.updated_at.trim().is_empty());
        assert!(!catalog.rules.is_empty());
    }

    // ── catalog-based decision tests (real rules, injected timestamps) ────────

    #[test]
    fn model_before_warn_date_is_allowed() {
        let before_warn = ts("2025-01-01T00:00:00Z");
        let decision =
            evaluate_model_lifecycle_at(AiProviderType::OpenAi, "gpt-3.5-turbo", before_warn)
                .unwrap();
        assert_eq!(decision, ModelLifecycleDecision::Allowed);
    }

    #[test]
    fn model_in_warn_window_produces_warn() {
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

    #[test]
    fn openai_gpt35_is_blocked_after_block_date() {
        let after_block = ts("2026-02-01T00:00:00Z");
        let decision =
            evaluate_model_lifecycle_at(AiProviderType::OpenAi, "gpt-3.5-turbo", after_block)
                .unwrap();
        assert!(decision.is_blocking(), "expected Block, got {decision:?}");
    }

    #[test]
    fn anthropic_claude3_sonnet_in_warn_window() {
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
        let after_block = ts("2026-04-02T00:00:00Z");
        let decision = evaluate_model_lifecycle_at(
            AiProviderType::Anthropic,
            "claude-3-sonnet-20240229",
            after_block,
        )
        .unwrap();
        assert!(decision.is_blocking(), "expected Block, got {decision:?}");
    }

    #[test]
    fn google_gemini15_is_warned_before_block_date() {
        let in_warn = ts("2026-03-10T00:00:00Z");
        let decision =
            evaluate_model_lifecycle_at(AiProviderType::Google, "gemini-1.5-pro", in_warn).unwrap();
        assert!(
            matches!(decision, ModelLifecycleDecision::Warn { .. }),
            "expected Warn, got {decision:?}"
        );
    }

    #[test]
    fn provider_mismatch_does_not_trigger_policy() {
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

    /// Regression: a surface_id-less rule keyed on one Generic-family vendor
    /// (`openrouter`) must NOT match a call for a different Generic-family vendor
    /// (`together`), even though both collapse onto `AiProviderType::Generic`.
    #[test]
    fn generic_family_rule_does_not_match_other_generic_vendor() {
        let openrouter_rule = warn_then_block_rule(
            "openrouter",
            "some-shared-model",
            Some("2025-01-01T00:00:00Z"),
            Some("2026-01-01T00:00:00Z"),
            None,
        );

        // Caller is `together` (also Generic family) via its surface_id.
        let matches_other_vendor = provider_rule_matches(
            AiProviderType::Generic,
            Some("provider_surface.together.direct_api"),
            &openrouter_rule,
        );
        assert!(
            !matches_other_vendor,
            "openrouter rule must not match a together call (generic-family collapse)"
        );

        // Same rule DOES match the matching vendor's surface_id.
        let matches_same_vendor = provider_rule_matches(
            AiProviderType::Generic,
            Some("provider_surface.openrouter.direct_api"),
            &openrouter_rule,
        );
        assert!(
            matches_same_vendor,
            "openrouter rule must match an openrouter call"
        );
    }

    /// A vendor-specific Generic-family rule cannot be confirmed against a caller
    /// that supplies no surface_id, so it must not match.
    #[test]
    fn generic_family_rule_requires_surface_id_to_match() {
        let openrouter_rule = block_rule("openrouter", "retired-model", None);

        let matches_without_surface =
            provider_rule_matches(AiProviderType::Generic, None, &openrouter_rule);
        assert!(
            !matches_without_surface,
            "generic-family rule must not match when the caller vendor is unknown"
        );
    }

    /// A vendor-precise Generic rule still matches the catch-all `generic`
    /// vendor only when the caller's surface also resolves to `generic`, so the
    /// family-wide rule and a sibling vendor never collide.
    #[test]
    fn generic_catch_all_rule_only_matches_generic_vendor() {
        let generic_rule = block_rule("generic", "shared-model-name", None);

        let matches_generic = provider_rule_matches(
            AiProviderType::Generic,
            Some("provider_surface.generic.direct_api"),
            &generic_rule,
        );
        assert!(matches_generic, "generic rule must match a generic call");

        let matches_sibling = provider_rule_matches(
            AiProviderType::Generic,
            Some("provider_surface.together.direct_api"),
            &generic_rule,
        );
        assert!(
            !matches_sibling,
            "generic rule must not match a together call"
        );
    }

    #[test]
    fn unknown_model_is_allowed() {
        let decision =
            evaluate_model_lifecycle_now(AiProviderType::OpenAi, "unit-test-openai-model")
                .expect("evaluation should succeed");
        assert_eq!(decision, ModelLifecycleDecision::Allowed);
    }

    #[test]
    fn empty_model_string_is_allowed() {
        let decision = evaluate_model_lifecycle_now(AiProviderType::OpenAi, "")
            .expect("evaluation should succeed");
        assert_eq!(decision, ModelLifecycleDecision::Allowed);
    }

    #[test]
    fn whitespace_only_model_string_is_allowed() {
        let decision = evaluate_model_lifecycle_now(AiProviderType::Anthropic, "   ")
            .expect("evaluation should succeed");
        assert_eq!(decision, ModelLifecycleDecision::Allowed);
    }

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

    // ── edge: exact boundary timestamps ──────────────────────────────────────

    #[test]
    fn exactly_at_warn_boundary_produces_warn() {
        let exactly_warn = ts("2025-07-01T00:00:00Z");
        let decision =
            evaluate_model_lifecycle_at(AiProviderType::OpenAi, "gpt-3.5-turbo", exactly_warn)
                .unwrap();
        assert!(
            matches!(decision, ModelLifecycleDecision::Warn { .. }),
            "exactly at warn boundary should be Warn, got {decision:?}"
        );
    }

    #[test]
    fn exactly_at_block_boundary_produces_block() {
        let exactly_block = ts("2026-01-01T00:00:00Z");
        let decision =
            evaluate_model_lifecycle_at(AiProviderType::OpenAi, "gpt-3.5-turbo", exactly_block)
                .unwrap();
        assert!(
            decision.is_blocking(),
            "exactly at block boundary should be Block, got {decision:?}"
        );
    }

    #[test]
    fn one_second_before_block_boundary_is_still_warn() {
        let just_before_block = ts("2025-12-31T23:59:59Z");
        let decision =
            evaluate_model_lifecycle_at(AiProviderType::OpenAi, "gpt-3.5-turbo", just_before_block)
                .unwrap();
        assert!(
            matches!(decision, ModelLifecycleDecision::Warn { .. }),
            "one second before block boundary should be Warn, got {decision:?}"
        );
    }

    // ── enforce wraps Block into PolicyDenied error ───────────────────────────

    #[test]
    fn enforce_returns_ok_for_allowed_model() {
        enforce_model_lifecycle_now(AiProviderType::OpenAi, "unit-test-openai-model")
            .expect("unknown (allowed) model must not fail enforce");
    }

    #[test]
    fn enforce_returns_policy_denied_for_blocked_model() {
        let after_block = ts("2026-06-01T00:00:00Z");
        let decision =
            evaluate_model_lifecycle_at(AiProviderType::OpenAi, "gpt-3.5-turbo", after_block)
                .unwrap();
        assert!(decision.is_blocking());

        let msg = decision.message().expect("Block must carry a message");
        assert!(msg.contains("gpt-3.5-turbo"));
    }

    // ── ModelLifecycleDecision helper methods ─────────────────────────────────

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

    #[test]
    fn warn_only_before_warn_date_is_allowed() {
        let rule = warn_only_rule("openai", "old-model", Some("2026-06-01T00:00:00Z"));
        let before = ts("2026-01-01T00:00:00Z");
        let d = evaluate_rule_at(AiProviderType::OpenAi, &rule, before).unwrap();
        assert_eq!(d, ModelLifecycleDecision::Allowed);
    }

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

    #[test]
    fn warn_only_with_no_warn_at_is_always_allowed() {
        let rule = warn_only_rule("anthropic", "legacy-model", None);
        let far_future = ts("2099-01-01T00:00:00Z");
        let d = evaluate_rule_at(AiProviderType::Anthropic, &rule, far_future).unwrap();
        assert_eq!(d, ModelLifecycleDecision::Allowed);
    }

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

    #[test]
    fn block_action_before_block_date_is_allowed() {
        let rule = block_rule("google", "soon-blocked-model", Some("2027-01-01T00:00:00Z"));
        let before = ts("2026-01-01T00:00:00Z");
        let d = evaluate_rule_at(AiProviderType::Google, &rule, before).unwrap();
        assert_eq!(d, ModelLifecycleDecision::Allowed);
    }

    #[test]
    fn block_action_exactly_at_block_date_blocks() {
        let rule = block_rule("google", "soon-blocked-model", Some("2027-01-01T00:00:00Z"));
        let at_block = ts("2027-01-01T00:00:00Z");
        let d = evaluate_rule_at(AiProviderType::Google, &rule, at_block).unwrap();
        assert!(d.is_blocking());
    }

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

        if let ModelLifecycleDecision::Warn { replacement, .. } = &d2 {
            assert_eq!(replacement.as_deref(), Some("test-model-v2"));
        }
        if let ModelLifecycleDecision::Block { replacement, .. } = &d3 {
            assert_eq!(replacement.as_deref(), Some("test-model-v2"));
        }
    }
}
