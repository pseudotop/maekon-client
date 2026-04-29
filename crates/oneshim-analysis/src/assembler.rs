use chrono::{DateTime, Utc};
use oneshim_core::models::analysis::ActivityPattern;
use oneshim_core::models::event::Event;
use serde::Serialize;

use crate::prompts::{FewShotExample, PromptBuilder, ANALYSIS_SYSTEM_PROMPT};

/// Injected PII filter function: takes raw text, returns sanitized text.
pub type PiiFilter = Box<dyn Fn(&str) -> String + Send + Sync>;

/// Assembled context ready for LLM analysis.
#[derive(Debug, Clone)]
pub struct AnalysisContext {
    /// Structured JSON context (~2-4K tokens).
    pub user_context_json: String,
    /// System prompt instructing the LLM.
    pub system_prompt: String,
}

/// Current segment statistics for LLM context enrichment.
#[derive(Debug, Clone)]
pub struct SegmentStats {
    pub duration_mins: u32,
    pub regime_label: Option<String>,
    pub event_count: u32,
    pub context_switches: u32,
    pub dominant_category: String,
    pub content_summary: Vec<ContentSummaryEntry>,
    /// Aggregated GUI patterns across all content activities in this segment.
    pub gui_patterns: Vec<String>,
}

/// A single content activity summary within a segment.
#[derive(Debug, Clone)]
pub struct ContentSummaryEntry {
    pub content: String,
    pub content_type: String,
    pub work_type: String,
    pub mins: u32,
    /// GUI activity summary line (e.g., "15 clicks, 3 saves, 2 test runs").
    /// When present, this enriches the content field in the LLM context.
    pub gui_summary_line: Option<String>,
    /// GUI behavioral patterns detected from this content activity (e.g. "TestDrivenDevelopment").
    pub gui_patterns: Vec<String>,
}

/// Current desktop activity snapshot.
#[derive(Debug, Clone)]
pub struct CurrentActivity {
    pub app_name: String,
    pub window_title: String,
    pub ocr_hint: Option<String>,
    pub focus_score: f32,
    pub deep_work_mins: u32,
    /// Accessibility-extracted text from the focused element.
    /// Only present at Basic/Off PII levels. PII-filtered before
    /// reaching this struct (filtered by AccessibilityExtractor).
    pub accessibility_text: Option<String>,
}

impl Default for CurrentActivity {
    fn default() -> Self {
        Self {
            app_name: String::new(),
            window_title: String::new(),
            ocr_hint: None,
            focus_score: 0.0,
            deep_work_mins: 0,
            accessibility_text: None,
        }
    }
}

/// Aggregated session metrics.
#[derive(Debug, Clone, Default)]
pub struct SessionMetrics {
    pub total_work_mins: u32,
    pub context_switches: u32,
    pub communication_ratio: f32,
}

/// A relevant historical entry retrieved via RAG vector search.
#[derive(Debug, Clone)]
pub struct RelevantHistoryEntry {
    /// Human-readable relative time (e.g., "2 hours ago", "3 days ago").
    pub when: String,
    /// Summary text of the historical activity.
    pub summary: String,
    /// Similarity score from vector search (0.0–1.0).
    pub similarity: f32,
}

/// Compute a human-readable relative time string from a timestamp.
pub fn humanize_time_ago(ts: DateTime<Utc>) -> String {
    let hours = (Utc::now() - ts).num_hours();
    if hours < 1 {
        "just now".to_string()
    } else if hours < 24 {
        format!("{hours} hours ago")
    } else {
        format!("{} days ago", hours / 24)
    }
}

/// Builds structured LLM context from raw activity data.
///
/// Accepts an injected PII filter closure so that the comprehensive filter
/// from `oneshim-vision` can be reused without creating a direct dependency
/// between adapter crates (per ADR-011 §1).
pub struct ContextAssembler {
    pii_filter: PiiFilter,
}

