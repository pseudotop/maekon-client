//! LLM provider port — defines the contract for interpreting user intent
//! from screen context via a remote large language model.
//! Implemented by `RemoteLlmProvider` in `maekon-network`.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU8, Ordering as AtomicOrdering};

use crate::error::CoreError;
use crate::models::intent_execution::{
    IntentCapabilityObservation, IntentConfigurationReason, IntentExecutionObserver,
    IntentProviderClass, IntentSkillContextKind, IntentSkillUse,
};
use crate::models::prompt_assembly::TrustedInstruction;
use crate::models::skill::SkillMeta;

// Tri-state constants for LlmCallHealth.
const LLM_HEALTH_UNKNOWN: u8 = 0;
const LLM_HEALTH_OK: u8 = 1;
const LLM_HEALTH_FAILED: u8 = 2;

/// Per-call LLM health observable for the automation intent path.
///
/// Lives in `maekon-core` so that adapter crates (`maekon-network` as writer,
/// `maekon-web` as reader) do not need an adapter-to-adapter dependency, which
/// is forbidden by the Hexagonal Architecture rules of this workspace.
///
/// Tri-state semantics via `AtomicU8`:
/// - UNKNOWN (0): default; no call attempted yet, or handle not wired.
/// - OK      (1): last LLM call completed with a successful response.
/// - FAILED  (2): last LLM call returned a network/status/parse error.
///
/// Exposed as `Option<bool>` via `as_option_bool()`:
/// - `None`        — UNKNOWN
/// - `Some(true)`  — OK
/// - `Some(false)` — FAILED
pub struct LlmCallHealth(AtomicU8);

impl Default for LlmCallHealth {
    fn default() -> Self {
        Self(AtomicU8::new(LLM_HEALTH_UNKNOWN))
    }
}

impl LlmCallHealth {
    /// Mark the last call as successful.
    pub fn record_ok(&self) {
        self.0.store(LLM_HEALTH_OK, AtomicOrdering::Relaxed);
    }

    /// Mark the last call as failed.
    pub fn record_failed(&self) {
        self.0.store(LLM_HEALTH_FAILED, AtomicOrdering::Relaxed);
    }

