//! Fallback and loopback newtype wrappers for `LlmProvider`.
//!
//! # FallbackLlmProvider
//! Mirrors [`maekon_analysis::fallback_analysis_provider::FallbackAnalysisProvider`].
//! Applied in the LocalModel arm to wrap a primary (Ollama) over a
//! `LocalLlmProvider` (keyword rule matcher) so that per-call transport failures
//! degrade gracefully.
//!
//! # F1-trap note
//! `LlmProvider::interpret_intent_with_skills` has a *trait default* that
//! delegates to `interpret_intent` (llm_provider.rs:65-68).  If only
//! `interpret_intent` were overridden here the inherited default would silently
//! drop `SkillContext` on every call.  Both methods must be overridden with the
//! same primary→fallback logic.  The fallback leg (rule matcher) does not
//! override `interpret_intent_with_skills`, so skills are always dropped on that
//! leg — that is acceptable and documented here.
//!
//! # LoopbackLlmProvider
//! Thin newtype that overrides `is_external() = false` for a
//! `RemoteLlmProvider` proven to target a loopback endpoint.
//! `RemoteLlmProvider::is_external()` is unconditionally `true`
//! (ai_llm_client/mod.rs:388-390); wrapping in `LoopbackLlmProvider` corrects
//! this for loopback-only Ollama so the privacy guard chain is not invoked
//! unnecessarily.

use std::sync::Arc;

use async_trait::async_trait;
use maekon_core::error::CoreError;
use maekon_core::models::intent_execution::{
    IntentExecutionObserver, IntentProviderClass, IntentSkillContextKind, IntentSkillUse,
};
use maekon_core::ports::llm_provider::{
    InterpretedAction, LlmProvider, ScreenContext, SkillContext,
};
use tracing::warn;

/// Chains two LLM providers: tries primary (Ollama), falls back on error.
///
/// `provider_name()` always delegates to the primary so the builder log and
/// `AiRuntimeStatus` show the real model id (e.g. "qwen3:0.6b").
///
/// `is_external()` is the logical OR of both legs — if either provider
/// requires external egress the composite requires it too.
pub(super) struct FallbackLlmProvider {
    primary: Arc<dyn LlmProvider>,
    fallback: Arc<dyn LlmProvider>,
}

impl FallbackLlmProvider {
    pub(super) fn new(primary: Arc<dyn LlmProvider>, fallback: Arc<dyn LlmProvider>) -> Self {
        Self { primary, fallback }
    }
}

#[async_trait]
impl LlmProvider for FallbackLlmProvider {
    async fn interpret_intent_observed(
        &self,
        screen_context: &ScreenContext,
        intent_hint: &str,
        skills: Option<&SkillContext>,
        observer: &IntentExecutionObserver,
    ) -> Result<InterpretedAction, CoreError> {
        match self
            .primary
            .interpret_intent_observed(screen_context, intent_hint, skills, observer)
            .await
        {
            Ok(action) => Ok(action),
            Err(_) => {
                // Preserve the original base-method fallback: it receives no skills.
                let mut capabilities = self.fallback.intent_capability_observation(skills);
                capabilities.skills_context_forwarded = Some(false);
                capabilities.skill_context_kind = IntentSkillContextKind::None;
                capabilities.skills_effective =
                    if self.fallback.intent_provider_class() == IntentProviderClass::Rule {
                        IntentSkillUse::Unsupported
                    } else {
                        IntentSkillUse::Unknown
                    };
                let attempt =
                    observer.begin_attempt(self.fallback.intent_provider_class(), capabilities);
                let result = self
                    .fallback
                    .interpret_intent(screen_context, intent_hint)
                    .await;
                observer.end_attempt(attempt, result.as_ref().map(|_| ()));
                result
            }
        }
    }

    fn intent_provider_class(&self) -> IntentProviderClass {
        self.primary.intent_provider_class()
    }

    fn intent_requested_provider_class(&self) -> IntentProviderClass {
        self.primary.intent_requested_provider_class()
    }

