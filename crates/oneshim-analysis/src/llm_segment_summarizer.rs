use std::sync::Arc;

use oneshim_core::models::tiered_memory::SegmentSummary;
use oneshim_core::ports::analysis_provider::AnalysisProvider;

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
/// Returns `None` if disabled, segment too short, or LLM call fails.
pub struct LlmSegmentSummarizer {
    analysis_provider: Arc<dyn AnalysisProvider>,
    pii_filter: PiiFilter,
    enabled: bool,
    min_segment_duration_secs: u64,
}

impl LlmSegmentSummarizer {
    pub fn new(
        provider: Arc<dyn AnalysisProvider>,
        pii_filter: PiiFilter,
        enabled: bool,
        min_duration: u64,
    ) -> Self {
        Self {
            analysis_provider: provider,
            pii_filter,
            enabled,
            min_segment_duration_secs: min_duration,
        }
    }

    /// Returns a shared reference to the underlying `AnalysisProvider`.
    ///
    /// Used by `DailyInsightGenerator` to reuse the same LLM connection
    /// for daily narrative generation.
    pub fn analysis_provider(&self) -> Arc<dyn AnalysisProvider> {
        self.analysis_provider.clone()
    }

    /// Generate an LLM summary for a closed segment.
    /// Returns `None` if disabled, segment too short, or LLM call fails.
    pub async fn summarize(&self, summary: &SegmentSummary) -> Option<String> {
        if !self.enabled || summary.duration_secs < self.min_segment_duration_secs {
            return None;
        }

        let context = self.build_segment_context(summary);
        match self
            .analysis_provider
            .summarize_text(&context, SEGMENT_SUMMARY_PROMPT)
            .await
        {
            Ok(text) => Some(text),
            Err(e) => {
                tracing::warn!("LLM segment summary failed: {e}");
                None
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
    use oneshim_core::error::CoreError;
    use oneshim_core::models::suggestion::Suggestion;
    use oneshim_core::models::tiered_memory::TriggerReason;
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
                code: oneshim_core::error_codes::ProviderCode::AnalysisFailed,
                message: "mock failure".into(),
            })
        }

        async fn summarize_text(
            &self,
            _context_json: &str,
            _system_prompt: &str,
        ) -> Result<String, CoreError> {
            Err(CoreError::Analysis {
                code: oneshim_core::error_codes::ProviderCode::AnalysisFailed,
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

        assert!(result.is_some());
        assert_eq!(result.unwrap(), "30-minute coding session in VSCode");
    }

    #[tokio::test]
    async fn disabled_returns_none() {
        let provider = Arc::new(MockAnalysisProvider {
            response: "should not be returned".to_string(),
        });
        let summarizer = LlmSegmentSummarizer::new(provider, identity_filter(), false, 60);

        let segment = make_segment(1800);
        let result = summarizer.summarize(&segment).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn short_segment_returns_none() {
        let provider = Arc::new(MockAnalysisProvider {
            response: "should not be returned".to_string(),
        });
        let summarizer = LlmSegmentSummarizer::new(provider, identity_filter(), true, 300); // min 5 mins

        let segment = make_segment(60); // only 1 minute
        let result = summarizer.summarize(&segment).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn llm_failure_returns_none() {
        let provider = Arc::new(FailingAnalysisProvider);
        let summarizer = LlmSegmentSummarizer::new(provider, identity_filter(), true, 60);

        let segment = make_segment(1800);
        let result = summarizer.summarize(&segment).await;
        assert!(result.is_none());
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
