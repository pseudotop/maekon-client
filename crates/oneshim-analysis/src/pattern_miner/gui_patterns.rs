use oneshim_core::models::gui_activity::GuiActivitySummary;
use oneshim_core::models::tiered_memory::WorkType;
use serde::{Deserialize, Serialize};

/// High scroll count threshold for code review detection.
const HIGH_SCROLL_THRESHOLD: u32 = 10;

/// GUI-derived behavioral patterns detected from a single `GuiActivitySummary` window.
///
/// These complement the existing event-level patterns in `PatternType` with
/// finer-grained, interaction-level signals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GuiPattern {
    /// `save_count >= 3` — iterative refinement behavior.
    FrequentSave,
    /// `test_run_count >= 1 AND save_count >= 1` with `WorkType::ActiveCoding` — TDD cycle.
    TestDrivenDevelopment,
    /// High `scroll_events` + `copy_paste_count >= 1` + reading/coding work type — code review.
    CodeReviewFlow,
    /// `test_run_count >= 2` + `undo_redo_count >= 1` — fix-test-fix cycle.
    DebuggingLoop,
    /// `tab_switches >= 3` + `search_count >= 1` — documentation/reference lookup.
    ReferenceHopping,
}

impl GuiPattern {
    /// Human-readable description of the pattern.
    pub fn description(&self) -> &'static str {
        match self {
            Self::FrequentSave => "Frequent saves indicate iterative refinement",
            Self::TestDrivenDevelopment => "Test-driven development cycle detected",
            Self::CodeReviewFlow => "Code review flow: reading + copying code",
            Self::DebuggingLoop => "Debugging loop: repeated test-fix cycles",
            Self::ReferenceHopping => "Reference hopping: tab switches + search lookups",
        }
    }
}

