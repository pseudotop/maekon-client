use oneshim_core::models::app_registry::{AppSubcategory, KeystrokeProfile};
use oneshim_core::models::event::{KeyboardActivity, MouseActivity};
use oneshim_core::models::tiered_memory::{EngagementMetrics, WorkType};
use oneshim_core::models::work_session::AppCategory;

/// Classify work type from input signals and app context.
///
/// Combines keyboard/mouse activity metrics with content label and app category
/// to determine the user's current work mode (coding, reviewing, writing, etc.).
pub struct WorkTypeClassifier;

impl WorkTypeClassifier {
    pub fn new() -> Self {
        Self
    }

    /// Classify the current work type and compute engagement metrics.
    pub fn classify(
        &self,
        keyboard: &KeyboardActivity,
        mouse: &MouseActivity,
        content_label: &str,
        app_category: AppCategory,
    ) -> (WorkType, EngagementMetrics) {
        let engagement = self.compute_engagement(keyboard, mouse);
        let work_type = self.infer_work_type(&engagement, content_label, app_category);
        (work_type, engagement)
    }

    /// Extended classify method that accepts app subcategory and keystroke profile.
    ///
    /// Falls back to the existing classify() behavior when subcategory is None
    /// or keystroke_profile is None.
    pub fn classify_extended(
        &self,
        keyboard: &KeyboardActivity,
        mouse: &MouseActivity,
        content_label: &str,
        app_category: AppCategory,
        app_subcategory: Option<AppSubcategory>,
        keystroke_profile: Option<&KeystrokeProfile>,
    ) -> (WorkType, EngagementMetrics) {
        let engagement = self.compute_engagement(keyboard, mouse);

        // Try subcategory-aware rules first
        if let (Some(subcategory), Some(profile)) = (app_subcategory, keystroke_profile) {
            if let Some(work_type) = self.infer_from_subcategory(&engagement, subcategory, profile)
            {
                return (work_type, engagement);
            }
        }

        // Fall back to existing rules
        let work_type = self.infer_work_type(&engagement, content_label, app_category);
        (work_type, engagement)
    }

    /// Subcategory-aware classification rules.
    ///
    /// Rule table (spec Section 7.2):
    /// Terminal   + enter_ratio>0.15 + keys>5/min   -> TerminalCommands
    /// Terminal   + keys<5/min + scroll>5/min        -> LogReading
    /// Terminal   + keys>40/min + arrow_ratio>0.2    -> ActiveCoding (TUI)
    /// Terminal   + keys>40/min                      -> ActiveCoding
    /// DocEditor  + keys>40/min + enter_ratio<0.05   -> DocumentWriting
    /// DocEditor  + keys<5/min + scroll>3/min        -> DocumentReading
    /// DocEditor  + keys>20/min + enter_ratio>0.1    -> Writing (list/outline)
    /// Chat       + keys>20/min + enter_ratio>0.1    -> ChatComposing
    /// Chat       + keys<5/min                       -> Reading
    /// Spreadsheet+ tab_ratio>0.15 + keys>10/min     -> FormFilling
    /// Spreadsheet+ scroll>5/min + keys<5/min        -> Reading
    fn infer_from_subcategory(
        &self,
        engagement: &EngagementMetrics,
        subcategory: AppSubcategory,
        profile: &KeystrokeProfile,
    ) -> Option<WorkType> {
        let kpm = engagement.keystrokes_per_min;
        // NOTE: scroll_events_per_min is a raw count per snapshot period
        // (not a true per-minute rate). Thresholds are calibrated for a
        // 5-30s snapshot interval. Normalize if snapshot interval changes.
        let spm = engagement.scroll_events_per_min;

        match subcategory {
            AppSubcategory::Terminal => {
                if profile.enter_ratio > 0.15 && kpm > 5.0 {
                    return Some(WorkType::TerminalCommands);
                }
                if kpm < 5.0 && spm > 5.0 {
                    return Some(WorkType::LogReading);
                }
                if kpm > 40.0 && profile.arrow_ratio > 0.2 {
                    return Some(WorkType::ActiveCoding);
                }
                if kpm > 40.0 {
                    return Some(WorkType::ActiveCoding);
                }
                None
            }
            AppSubcategory::DocumentEditor => {
                if kpm > 40.0 && profile.enter_ratio < 0.05 {
                    return Some(WorkType::DocumentWriting);
                }
                if kpm < 5.0 && spm > 3.0 {
                    return Some(WorkType::DocumentReading);
                }
                if kpm > 20.0 && profile.enter_ratio > 0.1 {
                    return Some(WorkType::Writing);
                }
                None
            }
            AppSubcategory::Chat => {
                if kpm > 20.0 && profile.enter_ratio > 0.1 {
                    return Some(WorkType::ChatComposing);
                }
                if kpm < 5.0 {
                    return Some(WorkType::Reading);
                }
                None
            }
            AppSubcategory::Spreadsheet => {
                if profile.tab_ratio > 0.15 && kpm > 10.0 {
                    return Some(WorkType::FormFilling);
                }
                if spm > 5.0 && kpm < 5.0 {
                    return Some(WorkType::Reading);
                }
                None
            }
            AppSubcategory::TuiEditor => {
                if kpm > 40.0 {
                    return Some(WorkType::ActiveCoding);
                }
                None
            }
            // Ide and other subcategories fall through to existing rules
            _ => None,
        }
    }

