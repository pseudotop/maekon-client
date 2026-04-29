use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::tiered_memory::WorkType;

/// Aggregated daily summary containing timeline, statistics, and LLM insight.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyDigest {
    pub date: NaiveDate,
    pub insight: Option<DailyInsight>,
    pub timeline: Vec<TimelineEntry>,
    pub statistics: DailyStatistics,
    pub generated_at: DateTime<Utc>,
}

/// LLM-generated narrative and key highlights for the day.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyInsight {
    pub narrative: String,
    pub highlights: Vec<DigestHighlight>,
}

/// A single highlight within a daily digest (achievement, warning, or suggestion).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DigestHighlight {
    pub highlight_type: HighlightType,
    pub text: String,
    pub segment_id: Option<String>,
}

/// Classification of a digest highlight.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum HighlightType {
    Achievement,
    Warning,
    Suggestion,
}

/// A single time block in the daily timetable view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineEntry {
    pub segment_id: String,
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
    pub duration_mins: u32,
    pub regime_label: String,
    pub regime_color: String,
    pub dominant_app: String,
    pub content_summary: Vec<ContentBrief>,
    pub annotation: Option<DigestHighlight>,
}

/// Brief description of work content within a timeline entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentBrief {
    pub content: String,
    pub work_type: WorkType,
    pub mins: u32,
}

/// Aggregate statistics for a single day.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DailyStatistics {
    pub deep_work_hours: f32,
    pub communication_hours: f32,
    pub meeting_hours: f32,
    pub context_switches: u32,
    pub longest_focus_mins: u32,
    pub longest_focus_content: String,
    pub regime_distribution: HashMap<String, u32>,
    pub comparison: Option<DayComparison>,
}

/// Delta comparison against a previous day (or rolling average).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DayComparison {
    pub deep_work_delta: f32,
    pub communication_delta: f32,
    pub context_switch_delta: i32,
}

// ── Shared classification helpers ─────────────────────────────
// Canonical implementations used by both oneshim-analysis (DailyDigestGenerator)
// and oneshim-web (dashboard handler) to eliminate logic duplication.

/// Regime color for timetable display.
pub fn regime_color(label: &str) -> &'static str {
    if label.contains("Deep Focus") || label.contains("Development") {
        "#3B82F6"
    } else if label.contains("Communication") {
        "#F59E0B"
    } else if label.contains("Research") {
        "#10B981"
    } else if label.contains("Meeting") {
        "#8B5CF6"
    } else if label.contains("Idle") {
        "#E5E7EB"
    } else {
        "#6B7280"
    }
}

/// Check if a segment represents deep work.
pub fn is_deep_work(regime_id: Option<&str>, dominant_category: &str) -> bool {
    regime_id.is_some_and(|r| r.contains("Deep Focus") || r.contains("Development"))
        || dominant_category == "Development"
}

/// Check if a segment represents communication.
pub fn is_communication(regime_id: Option<&str>, dominant_category: &str) -> bool {
    regime_id.is_some_and(|r| r.contains("Communication")) || dominant_category == "Communication"
}

/// Check if a segment represents a meeting.
pub fn is_meeting(dominant_category: &str) -> bool {
    dominant_category == "Meeting"
        || dominant_category.contains("Zoom")
        || dominant_category.contains("Meet")
}

// ── Markdown export ──────────────────────────────────────────
// Canonical implementation used by both oneshim-web (export endpoint) and
// oneshim-analysis (batch export).

/// Converts a `DailyDigest` into a human-readable Markdown document.
pub struct DigestExporter;

