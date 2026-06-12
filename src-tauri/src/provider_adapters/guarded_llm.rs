use std::sync::Arc;

use async_trait::async_trait;
use maekon_core::error::CoreError;
use maekon_core::ports::llm_provider::{
    InterpretedAction, LlmProvider, ScreenContext, SkillContext,
};

use super::types::ExternalOcrPrivacyGuard;

pub(super) struct GuardedLlmProvider {
    inner: Arc<dyn LlmProvider>,
    privacy_guard: ExternalOcrPrivacyGuard,
}

impl GuardedLlmProvider {
    pub(super) fn new(inner: Arc<dyn LlmProvider>, privacy_guard: ExternalOcrPrivacyGuard) -> Self {
        Self {
            inner,
            privacy_guard,
        }
    }
}

#[async_trait]
impl LlmProvider for GuardedLlmProvider {
    async fn interpret_intent(
        &self,
        screen_context: &ScreenContext,
        intent_hint: &str,
    ) -> Result<InterpretedAction, CoreError> {
        if !self.inner.is_external() {
            return self
                .inner
                .interpret_intent(screen_context, intent_hint)
                .await;
        }

        let sanitized = self
            .privacy_guard
            .prepare_screen_context_for_external_llm(screen_context, self.inner.provider_name())
            .await?;
        self.inner.interpret_intent(&sanitized, intent_hint).await
    }

    async fn interpret_intent_with_skills(
        &self,
        screen_context: &ScreenContext,
        intent_hint: &str,
        skill_ctx: &SkillContext,
    ) -> Result<InterpretedAction, CoreError> {
        if !self.inner.is_external() {
            return self
                .inner
                .interpret_intent_with_skills(screen_context, intent_hint, skill_ctx)
                .await;
        }

        let sanitized = self
            .privacy_guard
            .prepare_screen_context_for_external_llm(screen_context, self.inner.provider_name())
            .await?;
        self.inner
            .interpret_intent_with_skills(&sanitized, intent_hint, skill_ctx)
            .await
    }

    fn provider_name(&self) -> &str {
        self.inner.provider_name()
    }

    fn is_external(&self) -> bool {
        self.inner.is_external()
    }
}