    /// Compute per-minute rates and ratios from raw input counts.
    fn compute_engagement(
        &self,
        kb: &KeyboardActivity,
        mouse: &MouseActivity,
    ) -> EngagementMetrics {
        let keystrokes_per_min = kb.keystrokes_per_min as f32;
        let mouse_clicks_per_min = mouse.click_count as f32;
        let scroll_events_per_min = mouse.scroll_count as f32;

        // Shortcut ratio: fraction of keystrokes that are shortcuts
        let shortcut_ratio = if kb.total_keystrokes > 0 {
            kb.shortcut_count as f32 / kb.total_keystrokes as f32
        } else {
            0.0
        };

        let typing_burst_count = kb.typing_bursts;

        // Idle ratio: if no input at all, ratio is 1.0; otherwise estimate from
        // total input volume. A very rough heuristic: if total keystrokes and
        // mouse events are both below 5, consider mostly idle.
        let total_input = kb.total_keystrokes + mouse.click_count + mouse.scroll_count;
        let idle_ratio = if total_input == 0 {
            1.0
        } else if total_input < 5 {
            0.8
        } else {
            0.0
        };

        EngagementMetrics {
            keystrokes_per_min,
            mouse_clicks_per_min,
            scroll_events_per_min,
            shortcut_ratio,
            typing_burst_count,
            idle_ratio,
        }
    }