impl DigestExporter {
    /// Render a daily digest as Markdown.
    pub fn to_markdown(digest: &DailyDigest) -> String {
        let mut md = String::with_capacity(2048);
        md.push_str(&format!("# Daily Digest — {}\n\n", digest.date));

        // Insights
        if let Some(ref insight) = digest.insight {
            md.push_str("## Insights\n\n");
            md.push_str(&insight.narrative);
            md.push_str("\n\n");
            for h in &insight.highlights {
                let badge = match h.highlight_type {
                    HighlightType::Achievement => "Achievement",
                    HighlightType::Warning => "Warning",
                    HighlightType::Suggestion => "Suggestion",
                };
                md.push_str(&format!("- **{badge}**: {}\n", h.text));
            }
            md.push('\n');
        }

        // Timeline
        if !digest.timeline.is_empty() {
            md.push_str("## Timeline\n\n");
            md.push_str("| Time | Regime | App | Duration |\n");
            md.push_str("|------|--------|-----|----------|\n");
            for entry in &digest.timeline {
                let start = entry.start_time.format("%H:%M");
                let end = entry.end_time.format("%H:%M");
                md.push_str(&format!(
                    "| {} - {} | {} | {} | {}min |\n",
                    start, end, entry.regime_label, entry.dominant_app, entry.duration_mins,
                ));
            }
            md.push('\n');
        }

        // Statistics
        md.push_str("## Statistics\n\n");
        let s = &digest.statistics;
        md.push_str(&format!("- **Deep work**: {:.1}h\n", s.deep_work_hours));
        md.push_str(&format!(
            "- **Communication**: {:.1}h\n",
            s.communication_hours
        ));
        md.push_str(&format!("- **Meetings**: {:.1}h\n", s.meeting_hours));
        md.push_str(&format!("- **Context switches**: {}\n", s.context_switches));
        md.push_str(&format!(
            "- **Longest focus**: {}min ({})\n",
            s.longest_focus_mins, s.longest_focus_content,
        ));

        if let Some(ref cmp) = s.comparison {
            md.push_str("\n### Compared to average\n\n");
            md.push_str(&format!("- Deep work: {:+.1}h\n", cmp.deep_work_delta));
            md.push_str(&format!(
                "- Communication: {:+.1}h\n",
                cmp.communication_delta
            ));
            md.push_str(&format!(
                "- Context switches: {:+}\n",
                cmp.context_switch_delta
            ));
        }

        md.push_str(&format!(
            "\n---\n*Generated at {}*\n",
            digest.generated_at.format("%Y-%m-%d %H:%M UTC")
        ));

        md
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn daily_digest_serde_roundtrip() {
        let digest = DailyDigest {
            date: Utc::now().date_naive(),
            insight: Some(DailyInsight {
                narrative: "Great focus day".to_string(),
                highlights: vec![DigestHighlight {
                    highlight_type: HighlightType::Achievement,
                    text: "2h deep work block".to_string(),
                    segment_id: Some("seg-001".to_string()),
                }],
            }),
            timeline: vec![],
            statistics: DailyStatistics::default(),
            generated_at: Utc::now(),
        };
        let json = serde_json::to_string(&digest).unwrap();
        let back: DailyDigest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.date, digest.date);
        assert!(back.insight.is_some());
    }

