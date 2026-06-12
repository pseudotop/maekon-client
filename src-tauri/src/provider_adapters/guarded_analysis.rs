use std::sync::Arc;

use async_trait::async_trait;
use maekon_core::error::CoreError;
use maekon_core::models::suggestion::Suggestion;
use maekon_core::ports::analysis_provider::AnalysisProvider;

use super::types::ExternalOcrPrivacyGuard;

pub(super) struct GuardedAnalysisProvider {
    inner: Arc<dyn AnalysisProvider>,
    privacy_guard: ExternalOcrPrivacyGuard,
}

impl GuardedAnalysisProvider {
    pub(super) fn new(
        inner: Arc<dyn AnalysisProvider>,
        privacy_guard: ExternalOcrPrivacyGuard,
    ) -> Self {
        Self {
            inner,
            privacy_guard,
        }
    }
}

#[async_trait]
impl AnalysisProvider for GuardedAnalysisProvider {
    async fn analyze(
        &self,
        context_json: &str,
        system_prompt: &str,
    ) -> Result<Vec<Suggestion>, CoreError> {
        let sanitized = self
            .privacy_guard
            .prepare_text_for_external_llm(
                context_json,
                system_prompt,
                self.inner.provider_name(),
                "analysis",
            )
            .await?;
        self.inner
            .analyze(&sanitized.context_json, &sanitized.system_prompt)
            .await
    }

    async fn summarize_text(
        &self,
        context_json: &str,
        system_prompt: &str,
    ) -> Result<String, CoreError> {
        let sanitized = self
            .privacy_guard
            .prepare_text_for_external_llm(
                context_json,
                system_prompt,
                self.inner.provider_name(),
                "summary",
            )
            .await?;
        self.inner
            .summarize_text(&sanitized.context_json, &sanitized.system_prompt)
            .await
    }

    fn provider_name(&self) -> &str {
        self.inner.provider_name()
    }
}