    /// Infer work type from engagement metrics, content label, and app category.
    ///
    /// Rule table (evaluated top-to-bottom, first match wins):
    /// - high keystrokes + coding app            -> ActiveCoding
    /// - scroll + low keystrokes + code file     -> CodeReview
    /// - steady typing + document                -> Writing
    /// - scroll heavy + low keystrokes           -> Reading
    /// - continuous mouse + design app           -> Designing
    /// - no input + communication app (meeting)  -> PassiveMeeting
    /// - typing + communication app              -> ActiveMeeting
    /// - high shortcuts + low typing             -> Navigation
    /// - moderate clicking + browser             -> Browsing
    /// - otherwise                               -> Unknown
    fn infer_work_type(
        &self,
        engagement: &EngagementMetrics,
        content: &str,
        category: AppCategory,
    ) -> WorkType {
        let high_keystrokes = engagement.keystrokes_per_min > 60.0;
        let moderate_keystrokes = engagement.keystrokes_per_min > 40.0;
        let low_keystrokes = engagement.keystrokes_per_min <= 40.0;
        let has_scrolling = engagement.scroll_events_per_min > 3.0;
        let heavy_scrolling = engagement.scroll_events_per_min > 8.0;
        let high_mouse = engagement.mouse_clicks_per_min > 15.0;
        let high_clicks = engagement.mouse_clicks_per_min > 5.0;
        let high_shortcuts = engagement.shortcut_ratio > 0.3;
        let is_idle = engagement.idle_ratio > 0.5;
        let is_code_file = Self::looks_like_code_file(content);

        // ActiveCoding: high typing in a coding app
        if high_keystrokes && category == AppCategory::Development {
            return WorkType::ActiveCoding;
        }

        // CodeReview: scrolling through code with low typing
        if has_scrolling && low_keystrokes && (category == AppCategory::Development || is_code_file)
        {
            return WorkType::CodeReview;
        }

        // FormFilling: moderate typing + high clicks + browser/other category
        if moderate_keystrokes
            && high_clicks
            && matches!(category, AppCategory::Browser | AppCategory::Other)
        {
            return WorkType::FormFilling;
        }

        // Writing: steady typing in a document app
        if moderate_keystrokes
            && (category == AppCategory::Documentation || category == AppCategory::Browser)
            && !is_code_file
            && engagement.typing_burst_count > 0
        {
            return WorkType::Writing;
        }

        // Designing: continuous mouse activity in a design app
        if high_mouse && category == AppCategory::Design {
            return WorkType::Designing;
        }

        // PassiveMeeting: idle in a communication app (likely a video call)
        if is_idle && category == AppCategory::Communication {
            return WorkType::PassiveMeeting;
        }

        // ActiveMeeting: typing in a communication app
        if moderate_keystrokes && category == AppCategory::Communication {
            return WorkType::ActiveMeeting;
        }

        // Reading: heavy scroll, low keystrokes
        if heavy_scrolling && low_keystrokes {
            return WorkType::Reading;
        }

        // Navigation: high shortcut ratio, low actual typing
        if high_shortcuts && low_keystrokes {
            return WorkType::Navigation;
        }

        // Browsing: moderate clicking in a browser
        if category == AppCategory::Browser && engagement.mouse_clicks_per_min > 5.0 {
            return WorkType::Browsing;
        }

        WorkType::Unknown
    }

    /// Simple heuristic to detect code file content labels.
    fn looks_like_code_file(content: &str) -> bool {
        let code_extensions = [
            ".rs", ".py", ".js", ".ts", ".tsx", ".jsx", ".go", ".java", ".kt", ".cpp", ".c", ".h",
            ".rb", ".swift", ".dart", ".cs", ".vue", ".svelte", ".html", ".css", ".scss", ".json",
            ".yaml", ".yml", ".toml", ".xml",
        ];
        let lower = content.to_lowercase();
        code_extensions.iter().any(|ext| lower.ends_with(ext))
    }
}

impl Default for WorkTypeClassifier {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kb(keystrokes_per_min: u32, total: u32, shortcuts: u32, bursts: u32) -> KeyboardActivity {
        KeyboardActivity {
            keystrokes_per_min,
            total_keystrokes: total,
            typing_bursts: bursts,
            shortcut_count: shortcuts,
            correction_count: 0,
        }
    }

    fn mouse(clicks: u32, scrolls: u32) -> MouseActivity {
        MouseActivity {
            click_count: clicks,
            move_distance: 0.0,
            scroll_count: scrolls,
            last_position: None,
            double_click_count: 0,
            right_click_count: 0,
        }
    }

    #[test]
    fn active_coding() {
        let classifier = WorkTypeClassifier::new();
        let (work_type, _) = classifier.classify(
            &kb(65, 300, 10, 5),
            &mouse(5, 2),
            "main.rs",
            AppCategory::Development,
        );
        assert_eq!(work_type, WorkType::ActiveCoding);
    }

    #[test]
    fn code_review() {
        let classifier = WorkTypeClassifier::new();
        let (work_type, _) = classifier.classify(
            &kb(5, 20, 2, 0),
            &mouse(3, 15),
            "review.rs",
            AppCategory::Development,
        );
        assert_eq!(work_type, WorkType::CodeReview);
    }

