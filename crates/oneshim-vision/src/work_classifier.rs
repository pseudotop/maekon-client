//! Rule-based work type classifier.
//!
//! Implements [`WorkTypeClassifier`] using heuristic rules based on app name,
//! window title, and focused accessibility role.

use oneshim_core::models::tiered_memory::WorkType;
use oneshim_core::ports::work_classifier::WorkTypeClassifier;

/// Stateless rule-based implementation of [`WorkTypeClassifier`].
///
/// Classifies the current user activity into a [`WorkType`] based on:
/// - App name (case-insensitive matching against known app categories)
/// - Window title keywords
/// - Focused accessibility role (AXTextArea / edit → writing mode)
pub struct RuleBasedClassifier;

impl RuleBasedClassifier {
    pub fn new() -> Self {
        Self
    }
}

impl Default for RuleBasedClassifier {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkTypeClassifier for RuleBasedClassifier {
    fn classify(
        &self,
        app_name: &str,
        window_title: &str,
        focused_role: Option<&str>,
        _ocr_text_sample: Option<&str>,
    ) -> WorkType {
        let app_lower = app_name.to_lowercase();
        let title_lower = window_title.to_lowercase();

        // Helper: check if focused role indicates active text editing.
        let is_editing = focused_role
            .map(|r| {
                let r = r.to_lowercase();
                r == "axtextarea" || r == "edit"
            })
            .unwrap_or(false);

        // --- IDE apps ---
        if is_ide_app(&app_lower) {
            return if is_editing {
                WorkType::ActiveCoding
            } else {
                WorkType::CodeReview
            };
        }

        // --- Terminal apps ---
        if is_terminal_app(&app_lower) {
            return WorkType::TerminalCommands;
        }

        // --- Chat apps ---
        if is_chat_app(&app_lower) {
            return WorkType::ChatComposing;
        }

        // --- Browser apps ---
        if is_browser_app(&app_lower) {
            if title_lower.contains("github")
                || title_lower.contains("gitlab")
                || title_lower.contains("diff")
                || title_lower.contains("pull request")
            {
                return WorkType::CodeReview;
            }
            if title_lower.contains("docs")
                || title_lower.contains("wiki")
                || title_lower.contains("documentation")
                || title_lower.contains("readme")
            {
                return WorkType::DocumentReading;
            }
            return WorkType::Unknown;
        }

        // --- Document apps ---
        if is_document_app(&app_lower) {
            return if is_editing {
                WorkType::DocumentWriting
            } else {
                WorkType::DocumentReading
            };
        }

        // --- Log viewers ---
        if title_lower.ends_with(".log")
            || title_lower.contains(".log ")
            || title_lower.contains("console")
            || app_lower == "console"
        {
            return WorkType::LogReading;
        }

        WorkType::Unknown
    }
}

// ---------------------------------------------------------------------------
// App category helpers
// ---------------------------------------------------------------------------

fn is_ide_app(app_lower: &str) -> bool {
    matches!(
        app_lower,
        "code"
            | "visual studio code"
            | "intellij"
            | "webstorm"
            | "xcode"
            | "android studio"
            | "neovim"
            | "vim"
            | "emacs"
            | "sublime text"
            | "cursor"
            | "zed"
    )
}

fn is_terminal_app(app_lower: &str) -> bool {
    matches!(
        app_lower,
        "terminal"
            | "iterm2"
            | "warp"
            | "alacritty"
            | "kitty"
            | "windows terminal"
            | "powershell"
            | "cmd"
    )
}

fn is_chat_app(app_lower: &str) -> bool {
    matches!(
        app_lower,
        "slack" | "discord" | "teams" | "telegram" | "messages" | "zoom"
    )
}

fn is_browser_app(app_lower: &str) -> bool {
    matches!(
        app_lower,
        "safari" | "chrome" | "firefox" | "arc" | "edge" | "brave"
    )
}

fn is_document_app(app_lower: &str) -> bool {
    matches!(
        app_lower,
        "pages" | "word" | "google docs" | "notion" | "obsidian" | "typora"
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn clf() -> RuleBasedClassifier {
        RuleBasedClassifier::new()
    }

    #[test]
    fn ide_active_coding() {
        // Focused role "AXTextArea" → ActiveCoding
        let result = clf().classify("code", "main.rs — my-project", Some("AXTextArea"), None);
        assert_eq!(result, WorkType::ActiveCoding);
    }

    #[test]
    fn ide_code_review() {
        // No focused editing role → CodeReview
        let result = clf().classify("code", "main.rs — my-project", None, None);
        assert_eq!(result, WorkType::CodeReview);
    }

    #[test]
    fn ide_edit_role_active_coding() {
        // "edit" role also counts as active coding
        let result = clf().classify("xcode", "MyApp.swift", Some("edit"), None);
        assert_eq!(result, WorkType::ActiveCoding);
    }

    #[test]
    fn terminal_app() {
        let result = clf().classify("iterm2", "bash — 80×24", None, None);
        assert_eq!(result, WorkType::TerminalCommands);
    }

    #[test]
    fn chat_app_slack() {
        let result = clf().classify("slack", "#engineering", None, None);
        assert_eq!(result, WorkType::ChatComposing);
    }

    #[test]
    fn browser_github_pr() {
        let result = clf().classify(
            "chrome",
            "Fix login bug · Pull Request #42 · pseudotop/oneshim",
            None,
            None,
        );
        assert_eq!(result, WorkType::CodeReview);
    }

    #[test]
    fn browser_docs_page() {
        let result = clf().classify("safari", "React Docs — Getting Started", None, None);
        assert_eq!(result, WorkType::DocumentReading);
    }

    #[test]
    fn document_app_writing() {
        // Notion with AXTextArea → DocumentWriting
        let result = clf().classify("notion", "Weekly Report", Some("AXTextArea"), None);
        assert_eq!(result, WorkType::DocumentWriting);
    }

    #[test]
    fn log_file_title() {
        // Window title ending in ".log" — use a generic viewer app, not an IDE
        let result = clf().classify("logsurfer", "server.log", None, None);
        assert_eq!(result, WorkType::LogReading);
    }

    #[test]
    fn unknown_app() {
        let result = clf().classify("figma", "Design System v2", None, None);
        assert_eq!(result, WorkType::Unknown);
    }

    #[test]
    fn case_insensitive_app_name() {
        // "Terminal" should match "terminal" (lowercase)
        let result = clf().classify("Terminal", "zsh — 120×40", None, None);
        assert_eq!(result, WorkType::TerminalCommands);
    }
}
