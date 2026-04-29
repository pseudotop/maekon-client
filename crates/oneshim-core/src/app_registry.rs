use std::path::Path;

use crate::error::CoreError;
use crate::models::app_registry::{AccessibilityStrategy, AppProfile, AppSubcategory};
use crate::models::work_session::AppCategory;

/// Application profile registry.
///
/// Single source of truth for app identification, classification, and
/// behavioral hints. Replaces three scattered app lists.
///
/// Loading order:
/// 1. Built-in profiles (compiled into the binary, ~50 apps)
/// 2. User override file (~/.oneshim/app_profiles.json) merged on top
pub struct AppRegistry {
    profiles: Vec<AppProfile>,
}

impl AppRegistry {
    /// Create a registry with built-in profiles only.
    pub fn new() -> Self {
        Self {
            profiles: built_in_profiles(),
        }
    }

    /// Load user overrides from JSON file and merge with built-in profiles.
    ///
    /// User overrides can add new profiles, modify existing ones (matched
    /// by name_patterns overlap), or disable built-in profiles.
    pub fn load_user_overrides(&mut self, path: &Path) -> Result<(), CoreError> {
        let content = std::fs::read_to_string(path).map_err(|e| CoreError::Config {
            code: crate::error_codes::ConfigCode::Invalid,
            message: format!("Failed to read app profiles from {}: {}", path.display(), e),
        })?;
        let overrides: Vec<AppProfile> =
            serde_json::from_str(&content).map_err(|e| CoreError::Config {
                code: crate::error_codes::ConfigCode::Invalid,
                message: format!(
                    "Failed to parse app profiles from {}: {}",
                    path.display(),
                    e
                ),
            })?;

        for override_profile in overrides {
            // Check if any existing profile shares a name_pattern
            let existing_idx = self.profiles.iter().position(|p| {
                p.name_patterns.iter().any(|pat| {
                    override_profile
                        .name_patterns
                        .iter()
                        .any(|op| op.eq_ignore_ascii_case(pat))
                })
            });

            if let Some(idx) = existing_idx {
                self.profiles[idx] = override_profile;
            } else {
                self.profiles.push(override_profile);
            }
        }

        Ok(())
    }

    /// Look up the profile for a given app name. Returns the first matching
    /// enabled profile. O(n) scan over ~100 entries.
    pub fn lookup(&self, app_name: &str) -> Option<&AppProfile> {
        let lower = app_name.to_lowercase();
        self.profiles
            .iter()
            .find(|p| p.enabled && p.name_patterns.iter().any(|pat| lower.contains(pat)))
    }

    /// Convenience: get category + subcategory for an app name.
    /// Falls back to (AppCategory::from_app_name, AppSubcategory::Other)
    /// when no profile matches.
    pub fn classify(&self, app_name: &str) -> (AppCategory, AppSubcategory) {
        match self.lookup(app_name) {
            Some(profile) => (profile.category, profile.subcategory),
            None => (AppCategory::from_app_name(app_name), AppSubcategory::Other),
        }
    }

    /// Check if an app is sensitive (should suppress capture).
    pub fn is_sensitive(&self, app_name: &str) -> bool {
        self.lookup(app_name).is_some_and(|p| p.sensitive)
    }

    /// Number of profiles in the registry.
    pub fn len(&self) -> usize {
        self.profiles.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }
}

impl Default for AppRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Built-in profile data
// ---------------------------------------------------------------------------

fn p(
    name: &str,
    patterns: &[&str],
    cat: AppCategory,
    sub: AppSubcategory,
    sensitive: bool,
) -> AppProfile {
    AppProfile {
        name: name.to_string(),
        name_patterns: patterns.iter().map(|s| s.to_string()).collect(),
        category: cat,
        subcategory: sub,
        title_hints: vec![],
        accessibility_strategy: AccessibilityStrategy::None,
        sensitive,
        enabled: true,
    }
}