    #[test]
    fn writing_in_docs() {
        let classifier = WorkTypeClassifier::new();
        let (work_type, _) = classifier.classify(
            &kb(45, 200, 5, 3),
            &mouse(2, 1),
            "Project Plan",
            AppCategory::Documentation,
        );
        assert_eq!(work_type, WorkType::Writing);
    }

    #[test]
    fn reading_heavy_scroll() {
        let classifier = WorkTypeClassifier::new();
        let (work_type, _) = classifier.classify(
            &kb(2, 5, 0, 0),
            &mouse(1, 20),
            "Article",
            AppCategory::Browser,
        );
        assert_eq!(work_type, WorkType::Reading);
    }

    #[test]
    fn designing() {
        let classifier = WorkTypeClassifier::new();
        let (work_type, _) = classifier.classify(
            &kb(3, 10, 2, 0),
            &mouse(25, 5),
            "Design System",
            AppCategory::Design,
        );
        assert_eq!(work_type, WorkType::Designing);
    }

    #[test]
    fn passive_meeting() {
        let classifier = WorkTypeClassifier::new();
        let (work_type, _) = classifier.classify(
            &kb(0, 0, 0, 0),
            &mouse(0, 0),
            "Zoom Meeting",
            AppCategory::Communication,
        );
        assert_eq!(work_type, WorkType::PassiveMeeting);
    }

    #[test]
    fn active_meeting() {
        let classifier = WorkTypeClassifier::new();
        let (work_type, _) = classifier.classify(
            &kb(45, 100, 5, 2),
            &mouse(3, 1),
            "#general",
            AppCategory::Communication,
        );
        assert_eq!(work_type, WorkType::ActiveMeeting);
    }

    #[test]
    fn navigation_shortcuts() {
        let classifier = WorkTypeClassifier::new();
        let (work_type, _) = classifier.classify(
            &kb(5, 10, 5, 0),
            &mouse(2, 1),
            "Finder",
            AppCategory::System,
        );
        assert_eq!(work_type, WorkType::Navigation);
    }

    #[test]
    fn browsing() {
        let classifier = WorkTypeClassifier::new();
        let (work_type, _) = classifier.classify(
            &kb(0, 0, 0, 0),
            &mouse(10, 2),
            "Hacker News",
            AppCategory::Browser,
        );
        assert_eq!(work_type, WorkType::Browsing);
    }

    #[test]
    fn zero_input_non_comm() {
        let classifier = WorkTypeClassifier::new();
        let (work_type, engagement) =
            classifier.classify(&kb(0, 0, 0, 0), &mouse(0, 0), "Unknown", AppCategory::Other);
        assert_eq!(work_type, WorkType::Unknown);
        assert!(engagement.idle_ratio > 0.5);
    }

    #[test]
    fn engagement_metrics_computed() {
        let classifier = WorkTypeClassifier::new();
        let (_, engagement) = classifier.classify(
            &kb(45, 200, 20, 4),
            &mouse(12, 8),
            "main.rs",
            AppCategory::Development,
        );
        assert!((engagement.keystrokes_per_min - 45.0).abs() < f32::EPSILON);
        assert!((engagement.mouse_clicks_per_min - 12.0).abs() < f32::EPSILON);
        assert!((engagement.scroll_events_per_min - 8.0).abs() < f32::EPSILON);
        assert!((engagement.shortcut_ratio - 0.1).abs() < 0.01); // 20/200 = 0.1
        assert_eq!(engagement.typing_burst_count, 4);
    }

    #[test]
    fn code_file_detection() {
        assert!(WorkTypeClassifier::looks_like_code_file("main.rs"));
        assert!(WorkTypeClassifier::looks_like_code_file("index.tsx"));
        assert!(WorkTypeClassifier::looks_like_code_file("Cargo.toml"));
        assert!(!WorkTypeClassifier::looks_like_code_file("Budget Report"));
        assert!(!WorkTypeClassifier::looks_like_code_file("#general"));
    }

