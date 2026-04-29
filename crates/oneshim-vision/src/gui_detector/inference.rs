//! GUI element type inference and app-specific overrides.

use oneshim_core::models::frame::BoundingBox;
use oneshim_core::models::frame::OcrRegion;
use oneshim_core::models::gui_interaction::{GuiElement, GuiElementType};

use super::{GuiElementDetector, TAB_LABEL_MAX_LEN};

impl GuiElementDetector {
    /// Infer element type using first-match heuristic rules (backward-compatible).
    pub fn infer_element_type(&self, text: &str, bbox: &BoundingBox) -> GuiElementType {
        self.infer_element_type_scored(text, bbox).0
    }

    /// Multi-signal scored inference: evaluates all candidate types and returns
    /// the highest-scoring type along with a classification confidence (0.0--1.0).
    ///
    /// For ML-enhanced classification, use `build_gui_element_with_frame()` which
    /// consults the ML classifier when confidence > 0.7, falling back to this
    /// heuristic scoring otherwise.
    pub fn infer_element_type_scored(
        &self,
        text: &str,
        bbox: &BoundingBox,
    ) -> (GuiElementType, f32) {
        let lower = text.to_lowercase();
        let (screen_w, screen_h) = self.screen_resolution;
        let word_count = text.split_whitespace().count();

        let title_bar_max_y = (screen_h as f64 * 0.04) as u32;
        let tab_bar_max_y = (screen_h as f64 * 0.09) as u32;
        let status_bar_min_y = (screen_h as f64 * 0.95) as u32;
        let toolbar_max_y = title_bar_max_y * 2;
        let scrollbar_narrow = 20u32;

        // Collect (type, score) candidates from independent signals.
        let mut scores: Vec<(GuiElementType, f32)> = Vec::with_capacity(12);

        // --- Position signals ---
        if bbox.y < title_bar_max_y {
            scores.push((GuiElementType::TitleBar, 0.9));
        }
        if bbox.y < toolbar_max_y
            && bbox.width < 50
            && bbox.height < 50
            && text.chars().count() <= 2
        {
            scores.push((GuiElementType::ToolbarIcon, 0.8));
        }
        if bbox.y < tab_bar_max_y && text.len() < TAB_LABEL_MAX_LEN {
            scores.push((GuiElementType::TabLabel, 0.7));
        }
        if bbox.y >= status_bar_min_y {
            scores.push((GuiElementType::StatusBar, 0.9));
        }
        let is_narrow_h = bbox.width < scrollbar_narrow;
        let is_narrow_v = bbox.height < scrollbar_narrow;
        let is_narrow = is_narrow_h || is_narrow_v;
        let edge_margin: u32 = 20;
        let at_horizontal_edge = bbox.x <= edge_margin
            || bbox.x.saturating_add(bbox.width) >= screen_w.saturating_sub(edge_margin);
        let at_vertical_edge = bbox.y <= edge_margin
            || bbox.y.saturating_add(bbox.height) >= screen_h.saturating_sub(edge_margin);

        // Only classify as scrollbar when the element is narrow AND sits at a screen edge.
        // A narrow element in the centre of the screen is not a scrollbar.
        if is_narrow && (at_horizontal_edge || at_vertical_edge) {
            scores.push((GuiElementType::ScrollBar, 0.85));
        }

        // --- Text signals ---
        if lower.contains("http") || lower.contains("://") {
            scores.push((GuiElementType::Link, 0.95));
        }
        if Self::looks_like_menu_item(text) {
            scores.push((GuiElementType::MenuItem, 0.9));
        }
        if text.len() < 15
            && (lower.contains("save")
                || lower.contains("cancel")
                || lower.contains("ok")
                || lower.contains("submit")
                || lower.contains("close")
                || lower.contains("apply")
                || lower.contains("delete"))
        {
            scores.push((GuiElementType::Button, 0.8));
        }
        if lower.starts_with('\u{25B8}')
            || lower.starts_with('\u{25BE}')
            || lower.starts_with('\u{25BA}')
            || lower.starts_with('\u{25BC}')
        {
            scores.push((GuiElementType::TreeItem, 0.75));
        }

        // --- Fallback signals ---
        if word_count >= 3 {
            scores.push((GuiElementType::TextRegion, 0.4));
        }
        // Unknown always present as baseline
        scores.push((GuiElementType::Unknown, 0.1));

        // Pick the highest score. On tie, first inserted wins (position-based priority).
        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let winner_score = scores[0].1;
        let runner_up_score = scores.get(1).map_or(0.0, |s| s.1);
        // Confidence: how dominant the winner is over the runner-up.
        // Range: 0.5 (barely won) to 1.0 (no competition).
        let confidence = if winner_score + runner_up_score > 0.0 {
            (winner_score / (winner_score + runner_up_score)).max(0.5)
        } else {
            0.5
        };

        (scores[0].0.clone(), confidence)
    }

    /// Check if text looks like a menu item with a keyboard shortcut.
    pub(super) fn looks_like_menu_item(text: &str) -> bool {
        let shortcut_prefixes = ["Ctrl+", "Cmd+", "Alt+", "Shift+", "⌘", "⇧"];
        shortcut_prefixes.iter().any(|prefix| text.contains(prefix))
    }

    /// Like `correlate_click`, but applies app-specific element type overrides.
    pub fn correlate_click_with_app(
        &self,
        click_x: u32,
        click_y: u32,
        regions: &[OcrRegion],
        app_name: &str,
    ) -> Option<GuiElement> {
        self.correlate_click(click_x, click_y, regions)
            .map(|mut e| {
                if let Some(override_type) = self.app_override(app_name, &e.text, &e.bbox) {
                    e.element_type = override_type;
                }
                e
            })
    }

    /// App-specific element type overrides for well-known applications.
    fn app_override(
        &self,
        app_name: &str,
        text: &str,
        bbox: &BoundingBox,
    ) -> Option<GuiElementType> {
        let lower = app_name.to_lowercase();
        let (screen_w, screen_h) = self.screen_resolution;

        // IDE apps
        if [
            "code",
            "visual studio",
            "intellij",
            "pycharm",
            "webstorm",
            "xcode",
            "cursor",
            "zed",
        ]
        .iter()
        .any(|k| lower.contains(k))
        {
            let sidebar_max_x = screen_w / 5;
            if bbox.x < sidebar_max_x && bbox.y > (screen_h as f64 * 0.09) as u32 {
                return Some(GuiElementType::TreeItem);
            }
        }

        // Browser apps
        if ["chrome", "safari", "firefox", "edge", "arc", "brave"]
            .iter()
            .any(|k| lower.contains(k))
        {
            let url_bar_max_y = (screen_h as f64 * 0.08) as u32;
            if bbox.y < url_bar_max_y && (text.contains('.') || text.contains('/')) {
                return Some(GuiElementType::Link);
            }
        }

        // Chat/Communication apps
        if ["slack", "teams", "discord", "telegram", "messages"]
            .iter()
            .any(|k| lower.contains(k))
        {
            let sidebar_max_x = screen_w / 4;
            if bbox.x < sidebar_max_x && bbox.y > (screen_h as f64 * 0.09) as u32 {
                return Some(GuiElementType::TreeItem);
            }
        }

        None
    }
}