#[derive(Serialize)]
struct ContextPayload {
    current: CurrentSnapshot,
    recent_activity: Vec<RecentEvent>,
    patterns: Vec<PatternEntry>,
    session: SessionSnapshot,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_segment: Option<SegmentSnapshot>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    relevant_history: Vec<HistoryEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gui: Option<GuiSection>,
}

#[derive(Serialize)]
struct GuiSection {
    patterns: Vec<String>,
    actions: GuiActionCounts,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    top_elements: Vec<(String, String, u32)>,
}

#[derive(Serialize, Default)]
struct GuiActionCounts {
    saves: u32,
    test_runs: u32,
    searches: u32,
    builds: u32,
    undo_redos: u32,
    copy_pastes: u32,
}

#[derive(Serialize)]
struct HistoryEntry {
    when: String,
    summary: String,
    similarity: f32,
}

#[derive(Serialize)]
struct SegmentSnapshot {
    duration_mins: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    regime_label: Option<String>,
    event_count: u32,
    context_switches: u32,
    dominant_category: String,
    content_summary: Vec<ContentSummaryItem>,
}

#[derive(Serialize)]
struct ContentSummaryItem {
    content: String,
    content_type: String,
    work_type: String,
    mins: u32,
}

#[derive(Serialize)]
struct CurrentSnapshot {
    app: String,
    window: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    ocr_hint: Option<String>,
    focus_score: f32,
    deep_work_mins: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    accessibility_text: Option<String>,
}

#[derive(Serialize)]
struct RecentEvent {
    time: String,
    app: String,
    duration_secs: i64,
}

#[derive(Serialize)]
struct PatternEntry {
    #[serde(rename = "type")]
    pattern_type: String,
    desc: String,
    confidence: f32,
}

#[derive(Serialize)]
struct SessionSnapshot {
    total_work_mins: u32,
    context_switches: u32,
    communication_ratio: f32,
}

impl ContextAssembler {
    pub fn new(pii_filter: PiiFilter) -> Self {
        Self { pii_filter }
    }

    /// Build analysis context from activity data.
    ///
    /// When `segment_stats` is provided, a `"current_segment"` key is added to
    /// the JSON payload so the LLM can reason about the active work segment.
    pub fn build(
        &self,
        current: &CurrentActivity,
        events: &[Event],
        patterns: &[ActivityPattern],
        metrics: &SessionMetrics,
    ) -> AnalysisContext {
        self.build_with_segment(current, events, patterns, metrics, None)
    }

    /// Build analysis context with optional segment enrichment.
    pub fn build_with_segment(
        &self,
        current: &CurrentActivity,
        events: &[Event],
        patterns: &[ActivityPattern],
        metrics: &SessionMetrics,
        segment_stats: Option<&SegmentStats>,
    ) -> AnalysisContext {
        self.build_with_history(current, events, patterns, metrics, segment_stats, &[])
    }