    async fn interpret_intent(
        &self,
        screen_context: &ScreenContext,
        intent_hint: &str,
    ) -> Result<InterpretedAction, CoreError> {
        match self
            .primary
            .interpret_intent(screen_context, intent_hint)
            .await
        {
            Ok(action) => Ok(action),
            Err(e) => {
                // Structured log: err.code lets observability tooling distinguish
                // privacy-denied (policy.denied) from daemon-down (network.generic /
                // service.circuit_open) without regex-matching the Display body.
                warn!(
                    err.code = %e.code(),
                    primary = %self.primary.provider_name(),
                    "primary LLM failed, falling back to local rule-based provider: {e}"
                );
                self.fallback
                    .interpret_intent(screen_context, intent_hint)
                    .await
            }
        }
    }

    /// F1-trap override: must be explicit so `SkillContext` reaches the primary.
    /// The fallback leg (rule matcher) drops skills — LocalLlmProvider has no
    /// override and will ignore `skill_ctx` via the trait default.
    async fn interpret_intent_with_skills(
        &self,
        screen_context: &ScreenContext,
        intent_hint: &str,
        skill_ctx: &SkillContext,
    ) -> Result<InterpretedAction, CoreError> {
        match self
            .primary
            .interpret_intent_with_skills(screen_context, intent_hint, skill_ctx)
            .await
        {
            Ok(action) => Ok(action),
            Err(e) => {
                warn!(
                    err.code = %e.code(),
                    primary = %self.primary.provider_name(),
                    skills = skill_ctx.available_skills.len(),
                    "primary LLM (with skills) failed, falling back to local rule-based provider \
                     (skills will be ignored on fallback leg): {e}"
                );
                // Fallback leg drops skills — rule matcher has no skills support.
                self.fallback
                    .interpret_intent(screen_context, intent_hint)
                    .await
            }
        }
    }

    /// Delegate to primary so the model id surfaces in logs / status.
    fn provider_name(&self) -> &str {
        self.primary.provider_name()
    }

    /// Union of both legs: if either provider requires external egress, so does
    /// the composite.
    fn is_external(&self) -> bool {
        self.primary.is_external() || self.fallback.is_external()
    }

    fn egress_endpoint_urls(&self) -> Vec<&str> {
        let mut endpoints = self.primary.egress_endpoint_urls();
        endpoints.extend(self.fallback.egress_endpoint_urls());
        endpoints
    }
}

// ── LoopbackLlmProvider ────────────────────────────────────────────────────────

/// Thin newtype that forces `is_external() = false` for an inner provider that
/// is proven to target a loopback endpoint.
///
/// `RemoteLlmProvider::is_external()` is unconditionally `true` regardless of
/// host (ai_llm_client/mod.rs:388-390).  This newtype is applied only after
/// `endpoint_is_loopback()` returns `true`, so the override is sound: loopback
/// traffic cannot leave the machine and requires no external-egress consent.
///
/// Non-loopback endpoints keep the original `RemoteLlmProvider` so
/// `guard_external_llm_provider` enforces consent and PII filtering normally.
pub(super) struct LoopbackLlmProvider {
    inner: Arc<dyn LlmProvider>,
}

