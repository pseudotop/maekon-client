//! Freeze configuration intent without inventing a provider attempt (#12060).
use std::sync::Arc;

use async_trait::async_trait;
use maekon_core::config::{AiAccessMode, AiProviderConfig, LlmProviderType};
use maekon_core::error::CoreError;
use maekon_core::models::intent_execution::{
    IntentConfigurationReason, IntentExecutionObserver, IntentProviderClass,
};
use maekon_core::ports::llm_provider::{
    InterpretedAction, LlmProvider, ScreenContext, SkillContext,
};

pub(super) struct ConfiguredLlmProvider {
    inner: Arc<dyn LlmProvider>,
    requested: IntentProviderClass,
}

impl ConfiguredLlmProvider {
    pub(super) fn new(inner: Arc<dyn LlmProvider>, config: &AiProviderConfig) -> Self {
        let requested = match config.access_mode.normalized_for_ai_surfaces() {
            AiAccessMode::LocalModel => IntentProviderClass::LocalModel,
            AiAccessMode::ProviderSubscriptionCli => IntentProviderClass::RemoteModel,
            AiAccessMode::ProviderApiKey | AiAccessMode::ProviderOAuth => match config.llm_provider
            {
                LlmProviderType::Local => IntentProviderClass::LocalModel,
                LlmProviderType::Remote => IntentProviderClass::RemoteModel,
            },
        };
        Self { inner, requested }
    }
}