    #[test]
    fn highlight_type_serde() {
        let ht = HighlightType::Achievement;
        let json = serde_json::to_string(&ht).unwrap();
        assert_eq!(json, "\"ACHIEVEMENT\"");
        let back: HighlightType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, HighlightType::Achievement);
    }

    #[test]
    fn regime_color_mapping() {
        assert_eq!(regime_color("Deep Focus"), "#3B82F6");
        assert_eq!(regime_color("Development"), "#3B82F6");
        assert_eq!(regime_color("Communication"), "#F59E0B");
        assert_eq!(regime_color("Research"), "#10B981");
        assert_eq!(regime_color("Meeting"), "#8B5CF6");
        assert_eq!(regime_color("Idle"), "#E5E7EB");
        assert_eq!(regime_color("Unknown"), "#6B7280");
    }

    #[test]
    fn classification_helpers() {
        assert!(is_deep_work(Some("Deep Focus"), "Development"));
        assert!(is_deep_work(None, "Development"));
        assert!(!is_deep_work(Some("Communication"), "Communication"));

        assert!(is_communication(Some("Communication"), "Other"));
        assert!(is_communication(None, "Communication"));
        assert!(!is_communication(Some("Deep Focus"), "Development"));

        assert!(is_meeting("Meeting"));
        assert!(is_meeting("Zoom Call"));
        assert!(is_meeting("Google Meet"));
        assert!(!is_meeting("Development"));
    }

    #[test]
    fn day_comparison_serde() {
        let cmp = DayComparison {
            deep_work_delta: 0.5,
            communication_delta: -0.2,
            context_switch_delta: -3,
        };
        let json = serde_json::to_string(&cmp).unwrap();
        let back: DayComparison = serde_json::from_str(&json).unwrap();
        assert!((back.deep_work_delta - 0.5).abs() < f32::EPSILON);
    }

    // ── DigestExporter tests ─────────────────────────────────

    fn sample_digest_for_export() -> DailyDigest {
        DailyDigest {
            date: chrono::NaiveDate::from_ymd_opt(2026, 4, 6).unwrap(),
            insight: Some(DailyInsight {
                narrative: "Solid focus day.".to_string(),
                highlights: vec![
                    DigestHighlight {
                        highlight_type: HighlightType::Achievement,
                        text: "2h deep work block".to_string(),
                        segment_id: Some("seg-001".to_string()),
                    },
                    DigestHighlight {
                        highlight_type: HighlightType::Warning,
                        text: "High context switching".to_string(),
                        segment_id: None,
                    },
                ],
            }),
            timeline: vec![TimelineEntry {
                segment_id: "seg-001".to_string(),
                start_time: Utc::now(),
                end_time: Utc::now(),
                duration_mins: 120,
                regime_label: "Deep Focus".to_string(),
                regime_color: "#3B82F6".to_string(),
                dominant_app: "VS Code".to_string(),
                content_summary: vec![],
                annotation: None,
            }],
            statistics: DailyStatistics {
                deep_work_hours: 4.5,
                communication_hours: 1.2,
                meeting_hours: 0.5,
                context_switches: 12,
                longest_focus_mins: 120,
                longest_focus_content: "Rust refactoring".to_string(),
                regime_distribution: Default::default(),
                comparison: Some(DayComparison {
                    deep_work_delta: 1.0,
                    communication_delta: -0.3,
                    context_switch_delta: -2,
                }),
            },
            generated_at: Utc::now(),
        }
    }

    #[test]
    fn export_markdown_title() {
        let md = DigestExporter::to_markdown(&sample_digest_for_export());
        assert!(md.starts_with("# Daily Digest — 2026-04-06"));
    }

    #[test]
    fn export_markdown_insight_section() {
        let md = DigestExporter::to_markdown(&sample_digest_for_export());
        assert!(md.contains("## Insights"));
        assert!(md.contains("**Achievement**: 2h deep work block"));
        assert!(md.contains("**Warning**: High context switching"));
    }

    #[test]
    fn export_markdown_timeline_table() {
        let md = DigestExporter::to_markdown(&sample_digest_for_export());
        assert!(md.contains("## Timeline"));
        assert!(md.contains("Deep Focus"));
        assert!(md.contains("VS Code"));
        assert!(md.contains("120min"));
    }

    #[test]
    fn export_markdown_statistics() {
        let md = DigestExporter::to_markdown(&sample_digest_for_export());
        assert!(md.contains("**Deep work**: 4.5h"));
        assert!(md.contains("**Context switches**: 12"));
    }

    #[test]
    fn export_markdown_comparison() {
        let md = DigestExporter::to_markdown(&sample_digest_for_export());
        assert!(md.contains("### Compared to average"));
        assert!(md.contains("Deep work: +1.0h"));
    }

    #[test]
    fn export_markdown_no_insight() {
        let mut d = sample_digest_for_export();
        d.insight = None;
        let md = DigestExporter::to_markdown(&d);
        assert!(!md.contains("## Insights"));
    }

    #[test]
    fn export_markdown_no_timeline() {
        let mut d = sample_digest_for_export();
        d.timeline.clear();
        let md = DigestExporter::to_markdown(&d);
        assert!(!md.contains("## Timeline"));
    }
}