impl LoopbackLlmProvider {
    pub(super) fn new(inner: Arc<dyn LlmProvider>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl LlmProvider for LoopbackLlmProvider {
    fn intent_provider_class(&self) -> IntentProviderClass {
        IntentProviderClass::LocalModel
    }

    fn intent_forwards_skill_context(&self) -> Option<bool> {
        self.inner.intent_forwards_skill_context()
    }

    async fn interpret_intent(
        &self,
        screen_context: &ScreenContext,
        intent_hint: &str,
    ) -> Result<InterpretedAction, CoreError> {
        self.inner
            .interpret_intent(screen_context, intent_hint)
            .await
    }

    async fn interpret_intent_with_skills(
        &self,
        screen_context: &ScreenContext,
        intent_hint: &str,
        skill_ctx: &SkillContext,
    ) -> Result<InterpretedAction, CoreError> {
        self.inner
            .interpret_intent_with_skills(screen_context, intent_hint, skill_ctx)
            .await
    }

    fn provider_name(&self) -> &str {
        self.inner.provider_name()
    }

    /// Loopback override: proven loopback — not external egress.
    fn is_external(&self) -> bool {
        false
    }

    fn egress_endpoint_urls(&self) -> Vec<&str> {
        self.inner.egress_endpoint_urls()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use maekon_core::error::CoreError;
    use maekon_core::models::skill::SkillMeta;
    use maekon_core::ports::llm_provider::{
        InterpretedAction, LlmProvider, ScreenContext, SkillContext,
    };

    fn ok_action() -> InterpretedAction {
        InterpretedAction {
            target_text: Some("Save".to_string()),
            target_role: Some("button".to_string()),
            action_type: "click".to_string(),
            confidence: 0.9,
        }
    }

    fn screen_ctx() -> ScreenContext {
        ScreenContext {
            visible_texts: vec!["File".to_string(), "Save".to_string()],
            active_app: "Code".to_string(),
            active_window_title: "main.rs".to_string(),
            layout_description: None,
        }
    }

    // Always succeeds with ok_action, is_external=false.
    struct OkLlmProvider;
    #[async_trait]
    impl LlmProvider for OkLlmProvider {
        fn intent_provider_class(&self) -> IntentProviderClass {
            IntentProviderClass::LocalModel
        }
        fn intent_forwards_skill_context(&self) -> Option<bool> {
            Some(true)
        }
        async fn interpret_intent(
            &self,
            _ctx: &ScreenContext,
            _hint: &str,
        ) -> Result<InterpretedAction, CoreError> {
            Ok(ok_action())
        }
        async fn interpret_intent_with_skills(
            &self,
            _ctx: &ScreenContext,
            _hint: &str,
            _skill: &SkillContext,
        ) -> Result<InterpretedAction, CoreError> {
            Ok(ok_action())
        }
        fn provider_name(&self) -> &str {
            "ok-primary"
        }
        fn is_external(&self) -> bool {
            false
        }
    }

    // Always fails with Analysis error.
    struct FailLlmProvider;
    #[async_trait]
    impl LlmProvider for FailLlmProvider {
        fn intent_provider_class(&self) -> IntentProviderClass {
            IntentProviderClass::LocalModel
        }
        async fn interpret_intent(
            &self,
            _ctx: &ScreenContext,
            _hint: &str,
        ) -> Result<InterpretedAction, CoreError> {
            Err(CoreError::Analysis {
                code: maekon_core::error_codes::ProviderCode::AnalysisFailed,
                message: "daemon down".into(),
            })
        }
        fn provider_name(&self) -> &str {
            "fail-primary"
        }
        fn is_external(&self) -> bool {
            false
        }
    }

    // Succeeds via rule-matcher fallback pattern — distinguishable by action_type.
    struct RuleMatcherProvider;
    #[async_trait]
    impl LlmProvider for RuleMatcherProvider {
        fn intent_provider_class(&self) -> IntentProviderClass {
            IntentProviderClass::Rule
        }
        fn intent_forwards_skill_context(&self) -> Option<bool> {
            Some(false)
        }
        async fn interpret_intent_with_skills(
            &self,
            _: &ScreenContext,
            _: &str,
            _: &SkillContext,
        ) -> Result<InterpretedAction, CoreError> {
            panic!("fallback must retain the base-method route that drops skills")
        }
        async fn interpret_intent(
            &self,
            _ctx: &ScreenContext,
            _hint: &str,
        ) -> Result<InterpretedAction, CoreError> {
            Ok(InterpretedAction {
                target_text: None,
                target_role: None,
                action_type: "rule-match".to_string(),
                confidence: 0.5,
            })
        }
        fn provider_name(&self) -> &str {
            "local-rule-based"
        }
        fn is_external(&self) -> bool {
            false
        }
    }

    // External provider for is_external composition test.
    struct ExternalLlmProvider(Option<bool>);
    #[async_trait]
    impl LlmProvider for ExternalLlmProvider {
        fn intent_forwards_skill_context(&self) -> Option<bool> {
            self.0
        }
        async fn interpret_intent(
            &self,
            _ctx: &ScreenContext,
            _hint: &str,
        ) -> Result<InterpretedAction, CoreError> {
            Ok(ok_action())
        }
        fn provider_name(&self) -> &str {
            "external"
        }
        fn is_external(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn primary_ok_returns_primary_result() {
        let provider =
            FallbackLlmProvider::new(Arc::new(OkLlmProvider), Arc::new(RuleMatcherProvider));
        let result = provider
            .interpret_intent(&screen_ctx(), "click save")
            .await
            .unwrap();
        assert_eq!(result.action_type, "click");
        assert_eq!(result.confidence, 0.9);
    }

    #[tokio::test]
    async fn primary_error_uses_fallback() {
        let provider =
            FallbackLlmProvider::new(Arc::new(FailLlmProvider), Arc::new(RuleMatcherProvider));
        let result = provider
            .interpret_intent(&screen_ctx(), "click save")
            .await
            .unwrap();
        // Fallback (rule matcher) distinguishable by action_type.
        assert_eq!(result.action_type, "rule-match");
        assert_eq!(result.confidence, 0.5);
    }

    #[tokio::test]
    async fn both_error_propagates_fallback_error() {
        let provider =
            FallbackLlmProvider::new(Arc::new(FailLlmProvider), Arc::new(FailLlmProvider));
        let err = provider
            .interpret_intent(&screen_ctx(), "click save")
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::Analysis { .. }));
    }

    #[tokio::test]
    async fn provider_name_delegates_to_primary() {
        let provider =
            FallbackLlmProvider::new(Arc::new(OkLlmProvider), Arc::new(RuleMatcherProvider));
        assert_eq!(provider.provider_name(), "ok-primary");
    }

    #[tokio::test]
    async fn provider_name_delegates_to_failing_primary() {
        // Even when primary fails per-call, provider_name is still from primary.
        let provider =
            FallbackLlmProvider::new(Arc::new(FailLlmProvider), Arc::new(RuleMatcherProvider));
        assert_eq!(provider.provider_name(), "fail-primary");
    }

    #[test]
    fn is_external_is_false_when_both_local() {
        let provider =
            FallbackLlmProvider::new(Arc::new(OkLlmProvider), Arc::new(RuleMatcherProvider));
        assert!(!provider.is_external());
    }

    #[test]
    fn is_external_is_true_when_primary_is_external() {
        let provider = FallbackLlmProvider::new(
            Arc::new(ExternalLlmProvider(None)),
            Arc::new(RuleMatcherProvider),
        );
        assert!(provider.is_external());
    }

    #[test]
    fn is_external_is_true_when_fallback_is_external() {
        let provider =
            FallbackLlmProvider::new(Arc::new(OkLlmProvider), Arc::new(ExternalLlmProvider(None)));
        assert!(provider.is_external());
    }

    // F1-trap test: interpret_intent_with_skills must reach primary (not silently
    // drop SkillContext via the trait default).
    #[tokio::test]
    async fn skills_method_reaches_primary_when_healthy() {
        let provider =
            FallbackLlmProvider::new(Arc::new(OkLlmProvider), Arc::new(RuleMatcherProvider));
        let skill_ctx = SkillContext {
            available_skills: vec![SkillMeta {
                name: "file-save".to_string(),
                description: "saves the file".to_string(),
            }],
            active_skill: None,
        };
        let result = provider
            .interpret_intent_with_skills(&screen_ctx(), "save file", &skill_ctx)
            .await
            .unwrap();
        // OkLlmProvider.interpret_intent_with_skills returns ok_action (click).
        assert_eq!(result.action_type, "click");
    }

    // F1-trap test: when primary fails, fallback is used (skills dropped on that leg).
    #[tokio::test]
    async fn skills_method_falls_back_on_primary_error() {
        let provider =
            FallbackLlmProvider::new(Arc::new(FailLlmProvider), Arc::new(RuleMatcherProvider));
        let skill_ctx = SkillContext {
            available_skills: vec![SkillMeta {
                name: "file-save".to_string(),
                description: "saves the file".to_string(),
            }],
            active_skill: None,
        };
        // Fallback (rule matcher) invoked; it drops skills (only interpret_intent called).
        let result = provider
            .interpret_intent_with_skills(&screen_ctx(), "save file", &skill_ctx)
            .await
            .unwrap();
        assert_eq!(result.action_type, "rule-match");
    }

    #[tokio::test]
    async fn observed_fallback_records_actual_legs_and_distinct_skill_evidence() {
        use maekon_core::models::intent_execution::*;
        let skills = SkillContext {
            available_skills: vec![SkillMeta {
                name: "CANARY_SKILL".into(),
                description: "CANARY_BODY".into(),
            }],
            active_skill: None,
        };
        for primary_ok in [true, false] {
            let primary: Arc<dyn LlmProvider> = if primary_ok {
                Arc::new(OkLlmProvider)
            } else {
                Arc::new(FailLlmProvider)
            };
            let provider = FallbackLlmProvider::new(primary, Arc::new(RuleMatcherProvider));
            assert_eq!(
                provider.intent_provider_class(),
                IntentProviderClass::LocalModel
            );
            let observer = IntentExecutionObserver::new(IntentSubmission::default());
            observer.requested_provider(provider.intent_requested_provider_class());
            provider
                .interpret_intent_observed(&screen_ctx(), "CANARY_PROMPT", Some(&skills), &observer)
                .await
                .unwrap();
            observer.planning_succeeded();
            observer.finish(IntentActionOutcome::NotStarted, None);
            let receipt = observer.snapshot();
            assert_eq!(
                receipt.requested_provider_class,
                IntentProviderClass::LocalModel
            );
            assert_eq!(receipt.attempts.len(), if primary_ok { 1 } else { 2 });
            let primary = &receipt.attempts[0];
            assert!(primary.capabilities.skills_requested);
            if primary_ok {
                assert_eq!(receipt.planning_outcome, IntentPlanningOutcome::Llm);
                assert_eq!(primary.outcome, IntentAttemptOutcome::Succeeded);
                assert_eq!(primary.capabilities.skills_context_forwarded, Some(true));
                assert_eq!(
                    primary.capabilities.skill_context_kind,
                    IntentSkillContextKind::Catalog
                );
                assert_eq!(
                    primary.capabilities.skills_effective,
                    IntentSkillUse::Unknown
                );
            } else {
                let fallback = &receipt.attempts[1];
                assert_eq!(primary.outcome, IntentAttemptOutcome::InvalidOutput);
                assert_eq!(receipt.planning_outcome, IntentPlanningOutcome::Rule);
                assert_eq!(fallback.predecessor, Some(primary.attempt_id));
                assert_eq!(fallback.effective_provider_class, IntentProviderClass::Rule);
                assert_eq!(fallback.model_revision, IntentRevision::NotApplicable);
                assert!(fallback.capabilities.skills_requested);
                assert_eq!(fallback.capabilities.skills_context_forwarded, Some(false));
                assert_eq!(
                    fallback.capabilities.skills_effective,
                    IntentSkillUse::Unsupported
                );
                assert_eq!(
                    fallback.capabilities.skill_context_kind,
                    IntentSkillContextKind::None
                );
            }
            assert!(!serde_json::to_string(&receipt).unwrap().contains("CANARY"));
        }
    }

    #[tokio::test]
    async fn both_failed_legs_remain_failed_without_planning_success() {
        use maekon_core::models::intent_execution::*;
        let provider =
            FallbackLlmProvider::new(Arc::new(FailLlmProvider), Arc::new(FailLlmProvider));
        let observer = IntentExecutionObserver::new(IntentSubmission::default());
        let error = provider
            .interpret_intent_observed(&screen_ctx(), "save", None, &observer)
            .await
            .unwrap_err();
        observer.planning_finished(
            IntentPlanningOutcome::Error,
            Some(IntentSafeReason::from_error(&error)),
        );
        observer.finish(IntentActionOutcome::NotStarted, None);
        let receipt = observer.snapshot();
        assert_eq!(receipt.attempts.len(), 2);
        assert!(receipt
            .attempts
            .iter()
            .all(|attempt| attempt.outcome == IntentAttemptOutcome::InvalidOutput));
        assert_eq!(receipt.planning_outcome, IntentPlanningOutcome::Error);
        assert_eq!(receipt.action_outcome, IntentActionOutcome::NotStarted);
    }

    #[tokio::test]
    async fn loopback_observation_keeps_local_class_without_duplicate_attempt() {
        use maekon_core::models::intent_execution::*;
        for forwards in [Some(true), Some(false), None] {
            let provider = LoopbackLlmProvider::new(Arc::new(ExternalLlmProvider(forwards)));
            let observer = IntentExecutionObserver::new(IntentSubmission::default());
            let skills = SkillContext {
                available_skills: vec![SkillMeta {
                    name: "save".into(),
                    description: "save".into(),
                }],
                active_skill: None,
            };
            provider
                .interpret_intent_observed(&screen_ctx(), "save", Some(&skills), &observer)
                .await
                .unwrap();
            let receipt = observer.snapshot();
            assert_eq!(receipt.attempts.len(), 1);
            assert_eq!(
                receipt.attempts[0].capabilities.skills_context_forwarded,
                forwards
            );
            assert_eq!(
                receipt.attempts[0].effective_provider_class,
                IntentProviderClass::LocalModel
            );
        }
    }
}