    /// Build analysis context with optional segment enrichment and RAG-retrieved history.
    pub fn build_with_history(
        &self,
        current: &CurrentActivity,
        events: &[Event],
        patterns: &[ActivityPattern],
        metrics: &SessionMetrics,
        segment_stats: Option<&SegmentStats>,
        relevant_history: &[RelevantHistoryEntry],
    ) -> AnalysisContext {
        let recent_activity = self.extract_recent_events(events);

        let pattern_entries: Vec<PatternEntry> = patterns
            .iter()
            .map(|p| PatternEntry {
                pattern_type: format!("{:?}", p.pattern_type),
                desc: p.description.clone(),
                confidence: p.confidence,
            })
            .collect();

        let current_segment = segment_stats.map(|stats| SegmentSnapshot {
            duration_mins: stats.duration_mins,
            regime_label: stats.regime_label.clone(),
            event_count: stats.event_count,
            context_switches: stats.context_switches,
            dominant_category: stats.dominant_category.clone(),
            content_summary: stats
                .content_summary
                .iter()
                .map(|e| {
                    // When GUI summary line is present, enrich the content field
                    let content = if let Some(ref gui_line) = e.gui_summary_line {
                        format!("{} ({})", e.content, gui_line)
                    } else {
                        e.content.clone()
                    };
                    ContentSummaryItem {
                        content,
                        content_type: e.content_type.clone(),
                        work_type: e.work_type.clone(),
                        mins: e.mins,
                    }
                })
                .collect(),
        });

        let history_entries: Vec<HistoryEntry> = relevant_history
            .iter()
            .map(|h| HistoryEntry {
                when: h.when.clone(),
                summary: h.summary.clone(),
                similarity: h.similarity,
            })
            .collect();

        let gui = segment_stats.and_then(|stats| {
            let has_patterns = !stats.gui_patterns.is_empty();
            let has_gui_lines = stats
                .content_summary
                .iter()
                .any(|e| e.gui_summary_line.is_some());
            if !has_patterns && !has_gui_lines {
                return None;
            }

            let mut actions = GuiActionCounts::default();
            let all_top: Vec<(String, String, u32)> = Vec::new();

            // Parse action counts from gui_summary_line strings
            for entry in &stats.content_summary {
                if let Some(ref line) = entry.gui_summary_line {
                    Self::parse_gui_action_counts(line, &mut actions);
                }
            }

            Some(GuiSection {
                patterns: stats.gui_patterns.clone(),
                actions,
                top_elements: all_top,
            })
        });

        let payload = ContextPayload {
            current: CurrentSnapshot {
                app: current.app_name.clone(),
                window: self.filter_pii(&current.window_title),
                ocr_hint: current.ocr_hint.as_ref().map(|t| self.filter_pii(t)),
                focus_score: current.focus_score,
                deep_work_mins: current.deep_work_mins,
                accessibility_text: current
                    .accessibility_text
                    .as_ref()
                    .map(|t| self.filter_pii(t)),
            },
            recent_activity,
            patterns: pattern_entries,
            session: SessionSnapshot {
                total_work_mins: metrics.total_work_mins,
                context_switches: metrics.context_switches,
                communication_ratio: metrics.communication_ratio,
            },
            current_segment,
            relevant_history: history_entries,
            gui,
        };

        let user_context_json =
            serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string());

