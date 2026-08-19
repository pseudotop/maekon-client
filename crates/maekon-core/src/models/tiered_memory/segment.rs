use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::models::analysis::ActivityPattern;

use super::content::{ContainerInfo, ContentActivity};
use super::trigger::TriggerReason;

// ---------------------------------------------------------------------------
// SegmentSummary — output of one closed segment
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SegmentSummary {
    pub segment_id: String,
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
    pub duration_secs: u64,
    pub regime_id: Option<String>,
    pub trigger_reason: TriggerReason,
    pub event_count: u32,
    pub app_breakdown: HashMap<String, u64>,
    pub category_breakdown: HashMap<String, u64>,
    pub context_switch_count: u32,
    pub dominant_category: String,
    pub avg_importance: f32,
    pub patterns_detected: Vec<ActivityPattern>,
    pub content_activities: Vec<ContentActivity>,
    pub container: Option<ContainerInfo>,
    pub llm_summary: Option<String>,
}

impl SegmentSummary {
    /// Compose the keyword-searchable text for this segment — the input to the
    /// FTS5 content index (#8051).
    ///
    /// Combines, in priority order, the LLM summary (when present), the
    /// dominant category, each per-content activity label, and each
    /// application name. This is the single source of truth shared by the live
    /// segment-close indexing path (`analysis_pipeline::segment`) and the
    /// historical backfill (`SqliteStorage::backfill_unindexed_segments_fts`)
    /// so both produce identical index content.
    ///
    /// At segment-close time `llm_summary` is still `None` (summarization is an
    /// async, optional Phase-2 step); the base fields alone therefore keep the
    /// segment keyword-searchable even when LLM summarization is disabled.
    pub fn searchable_content(&self) -> String {
        compose_searchable_content(
            self.llm_summary.as_deref(),
            &self.dominant_category,
            self.content_activities
                .iter()
                .map(|c| c.content_label.as_str()),
            self.app_breakdown.keys().map(String::as_str),
        )
    }
}

/// Build FTS-searchable text from a segment's constituent fields.
///
/// Kept as a free function (rather than only a `SegmentSummary` method) so the
/// storage-layer backfill — which reconstructs these fields from persisted JSON
/// columns rather than a live `SegmentSummary` — produces identical index
/// content. Whitespace-only fragments are skipped and fragments are
/// de-duplicated (order-preserving) so repeated tokens (e.g. an app name that
/// equals a content label) do not bloat the index.
pub fn compose_searchable_content<'a>(
    llm_summary: Option<&'a str>,
    dominant_category: &'a str,
    content_labels: impl IntoIterator<Item = &'a str>,
    app_names: impl IntoIterator<Item = &'a str>,
) -> String {
    fn push_unique<'b>(parts: &mut Vec<&'b str>, value: &'b str) {
        let trimmed = value.trim();
        if !trimmed.is_empty() && !parts.contains(&trimmed) {
            parts.push(trimmed);
        }
    }

    let mut parts: Vec<&str> = Vec::new();
    if let Some(summary) = llm_summary {
        push_unique(&mut parts, summary);
    }
    push_unique(&mut parts, dominant_category);
    for label in content_labels {
        push_unique(&mut parts, label);
    }
    for app in app_names {
        push_unique(&mut parts, app);
    }
    parts.join(" ")
}

#[cfg(test)]
mod searchable_content_tests {
    use super::*;
    use crate::models::tiered_memory::{ContentType, EngagementMetrics, WorkType};

    fn content(label: &str) -> ContentActivity {
        ContentActivity {
            content_label: label.to_string(),
            content_type: ContentType::File,
            start_time: Utc::now(),
            duration_secs: 60,
            confidence: 0.9,
            work_type: WorkType::ActiveCoding,
            engagement: EngagementMetrics::default(),
            gui_summary: None,
        }
    }

    fn summary() -> SegmentSummary {
        let now = Utc::now();
        SegmentSummary {
            segment_id: "seg-1".to_string(),
            start_time: now,
            end_time: now,
            duration_secs: 0,
            regime_id: None,
            trigger_reason: TriggerReason::ScoreHigh,
            event_count: 0,
            app_breakdown: HashMap::new(),
            category_breakdown: HashMap::new(),
            context_switch_count: 0,
            dominant_category: "Development".to_string(),
            avg_importance: 0.0,
            patterns_detected: vec![],
            content_activities: vec![],
            container: None,
            llm_summary: None,
        }
    }

    #[test]
    fn base_content_indexed_without_llm_summary() {
        let mut s = summary();
        s.content_activities = vec![content("authentication.rs"), content("login_form.tsx")];
        s.app_breakdown.insert("VS Code".to_string(), 120);

        let text = s.searchable_content();
        // No LLM summary yet — base fields alone must be searchable.
        assert!(text.contains("Development"));
        assert!(text.contains("authentication.rs"));
        assert!(text.contains("login_form.tsx"));
        assert!(text.contains("VS Code"));
    }

    #[test]
    fn llm_summary_included_when_present() {
        let mut s = summary();
        s.llm_summary = Some("Refactored the OAuth token refresh path".to_string());
        let text = s.searchable_content();
        assert!(text.contains("OAuth token refresh"));
        assert!(text.contains("Development"));
    }

    #[test]
    fn fragments_are_deduplicated_and_trimmed() {
        // dominant_category == an app name; a blank label must be dropped.
        let text =
            compose_searchable_content(None, "Browser", ["  ", "docs.rs"], ["Browser", "Firefox"]);
        // "Browser" appears once, blank dropped, others present.
        assert_eq!(text.matches("Browser").count(), 1);
        assert!(text.contains("docs.rs"));
        assert!(text.contains("Firefox"));
    }
}
