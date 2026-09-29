use std::sync::Arc;

use maekon_core::models::ai_summary::{
    AiSummaryArtifact, AiSummaryFailureReason, AiSummaryProviderClass,
};
use maekon_core::models::tiered_memory::SegmentSummary;
use maekon_core::ports::analysis_provider::AnalysisProvider;

use crate::PiiFilter;

/// Prompt used to instruct the LLM for segment summarization.
pub const SEGMENT_SUMMARY_PROMPT: &str = r#"You are summarizing a desktop work session segment.
Given the segment data, write a concise 1-2 sentence summary.
Examples:
- "45-minute deep coding session on auth.rs with 3 brief Slack interruptions"
- "Research session: browsing docs about async Rust patterns"
Respond with ONLY the summary text."#;

/// Generates natural language summaries for closed segments via an LLM.
///
/// Uses the `AnalysisProvider::summarize_text()` method to call the LLM.
/// Applies PII filtering to content activity labels before sending.
/// Returns a privacy-safe artifact for generated and unavailable outcomes.
pub struct LlmSegmentSummarizer {
    analysis_provider: Arc<dyn AnalysisProvider>,
    pii_filter: PiiFilter,
    enabled: bool,
    min_segment_duration_secs: u64,
    provider_class: AiSummaryProviderClass,
}

impl LlmSegmentSummarizer {
    pub fn new(
        provider: Arc<dyn AnalysisProvider>,
        pii_filter: PiiFilter,
        enabled: bool,
        min_duration: u64,
    ) -> Self {
        Self::new_with_provider_class(
            provider,
            pii_filter,
            enabled,
            min_duration,
            AiSummaryProviderClass::Unknown,
        )
    }

    pub fn new_with_provider_class(
        provider: Arc<dyn AnalysisProvider>,
        pii_filter: PiiFilter,
        enabled: bool,
        min_duration: u64,
        provider_class: AiSummaryProviderClass,
    ) -> Self {
        Self {
            analysis_provider: provider,
            pii_filter,
            enabled,
            min_segment_duration_secs: min_duration,
            provider_class,
        }
    }

    /// Returns a shared reference to the underlying `AnalysisProvider`.
    ///
    /// Used by `DailyInsightGenerator` to reuse the same LLM connection
    /// for daily narrative generation.
    pub fn analysis_provider(&self) -> Arc<dyn AnalysisProvider> {
        self.analysis_provider.clone()
    }

    pub fn provider_class(&self) -> AiSummaryProviderClass {
        self.provider_class
    }

    /// Generate an LLM summary for a closed segment.
    /// Returns a privacy-safe artifact for every outcome so the caller can
    /// persist generated, disabled, short-segment, and provider-failure states
    /// without retaining raw prompts or provider error bodies.
    pub async fn summarize(&self, summary: &SegmentSummary) -> AiSummaryArtifact {
        if !self.enabled {
            return AiSummaryArtifact::unavailable(
                Some(self.provider_class),
                AiSummaryFailureReason::PipelineDisabled,
            );
        }
        if summary.duration_secs < self.min_segment_duration_secs {
            return AiSummaryArtifact::unavailable(
                Some(self.provider_class),
                AiSummaryFailureReason::BelowMinimumDuration,
            );
        }

        let context = self.build_segment_context(summary);
        match self
            .analysis_provider
            .summarize_text(&context, SEGMENT_SUMMARY_PROMPT)
            .await
        {
            Ok(text) => {
                // Provider output may echo user context. Apply the same privacy
                // filter again immediately before persistence/presentation.
                let filtered_text = (self.pii_filter)(&text);
                if filtered_text.trim().is_empty() {
                    AiSummaryArtifact::unavailable(
                        Some(self.provider_class),
                        AiSummaryFailureReason::InvalidResponse,
                    )
                } else {
                    AiSummaryArtifact::generated(
                        filtered_text.trim().to_string(),
                        self.provider_class,
                        chrono::Utc::now(),
                    )
                }
            }
            Err(e) => {
                tracing::warn!("LLM segment summary failed: {e}");
                AiSummaryArtifact::unavailable(
                    Some(self.provider_class),
                    AiSummaryFailureReason::ProviderFailed,
                )
            }
        }
    }

