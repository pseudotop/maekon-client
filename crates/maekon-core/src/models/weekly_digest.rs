use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::tiered_memory::WorkType;

/// Weekly productivity digest aggregating segment data over a 7-day period.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeeklyDigest {
    pub week_start: DateTime<Utc>,
    pub week_end: DateTime<Utc>,
    pub total_tracked_hours: f32,
    pub regime_breakdown: HashMap<String, f32>,
    pub category_breakdown: HashMap<String, f32>,
    pub top_content: Vec<ContentRanking>,
    pub deep_work_hours: f32,
    pub communication_hours: f32,
    pub context_switches_total: u32,
    pub longest_deep_work_segment_mins: u32,
    pub comparison: Option<WeekComparison>,
    pub llm_narrative: Option<String>,
}

/// Ranked content item within a weekly digest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentRanking {
    pub content_label: String,
    pub total_mins: u32,
    pub dominant_work_type: WorkType,
}

/// Week-over-week comparison metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeekComparison {
    pub deep_work_delta_hours: f32,
    pub communication_delta_hours: f32,
    pub context_switch_delta: i32,
    pub trend_summary: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weekly_digest_serde_roundtrip() {
        let digest = WeeklyDigest {
            week_start: Utc::now(),
            week_end: Utc::now(),
            total_tracked_hours: 38.5,
            regime_breakdown: HashMap::from([
                ("Deep Focus".to_string(), 20.0),
                ("Communication".to_string(), 10.0),
            ]),
            category_breakdown: HashMap::from([
                ("Development".to_string(), 25.0),
                ("Communication".to_string(), 8.0),
            ]),
            top_content: vec![ContentRanking {
                content_label: "VSCode: main.rs".to_string(),
                total_mins: 120,
                dominant_work_type: WorkType::ActiveCoding,
            }],
            deep_work_hours: 20.0,
            communication_hours: 10.0,
            context_switches_total: 45,
            longest_deep_work_segment_mins: 90,
            comparison: Some(WeekComparison {
                deep_work_delta_hours: 2.5,
                communication_delta_hours: -1.0,
                context_switch_delta: -5,
                trend_summary: "More focused this week".to_string(),
            }),
            llm_narrative: Some("A productive week with deep focus sessions".to_string()),
        };
        let json = serde_json::to_string(&digest).unwrap();
        let back: WeeklyDigest = serde_json::from_str(&json).unwrap();
        assert!((back.total_tracked_hours - 38.5).abs() < f32::EPSILON);
        assert_eq!(back.top_content.len(), 1);
        assert!(back.comparison.is_some());
        assert!(back.llm_narrative.is_some());
    }

    #[test]
    fn content_ranking_serde_roundtrip() {
        let ranking = ContentRanking {
            content_label: "Chrome: GitHub PR".to_string(),
            total_mins: 45,
            dominant_work_type: WorkType::CodeReview,
        };
        let json = serde_json::to_string(&ranking).unwrap();
        let back: ContentRanking = serde_json::from_str(&json).unwrap();
        assert_eq!(back.content_label, "Chrome: GitHub PR");
        assert_eq!(back.total_mins, 45);
        assert_eq!(back.dominant_work_type, WorkType::CodeReview);
    }

    #[test]
    fn week_comparison_serde_roundtrip() {
        let comp = WeekComparison {
            deep_work_delta_hours: -3.0,
            communication_delta_hours: 1.5,
            context_switch_delta: 10,
            trend_summary: "More meetings this week".to_string(),
        };
        let json = serde_json::to_string(&comp).unwrap();
        let back: WeekComparison = serde_json::from_str(&json).unwrap();
        assert!((back.deep_work_delta_hours - (-3.0)).abs() < f32::EPSILON);
        assert_eq!(back.context_switch_delta, 10);
    }

    #[test]
    fn weekly_digest_no_comparison_no_narrative() {
        let digest = WeeklyDigest {
            week_start: Utc::now(),
            week_end: Utc::now(),
            total_tracked_hours: 0.0,
            regime_breakdown: HashMap::new(),
            category_breakdown: HashMap::new(),
            top_content: vec![],
            deep_work_hours: 0.0,
            communication_hours: 0.0,
            context_switches_total: 0,
            longest_deep_work_segment_mins: 0,
            comparison: None,
            llm_narrative: None,
        };
        let json = serde_json::to_string(&digest).unwrap();
        let back: WeeklyDigest = serde_json::from_str(&json).unwrap();
        assert!(back.comparison.is_none());
        assert!(back.llm_narrative.is_none());
        assert!(back.top_content.is_empty());
    }
}
