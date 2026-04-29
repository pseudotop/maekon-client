use serde::{Deserialize, Serialize};

use super::tiered_memory::ContentType;
use super::work_session::AppCategory;

/// Fine-grained application subcategory within an AppCategory.
///
/// AppCategory remains unchanged (no breaking change). AppSubcategory
/// provides the additional granularity needed for text-heavy app
/// intelligence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AppSubcategory {
    // Development
    Terminal,
    Ide,
    TuiEditor,
    ApiTool,
    GitGui,
    // Documentation
    DocumentEditor,
    Spreadsheet,
    Presentation,
    // Communication
    Chat,
    Email,
    VideoCall,
    // Browser
    Browser,
    // Design
    Design,
    // Media
    Media,
    // System
    System,
    #[default]
    Other,
}

/// Profile describing an application's characteristics for text intelligence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppProfile {
    pub name: String,
    pub name_patterns: Vec<String>,
    pub category: AppCategory,
    pub subcategory: AppSubcategory,
    #[serde(default)]
    pub title_hints: Vec<TitleParseHint>,
    #[serde(default)]
    pub accessibility_strategy: AccessibilityStrategy,
    #[serde(default)]
    pub sensitive: bool,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TitleParseHint {
    pub separator: String,
    pub content_position: String,
    pub content_type: ContentType,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessibilityStrategy {
    #[default]
    None,
    Native,
    Osascript,
}

/// Key category for input pattern analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyCategory {
    Enter,
    Tab,
    Arrow,
    Backspace,
    Special,
    Regular,
}

/// Keystroke profile computed from per-category counters.
///
/// Each ratio is `category_count / total_keystrokes`. When total_keystrokes
/// is 0, all ratios are 0.0.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct KeystrokeProfile {
    pub enter_ratio: f32,
    pub tab_ratio: f32,
    pub arrow_ratio: f32,
    pub backspace_ratio: f32,
    pub special_ratio: f32,
    pub total_keystrokes: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subcategory_serde_roundtrip() {
        let val = AppSubcategory::Terminal;
        let json = serde_json::to_string(&val).unwrap();
        assert_eq!(json, r#""terminal""#);
        let back: AppSubcategory = serde_json::from_str(&json).unwrap();
        assert_eq!(back, AppSubcategory::Terminal);
    }

    #[test]
    fn subcategory_default_is_other() {
        assert_eq!(AppSubcategory::default(), AppSubcategory::Other);
    }

    #[test]
    fn accessibility_strategy_default_is_none() {
        assert_eq!(
            AccessibilityStrategy::default(),
            AccessibilityStrategy::None
        );
    }

    #[test]
    fn keystroke_profile_default_is_zero() {
        let p = KeystrokeProfile::default();
        assert_eq!(p.total_keystrokes, 0);
        assert!((p.enter_ratio).abs() < f32::EPSILON);
    }

    #[test]
    fn app_profile_serde_defaults() {
        let json = r#"{
            "name": "Test",
            "name_patterns": ["test"],
            "category": "development",
            "subcategory": "terminal"
        }"#;
        let profile: AppProfile = serde_json::from_str(json).unwrap();
        assert!(profile.enabled);
        assert!(!profile.sensitive);
        assert!(profile.title_hints.is_empty());
        assert_eq!(profile.accessibility_strategy, AccessibilityStrategy::None);
    }
}
