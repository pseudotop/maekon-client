use std::sync::Arc;

use tracing::{debug, warn};

use oneshim_core::models::daily_digest::{
    DailyDigest, DailyInsight, DigestHighlight, HighlightType,
};
use oneshim_core::ports::analysis_provider::AnalysisProvider;

use crate::PiiFilter;

/// Prompt instructing the LLM to produce a JSON daily insight.
const DAILY_INSIGHT_PROMPT: &str = r#"You are a productivity coach analyzing a day of desktop work activity.

Given the day's activity data (segments with apps, content, work types, durations),
provide:
1. A 2-3 sentence narrative summary of the day
2. Up to 5 highlights — achievements, warnings, or suggestions
   Each highlight references a specific segment or pattern.

Respond in JSON:
{
  "narrative": "...",
  "highlights": [
    {"type": "achievement", "text": "...", "segment_id": "seg-42"},
    {"type": "warning", "text": "...", "segment_id": "seg-43"},
    {"type": "suggestion", "text": "..."}
  ]
}"#;

/// Generates LLM-powered narrative and highlights for a daily digest.
pub struct DailyInsightGenerator {
    analysis_provider: Arc<dyn AnalysisProvider>,
    pii_filter: PiiFilter,
}

impl DailyInsightGenerator {
    pub fn new(analysis_provider: Arc<dyn AnalysisProvider>, pii_filter: PiiFilter) -> Self {
        Self {
            analysis_provider,
            pii_filter,
        }
    }

    /// Generate a `DailyInsight` from LLM analysis of the digest.
    ///
    /// Returns `None` if the LLM is unavailable.
    /// Falls back to a statistics-based narrative if JSON parsing fails.
    pub async fn generate(&self, digest: &DailyDigest) -> Option<DailyInsight> {
        let context = self.build_context(digest);
        let filtered_context = (self.pii_filter)(&context);

        match self
            .analysis_provider
            .summarize_text(&filtered_context, DAILY_INSIGHT_PROMPT)
            .await
        {
            Ok(response) => {
                debug!(response_len = response.len(), "LLM daily insight response");
                // D5 iter-9: parse THEN sanitize narrative + highlight texts.
                // LLM response may echo back user context (app names, activity
                // descriptions) that slipped through input sanitization.
                match Self::parse_insight_response(&response).map(|mut insight| {
                    insight.narrative = (self.pii_filter)(&insight.narrative);
                    for highlight in &mut insight.highlights {
                        highlight.text = (self.pii_filter)(&highlight.text);
                    }
                    insight
                }) {
                    Some(insight) => Some(insight),
                    None => {
                        warn!("Failed to parse LLM response, using fallback narrative");
                        Some(Self::fallback_insight(&digest.statistics))
                    }
                }
            }
            Err(e) => {
                warn!(error = %e, "LLM unavailable for daily insight");
                None
            }
        }
    }

    /// Build a JSON context string from the digest for the LLM prompt.
    fn build_context(&self, digest: &DailyDigest) -> String {
        let mut parts = Vec::new();
        parts.push(format!("Date: {}", digest.date));
        parts.push(format!(
            "Statistics: deep_work={:.1}h, communication={:.1}h, meeting={:.1}h, context_switches={}, longest_focus={}min",
            digest.statistics.deep_work_hours,
            digest.statistics.communication_hours,
            digest.statistics.meeting_hours,
            digest.statistics.context_switches,
            digest.statistics.longest_focus_mins,
        ));

        if !digest.statistics.longest_focus_content.is_empty() {
            parts.push(format!(
                "Longest focus content: {}",
                digest.statistics.longest_focus_content
            ));
        }

        parts.push("Timeline:".to_string());
        for entry in &digest.timeline {
            let content_desc: Vec<String> = entry
                .content_summary
                .iter()
                .map(|c| format!("{} ({}min)", c.content, c.mins))
                .collect();
            parts.push(format!(
                "  [{} - {}] {} ({}) {}min — {} | {}",
                entry.start_time.format("%H:%M"),
                entry.end_time.format("%H:%M"),
                entry.segment_id,
                entry.regime_label,
                entry.duration_mins,
                entry.dominant_app,
                content_desc.join(", "),
            ));
        }

        if let Some(comp) = &digest.statistics.comparison {
            parts.push(format!(
                "vs previous day: deep_work {:+.1}h, communication {:+.1}h, context_switches {:+}",
                comp.deep_work_delta, comp.communication_delta, comp.context_switch_delta,
            ));
        }

        parts.join("\n")
    }