    /// Build a JSON context string from the segment summary for the LLM.
    fn build_segment_context(&self, summary: &SegmentSummary) -> String {
        serde_json::json!({
            "duration_mins": summary.duration_secs / 60,
            "dominant_category": summary.dominant_category,
            "apps": summary.app_breakdown,
            "context_switches": summary.context_switch_count,
            "content": summary.content_activities.iter().map(|a| {
                serde_json::json!({
                    "content": (self.pii_filter)(&a.content_label),
                    "work_type": format!("{:?}", a.work_type),
                    "mins": a.duration_secs / 60
                })
            }).collect::<Vec<_>>()
        })
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::Utc;
    use maekon_core::error::CoreError;
    use maekon_core::models::suggestion::Suggestion;
    use maekon_core::models::tiered_memory::TriggerReason;
    use std::collections::HashMap;

    /// Mock AnalysisProvider that returns a fixed summary.
    struct MockAnalysisProvider {
        response: String,
    }

    #[async_trait]
    impl AnalysisProvider for MockAnalysisProvider {
        async fn analyze(
            &self,
            _context_json: &str,
            _system_prompt: &str,
        ) -> Result<Vec<Suggestion>, CoreError> {
            Ok(vec![])
        }

        async fn summarize_text(
            &self,
            _context_json: &str,
            _system_prompt: &str,
        ) -> Result<String, CoreError> {
            Ok(self.response.clone())
        }

        fn provider_name(&self) -> &str {
            "mock"
        }
    }

    /// Mock that always fails.
    struct FailingAnalysisProvider;

    #[async_trait]
    impl AnalysisProvider for FailingAnalysisProvider {
        async fn analyze(
            &self,
            _context_json: &str,
            _system_prompt: &str,
        ) -> Result<Vec<Suggestion>, CoreError> {
            Err(CoreError::Analysis {
                code: maekon_core::error_codes::ProviderCode::AnalysisFailed,
                message: "mock failure".into(),
            })
        }

        async fn summarize_text(
            &self,
            _context_json: &str,
            _system_prompt: &str,
        ) -> Result<String, CoreError> {
            Err(CoreError::Analysis {
                code: maekon_core::error_codes::ProviderCode::AnalysisFailed,
                message: "mock failure".into(),
            })
        }

        fn provider_name(&self) -> &str {
            "failing-mock"
        }
    }

    fn make_segment(duration_secs: u64) -> SegmentSummary {
        SegmentSummary {
            segment_id: "seg-test-001".to_string(),
            start_time: Utc::now(),
            end_time: Utc::now(),
            duration_secs,
            regime_id: None,
            trigger_reason: TriggerReason::ForcedMaxDuration,
            event_count: 50,
            app_breakdown: HashMap::from([("VSCode".to_string(), 1800)]),
            category_breakdown: HashMap::from([("Development".to_string(), 1800)]),
            context_switch_count: 3,
            dominant_category: "Development".to_string(),
            avg_importance: 0.7,
            patterns_detected: vec![],
            content_activities: vec![],
            container: None,
            llm_summary: None,
        }
    }

    fn identity_filter() -> PiiFilter {
        Box::new(|s: &str| s.to_string())
    }

    #[tokio::test]
    async fn summarize_returns_text() {
        let provider = Arc::new(MockAnalysisProvider {
            response: "30-minute coding session in VSCode".to_string(),
        });
        let summarizer = LlmSegmentSummarizer::new(provider, identity_filter(), true, 60);

        let segment = make_segment(1800); // 30 mins
        let result = summarizer.summarize(&segment).await;

        assert!(result.is_generated());
        assert_eq!(
            result.text.as_deref(),
            Some("30-minute coding session in VSCode")
        );
    }

    #[test]
    fn exposes_the_configured_provider_class() {
        let provider = Arc::new(MockAnalysisProvider {
            response: "unused".to_string(),
        });
        let summarizer = LlmSegmentSummarizer::new_with_provider_class(
            provider,
            identity_filter(),
            true,
            60,
            AiSummaryProviderClass::ExternalApi,
        );

        assert_eq!(
            summarizer.provider_class(),
            AiSummaryProviderClass::ExternalApi
        );
    }

    #[tokio::test]
    async fn segment_at_minimum_duration_is_eligible() {
        let provider = Arc::new(MockAnalysisProvider {
            response: "five-minute focused session".to_string(),
        });
        let summarizer = LlmSegmentSummarizer::new(provider, identity_filter(), true, 300);

        let result = summarizer.summarize(&make_segment(300)).await;

        assert!(result.is_generated());
        assert_eq!(result.text.as_deref(), Some("five-minute focused session"));
    }

    #[tokio::test]
    async fn provider_output_is_filtered_before_it_becomes_a_persisted_artifact() {
        let provider = Arc::new(MockAnalysisProvider {
            response: "Alice reviewed the launch plan".to_string(),
        });
        let summarizer = LlmSegmentSummarizer::new(
            provider,
            Box::new(|text| text.replace("Alice", "[REDACTED]")),
            true,
            60,
        );

        let result = summarizer.summarize(&make_segment(1800)).await;

        assert_eq!(
            result.text.as_deref(),
            Some("[REDACTED] reviewed the launch plan")
        );
        assert!(result.is_generated());
    }

    #[tokio::test]
    async fn disabled_returns_none() {
        let provider = Arc::new(MockAnalysisProvider {
            response: "should not be returned".to_string(),
        });
        let summarizer = LlmSegmentSummarizer::new(provider, identity_filter(), false, 60);

        let segment = make_segment(1800);
        let result = summarizer.summarize(&segment).await;
        assert_eq!(
            result.failure_reason,
            Some(AiSummaryFailureReason::PipelineDisabled)
        );
    }

    #[tokio::test]
    async fn short_segment_returns_none() {
        let provider = Arc::new(MockAnalysisProvider {
            response: "should not be returned".to_string(),
        });
        let summarizer = LlmSegmentSummarizer::new(provider, identity_filter(), true, 300); // min 5 mins

        let segment = make_segment(60); // only 1 minute
        let result = summarizer.summarize(&segment).await;
        assert_eq!(
            result.failure_reason,
            Some(AiSummaryFailureReason::BelowMinimumDuration)
        );
    }

    #[tokio::test]
    async fn llm_failure_returns_none() {
        let provider = Arc::new(FailingAnalysisProvider);
        let summarizer = LlmSegmentSummarizer::new(provider, identity_filter(), true, 60);

        let segment = make_segment(1800);
        let result = summarizer.summarize(&segment).await;
        assert_eq!(
            result.failure_reason,
            Some(AiSummaryFailureReason::ProviderFailed)
        );
    }

    #[test]
    fn build_segment_context_produces_valid_json() {
        let provider = Arc::new(MockAnalysisProvider {
            response: "unused".to_string(),
        });
        let summarizer = LlmSegmentSummarizer::new(provider, identity_filter(), true, 60);

        let segment = make_segment(1800);
        let json = summarizer.build_segment_context(&segment);

        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["duration_mins"], 30);
        assert_eq!(parsed["dominant_category"], "Development");
        assert_eq!(parsed["context_switches"], 3);
    }
}