#[async_trait]
impl LlmProvider for ConfiguredLlmProvider {
    async fn interpret_intent_observed(
        &self,
        screen: &ScreenContext,
        hint: &str,
        skills: Option<&SkillContext>,
        observer: &IntentExecutionObserver,
    ) -> Result<InterpretedAction, CoreError> {
        // Delegate actual leg observation. Wrapping a preselected rule-only
        // adapter must not manufacture a failed model call or a wrapper attempt.
        self.inner
            .interpret_intent_observed(screen, hint, skills, observer)
            .await
    }
    async fn interpret_intent(
        &self,
        screen: &ScreenContext,
        hint: &str,
    ) -> Result<InterpretedAction, CoreError> {
        self.inner.interpret_intent(screen, hint).await
    }
    async fn interpret_intent_with_skills(
        &self,
        screen: &ScreenContext,
        hint: &str,
        skills: &SkillContext,
    ) -> Result<InterpretedAction, CoreError> {
        self.inner
            .interpret_intent_with_skills(screen, hint, skills)
            .await
    }
    fn intent_requested_provider_class(&self) -> IntentProviderClass {
        self.requested
    }
    fn intent_provider_class(&self) -> IntentProviderClass {
        self.inner.intent_provider_class()
    }
    fn intent_configuration_reason(&self) -> Option<IntentConfigurationReason> {
        (self.inner.intent_provider_class() == IntentProviderClass::Rule)
            .then_some(IntentConfigurationReason::RuleOnlyDegradation)
    }
    fn intent_forwards_skill_context(&self) -> Option<bool> {
        self.inner.intent_forwards_skill_context()
    }
    fn provider_name(&self) -> &str {
        self.inner.provider_name()
    }
    fn is_external(&self) -> bool {
        self.inner.is_external()
    }
    fn egress_endpoint_urls(&self) -> Vec<&str> {
        self.inner.egress_endpoint_urls()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_automation::local_llm::LocalLlmProvider;
    use maekon_core::models::intent_execution::*;

    struct RemoteProvider;
    #[async_trait]
    impl LlmProvider for RemoteProvider {
        async fn interpret_intent(
            &self,
            screen: &ScreenContext,
            hint: &str,
        ) -> Result<InterpretedAction, CoreError> {
            assert_eq!(screen.active_app, "CANARY_APP");
            assert_eq!(hint, "CANARY_HINT");
            Ok(InterpretedAction {
                target_text: None,
                target_role: None,
                action_type: "base".into(),
                confidence: 0.9,
            })
        }
        async fn interpret_intent_with_skills(
            &self,
            screen: &ScreenContext,
            hint: &str,
            skills: &SkillContext,
        ) -> Result<InterpretedAction, CoreError> {
            assert_eq!(skills.available_skills[0].name, "CANARY_SKILL");
            let mut action = self.interpret_intent(screen, hint).await?;
            action.action_type = "skills".into();
            Ok(action)
        }
        fn intent_provider_class(&self) -> IntentProviderClass {
            IntentProviderClass::RemoteModel
        }
        fn intent_forwards_skill_context(&self) -> Option<bool> {
            Some(true)
        }
        fn provider_name(&self) -> &str {
            "CANARY_PROVIDER"
        }
        fn is_external(&self) -> bool {
            true
        }
        fn egress_endpoint_urls(&self) -> Vec<&str> {
            vec!["https://canary.invalid/v1"]
        }
    }

    #[tokio::test]
    async fn configured_wrapper_preserves_both_call_routes_and_external_metadata() {
        let mut config = AiProviderConfig {
            access_mode: AiAccessMode::ProviderApiKey,
            llm_provider: LlmProviderType::Remote,
            ..Default::default()
        };
        let provider = ConfiguredLlmProvider::new(Arc::new(RemoteProvider), &config);
        config.llm_provider = LlmProviderType::Local;
        assert_eq!(
            provider.intent_requested_provider_class(),
            IntentProviderClass::RemoteModel
        );
        assert_eq!(config.llm_provider, LlmProviderType::Local);
        assert_eq!(
            provider.intent_provider_class(),
            IntentProviderClass::RemoteModel
        );
        assert_eq!(provider.intent_configuration_reason(), None);
        assert_eq!(provider.intent_forwards_skill_context(), Some(true));
        assert_eq!(provider.provider_name(), "CANARY_PROVIDER");
        assert!(provider.is_external());
        assert_eq!(
            provider.egress_endpoint_urls(),
            vec!["https://canary.invalid/v1"]
        );
        let screen = ScreenContext {
            visible_texts: vec![],
            active_app: "CANARY_APP".into(),
            active_window_title: String::new(),
            layout_description: None,
        };
        let skills = SkillContext {
            available_skills: vec![maekon_core::models::skill::SkillMeta {
                name: "CANARY_SKILL".into(),
                description: "CANARY_BODY".into(),
            }],
            active_skill: None,
        };
        for with_skills in [false, true] {
            let ordinary = if with_skills {
                provider
                    .interpret_intent_with_skills(&screen, "CANARY_HINT", &skills)
                    .await
            } else {
                provider.interpret_intent(&screen, "CANARY_HINT").await
            }
            .unwrap();
            let observer = IntentExecutionObserver::new(IntentSubmission::default());
            let observed = provider
                .interpret_intent_observed(
                    &screen,
                    "CANARY_HINT",
                    with_skills.then_some(&skills),
                    &observer,
                )
                .await
                .unwrap();
            assert_eq!(
                ordinary.action_type,
                if with_skills { "skills" } else { "base" }
            );
            assert_eq!(
                serde_json::to_value(ordinary).unwrap(),
                serde_json::to_value(observed).unwrap()
            );
            let receipt = observer.snapshot();
            assert_eq!(receipt.attempts.len(), 1);
            assert_eq!(
                receipt.attempts[0].effective_provider_class,
                IntentProviderClass::RemoteModel
            );
            assert_eq!(
                receipt.attempts[0].capabilities.skills_context_forwarded,
                Some(with_skills)
            );
            assert!(!serde_json::to_string(&receipt).unwrap().contains("CANARY"));
        }
    }

    #[tokio::test]
    async fn configured_local_model_with_rule_only_adapter_has_one_actual_attempt() {
        let provider = ConfiguredLlmProvider::new(
            Arc::new(LocalLlmProvider::new()),
            &AiProviderConfig {
                access_mode: AiAccessMode::LocalModel,
                ..Default::default()
            },
        );
        assert_eq!(provider.intent_forwards_skill_context(), Some(false));
        let observer = IntentExecutionObserver::new(IntentSubmission::default());
        observer.configure(
            provider.intent_requested_provider_class(),
            provider.intent_configuration_reason(),
        );
        let context = ScreenContext {
            visible_texts: vec![],
            active_app: String::new(),
            active_window_title: String::new(),
            layout_description: None,
        };
        provider
            .interpret_intent_observed(&context, "save", None, &observer)
            .await
            .unwrap();
        observer.planning_succeeded();
        observer.finish(IntentActionOutcome::NotStarted, None);
        let receipt = observer.snapshot();
        assert_eq!(
            receipt.requested_provider_class,
            IntentProviderClass::LocalModel
        );
        assert_eq!(
            receipt.configuration_reason,
            Some(IntentConfigurationReason::RuleOnlyDegradation)
        );
        assert_eq!(receipt.attempts.len(), 1);
        assert_eq!(
            receipt.attempts[0].effective_provider_class,
            IntentProviderClass::Rule
        );
        assert_eq!(receipt.attempts[0].outcome, IntentAttemptOutcome::Succeeded);
        assert_eq!(receipt.planning_outcome, IntentPlanningOutcome::Rule);
        assert!(!provider.is_external());
    }

    #[test]
    fn requested_provider_uses_the_selected_surface_and_never_a_raw_model_name() {
        for (mode, llm, expected) in [
            (
                AiAccessMode::ProviderSubscriptionCli,
                LlmProviderType::Local,
                IntentProviderClass::RemoteModel,
            ),
            (
                AiAccessMode::ProviderApiKey,
                LlmProviderType::Local,
                IntentProviderClass::LocalModel,
            ),
            (
                AiAccessMode::ProviderOAuth,
                LlmProviderType::Remote,
                IntentProviderClass::RemoteModel,
            ),
        ] {
            let provider = ConfiguredLlmProvider::new(
                Arc::new(LocalLlmProvider::new()),
                &AiProviderConfig {
                    access_mode: mode,
                    llm_provider: llm,
                    ..Default::default()
                },
            );
            assert_eq!(provider.intent_requested_provider_class(), expected);
        }
    }
}