    /// Parse JSON insight from LLM response text.
    ///
    /// Handles markdown fences and extracts the first JSON object.
    fn parse_insight_response(text: &str) -> Option<DailyInsight> {
        let json_str = Self::extract_json(text)?;

        let value: serde_json::Value = serde_json::from_str(&json_str).ok()?;
        let narrative = value.get("narrative")?.as_str()?.to_string();
        if narrative.is_empty() {
            return None;
        }

        let highlights = value
            .get("highlights")
            .and_then(|h| h.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| {
                        let text = item.get("text")?.as_str()?.to_string();
                        let highlight_type = match item
                            .get("type")
                            .and_then(|t| t.as_str())
                            .unwrap_or("suggestion")
                        {
                            "achievement" => HighlightType::Achievement,
                            "warning" => HighlightType::Warning,
                            _ => HighlightType::Suggestion,
                        };
                        let segment_id = item
                            .get("segment_id")
                            .and_then(|s| s.as_str())
                            .map(String::from);
                        Some(DigestHighlight {
                            highlight_type,
                            text,
                            segment_id,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Some(DailyInsight {
            narrative,
            highlights,
        })
    }

    /// Extract JSON object from text that may be wrapped in markdown fences.
    fn extract_json(text: &str) -> Option<String> {
        let trimmed = text.trim();

        // Strip markdown fences if present
        let stripped = if trimmed.starts_with("```") {
            let inner = trimmed
                .strip_prefix("```json")
                .or_else(|| trimmed.strip_prefix("```"))
                .unwrap_or(trimmed);
            inner.strip_suffix("```").unwrap_or(inner).trim()
        } else {
            trimmed
        };

        // Find first { to last }
        let start = stripped.find('{')?;
        let end = stripped.rfind('}')?;
        if start >= end {
            return None;
        }

        Some(stripped[start..=end].to_string())
    }

    /// Fallback insight using basic statistics when LLM parsing fails.
    fn fallback_insight(
        stats: &oneshim_core::models::daily_digest::DailyStatistics,
    ) -> DailyInsight {
        DailyInsight {
            narrative: format!(
                "Today: deep work {:.1}h, communication {:.1}h, {} context switches.",
                stats.deep_work_hours, stats.communication_hours, stats.context_switches,
            ),
            highlights: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use oneshim_core::error::CoreError;
    use oneshim_core::models::daily_digest::DailyStatistics;
    use oneshim_core::models::suggestion::Suggestion;
    use std::collections::HashMap;

    // Mock AnalysisProvider for testing.
    // Uses Option<String>: Some(text) = success, None = LLM failure.
    struct MockAnalysisProvider {
        response_text: Option<String>,
    }

    #[async_trait::async_trait]
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
            match &self.response_text {
                Some(text) => Ok(text.clone()),
                None => Err(CoreError::Analysis {
                    code: oneshim_core::error_codes::ProviderCode::AnalysisFailed,
                    message: "LLM unavailable".into(),
                }),
            }
        }

        fn provider_name(&self) -> &str {
            "mock"
        }
    }

    fn make_test_digest() -> DailyDigest {
        DailyDigest {
            date: Utc::now().date_naive(),
            insight: None,
            timeline: vec![],
            statistics: DailyStatistics {
                deep_work_hours: 4.2,
                communication_hours: 0.5,
                meeting_hours: 0.25,
                context_switches: 8,
                longest_focus_mins: 120,
                longest_focus_content: "auth.rs".to_string(),
                regime_distribution: HashMap::new(),
                comparison: None,
            },
            generated_at: Utc::now(),
        }
    }

    fn identity_filter() -> PiiFilter {
        Box::new(|s: &str| s.to_string())
    }

    #[tokio::test]
    async fn valid_json_response_produces_insight() {
        let response = r#"{
            "narrative": "Great focus day with strong morning session.",
            "highlights": [
                {"type": "achievement", "text": "2h deep work block", "segment_id": "seg-42"},
                {"type": "warning", "text": "Late afternoon fatigue detected"}
            ]
        }"#;

        let provider = Arc::new(MockAnalysisProvider {
            response_text: Some(response.to_string()),
        });
        let generator = DailyInsightGenerator::new(provider, identity_filter());
        let digest = make_test_digest();

        let insight = generator.generate(&digest).await;
        assert!(insight.is_some());

        let insight = insight.unwrap();
        assert!(insight.narrative.contains("Great focus day"));
        assert_eq!(insight.highlights.len(), 2);
        assert_eq!(
            insight.highlights[0].highlight_type,
            HighlightType::Achievement
        );
        assert_eq!(insight.highlights[0].segment_id, Some("seg-42".to_string()));
        assert_eq!(insight.highlights[1].highlight_type, HighlightType::Warning);
        assert!(insight.highlights[1].segment_id.is_none());
    }

    #[tokio::test]
    async fn markdown_wrapped_response_parsed_correctly() {
        let response = r#"```json
{
    "narrative": "Productive coding session.",
    "highlights": [
        {"type": "suggestion", "text": "Try Pomodoro technique"}
    ]
}
```"#;

        let provider = Arc::new(MockAnalysisProvider {
            response_text: Some(response.to_string()),
        });
        let generator = DailyInsightGenerator::new(provider, identity_filter());

        let insight = generator.generate(&make_test_digest()).await;
        assert!(insight.is_some());
        let insight = insight.unwrap();
        assert!(insight.narrative.contains("Productive coding"));
        assert_eq!(insight.highlights.len(), 1);
        assert_eq!(
            insight.highlights[0].highlight_type,
            HighlightType::Suggestion
        );
    }

    #[tokio::test]
    async fn malformed_response_falls_back_to_stats() {
        let response = "This is not JSON at all, just some text about the day.";

        let provider = Arc::new(MockAnalysisProvider {
            response_text: Some(response.to_string()),
        });
        let generator = DailyInsightGenerator::new(provider, identity_filter());
        let digest = make_test_digest();

        let insight = generator.generate(&digest).await;
        assert!(insight.is_some());
        let insight = insight.unwrap();
        // Fallback narrative uses stats
        assert!(insight.narrative.contains("4.2"));
        assert!(insight.narrative.contains("0.5"));
        assert!(insight.highlights.is_empty());
    }

    #[tokio::test]
    async fn llm_failure_returns_none() {
        let provider = Arc::new(MockAnalysisProvider {
            response_text: None,
        });
        let generator = DailyInsightGenerator::new(provider, identity_filter());

        let insight = generator.generate(&make_test_digest()).await;
        assert!(insight.is_none());
    }

    #[test]
    fn extract_json_from_plain_text() {
        let text = r#"Some preamble {"narrative": "ok", "highlights": []} trailing text"#;
        let json = DailyInsightGenerator::extract_json(text);
        assert!(json.is_some());
        let parsed: serde_json::Value = serde_json::from_str(&json.unwrap()).unwrap();
        assert_eq!(parsed["narrative"], "ok");
    }

    #[test]
    fn extract_json_empty_returns_none() {
        assert!(DailyInsightGenerator::extract_json("no braces here").is_none());
    }

    #[test]
    fn parse_empty_narrative_returns_none() {
        let text = r#"{"narrative": "", "highlights": []}"#;
        assert!(DailyInsightGenerator::parse_insight_response(text).is_none());
    }
}