/// Build the default set of ~75 application profiles.
///
/// Profiles are ordered most-specific first within each category to ensure
/// correct matching (e.g. "xcode" before "code"). Type aliases `C` and `S`
/// disambiguate overlapping variant names between `AppCategory` and
/// `AppSubcategory`.
fn built_in_profiles() -> Vec<AppProfile> {
    use AppCategory as C;
    use AppSubcategory as S;

    vec![
        // -- Sensitive (checked first by is_sensitive callers) --
        p("1Password", &["1password"], C::System, S::System, true),
        p("LastPass", &["lastpass"], C::System, S::System, true),
        p("Bitwarden", &["bitwarden"], C::System, S::System, true),
        p("Dashlane", &["dashlane"], C::System, S::System, true),
        p("KeePass", &["keepass"], C::System, S::System, true),
        p("Enpass", &["enpass"], C::System, S::System, true),
        p("NordPass", &["nordpass"], C::System, S::System, true),
        // -- Development: Terminals --
        p("iTerm2", &["iterm"], C::Development, S::Terminal, false),
        p("Warp", &["warp"], C::Development, S::Terminal, false),
        p(
            "Alacritty",
            &["alacritty"],
            C::Development,
            S::Terminal,
            false,
        ),
        p("kitty", &["kitty"], C::Development, S::Terminal, false),
        p("Hyper", &["hyper"], C::Development, S::Terminal, false),
        p(
            "Terminal",
            &["terminal.app", "terminal"],
            C::Development,
            S::Terminal,
            false,
        ),
        p("Konsole", &["konsole"], C::Development, S::Terminal, false),
        p(
            "Windows Terminal",
            &["windows terminal", "windowsterminal", "wt.exe"],
            C::Development,
            S::Terminal,
            false,
        ),
        // -- Development: IDEs (order matters: xcode before code) --
        p("Xcode", &["xcode"], C::Development, S::Ide, false),
        p(
            "Android Studio",
            &["android studio"],
            C::Development,
            S::Ide,
            false,
        ),
        p(
            "Visual Studio Code",
            &["visual studio code", "code"],
            C::Development,
            S::Ide,
            false,
        ),
        p("Cursor", &["cursor"], C::Development, S::Ide, false),
        p(
            "IntelliJ IDEA",
            &["intellij"],
            C::Development,
            S::Ide,
            false,
        ),
        p("WebStorm", &["webstorm"], C::Development, S::Ide, false),
        p("PyCharm", &["pycharm"], C::Development, S::Ide, false),
        p("GoLand", &["goland"], C::Development, S::Ide, false),
        p("RustRover", &["rustrover"], C::Development, S::Ide, false),
        p("CLion", &["clion"], C::Development, S::Ide, false),
        p("Rider", &["rider"], C::Development, S::Ide, false),
        // -- Development: TUI Editors --
        p(
            "Neovim",
            &["neovim", "nvim"],
            C::Development,
            S::TuiEditor,
            false,
        ),
        p("Vim", &["vim"], C::Development, S::TuiEditor, false),
        p("Emacs", &["emacs"], C::Development, S::TuiEditor, false),
        // -- Development: API Tools --
        p("Postman", &["postman"], C::Development, S::ApiTool, false),
        p("Insomnia", &["insomnia"], C::Development, S::ApiTool, false),
        p("Bruno", &["bruno"], C::Development, S::ApiTool, false),
        // -- Development: Git GUI --
        p(
            "SourceTree",
            &["sourcetree"],
            C::Development,
            S::GitGui,
            false,
        ),
        p(
            "GitKraken",
            &["gitkraken"],
            C::Development,
            S::GitGui,
            false,
        ),
        p("Fork", &["fork"], C::Development, S::GitGui, false),
        // -- Documentation: Document Editors --
        p(
            "Notion",
            &["notion"],
            C::Documentation,
            S::DocumentEditor,
            false,
        ),
        p(
            "Obsidian",
            &["obsidian"],
            C::Documentation,
            S::DocumentEditor,
            false,
        ),
        p(
            "Typora",
            &["typora"],
            C::Documentation,
            S::DocumentEditor,
            false,
        ),
        p(
            "Microsoft Word",
            &["word"],
            C::Documentation,
            S::DocumentEditor,
            false,
        ),
        p(
            "Google Docs",
            &["google docs"],
            C::Documentation,
            S::DocumentEditor,
            false,
        ),
        p(
            "Pages",
            &["pages"],
            C::Documentation,
            S::DocumentEditor,
            false,
        ),
        // -- Documentation: Spreadsheets --
        p(
            "Microsoft Excel",
            &["excel"],
            C::Documentation,
            S::Spreadsheet,
            false,
        ),
        p(
            "Numbers",
            &["numbers"],
            C::Documentation,
            S::Spreadsheet,
            false,
        ),
        p(
            "Google Sheets",
            &["sheets"],
            C::Documentation,
            S::Spreadsheet,
            false,
        ),
        // -- Documentation: Presentations --
        p(
            "PowerPoint",
            &["powerpoint"],
            C::Documentation,
            S::Presentation,
            false,
        ),
        p(
            "Keynote",
            &["keynote"],
            C::Documentation,
            S::Presentation,
            false,
        ),
        // -- Communication: Chat --
        p("Slack", &["slack"], C::Communication, S::Chat, false),
        p("Discord", &["discord"], C::Communication, S::Chat, false),
        p(
            "Microsoft Teams",
            &["teams"],
            C::Communication,
            S::Chat,
            false,
        ),
        p(
            "KakaoTalk",
            &["kakaotalk"],
            C::Communication,
            S::Chat,
            false,
        ),
        p("Telegram", &["telegram"], C::Communication, S::Chat, false),
        p("WhatsApp", &["whatsapp"], C::Communication, S::Chat, false),
        // -- Communication: Email --
        p("Mail", &["mail"], C::Communication, S::Email, false),
        p("Outlook", &["outlook"], C::Communication, S::Email, false),
        p(
            "Thunderbird",
            &["thunderbird"],
            C::Communication,
            S::Email,
            false,
        ),
        p("Gmail", &["gmail"], C::Communication, S::Email, false),
        // -- Communication: Video --
        p("Zoom", &["zoom"], C::Communication, S::VideoCall, false),
        p(
            "FaceTime",
            &["facetime"],
            C::Communication,
            S::VideoCall,
            false,
        ),
        // -- Browser --
        p("Google Chrome", &["chrome"], C::Browser, S::Browser, false),
        p("Safari", &["safari"], C::Browser, S::Browser, false),
        p("Firefox", &["firefox"], C::Browser, S::Browser, false),
        p("Microsoft Edge", &["edge"], C::Browser, S::Browser, false),
        p("Arc", &["arc"], C::Browser, S::Browser, false),
        p("Brave", &["brave"], C::Browser, S::Browser, false),
        p("Opera", &["opera"], C::Browser, S::Browser, false),
        // -- Design --
        p("Figma", &["figma"], C::Design, S::Design, false),
        p("Sketch", &["sketch"], C::Design, S::Design, false),
        p("Photoshop", &["photoshop"], C::Design, S::Design, false),
        p("Illustrator", &["illustrator"], C::Design, S::Design, false),
        p("Canva", &["canva"], C::Design, S::Design, false),
        // -- Media --
        p("Spotify", &["spotify"], C::Media, S::Media, false),
        p("YouTube", &["youtube"], C::Media, S::Media, false),
        p("VLC", &["vlc"], C::Media, S::Media, false),
        // -- System --
        p("Finder", &["finder"], C::System, S::System, false),
        p(
            "Activity Monitor",
            &["activity monitor"],
            C::System,
            S::System,
            false,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_case_insensitive() {
        let registry = AppRegistry::new();
        let profile = registry.lookup("iTerm2").unwrap();
        assert_eq!(profile.subcategory, AppSubcategory::Terminal);

        let profile2 = registry.lookup("ITERM").unwrap();
        assert_eq!(profile2.subcategory, AppSubcategory::Terminal);
    }

    #[test]
    fn classify_known_app() {
        let registry = AppRegistry::new();
        let (cat, sub) = registry.classify("Visual Studio Code");
        assert_eq!(cat, AppCategory::Development);
        assert_eq!(sub, AppSubcategory::Ide);
    }

    #[test]
    fn classify_unknown_app_falls_back() {
        let registry = AppRegistry::new();
        let (cat, sub) = registry.classify("SomeRandomApp");
        assert_eq!(cat, AppCategory::Other);
        assert_eq!(sub, AppSubcategory::Other);
    }

    #[test]
    fn is_sensitive_password_manager() {
        let registry = AppRegistry::new();
        assert!(registry.is_sensitive("1Password"));
        assert!(registry.is_sensitive("Bitwarden"));
        assert!(!registry.is_sensitive("Visual Studio Code"));
    }

    #[test]
    fn xcode_before_code_ordering() {
        let registry = AppRegistry::new();
        // "xcode" should match Xcode IDE, not VSCode
        let profile = registry.lookup("Xcode").unwrap();
        assert_eq!(profile.name, "Xcode");
    }

    #[test]
    fn vscode_matches_code_pattern() {
        let registry = AppRegistry::new();
        let profile = registry.lookup("Code").unwrap();
        assert_eq!(profile.subcategory, AppSubcategory::Ide);
    }

    #[test]
    fn terminal_subcategories() {
        let registry = AppRegistry::new();
        for app in &["iTerm2", "Warp", "Alacritty", "kitty"] {
            let (_, sub) = registry.classify(app);
            assert_eq!(sub, AppSubcategory::Terminal, "failed for {app}");
        }
    }

    #[test]
    fn chat_subcategories() {
        let registry = AppRegistry::new();
        for app in &["Slack", "Discord", "Teams"] {
            let (_, sub) = registry.classify(app);
            assert_eq!(sub, AppSubcategory::Chat, "failed for {app}");
        }
    }

    #[test]
    fn document_editor_subcategories() {
        let registry = AppRegistry::new();
        for app in &["Notion", "Obsidian", "Word"] {
            let (_, sub) = registry.classify(app);
            assert_eq!(sub, AppSubcategory::DocumentEditor, "failed for {app}");
        }
    }

    #[test]
    fn spreadsheet_subcategories() {
        let registry = AppRegistry::new();
        let (_, sub) = registry.classify("Excel");
        assert_eq!(sub, AppSubcategory::Spreadsheet);
    }

    #[test]
    fn built_in_profile_count() {
        let registry = AppRegistry::new();
        assert!(
            registry.len() >= 50,
            "expected 50+ profiles, got {}",
            registry.len()
        );
    }

    #[test]
    fn disabled_profile_skipped() {
        let mut registry = AppRegistry::new();
        // Simulate user override that disables iTerm2
        let json = r#"[{
            "name": "iTerm2",
            "name_patterns": ["iterm"],
            "category": "development",
            "subcategory": "terminal",
            "enabled": false
        }]"#;
        let tmp = std::env::temp_dir().join("test_app_profiles.json");
        std::fs::write(&tmp, json).unwrap();
        registry.load_user_overrides(&tmp).unwrap();
        std::fs::remove_file(&tmp).ok();

        assert!(registry.lookup("iTerm2").is_none());
    }

    #[test]
    fn user_override_adds_new_profile() {
        let mut registry = AppRegistry::new();
        let json = r#"[{
            "name": "MyCustomApp",
            "name_patterns": ["mycustomapp"],
            "category": "development",
            "subcategory": "ide"
        }]"#;
        let tmp = std::env::temp_dir().join("test_custom_profiles.json");
        std::fs::write(&tmp, json).unwrap();
        registry.load_user_overrides(&tmp).unwrap();
        std::fs::remove_file(&tmp).ok();

        let profile = registry.lookup("MyCustomApp").unwrap();
        assert_eq!(profile.subcategory, AppSubcategory::Ide);
    }
}