    /// Convert to `Option<bool>`:
    /// - `None`         — UNKNOWN (no call yet / not wired)
    /// - `Some(true)`   — last call succeeded
    /// - `Some(false)`  — last call failed
    pub fn as_option_bool(&self) -> Option<bool> {
        match self.0.load(AtomicOrdering::Relaxed) {
            LLM_HEALTH_OK => Some(true),
            LLM_HEALTH_FAILED => Some(false),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenContext {
    pub visible_texts: Vec<String>,
    pub active_app: String,
    pub active_window_title: String,
    pub layout_description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterpretedAction {
    pub target_text: Option<String>,
    pub target_role: Option<String>,
    pub action_type: String,
    pub confidence: f64,
}

/// Optional skill context injected into the system prompt.
#[derive(Debug, Clone, Default)]
pub struct SkillContext {
    /// Available skill summaries (progressive disclosure — names only).
    pub available_skills: Vec<SkillMeta>,
    /// The activated skill, if one is cleared for instruction position.
    ///
    /// Typed as [`TrustedInstruction`] rather than `Option<String>` on purpose
    /// (#8588): a `String` field here would let any caller — including one
    /// holding a Slack message or a scraped document — put arbitrary text into
    /// the system prompt. `TrustedInstruction` can only be built from a verified
    /// Skill Pack activation, so that assignment does not compile.
    pub active_skill: Option<TrustedInstruction>,
}

impl SkillContext {
    pub fn observation_kind(&self) -> IntentSkillContextKind {
        if self.active_skill.is_some() {
            IntentSkillContextKind::Activated
        } else if !self.available_skills.is_empty() {
            IntentSkillContextKind::Catalog
        } else {
            IntentSkillContextKind::None
        }
    }
}

/// LLM provider port — interprets user intent from screen context.
///
/// # Errors
/// - `CoreError::Analysis` (wire: `provider.analysis_failed`) for LLM-side
///   failures: empty response, non-parseable intent output, malformed JSON.
/// - HTTP-layer failures follow the canonical semantic status mapping:
///   `CoreError::Auth` (401/403), `CoreError::RequestTimeout` (408/504),
///   `CoreError::RateLimit` (429), `CoreError::ServiceUnavailable` (502/503).
///   See `docs/guides/http-status-error-mapping.md`.
/// - `CoreError::Config` with `ConfigCode::UnsupportedProviderBedrock`
///   (wire: `provider.bedrock.unsupported`) when an adapter resolves an
///   AWS Bedrock provider — AWS SigV4 is intentionally unsupported per
///   ADR-019 §3 (re-introduction requires the §5 8-step checklist).
/// - `CoreError::Network` (wire: `network.generic`) for pre-response
///   transport failures (DNS, connection refused).
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn interpret_intent(
        &self,
        screen_context: &ScreenContext,
        intent_hint: &str,
    ) -> Result<InterpretedAction, CoreError>;

    /// Interpret intent with optional skill context injected into the prompt.
    async fn interpret_intent_with_skills(
        &self,
        screen_context: &ScreenContext,
        intent_hint: &str,
        _skill_ctx: &SkillContext,
    ) -> Result<InterpretedAction, CoreError> {
        // Default: ignore skill context, delegate to base method.
        self.interpret_intent(screen_context, intent_hint).await
    }

    /// Additive observation path: legacy implementors remain valid and unknown.
    /// Composite providers override this to observe each leg independently.
    async fn interpret_intent_observed(
        &self,
        screen_context: &ScreenContext,
        intent_hint: &str,
        skills: Option<&SkillContext>,
        observer: &IntentExecutionObserver,
    ) -> Result<InterpretedAction, CoreError> {
        let capabilities = self.intent_capability_observation(skills);
        let attempt = observer.begin_attempt(self.intent_provider_class(), capabilities);
        let result = match skills {
            Some(skills) => {
                self.interpret_intent_with_skills(screen_context, intent_hint, skills)
                    .await
            }
            None => self.interpret_intent(screen_context, intent_hint).await,
        };
        observer.end_attempt(attempt, result.as_ref().map(|_| ()));
        result
    }

    /// Actual adapter class, never inferred from a configured model name.
    fn intent_provider_class(&self) -> IntentProviderClass {
        Default::default()
    }

    /// Configuration intent may differ from the sole constructed rule provider.
    fn intent_requested_provider_class(&self) -> IntentProviderClass {
        self.intent_provider_class()
    }

    fn intent_configuration_reason(&self) -> Option<IntentConfigurationReason> {
        None
    }

    /// None means skill forwarding is not observed by a legacy adapter.
    fn intent_forwards_skill_context(&self) -> Option<bool> {
        None
    }

    fn intent_capability_observation(
        &self,
        skills: Option<&SkillContext>,
    ) -> IntentCapabilityObservation {
        let kind = skills
            .map(SkillContext::observation_kind)
            .unwrap_or_default();
        let requested = kind != IntentSkillContextKind::None;
        let forwards = self.intent_forwards_skill_context();
        IntentCapabilityObservation {
            skills_requested: requested,
            skills_context_forwarded: if requested { forwards } else { Some(false) },
            skill_context_kind: if forwards == Some(true) {
                kind
            } else {
                IntentSkillContextKind::None
            },
            skills_effective: if forwards == Some(false) {
                IntentSkillUse::Unsupported
            } else {
                // Forwarding a prompt is never proof that the model used a skill.
                IntentSkillUse::Unknown
            },
        }
    }

    fn provider_name(&self) -> &str;

    fn is_external(&self) -> bool;

    /// Network endpoints that may carry screen context or intent prompts.
    ///
    /// In-process/local providers return an empty list. Providers that report
    /// `is_external() == false` while still using an endpoint must expose that
    /// URL so guard decorators can independently verify that every such
    /// endpoint is loopback before allowing an unguarded local fast path.
    fn egress_endpoint_urls(&self) -> Vec<&str> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::intent_execution::{
        IntentAttemptOutcome, IntentSafeReason, IntentSubmission, OpaqueIntentId,
    };
    use std::sync::Arc;

    struct RecordingProvider {
        forwards: Option<bool>,
        calls: parking_lot::Mutex<Vec<(String, String)>>,
    }

    impl RecordingProvider {
        fn new(forwards: Option<bool>) -> Self {
            Self {
                forwards,
                calls: parking_lot::Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl LlmProvider for RecordingProvider {
        async fn interpret_intent(
            &self,
            screen: &ScreenContext,
            hint: &str,
        ) -> Result<InterpretedAction, CoreError> {
            self.calls.lock().push(("plain".into(), hint.into()));
            if hint == "timeout" {
                return Err(CoreError::RequestTimeout {
                    code: crate::error_codes::NetworkCode::Timeout,
                    timeout_ms: 37,
                });
            }
            Ok(InterpretedAction {
                target_text: screen.visible_texts.first().cloned(),
                target_role: None,
                action_type: "click".into(),
                confidence: 0.9,
            })
        }

        async fn interpret_intent_with_skills(
            &self,
            screen: &ScreenContext,
            hint: &str,
            skills: &SkillContext,
        ) -> Result<InterpretedAction, CoreError> {
            let body = skills
                .active_skill
                .as_ref()
                .map(|value| value.body())
                .unwrap_or("catalog only");
            self.calls.lock().push((body.into(), hint.into()));
            Ok(InterpretedAction {
                target_text: screen.visible_texts.first().cloned(),
                target_role: None,
                action_type: "click".into(),
                confidence: 0.9,
            })
        }

        fn provider_name(&self) -> &str {
            "synthetic"
        }
        fn is_external(&self) -> bool {
            false
        }
        fn intent_provider_class(&self) -> IntentProviderClass {
            IntentProviderClass::LocalModel
        }
        fn intent_forwards_skill_context(&self) -> Option<bool> {
            self.forwards
        }
    }

    fn observation() -> IntentExecutionObserver {
        IntentExecutionObserver::new(IntentSubmission {
            submission_id: Some(OpaqueIntentId::new()),
            retry_of: None,
        })
    }

    struct LegacyProvider(RecordingProvider);

    #[async_trait]
    impl LlmProvider for LegacyProvider {
        async fn interpret_intent(
            &self,
            screen: &ScreenContext,
            hint: &str,
        ) -> Result<InterpretedAction, CoreError> {
            self.0.interpret_intent(screen, hint).await
        }
        fn provider_name(&self) -> &str {
            "legacy"
        }
        fn is_external(&self) -> bool {
            false
        }
    }

    #[tokio::test]
    async fn legacy_provider_keeps_unknown_metadata_and_original_dispatch() {
        let provider = LegacyProvider(RecordingProvider::new(None));
        assert_eq!(
            provider.intent_provider_class(),
            IntentProviderClass::Unknown
        );
        assert_eq!(
            provider.intent_requested_provider_class(),
            IntentProviderClass::Unknown
        );
        assert_eq!(provider.intent_configuration_reason(), None);
        assert_eq!(provider.intent_forwards_skill_context(), None);
        let screen = ScreenContext {
            visible_texts: vec!["Save".into()],
            active_app: String::new(),
            active_window_title: String::new(),
            layout_description: None,
        };
        let observer = observation();
        let result = provider
            .interpret_intent_observed(
                &screen,
                "legacy hint",
                Some(&activated_context()),
                &observer,
            )
            .await
            .unwrap();
        assert_eq!(result.target_text.as_deref(), Some("Save"));
        assert_eq!(
            *provider.0.calls.lock(),
            vec![("plain".into(), "legacy hint".into())]
        );
        let receipt = observer.snapshot();
        assert_eq!(receipt.attempts.len(), 1);
        assert_eq!(
            receipt.attempts[0].effective_provider_class,
            IntentProviderClass::Unknown
        );
        assert_eq!(
            receipt.attempts[0].capabilities.skills_context_forwarded,
            None
        );
        assert_eq!(
            receipt.attempts[0].capabilities.skills_effective,
            IntentSkillUse::Unknown
        );
    }

    fn activated_context() -> SkillContext {
        use crate::models::skill_pack::tests_support::{
            available, entry, granted_install, grants, GOOD_BODY,
        };
        use crate::models::skill_pack::{
            resolve_activation, SkillActivationOutcome, SkillActivationRequest, SkillSelectionKind,
        };
        let outcome = resolve_activation(SkillActivationRequest {
            entry: &entry(),
            install: &granted_install("1.0.0"),
            availability: &available(),
            presented_body: GOOD_BODY,
            selection: Some(&SkillSelectionKind::ExplicitUserSelection),
            effective_grants: &grants(&[]),
            reference_graph: &std::collections::BTreeMap::new(),
            now: chrono::Utc::now(),
            lifetime_secs: 600,
        });
        let SkillActivationOutcome::Activated(active) = outcome else {
            panic!("trusted fixture must activate")
        };
        SkillContext {
            available_skills: vec![SkillMeta {
                name: "catalog".into(),
                description: "fixture".into(),
            }],
            active_skill: Some(TrustedInstruction::from_activation(&active)),
        }
    }

    #[test]
    fn observed_skill_capabilities_distinguish_request_forwarding_and_effect() {
        use IntentSkillContextKind::{Activated, Catalog, None as NoContext};
        use IntentSkillUse::{Unknown, Unsupported};
        let activated = activated_context();
        let catalog = SkillContext {
            available_skills: activated.available_skills.clone(),
            active_skill: None,
        };
        let empty = SkillContext {
            available_skills: vec![],
            active_skill: None,
        };
        assert_eq!(activated.observation_kind(), Activated);
        assert_eq!(catalog.observation_kind(), Catalog);
        assert_eq!(empty.observation_kind(), NoContext);
        // Forwarding the verified body never asserts that the model used it.
        for (forwards, context, requested, forwarded, kind, effective) in [
            (
                Some(true),
                Some(&activated),
                true,
                Some(true),
                Activated,
                Unknown,
            ),
            (
                Some(true),
                Some(&catalog),
                true,
                Some(true),
                Catalog,
                Unknown,
            ),
            (
                Some(false),
                Some(&activated),
                true,
                Some(false),
                NoContext,
                Unsupported,
            ),
            (None, Some(&catalog), true, None, NoContext, Unknown),
            (
                Some(true),
                Some(&empty),
                false,
                Some(false),
                NoContext,
                Unknown,
            ),
            (
                Some(false),
                None,
                false,
                Some(false),
                NoContext,
                Unsupported,
            ),
            (None, None, false, Some(false), NoContext, Unknown),
        ] {
            assert_eq!(
                RecordingProvider::new(forwards).intent_capability_observation(context),
                IntentCapabilityObservation {
                    skills_requested: requested,
                    skills_context_forwarded: forwarded,
                    skill_context_kind: kind,
                    skills_effective: effective
                }
            );
        }
    }

    #[tokio::test]
    async fn observed_provider_preserves_inputs_results_and_bounded_error_facts() {
        let provider = RecordingProvider::new(Some(true));
        let screen = ScreenContext {
            visible_texts: vec!["Save".into()],
            active_app: "fixture".into(),
            active_window_title: "window-canary".into(),
            layout_description: None,
        };
        let observer = observation();
        assert_eq!(
            provider.intent_requested_provider_class(),
            IntentProviderClass::LocalModel
        );
        assert_eq!(provider.intent_configuration_reason(), None);
        let action = provider
            .interpret_intent_observed(&screen, "plain hint", None, &observer)
            .await
            .unwrap();
        assert_eq!(action.target_text.as_deref(), Some("Save"));
        let skills = activated_context();
        let action = provider
            .interpret_intent_observed(&screen, "skill hint", Some(&skills), &observer)
            .await
            .unwrap();
        assert_eq!(action.action_type, "click");
        let error = provider
            .interpret_intent_observed(&screen, "timeout", None, &observer)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            CoreError::RequestTimeout { timeout_ms: 37, .. }
        ));
        assert_eq!(
            *provider.calls.lock(),
            vec![
                ("plain".into(), "plain hint".into()),
                (
                    crate::models::skill_pack::tests_support::GOOD_BODY.into(),
                    "skill hint".into()
                ),
                ("plain".into(), "timeout".into())
            ]
        );
        let receipt = observer.snapshot();
        assert_eq!(receipt.attempts.len(), 3);
        assert_eq!(
            receipt.attempts[0].effective_provider_class,
            IntentProviderClass::LocalModel
        );
        assert_eq!(receipt.attempts[0].outcome, IntentAttemptOutcome::Succeeded);
        assert_eq!(receipt.attempts[1].outcome, IntentAttemptOutcome::Succeeded);
        assert_eq!(
            receipt.attempts[1].capabilities.skill_context_kind,
            IntentSkillContextKind::Activated
        );
        assert_eq!(
            receipt.attempts[1].capabilities.skills_effective,
            IntentSkillUse::Unknown
        );
        assert_eq!(receipt.attempts[2].outcome, IntentAttemptOutcome::Timeout);
        assert_eq!(receipt.attempts[2].reason, Some(IntentSafeReason::Timeout));
        let wire = serde_json::to_string(&receipt).unwrap();
        for private in [
            "plain hint",
            "skill hint",
            "window-canary",
            "Save",
            crate::models::skill_pack::tests_support::GOOD_BODY,
        ] {
            assert!(!wire.contains(private));
        }
    }

    // ── LlmCallHealth unit tests (#5734) ─────────────────────────────────

    /// Default state is UNKNOWN — `as_option_bool()` returns `None`.
    #[test]
    fn llm_call_health_default_is_unknown() {
        let h = LlmCallHealth::default();
        assert_eq!(h.as_option_bool(), None);
    }

    /// UNKNOWN → OK transition: `as_option_bool()` returns `Some(true)`.
    #[test]
    fn llm_call_health_record_ok_transitions_to_some_true() {
        let h = LlmCallHealth::default();
        h.record_ok();
        assert_eq!(h.as_option_bool(), Some(true));
    }

    /// UNKNOWN → FAILED transition: `as_option_bool()` returns `Some(false)`.
    #[test]
    fn llm_call_health_record_failed_transitions_to_some_false() {
        let h = LlmCallHealth::default();
        h.record_failed();
        assert_eq!(h.as_option_bool(), Some(false));
    }

    /// OK → FAILED transition: subsequent failure overwrites prior success.
    #[test]
    fn llm_call_health_ok_then_failed_returns_some_false() {
        let h = LlmCallHealth::default();
        h.record_ok();
        h.record_failed();
        assert_eq!(h.as_option_bool(), Some(false));
    }

    /// FAILED → OK transition: recovery after a failure is observable.
    #[test]
    fn llm_call_health_failed_then_ok_returns_some_true() {
        let h = LlmCallHealth::default();
        h.record_failed();
        h.record_ok();
        assert_eq!(h.as_option_bool(), Some(true));
    }

    /// Arc-shared handle: both writer and reader see the same state.
    #[test]
    fn llm_call_health_arc_shared_write_visible_from_clone() {
        let writer = Arc::new(LlmCallHealth::default());
        let reader = Arc::clone(&writer);
        assert_eq!(reader.as_option_bool(), None);
        writer.record_ok();
        assert_eq!(reader.as_option_bool(), Some(true));
        writer.record_failed();
        assert_eq!(reader.as_option_bool(), Some(false));
    }

    // ── Original LlmProvider port tests ──────────────────────────────────

    #[test]
    fn screen_context_serde() {
        let ctx = ScreenContext {
            visible_texts: vec!["file".to_string(), "edit".to_string()],
            active_app: "Visual Studio Code".to_string(),
            active_window_title: "main.rs — VSCode".to_string(),
            layout_description: None,
        };
        let json = serde_json::to_string(&ctx).unwrap();
        let deser: ScreenContext = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.visible_texts.len(), 2);
        assert_eq!(deser.active_app, "Visual Studio Code");
    }

    #[test]
    fn interpreted_action_serde() {
        let action = InterpretedAction {
            target_text: Some("save".to_string()),
            target_role: Some("button".to_string()),
            action_type: "click".to_string(),
            confidence: 0.85,
        };
        let json = serde_json::to_string(&action).unwrap();
        let deser: InterpretedAction = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.target_text.unwrap(), "save");
        assert!((deser.confidence - 0.85).abs() < f64::EPSILON);
    }
}