    #[test]
    fn form_filling() {
        let classifier = WorkTypeClassifier::new();
        let (work_type, _) = classifier.classify(
            &kb(45, 200, 2, 1),
            &mouse(10, 1),
            "Registration Form",
            AppCategory::Browser,
        );
        // FormFilling: moderate typing + high clicks + browser category
        assert_eq!(work_type, WorkType::FormFilling);
    }

    // ── classify_extended / infer_from_subcategory tests ──

    use oneshim_core::models::app_registry::{AppSubcategory, KeystrokeProfile};

    fn profile(
        enter: f32,
        tab: f32,
        arrow: f32,
        backspace: f32,
        special: f32,
        total: u32,
    ) -> KeystrokeProfile {
        KeystrokeProfile {
            enter_ratio: enter,
            tab_ratio: tab,
            arrow_ratio: arrow,
            backspace_ratio: backspace,
            special_ratio: special,
            total_keystrokes: total,
        }
    }

    // ── Terminal rules ──

    #[test]
    fn terminal_commands_high_enter() {
        let c = WorkTypeClassifier::new();
        let (wt, _) = c.classify_extended(
            &kb(30, 100, 0, 0),
            &mouse(0, 0),
            "",
            AppCategory::Development,
            Some(AppSubcategory::Terminal),
            Some(&profile(0.20, 0.0, 0.0, 0.0, 0.0, 100)),
        );
        assert_eq!(wt, WorkType::TerminalCommands);
    }

    #[test]
    fn terminal_log_reading_low_keys_high_scroll() {
        let c = WorkTypeClassifier::new();
        let (wt, _) = c.classify_extended(
            &kb(2, 5, 0, 0),
            &mouse(0, 10),
            "",
            AppCategory::Development,
            Some(AppSubcategory::Terminal),
            Some(&profile(0.0, 0.0, 0.0, 0.0, 0.0, 5)),
        );
        assert_eq!(wt, WorkType::LogReading);
    }

    #[test]
    fn terminal_tui_editor_arrows() {
        let c = WorkTypeClassifier::new();
        let (wt, _) = c.classify_extended(
            &kb(50, 200, 0, 0),
            &mouse(0, 0),
            "",
            AppCategory::Development,
            Some(AppSubcategory::Terminal),
            Some(&profile(0.05, 0.0, 0.25, 0.0, 0.0, 200)),
        );
        assert_eq!(wt, WorkType::ActiveCoding);
    }

    #[test]
    fn terminal_active_coding_fast_typing() {
        let c = WorkTypeClassifier::new();
        let (wt, _) = c.classify_extended(
            &kb(50, 200, 0, 0),
            &mouse(0, 0),
            "",
            AppCategory::Development,
            Some(AppSubcategory::Terminal),
            Some(&profile(0.05, 0.0, 0.05, 0.1, 0.0, 200)),
        );
        assert_eq!(wt, WorkType::ActiveCoding);
    }

    // ── Document Editor rules ──

    #[test]
    fn doc_editor_writing_high_keys_low_enter() {
        let c = WorkTypeClassifier::new();
        let (wt, _) = c.classify_extended(
            &kb(50, 200, 0, 2),
            &mouse(0, 0),
            "",
            AppCategory::Documentation,
            Some(AppSubcategory::DocumentEditor),
            Some(&profile(0.02, 0.0, 0.0, 0.05, 0.0, 200)),
        );
        assert_eq!(wt, WorkType::DocumentWriting);
    }

    #[test]
    fn doc_editor_reading_low_keys_scrolling() {
        let c = WorkTypeClassifier::new();
        let (wt, _) = c.classify_extended(
            &kb(2, 5, 0, 0),
            &mouse(0, 5),
            "",
            AppCategory::Documentation,
            Some(AppSubcategory::DocumentEditor),
            Some(&profile(0.0, 0.0, 0.0, 0.0, 0.0, 5)),
        );
        assert_eq!(wt, WorkType::DocumentReading);
    }

