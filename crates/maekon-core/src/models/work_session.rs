use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AppCategory {
    Communication,
    Development,
    Documentation,
    Browser,
    Design,
    Media,
    System,
    #[default]
    Other,
}

impl AppCategory {
    pub fn from_app_name(app_name: &str) -> Self {
        let name = app_name.to_lowercase();

        if name.contains("slack")
            || name.contains("teams")
            || name.contains("discord")
            || name.contains("zoom")
            || name.contains("meet")
            || name.contains("mail")
            || name.contains("outlook")
            || name.contains("gmail")
            || name.contains("messages")
            || name.contains("kakaotalk")
            || name.contains("telegram")
            || name.contains("thunderbird")
            || name.contains("whatsapp")
        {
            return Self::Communication;
        }

        if name.contains("code")
            || name.contains("visual studio")
            || name.contains("intellij")
            || name.contains("pycharm")
            || name.contains("webstorm")
            || name.contains("android studio")
            || name.contains("xcode")
            || name.contains("terminal")
            || name.contains("iterm")
            || name.contains("warp")
            || name.contains("alacritty")
            || name.contains("cursor")
            || name.contains("vim")
            || name.contains("neovim")
            || name.contains("emacs")
            || name.contains("git")
            || name.contains("sourcetree")
            || name.contains("postman")
            || name.contains("insomnia")
        {
            return Self::Development;
        }

        if name.contains("notion")
            || name.contains("confluence")
            || name.contains("word")
            || name.contains("excel")
            || name.contains("powerpoint")
            || name.contains("pages")
            || name.contains("numbers")
            || name.contains("keynote")
            || name.contains("google docs")
            || name.contains("obsidian")
            || name.contains("typora")
        {
            return Self::Documentation;
        }

        if name.contains("chrome")
            || name.contains("safari")
            || name.contains("firefox")
            || name.contains("edge")
            || name.contains("arc")
            || name.contains("brave")
            || name.contains("opera")
        {
            return Self::Browser;
        }

        if name.contains("figma")
            || name.contains("sketch")
            || name.contains("photoshop")
            || name.contains("illustrator")
            || name.contains("canva")
        {
            return Self::Design;
        }

        if name.contains("spotify")
            || name.contains("music")
            || name.contains("youtube")
            || name.contains("netflix")
            || name.contains("vlc")
        {
            return Self::Media;
        }

        if name.contains("finder")
            || name.contains("explorer")
            || name.contains("settings")
            || name.contains("system preferences")
            || name.contains("activity monitor")
            || name.contains("task manager")
        {
            return Self::System;
        }

        Self::Other
    }

