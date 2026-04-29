//! Post-hoc WorkType refinement using GUI activity signals.
//!
//! The initial `WorkType` classification from `WorkTypeClassifier` uses only
//! keyboard/mouse rates and app category. `GuiWorkTypeRefiner` corrects it
//! using richer signals from the `GuiActivitySummary` — semantic action
//! counts, interaction patterns, and element types.

use oneshim_core::models::gui_activity::GuiActivitySummary;
use oneshim_core::models::tiered_memory::WorkType;

/// Refines the initial WorkType classification using GUI activity data.
///
/// Rules (from spec §5.3):
/// - `Unknown` + heavy text entries → `FormFilling`
/// - `Unknown` + save actions → `ActiveCoding`
/// - `Unknown` + high clicks, low keystrokes → `Browsing`
/// - `Reading` + frequent element clicking → `Navigation`
/// - `Browsing` + text entry dominance → `FormFilling`
/// - All others: unchanged
pub struct GuiWorkTypeRefiner;

impl GuiWorkTypeRefiner {
    /// Refine the initial work type using GUI summary data.
    pub fn refine(&self, initial: WorkType, summary: &GuiActivitySummary) -> WorkType {
        match initial {
            WorkType::Unknown => {
                // Save actions → ActiveCoding
                if summary.save_count > 0 {
                    return WorkType::ActiveCoding;
                }
                // Heavy text entries → FormFilling
                if summary.text_entries > 5 {
                    return WorkType::FormFilling;
                }
                // High clicks + low keystrokes → Browsing
                let total_clicks =
                    summary.button_clicks + summary.tab_switches + summary.menu_accesses;
                if total_clicks > 10 && summary.text_entries < 5 {
                    return WorkType::Browsing;
                }
                WorkType::Unknown
            }
            WorkType::Reading => {
                // Frequent clicking while "reading" → Navigation
                let total_clicks =
                    summary.button_clicks + summary.tab_switches + summary.menu_accesses;
                if total_clicks > 5 {
                    return WorkType::Navigation;
                }
                WorkType::Reading
            }
            WorkType::Browsing => {
                // Text entry dominance while "browsing" → FormFilling
                if summary.text_entries > 5 {
                    return WorkType::FormFilling;
                }
                WorkType::Browsing
            }
            // All other work types: preserve as-is
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_summary_with(
        button_clicks: u32,
        text_entries: u32,
        tab_switches: u32,
        menu_accesses: u32,
        save_count: u32,
    ) -> GuiActivitySummary {
        let now = Utc::now();
        GuiActivitySummary {
            app_name: "App".to_string(),
            window_title: "Window".to_string(),
            content_label: "file.rs".to_string(),
            start_time: now,
            end_time: now,
            duration_secs: 60,
            button_clicks,
            text_entries,
            tab_switches,
            menu_accesses,
            tree_navigations: 0,
            scroll_events: 0,
            save_count,
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

    #[test]
    fn unknown_with_saves_becomes_active_coding() {
        let refiner = GuiWorkTypeRefiner;
        let summary = make_summary_with(5, 2, 0, 0, 3);
        assert_eq!(
            refiner.refine(WorkType::Unknown, &summary),
            WorkType::ActiveCoding
        );
    }

    #[test]
    fn unknown_with_heavy_text_becomes_form_filling() {
        let refiner = GuiWorkTypeRefiner;
        let summary = make_summary_with(2, 10, 0, 0, 0);
        assert_eq!(
            refiner.refine(WorkType::Unknown, &summary),
            WorkType::FormFilling
        );
    }

    #[test]
    fn unknown_with_high_clicks_low_keys_becomes_browsing() {
        let refiner = GuiWorkTypeRefiner;
        let summary = make_summary_with(15, 2, 0, 0, 0);
        assert_eq!(
            refiner.refine(WorkType::Unknown, &summary),
            WorkType::Browsing
        );
    }

    #[test]
    fn reading_with_clicks_becomes_navigation() {
        let refiner = GuiWorkTypeRefiner;
        let summary = make_summary_with(6, 0, 0, 0, 0);
        assert_eq!(
            refiner.refine(WorkType::Reading, &summary),
            WorkType::Navigation
        );
    }

    #[test]
    fn reading_without_clicks_stays_reading() {
        let refiner = GuiWorkTypeRefiner;
        let summary = make_summary_with(2, 0, 0, 0, 0);
        assert_eq!(
            refiner.refine(WorkType::Reading, &summary),
            WorkType::Reading
        );
    }

    #[test]
    fn browsing_with_text_becomes_form_filling() {
        let refiner = GuiWorkTypeRefiner;
        let summary = make_summary_with(5, 8, 0, 0, 0);
        assert_eq!(
            refiner.refine(WorkType::Browsing, &summary),
            WorkType::FormFilling
        );
    }

    #[test]
    fn active_coding_unchanged() {
        let refiner = GuiWorkTypeRefiner;
        let summary = make_summary_with(5, 5, 0, 0, 0);
        assert_eq!(
            refiner.refine(WorkType::ActiveCoding, &summary),
            WorkType::ActiveCoding
        );
    }

    #[test]
    fn unknown_with_no_signals_stays_unknown() {
        let refiner = GuiWorkTypeRefiner;
        let summary = make_summary_with(0, 0, 0, 0, 0);
        assert_eq!(
            refiner.refine(WorkType::Unknown, &summary),
            WorkType::Unknown
        );
    }

    #[test]
    fn tab_and_menu_count_toward_clicks() {
        let refiner = GuiWorkTypeRefiner;
        // button_clicks=3, tab_switches=5, menu_accesses=4 → total 12 > 10
        let summary = make_summary_with(3, 2, 5, 4, 0);
        assert_eq!(
            refiner.refine(WorkType::Unknown, &summary),
            WorkType::Browsing
        );
    }
}