/// Detect GUI-derived behavioral patterns from a single activity summary window.
///
/// Each pattern is independent — multiple patterns can be detected from the same
/// summary. Returns an empty vec when no thresholds are met.
pub fn detect_gui_patterns(summary: &GuiActivitySummary, work_type: WorkType) -> Vec<GuiPattern> {
    let mut patterns = Vec::new();

    // 1. FrequentSave — save_count >= 3
    if summary.save_count >= 3 {
        patterns.push(GuiPattern::FrequentSave);
    }

    // 2. TestDrivenDevelopment — test_run_count >= 1 AND save_count >= 1, ActiveCoding
    if summary.test_run_count >= 1 && summary.save_count >= 1 && work_type == WorkType::ActiveCoding
    {
        patterns.push(GuiPattern::TestDrivenDevelopment);
    }

    // 3. CodeReviewFlow — high scroll + copy_paste >= 1, Reading or ActiveCoding
    if summary.scroll_events >= HIGH_SCROLL_THRESHOLD
        && summary.copy_paste_count >= 1
        && matches!(work_type, WorkType::Reading | WorkType::ActiveCoding)
    {
        patterns.push(GuiPattern::CodeReviewFlow);
    }

    // 4. DebuggingLoop — test_run_count >= 2 + undo_redo_count >= 1
    if summary.test_run_count >= 2 && summary.undo_redo_count >= 1 {
        patterns.push(GuiPattern::DebuggingLoop);
    }

    // 5. ReferenceHopping — tab_switches >= 3 + search_count >= 1
    if summary.tab_switches >= 3 && summary.search_count >= 1 {
        patterns.push(GuiPattern::ReferenceHopping);
    }

    patterns
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    /// Helper to build a minimal `GuiActivitySummary` with all counts zeroed.
    fn base_summary() -> GuiActivitySummary {
        GuiActivitySummary {
            app_name: "VS Code".to_string(),
            window_title: "main.rs — VS Code".to_string(),
            content_label: "main.rs".to_string(),
            start_time: Utc::now(),
            end_time: Utc::now(),
            duration_secs: 300,
            button_clicks: 0,
            text_entries: 0,
            tab_switches: 0,
            menu_accesses: 0,
            tree_navigations: 0,
            scroll_events: 0,
            save_count: 0,
            test_run_count: 0,
            search_count: 0,
            build_count: 0,
            undo_redo_count: 0,
            copy_paste_count: 0,
            top_elements: vec![],
            unmatched_click_count: 0,
            summary_line: String::new(),
        }
    }

    // -----------------------------------------------------------------------
    // FrequentSave
    // -----------------------------------------------------------------------

    #[test]
    fn frequent_save_positive() {
        let mut s = base_summary();
        s.save_count = 3;
        let patterns = detect_gui_patterns(&s, WorkType::ActiveCoding);
        assert!(patterns.contains(&GuiPattern::FrequentSave));
    }

    #[test]
    fn frequent_save_above_threshold() {
        let mut s = base_summary();
        s.save_count = 10;
        let patterns = detect_gui_patterns(&s, WorkType::Unknown);
        assert!(patterns.contains(&GuiPattern::FrequentSave));
    }

    #[test]
    fn frequent_save_negative() {
        let mut s = base_summary();
        s.save_count = 2;
        let patterns = detect_gui_patterns(&s, WorkType::ActiveCoding);
        assert!(!patterns.contains(&GuiPattern::FrequentSave));
    }

    // -----------------------------------------------------------------------
    // TestDrivenDevelopment
    // -----------------------------------------------------------------------

    #[test]
    fn tdd_positive() {
        let mut s = base_summary();
        s.test_run_count = 1;
        s.save_count = 1;
        let patterns = detect_gui_patterns(&s, WorkType::ActiveCoding);
        assert!(patterns.contains(&GuiPattern::TestDrivenDevelopment));
    }

    #[test]
    fn tdd_negative_wrong_work_type() {
        let mut s = base_summary();
        s.test_run_count = 1;
        s.save_count = 1;
        let patterns = detect_gui_patterns(&s, WorkType::Reading);
        assert!(!patterns.contains(&GuiPattern::TestDrivenDevelopment));
    }

    #[test]
    fn tdd_negative_no_test_runs() {
        let mut s = base_summary();
        s.test_run_count = 0;
        s.save_count = 5;
        let patterns = detect_gui_patterns(&s, WorkType::ActiveCoding);
        assert!(!patterns.contains(&GuiPattern::TestDrivenDevelopment));
    }

    #[test]
    fn tdd_negative_no_saves() {
        let mut s = base_summary();
        s.test_run_count = 3;
        s.save_count = 0;
        let patterns = detect_gui_patterns(&s, WorkType::ActiveCoding);
        assert!(!patterns.contains(&GuiPattern::TestDrivenDevelopment));
    }

    // -----------------------------------------------------------------------
    // CodeReviewFlow
    // -----------------------------------------------------------------------

    #[test]
    fn code_review_positive_reading() {
        let mut s = base_summary();
        s.scroll_events = 15;
        s.copy_paste_count = 2;
        let patterns = detect_gui_patterns(&s, WorkType::Reading);
        assert!(patterns.contains(&GuiPattern::CodeReviewFlow));
    }

    #[test]
    fn code_review_positive_active_coding() {
        let mut s = base_summary();
        s.scroll_events = 10;
        s.copy_paste_count = 1;
        let patterns = detect_gui_patterns(&s, WorkType::ActiveCoding);
        assert!(patterns.contains(&GuiPattern::CodeReviewFlow));
    }

    #[test]
    fn code_review_negative_low_scroll() {
        let mut s = base_summary();
        s.scroll_events = 5;
        s.copy_paste_count = 3;
        let patterns = detect_gui_patterns(&s, WorkType::Reading);
        assert!(!patterns.contains(&GuiPattern::CodeReviewFlow));
    }

    #[test]
    fn code_review_negative_no_copy_paste() {
        let mut s = base_summary();
        s.scroll_events = 20;
        s.copy_paste_count = 0;
        let patterns = detect_gui_patterns(&s, WorkType::Reading);
        assert!(!patterns.contains(&GuiPattern::CodeReviewFlow));
    }

    #[test]
    fn code_review_negative_wrong_work_type() {
        let mut s = base_summary();
        s.scroll_events = 15;
        s.copy_paste_count = 2;
        let patterns = detect_gui_patterns(&s, WorkType::Browsing);
        assert!(!patterns.contains(&GuiPattern::CodeReviewFlow));
    }

    // -----------------------------------------------------------------------
    // DebuggingLoop
    // -----------------------------------------------------------------------

    #[test]
    fn debugging_loop_positive() {
        let mut s = base_summary();
        s.test_run_count = 2;
        s.undo_redo_count = 1;
        let patterns = detect_gui_patterns(&s, WorkType::ActiveCoding);
        assert!(patterns.contains(&GuiPattern::DebuggingLoop));
    }

    #[test]
    fn debugging_loop_positive_any_work_type() {
        let mut s = base_summary();
        s.test_run_count = 5;
        s.undo_redo_count = 3;
        let patterns = detect_gui_patterns(&s, WorkType::Unknown);
        assert!(patterns.contains(&GuiPattern::DebuggingLoop));
    }

    #[test]
    fn debugging_loop_negative_too_few_tests() {
        let mut s = base_summary();
        s.test_run_count = 1;
        s.undo_redo_count = 3;
        let patterns = detect_gui_patterns(&s, WorkType::ActiveCoding);
        assert!(!patterns.contains(&GuiPattern::DebuggingLoop));
    }

    #[test]
    fn debugging_loop_negative_no_undo_redo() {
        let mut s = base_summary();
        s.test_run_count = 5;
        s.undo_redo_count = 0;
        let patterns = detect_gui_patterns(&s, WorkType::ActiveCoding);
        assert!(!patterns.contains(&GuiPattern::DebuggingLoop));
    }

    // -----------------------------------------------------------------------
    // ReferenceHopping
    // -----------------------------------------------------------------------

    #[test]
    fn reference_hopping_positive() {
        let mut s = base_summary();
        s.tab_switches = 3;
        s.search_count = 1;
        let patterns = detect_gui_patterns(&s, WorkType::Reading);
        assert!(patterns.contains(&GuiPattern::ReferenceHopping));
    }

    #[test]
    fn reference_hopping_positive_any_work_type() {
        let mut s = base_summary();
        s.tab_switches = 10;
        s.search_count = 5;
        let patterns = detect_gui_patterns(&s, WorkType::Browsing);
        assert!(patterns.contains(&GuiPattern::ReferenceHopping));
    }

    #[test]
    fn reference_hopping_negative_too_few_tabs() {
        let mut s = base_summary();
        s.tab_switches = 2;
        s.search_count = 3;
        let patterns = detect_gui_patterns(&s, WorkType::Reading);
        assert!(!patterns.contains(&GuiPattern::ReferenceHopping));
    }

    #[test]
    fn reference_hopping_negative_no_search() {
        let mut s = base_summary();
        s.tab_switches = 10;
        s.search_count = 0;
        let patterns = detect_gui_patterns(&s, WorkType::Reading);
        assert!(!patterns.contains(&GuiPattern::ReferenceHopping));
    }

    // -----------------------------------------------------------------------
    // Multi-pattern & edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn multiple_patterns_detected() {
        let mut s = base_summary();
        s.save_count = 5;
        s.test_run_count = 3;
        s.undo_redo_count = 2;
        s.tab_switches = 4;
        s.search_count = 2;
        let patterns = detect_gui_patterns(&s, WorkType::ActiveCoding);
        assert!(patterns.contains(&GuiPattern::FrequentSave));
        assert!(patterns.contains(&GuiPattern::TestDrivenDevelopment));
        assert!(patterns.contains(&GuiPattern::DebuggingLoop));
        assert!(patterns.contains(&GuiPattern::ReferenceHopping));
        assert_eq!(patterns.len(), 4);
    }

    #[test]
    fn zero_interactions_returns_empty() {
        let s = base_summary();
        let patterns = detect_gui_patterns(&s, WorkType::Unknown);
        assert!(patterns.is_empty());
    }

    #[test]
    fn gui_pattern_description_not_empty() {
        let variants = [
            GuiPattern::FrequentSave,
            GuiPattern::TestDrivenDevelopment,
            GuiPattern::CodeReviewFlow,
            GuiPattern::DebuggingLoop,
            GuiPattern::ReferenceHopping,
        ];
        for v in &variants {
            assert!(!v.description().is_empty(), "{v:?} has empty description");
        }
    }

    #[test]
    fn gui_pattern_serde_roundtrip() {
        let patterns = vec![
            GuiPattern::FrequentSave,
            GuiPattern::TestDrivenDevelopment,
            GuiPattern::CodeReviewFlow,
            GuiPattern::DebuggingLoop,
            GuiPattern::ReferenceHopping,
        ];
        let json = serde_json::to_string(&patterns).unwrap();
        let parsed: Vec<GuiPattern> = serde_json::from_str(&json).unwrap();
        assert_eq!(patterns, parsed);
    }
}