    /// Parse a category name string (case-insensitive) into an `AppCategory`.
    ///
    /// This is the inverse of the serde `rename_all = "snake_case"` names.
    /// Falls back to `Other` for unknown values. Display labels are the frontend's
    /// concern (it receives the serde variant and translates it via i18n).
    pub fn from_category_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "communication" => Self::Communication,
            "development" => Self::Development,
            "documentation" => Self::Documentation,
            "browser" => Self::Browser,
            "design" => Self::Design,
            "media" => Self::Media,
            "system" => Self::System,
            "other" => Self::Other,
            _ => Self::Other,
        }
    }

    pub fn is_communication(&self) -> bool {
        matches!(self, Self::Communication)
    }

    pub fn is_deep_work(&self) -> bool {
        matches!(self, Self::Development | Self::Documentation | Self::Design)
    }

    /// Convenience: classify an app name as a coding/development app.
    pub fn is_coding(app_name: &str) -> bool {
        matches!(Self::from_app_name(app_name), AppCategory::Development)
    }

    /// Convenience: classify an app name as a communication app.
    pub fn is_communication_app(app_name: &str) -> bool {
        matches!(Self::from_app_name(app_name), AppCategory::Communication)
    }

    /// Convenience: classify an app name as a browser.
    pub fn is_browser(app_name: &str) -> bool {
        matches!(Self::from_app_name(app_name), AppCategory::Browser)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Active,
    EndedByIdle,
    EndedBySwitch,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkSession {
    pub id: i64,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub primary_app: String,
    pub category: AppCategory,
    pub state: SessionState,
    pub interruption_count: u32,
    pub deep_work_secs: u64,
    pub duration_secs: u64,
}

impl WorkSession {
    pub fn new(id: i64, app_name: String) -> Self {
        let category = AppCategory::from_app_name(&app_name);
        Self {
            id,
            started_at: Utc::now(),
            ended_at: None,
            primary_app: app_name,
            category,
            state: SessionState::Active,
            interruption_count: 0,
            deep_work_secs: 0,
            duration_secs: 0,
        }
    }

    pub fn is_active(&self) -> bool {
        self.state == SessionState::Active
    }

    pub fn focus_score(&self) -> f32 {
        if self.duration_secs == 0 {
            return 0.0;
        }

        let deep_work_ratio = self.deep_work_secs as f32 / self.duration_secs as f32;
        let interruption_penalty = (self.interruption_count as f32 * 0.1).min(0.5);

        (deep_work_ratio - interruption_penalty).clamp(0.0, 1.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Interruption {
    pub id: i64,
    pub interrupted_at: DateTime<Utc>,
    pub from_app: String,
    pub from_category: AppCategory,
    pub to_app: String,
    pub to_category: AppCategory,
    pub snapshot_frame_id: Option<i64>,
    pub resumed_at: Option<DateTime<Utc>>,
    pub resumed_to_app: Option<String>,
    pub duration_secs: Option<u64>,
}

impl Interruption {
    pub fn new(id: i64, from_app: String, to_app: String, snapshot_frame_id: Option<i64>) -> Self {
        Self {
            id,
            interrupted_at: Utc::now(),
            from_category: AppCategory::from_app_name(&from_app),
            from_app,
            to_category: AppCategory::from_app_name(&to_app),
            to_app,
            snapshot_frame_id,
            resumed_at: None,
            resumed_to_app: None,
            duration_secs: None,
        }
    }

    pub fn mark_resumed(&mut self, resumed_to_app: String) {
        let now = Utc::now();
        self.resumed_at = Some(now);
        self.resumed_to_app = Some(resumed_to_app);
        self.duration_secs = Some((now - self.interrupted_at).num_seconds() as u64);
    }

    pub fn resumed_to_original(&self) -> bool {
        self.resumed_to_app
            .as_ref()
            .map(|app| app == &self.from_app)
            .unwrap_or(false)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FocusMetrics {
    pub period: crate::types::TimeWindow,
    pub total_active_secs: u64,
    pub deep_work_secs: u64,
    pub communication_secs: u64,
    pub context_switches: u32,
    pub interruption_count: u32,
    pub avg_focus_duration_secs: u64,
    pub max_focus_duration_secs: u64,
    pub focus_score: f32,
}

impl FocusMetrics {
    /// Construct a FocusMetrics with default scalar fields.
    ///
    /// Returns Result because TimeWindow::new validates `start <= end`. Internal
    /// callers using cron-aligned `date_to_period_range` may use
    /// `.expect("date_to_period_range produces valid window")`.
    ///
    /// # Errors
    /// Returns [`crate::types::TimeWindowError::InvertedBounds`] if `start > end`.
    pub fn new(
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Self, crate::types::TimeWindowError> {
        let period = crate::types::TimeWindow::new(start, end)?;
        Ok(Self {
            period,
            total_active_secs: 0,
            deep_work_secs: 0,
            communication_secs: 0,
            context_switches: 0,
            interruption_count: 0,
            avg_focus_duration_secs: 0,
            max_focus_duration_secs: 0,
            focus_score: 0.0,
        })
    }

    pub fn communication_ratio(&self) -> f32 {
        if self.total_active_secs == 0 {
            return 0.0;
        }
        self.communication_secs as f32 / self.total_active_secs as f32
    }

    pub fn deep_work_ratio(&self) -> f32 {
        if self.total_active_secs == 0 {
            return 0.0;
        }
        self.deep_work_secs as f32 / self.total_active_secs as f32
    }

    pub fn interruptions_per_hour(&self) -> f32 {
        let hours = self.period.duration().num_seconds() as f32 / 3600.0;
        if hours == 0.0 {
            return 0.0;
        }
        self.interruption_count as f32 / hours
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryUsage {
    pub category: AppCategory,
    pub duration_secs: u64,
    pub ratio: f32,
    pub session_count: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_category_from_name() {
        assert_eq!(
            AppCategory::from_app_name("Slack"),
            AppCategory::Communication
        );
        assert_eq!(
            AppCategory::from_app_name("Visual Studio Code"),
            AppCategory::Development
        );
        assert_eq!(
            AppCategory::from_app_name("Google Chrome"),
            AppCategory::Browser
        );
        assert_eq!(
            AppCategory::from_app_name("Notion"),
            AppCategory::Documentation
        );
        assert_eq!(AppCategory::from_app_name("Figma"), AppCategory::Design);
        assert_eq!(
            AppCategory::from_app_name("Unknown App"),
            AppCategory::Other
        );
    }

    #[test]
    fn app_category_is_communication() {
        assert!(AppCategory::Communication.is_communication());
        assert!(!AppCategory::Development.is_communication());
    }

    #[test]
    fn app_category_is_deep_work() {
        assert!(AppCategory::Development.is_deep_work());
        assert!(AppCategory::Documentation.is_deep_work());
        assert!(!AppCategory::Communication.is_deep_work());
        assert!(!AppCategory::Browser.is_deep_work());
    }

    #[test]
    fn work_session_focus_score() {
        let mut session = WorkSession::new(1, "Code".to_string());
        session.duration_secs = 3600; // 1 hour
        session.deep_work_secs = 3000; // 50 min
        session.interruption_count = 2;

        let score = session.focus_score();
        // deep_work_ratio = 3000/3600 = 0.833
        // interruption_penalty = 2 * 0.1 = 0.2
        // score = 0.833 - 0.2 = 0.633
        assert!(score > 0.6 && score < 0.7);
    }

    #[test]
    fn interruption_resumed_to_original() {
        let mut interruption =
            Interruption::new(1, "Code".to_string(), "Slack".to_string(), Some(100));

        assert!(!interruption.resumed_to_original());

        interruption.mark_resumed("Code".to_string());
        assert!(interruption.resumed_to_original());
    }

    #[test]
    fn focus_metrics_ratios() {
        let now = Utc::now();
        let mut metrics = FocusMetrics::new(now, now + chrono::Duration::hours(1))
            .expect("trusted test bounds: now <= now + 1h");
        metrics.total_active_secs = 3600;
        metrics.deep_work_secs = 2400; // 40 min
        metrics.communication_secs = 1200; // 20 min
        assert!((metrics.deep_work_ratio() - 0.667).abs() < 0.01);
        assert!((metrics.communication_ratio() - 0.333).abs() < 0.01);
    }
}

/// #10197 Wave 1: mutation guards for the app classifier and focus metrics.
///
/// The full-crate measurement (run 31027028682) left 74 surviving mutants in
/// this file, 55 of them `||` -> `&&` inside `from_app_name`'s keyword chains:
/// the existing test checked ONE keyword per category, so every other arm
/// could flip silently. The classifier feeds deep-work/communication ratios,
/// so a silently narrowed chain skews focus analytics rather than crashing.
///
/// The table below carries one input per keyword arm. Each input is chosen to
/// match its own arm FIRST (chains are ordered, first category wins), which
/// makes each `||` independently load-bearing.
#[cfg(test)]
mod mutation_guard_tests {
    use super::*;
    use chrono::{Duration as ChronoDuration, Utc};

    /// One entry per keyword arm, in chain order. Two known shadowed arms are
    /// deliberately still listed with their own keyword ("gmail" contains
    /// "mail", "neovim" contains "vim" — the earlier arm always matches first,
    /// so those rows guard chain-restructuring mutants rather than the arm
    /// itself; a survivor pinned to exactly those operators is equivalent).
    const KEYWORD_TABLE: &[(&str, AppCategory)] = &[
        // Communication
        ("slack", AppCategory::Communication),
        ("teams", AppCategory::Communication),
        ("discord", AppCategory::Communication),
        ("zoom", AppCategory::Communication),
        ("meet", AppCategory::Communication),
        ("mail", AppCategory::Communication),
        ("outlook", AppCategory::Communication),
        ("gmail", AppCategory::Communication),
        ("messages", AppCategory::Communication),
        ("kakaotalk", AppCategory::Communication),
        ("telegram", AppCategory::Communication),
        ("thunderbird", AppCategory::Communication),
        ("whatsapp", AppCategory::Communication),
        // Development
        ("code", AppCategory::Development),
        ("visual studio", AppCategory::Development),
        ("intellij", AppCategory::Development),
        ("pycharm", AppCategory::Development),
        ("webstorm", AppCategory::Development),
        ("android studio", AppCategory::Development),
        ("xcode", AppCategory::Development),
        ("terminal", AppCategory::Development),
        ("iterm", AppCategory::Development),
        ("warp", AppCategory::Development),
        ("alacritty", AppCategory::Development),
        ("cursor", AppCategory::Development),
        ("vim", AppCategory::Development),
        ("neovim", AppCategory::Development),
        ("emacs", AppCategory::Development),
        ("git", AppCategory::Development),
        ("sourcetree", AppCategory::Development),
        ("postman", AppCategory::Development),
        ("insomnia", AppCategory::Development),
        // Documentation
        ("notion", AppCategory::Documentation),
        ("confluence", AppCategory::Documentation),
        ("word", AppCategory::Documentation),
        ("excel", AppCategory::Documentation),
        ("powerpoint", AppCategory::Documentation),
        ("pages", AppCategory::Documentation),
        ("numbers", AppCategory::Documentation),
        ("keynote", AppCategory::Documentation),
        ("google docs", AppCategory::Documentation),
        ("obsidian", AppCategory::Documentation),
        ("typora", AppCategory::Documentation),
        // Browser
        ("chrome", AppCategory::Browser),
        ("safari", AppCategory::Browser),
        ("firefox", AppCategory::Browser),
        ("edge", AppCategory::Browser),
        ("arc", AppCategory::Browser),
        ("brave", AppCategory::Browser),
        ("opera", AppCategory::Browser),
        // Design
        ("figma", AppCategory::Design),
        ("sketch", AppCategory::Design),
        ("photoshop", AppCategory::Design),
        ("illustrator", AppCategory::Design),
        ("canva", AppCategory::Design),
        // Media
        ("spotify", AppCategory::Media),
        ("music", AppCategory::Media),
        ("youtube", AppCategory::Media),
        ("netflix", AppCategory::Media),
        ("vlc", AppCategory::Media),
        // System
        ("finder", AppCategory::System),
        ("explorer", AppCategory::System),
        ("settings", AppCategory::System),
        ("system preferences", AppCategory::System),
        ("activity monitor", AppCategory::System),
        ("task manager", AppCategory::System),
    ];

    #[test]
    fn every_keyword_arm_classifies_on_its_own() {
        for (input, expected) in KEYWORD_TABLE {
            assert_eq!(
                AppCategory::from_app_name(input),
                *expected,
                "keyword {input:?} must reach {expected:?} through its own arm"
            );
        }
    }

    #[test]
    fn classification_is_case_insensitive_and_substring_based() {
        // Real app names embed the keyword; the classifier lowercases first.
        assert_eq!(
            AppCategory::from_app_name("KakaoTalk Desktop"),
            AppCategory::Communication
        );
        assert_eq!(
            AppCategory::from_app_name("Activity Monitor"),
            AppCategory::System
        );
    }

    #[test]
    fn every_category_string_round_trips() {
        // from_category_str's match arms are delete-arm mutant targets: a
        // deleted arm falls through to Other and only an exact assertion per
        // arm catches it. The strings mirror serde's snake_case names.
        let cases = [
            ("communication", AppCategory::Communication),
            ("development", AppCategory::Development),
            ("documentation", AppCategory::Documentation),
            ("browser", AppCategory::Browser),
            ("design", AppCategory::Design),
            ("media", AppCategory::Media),
            ("system", AppCategory::System),
            ("other", AppCategory::Other),
        ];
        for (s, expected) in cases {
            assert_eq!(AppCategory::from_category_str(s), expected, "arm {s:?}");
            // Case-insensitivity is part of the contract.
            assert_eq!(
                AppCategory::from_category_str(&s.to_uppercase()),
                expected,
                "uppercase arm {s:?}"
            );
        }
        assert_eq!(
            AppCategory::from_category_str("no-such-category"),
            AppCategory::Other
        );
    }

    #[test]
    fn is_active_reflects_the_session_state() {
        let mut session = WorkSession::new(1, "slack".to_string());
        assert!(session.is_active(), "a new session starts Active");
        session.state = SessionState::EndedByIdle;
        assert!(!session.is_active(), "an ended session is not active");
    }

    #[test]
    fn interruptions_per_hour_divides_count_by_window_hours() {
        // A 2-hour window with 6 interruptions is 3.0/h — a value that
        // distinguishes the real quotient from the constant-replacement
        // mutants (0.0 / 1.0 / -1.0) AND from count-only or hours-only forms.
        let start = Utc::now();
        let end = start + ChronoDuration::hours(2);
        let mut metrics = FocusMetrics::new(start, end).expect("valid window");
        metrics.interruption_count = 6;
        let per_hour = metrics.interruptions_per_hour();
        assert!(
            (per_hour - 3.0).abs() < f32::EPSILON,
            "6 interruptions over 2h must be 3.0/h, got {per_hour}"
        );
    }

    #[test]
    fn interruptions_per_hour_is_zero_for_an_empty_window() {
        let start = Utc::now();
        let mut metrics = FocusMetrics::new(start, start).expect("degenerate window is valid");
        metrics.interruption_count = 5;
        assert_eq!(
            metrics.interruptions_per_hour(),
            0.0,
            "zero-length window must not divide by zero"
        );
    }
}