    #[test]
    fn doc_editor_outline_writing() {
        let c = WorkTypeClassifier::new();
        let (wt, _) = c.classify_extended(
            &kb(25, 100, 0, 1),
            &mouse(0, 0),
            "",
            AppCategory::Documentation,
            Some(AppSubcategory::DocumentEditor),
            Some(&profile(0.12, 0.0, 0.0, 0.0, 0.0, 100)),
        );
        assert_eq!(wt, WorkType::Writing);
    }

    // ── Chat rules ──

    #[test]
    fn chat_composing() {
        let c = WorkTypeClassifier::new();
        let (wt, _) = c.classify_extended(
            &kb(30, 100, 0, 1),
            &mouse(2, 0),
            "",
            AppCategory::Communication,
            Some(AppSubcategory::Chat),
            Some(&profile(0.12, 0.0, 0.0, 0.05, 0.0, 100)),
        );
        assert_eq!(wt, WorkType::ChatComposing);
    }

    #[test]
    fn chat_reading() {
        let c = WorkTypeClassifier::new();
        let (wt, _) = c.classify_extended(
            &kb(2, 5, 0, 0),
            &mouse(0, 3),
            "",
            AppCategory::Communication,
            Some(AppSubcategory::Chat),
            Some(&profile(0.0, 0.0, 0.0, 0.0, 0.0, 5)),
        );
        assert_eq!(wt, WorkType::Reading);
    }

    // ── Spreadsheet rules ──

    #[test]
    fn spreadsheet_form_filling() {
        let c = WorkTypeClassifier::new();
        let (wt, _) = c.classify_extended(
            &kb(15, 60, 0, 0),
            &mouse(5, 0),
            "",
            AppCategory::Documentation,
            Some(AppSubcategory::Spreadsheet),
            Some(&profile(0.0, 0.20, 0.0, 0.0, 0.0, 60)),
        );
        assert_eq!(wt, WorkType::FormFilling);
    }

    #[test]
    fn spreadsheet_reading() {
        let c = WorkTypeClassifier::new();
        let (wt, _) = c.classify_extended(
            &kb(2, 5, 0, 0),
            &mouse(0, 10),
            "",
            AppCategory::Documentation,
            Some(AppSubcategory::Spreadsheet),
            Some(&profile(0.0, 0.0, 0.0, 0.0, 0.0, 5)),
        );
        assert_eq!(wt, WorkType::Reading);
    }

    // ── TUI Editor ──

    #[test]
    fn tui_editor_active_coding() {
        let c = WorkTypeClassifier::new();
        let (wt, _) = c.classify_extended(
            &kb(50, 200, 0, 0),
            &mouse(0, 0),
            "",
            AppCategory::Development,
            Some(AppSubcategory::TuiEditor),
            Some(&profile(0.0, 0.0, 0.3, 0.0, 0.1, 200)),
        );
        assert_eq!(wt, WorkType::ActiveCoding);
    }

    // ── Fallback ──

    #[test]
    fn ide_subcategory_falls_through() {
        let c = WorkTypeClassifier::new();
        let (wt, _) = c.classify_extended(
            &kb(65, 300, 10, 5),
            &mouse(5, 2),
            "main.rs",
            AppCategory::Development,
            Some(AppSubcategory::Ide),
            Some(&profile(0.02, 0.0, 0.0, 0.1, 0.0, 300)),
        );
        // Falls through to existing rules -> ActiveCoding
        assert_eq!(wt, WorkType::ActiveCoding);
    }

    #[test]
    fn no_subcategory_uses_existing_rules() {
        let c = WorkTypeClassifier::new();
        let (wt1, _) = c.classify(
            &kb(65, 300, 10, 5),
            &mouse(5, 2),
            "main.rs",
            AppCategory::Development,
        );
        let (wt2, _) = c.classify_extended(
            &kb(65, 300, 10, 5),
            &mouse(5, 2),
            "main.rs",
            AppCategory::Development,
            None,
            None,
        );
        assert_eq!(wt1, wt2);
    }
}