        AnalysisContext {
            user_context_json,
            system_prompt: ANALYSIS_SYSTEM_PROMPT.to_string(),
        }
    }

    /// Build analysis context with few-shot examples and optional regime hint.
    /// Uses PromptBuilder instead of the hardcoded ANALYSIS_SYSTEM_PROMPT.
    #[allow(clippy::too_many_arguments)]
    pub fn build_with_few_shot(
        &self,
        current: &CurrentActivity,
        events: &[Event],
        patterns: &[ActivityPattern],
        metrics: &SessionMetrics,
        segment_stats: Option<&SegmentStats>,
        relevant_history: &[RelevantHistoryEntry],
        few_shot_examples: &[FewShotExample],
        regime_hint: Option<&str>,
    ) -> AnalysisContext {
        // Build the context JSON payload (reuse existing method)
        let base = self.build_with_history(
            current,
            events,
            patterns,
            metrics,
            segment_stats,
            relevant_history,
        );

        // Build enriched system prompt
        let mut pb = PromptBuilder::new().with_examples(few_shot_examples.to_vec());
        if let Some(regime) = regime_hint {
            pb = pb.with_regime(regime);
        }

        AnalysisContext {
            user_context_json: base.user_context_json,
            system_prompt: pb.build(),
        }
    }

    fn extract_recent_events(&self, events: &[Event]) -> Vec<RecentEvent> {
        let ctx_events: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                Event::Context(ctx) => Some(ctx),
                _ => None,
            })
            .collect();

        let mut result = Vec::new();
        for pair in ctx_events.windows(2) {
            let duration = (pair[1].timestamp - pair[0].timestamp).num_seconds();
            result.push(RecentEvent {
                time: pair[0].timestamp.format("%H:%M").to_string(),
                app: pair[0].app_name.clone(),
                duration_secs: duration.abs(),
            });
        }
        // Add last event with 0 duration (still active)
        if let Some(last) = ctx_events.last() {
            result.push(RecentEvent {
                time: last.timestamp.format("%H:%M").to_string(),
                app: last.app_name.clone(),
                duration_secs: 0,
            });
        }

        result
    }

    /// Parse action counts from a gui_summary_line like "5 clicks, 3 saves, 2 test runs".
    fn parse_gui_action_counts(line: &str, counts: &mut GuiActionCounts) {
        let lower = line.to_lowercase();
        for part in lower.split(',') {
            let part = part.trim();
            if let Some(n) = Self::extract_leading_number(part) {
                if part.contains("save") {
                    counts.saves += n;
                } else if part.contains("test") {
                    counts.test_runs += n;
                } else if part.contains("search") || part.contains("find") {
                    counts.searches += n;
                } else if part.contains("build") {
                    counts.builds += n;
                } else if part.contains("undo") || part.contains("redo") {
                    counts.undo_redos += n;
                } else if part.contains("copy") || part.contains("paste") {
                    counts.copy_pastes += n;
                }
            }
        }
    }

    fn extract_leading_number(s: &str) -> Option<u32> {
        s.split_whitespace()
            .next()
            .and_then(|tok| tok.parse::<u32>().ok())
    }

    fn filter_pii(&self, text: &str) -> String {
        (self.pii_filter)(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use oneshim_core::models::event::ContextEvent;

    /// Identity filter (no masking) for tests that don't exercise PII.
    fn noop_filter() -> PiiFilter {
        Box::new(|text: &str| text.to_string())
    }

    /// Simple test filter that replaces emails with [EMAIL] and long secrets with [REDACTED].
    fn test_pii_filter() -> PiiFilter {
        Box::new(|text: &str| {
            let mut result = text.to_string();
            // Mask email-like patterns
            if let Some(at_pos) = result.find('@') {
                let start = result[..at_pos]
                    .rfind(|c: char| c.is_whitespace() || c == '<' || c == '(' || c == ',')
                    .map(|p| p + 1)
                    .unwrap_or(0);
                let end = result[at_pos + 1..]
                    .find(|c: char| c.is_whitespace() || c == '>' || c == ')' || c == ',')
                    .map(|p| at_pos + 1 + p)
                    .unwrap_or(result.len());
                if end > at_pos + 1 && result[at_pos + 1..end].contains('.') {
                    result = format!("{}[EMAIL]{}", &result[..start], &result[end..]);
                }
            }
            // Mask long secret-like tokens (32+ chars with mixed case/digits)
            let words: Vec<&str> = result.split('=').collect();
            if words.len() == 2 {
                let token = words[1].trim();
                if token.len() >= 32
                    && token.chars().any(|c| c.is_uppercase())
                    && token.chars().any(|c| c.is_ascii_digit())
                {
                    return format!("{}=[REDACTED]", words[0]);
                }
            }
            result
        })
    }

    fn make_current() -> CurrentActivity {
        CurrentActivity {
            app_name: "VSCode".to_string(),
            window_title: "main.rs - oneshim".to_string(),
            ocr_hint: Some("fn analyze()".to_string()),
            focus_score: 0.82,
            deep_work_mins: 45,
            accessibility_text: None,
        }
    }

    fn make_events() -> Vec<Event> {
        vec![
            Event::Context(ContextEvent {
                app_name: "Slack".to_string(),
                window_title: "General".to_string(),
                timestamp: Utc::now() - Duration::minutes(10),
                ..Default::default()
            }),
            Event::Context(ContextEvent {
                app_name: "VSCode".to_string(),
                window_title: "main.rs".to_string(),
                timestamp: Utc::now() - Duration::minutes(5),
                ..Default::default()
            }),
        ]
    }

    fn make_metrics() -> SessionMetrics {
        SessionMetrics {
            total_work_mins: 180,
            context_switches: 24,
            communication_ratio: 0.35,
        }
    }

    #[test]
    fn build_context_produces_valid_json() {
        let assembler = ContextAssembler::new(noop_filter());
        let ctx = assembler.build(&make_current(), &make_events(), &[], &make_metrics());

        let parsed: serde_json::Value =
            serde_json::from_str(&ctx.user_context_json).expect("should be valid JSON");
        assert_eq!(parsed["current"]["app"], "VSCode");
        assert_eq!(parsed["session"]["total_work_mins"], 180);
    }

    #[test]
    fn pii_filter_masks_email() {
        let assembler = ContextAssembler::new(test_pii_filter());
        let current = CurrentActivity {
            app_name: "Chrome".to_string(),
            window_title: "Inbox - user@example.com".to_string(),
            ocr_hint: None,
            focus_score: 0.5,
            deep_work_mins: 10,
            accessibility_text: None,
        };
        let ctx = assembler.build(&current, &[], &[], &make_metrics());
        assert!(ctx.user_context_json.contains("[EMAIL]"));
        assert!(!ctx.user_context_json.contains("user@example.com"));
    }

    #[test]
    fn empty_events_produces_empty_recent() {
        let assembler = ContextAssembler::new(noop_filter());
        let ctx = assembler.build(&make_current(), &[], &[], &make_metrics());
        let parsed: serde_json::Value = serde_json::from_str(&ctx.user_context_json).unwrap();
        assert!(parsed["recent_activity"].as_array().unwrap().is_empty());
    }

    #[test]
    fn system_prompt_is_not_empty() {
        let assembler = ContextAssembler::new(noop_filter());
        let ctx = assembler.build(&make_current(), &[], &[], &make_metrics());
        assert!(!ctx.system_prompt.is_empty());
        assert!(ctx.system_prompt.contains("productivity"));
    }

    #[test]
    fn pii_filter_masks_secret() {
        let assembler = ContextAssembler::new(test_pii_filter());
        let current = CurrentActivity {
            app_name: "Terminal".to_string(),
            window_title: "export API_KEY=xK9mPqR2sT4uV6wX8yZ0aB3cD5eF7gH9iJ1".to_string(),
            ocr_hint: None,
            focus_score: 0.3,
            deep_work_mins: 5,
            accessibility_text: None,
        };
        let ctx = assembler.build(&current, &[], &[], &make_metrics());
        assert!(ctx.user_context_json.contains("[REDACTED]"));
        assert!(!ctx
            .user_context_json
            .contains("xK9mPqR2sT4uV6wX8yZ0aB3cD5eF7gH9iJ1"));
    }

    #[test]
    fn pii_off_does_not_filter() {
        let assembler = ContextAssembler::new(noop_filter());
        let current = CurrentActivity {
            app_name: "Chrome".to_string(),
            window_title: "user@example.com".to_string(),
            ocr_hint: None,
            focus_score: 0.5,
            deep_work_mins: 10,
            accessibility_text: None,
        };
        let ctx = assembler.build(&current, &[], &[], &make_metrics());
        assert!(ctx.user_context_json.contains("user@example.com"));
    }

    #[test]
    fn recent_events_include_duration() {
        let assembler = ContextAssembler::new(noop_filter());
        let ctx = assembler.build(&make_current(), &make_events(), &[], &make_metrics());
        let parsed: serde_json::Value = serde_json::from_str(&ctx.user_context_json).unwrap();
        let recent = parsed["recent_activity"].as_array().unwrap();
        // 2 context events -> 1 pair + 1 last = 2 entries
        assert_eq!(recent.len(), 2);
        // First entry should have ~300s duration (5 min gap)
        assert!(recent[0]["duration_secs"].as_i64().unwrap() > 0);
        // Last entry should have 0 duration (still active)
        assert_eq!(recent[1]["duration_secs"].as_i64().unwrap(), 0);
    }

    #[test]
    fn build_without_segment_omits_key() {
        let assembler = ContextAssembler::new(noop_filter());
        let ctx = assembler.build(&make_current(), &[], &[], &make_metrics());
        let parsed: serde_json::Value = serde_json::from_str(&ctx.user_context_json).unwrap();
        assert!(parsed.get("current_segment").is_none());
    }

    #[test]
    fn build_with_history_includes_relevant_history() {
        let assembler = ContextAssembler::new(noop_filter());
        let history = vec![
            RelevantHistoryEntry {
                when: "2 hours ago".to_string(),
                summary: "Deep coding on auth.rs".to_string(),
                similarity: 0.85,
            },
            RelevantHistoryEntry {
                when: "yesterday".to_string(),
                summary: "Auth module testing".to_string(),
                similarity: 0.72,
            },
        ];
        let ctx = assembler.build_with_history(
            &make_current(),
            &[],
            &[],
            &make_metrics(),
            None,
            &history,
        );
        let parsed: serde_json::Value = serde_json::from_str(&ctx.user_context_json).unwrap();
        let hist = parsed["relevant_history"].as_array().unwrap();
        assert_eq!(hist.len(), 2);
        assert_eq!(hist[0]["when"], "2 hours ago");
        assert_eq!(hist[0]["summary"], "Deep coding on auth.rs");
        assert!((hist[0]["similarity"].as_f64().unwrap() - 0.85).abs() < 0.01);
        assert_eq!(hist[1]["when"], "yesterday");
    }

    #[test]
    fn build_with_empty_history_omits_key() {
        let assembler = ContextAssembler::new(noop_filter());
        let ctx =
            assembler.build_with_history(&make_current(), &[], &[], &make_metrics(), None, &[]);
        let parsed: serde_json::Value = serde_json::from_str(&ctx.user_context_json).unwrap();
        // Empty history is skip_serializing_if = "Vec::is_empty"
        assert!(parsed.get("relevant_history").is_none());
    }

    #[test]
    fn humanize_time_ago_just_now() {
        let ts = Utc::now();
        assert_eq!(humanize_time_ago(ts), "just now");
    }

    #[test]
    fn humanize_time_ago_hours() {
        let ts = Utc::now() - Duration::hours(5);
        assert_eq!(humanize_time_ago(ts), "5 hours ago");
    }

    #[test]
    fn humanize_time_ago_days() {
        let ts = Utc::now() - Duration::hours(50);
        assert_eq!(humanize_time_ago(ts), "2 days ago");
    }

    #[test]
    fn build_with_segment_includes_segment() {
        let assembler = ContextAssembler::new(noop_filter());
        let stats = SegmentStats {
            duration_mins: 12,
            regime_label: Some("deep_work".to_string()),
            event_count: 45,
            context_switches: 3,
            dominant_category: "Development".to_string(),
            content_summary: vec![ContentSummaryEntry {
                content: "main.rs".to_string(),
                content_type: "File".to_string(),
                work_type: "ActiveCoding".to_string(),
                mins: 10,
                gui_summary_line: None,
                gui_patterns: vec![],
            }],
            gui_patterns: vec![],
        };
        let ctx =
            assembler.build_with_segment(&make_current(), &[], &[], &make_metrics(), Some(&stats));
        let parsed: serde_json::Value = serde_json::from_str(&ctx.user_context_json).unwrap();
        let seg = &parsed["current_segment"];
        assert_eq!(seg["duration_mins"], 12);
        assert_eq!(seg["regime_label"], "deep_work");
        assert_eq!(seg["event_count"], 45);
        assert_eq!(seg["context_switches"], 3);
        assert_eq!(seg["dominant_category"], "Development");
        assert_eq!(seg["content_summary"][0]["content"], "main.rs");
        assert_eq!(seg["content_summary"][0]["mins"], 10);
    }

    #[test]
    fn build_with_segment_enriches_gui_summary_line() {
        let assembler = ContextAssembler::new(noop_filter());
        let stats = SegmentStats {
            duration_mins: 15,
            regime_label: None,
            event_count: 30,
            context_switches: 2,
            dominant_category: "Development".to_string(),
            content_summary: vec![ContentSummaryEntry {
                content: "auth.rs".to_string(),
                content_type: "File".to_string(),
                work_type: "ActiveCoding".to_string(),
                mins: 15,
                gui_summary_line: Some("3 saves, 2 test runs".to_string()),
                gui_patterns: vec![],
            }],
            gui_patterns: vec![],
        };
        let ctx =
            assembler.build_with_segment(&make_current(), &[], &[], &make_metrics(), Some(&stats));
        let parsed: serde_json::Value = serde_json::from_str(&ctx.user_context_json).unwrap();
        let content = parsed["current_segment"]["content_summary"][0]["content"]
            .as_str()
            .unwrap();
        assert_eq!(content, "auth.rs (3 saves, 2 test runs)");
    }

    #[test]
    fn build_with_accessibility_text_included() {
        let assembler = ContextAssembler::new(noop_filter());
        let current = CurrentActivity {
            app_name: "Terminal".to_string(),
            window_title: "iTerm2".to_string(),
            ocr_hint: None,
            focus_score: 0.6,
            deep_work_mins: 10,
            accessibility_text: Some("$ cargo test --workspace".to_string()),
        };
        let ctx = assembler.build(&current, &[], &[], &make_metrics());
        let parsed: serde_json::Value = serde_json::from_str(&ctx.user_context_json).unwrap();
        assert_eq!(
            parsed["current"]["accessibility_text"],
            "$ cargo test --workspace"
        );
    }

    #[test]
    fn build_without_accessibility_text_omits_key() {
        let assembler = ContextAssembler::new(noop_filter());
        let ctx = assembler.build(&make_current(), &[], &[], &make_metrics());
        let parsed: serde_json::Value = serde_json::from_str(&ctx.user_context_json).unwrap();
        assert!(parsed["current"].get("accessibility_text").is_none());
    }

    #[test]
    fn accessibility_text_pii_filtered() {
        let assembler = ContextAssembler::new(test_pii_filter());
        let current = CurrentActivity {
            app_name: "Terminal".to_string(),
            window_title: "iTerm2".to_string(),
            ocr_hint: None,
            focus_score: 0.6,
            deep_work_mins: 10,
            accessibility_text: Some("ssh user@example.com".to_string()),
        };
        let ctx = assembler.build(&current, &[], &[], &make_metrics());
        assert!(ctx.user_context_json.contains("[EMAIL]"));
        assert!(!ctx.user_context_json.contains("user@example.com"));
    }

    #[test]
    fn gui_section_included_when_patterns_present() {
        let assembler = ContextAssembler::new(noop_filter());
        let stats = SegmentStats {
            duration_mins: 10,
            regime_label: None,
            event_count: 20,
            context_switches: 1,
            dominant_category: "Development".to_string(),
            content_summary: vec![ContentSummaryEntry {
                content: "main.rs".to_string(),
                content_type: "File".to_string(),
                work_type: "ActiveCoding".to_string(),
                mins: 10,
                gui_summary_line: Some("3 saves, 1 test runs".to_string()),
                gui_patterns: vec!["TestDrivenDevelopment".to_string()],
            }],
            gui_patterns: vec!["TestDrivenDevelopment".to_string()],
        };
        let ctx =
            assembler.build_with_segment(&make_current(), &[], &[], &make_metrics(), Some(&stats));
        assert!(
            ctx.user_context_json.contains("\"gui\""),
            "gui section should be present"
        );
        assert!(ctx.user_context_json.contains("TestDrivenDevelopment"));
        assert!(ctx.user_context_json.contains("\"saves\":3"));
        assert!(ctx.user_context_json.contains("\"test_runs\":1"));
    }

    #[test]
    fn gui_section_omitted_when_no_gui_data() {
        let assembler = ContextAssembler::new(noop_filter());
        let stats = SegmentStats {
            duration_mins: 5,
            regime_label: None,
            event_count: 10,
            context_switches: 0,
            dominant_category: "Development".to_string(),
            content_summary: vec![ContentSummaryEntry {
                content: "readme.md".to_string(),
                content_type: "File".to_string(),
                work_type: "Reading".to_string(),
                mins: 5,
                gui_summary_line: None,
                gui_patterns: vec![],
            }],
            gui_patterns: vec![],
        };
        let ctx =
            assembler.build_with_segment(&make_current(), &[], &[], &make_metrics(), Some(&stats));
        assert!(
            !ctx.user_context_json.contains("\"gui\""),
            "gui section should be absent when no gui data"
        );
    }
}
