use oneshim_core::models::tiered_memory::{ContainerType, ContentType};

mod helpers;
mod parsers;
#[cfg(test)]
mod tests;

/// Extracted content information from a window title bar.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedContent {
    pub content_label: String,
    pub content_type: ContentType,
    pub confidence: f32,
    /// Project or repository name extracted from IDE/terminal title bars.
    /// e.g., VS Code "main.rs - oneshim-client - Visual Studio Code" → "oneshim-client"
    pub project: Option<String>,
    /// Domain hint extracted from browser page titles via heuristic prefix matching.
    /// e.g., "Gmail - Inbox (3) - Google Chrome" → "gmail"
    pub domain_hint: Option<String>,
}

/// Known container applications (RDP / VM / VNC / Citrix).
const CONTAINER_APPS: &[(&str, ContainerType)] = &[
    ("microsoft remote desktop", ContainerType::Rdp),
    ("windows app", ContainerType::Rdp),
    ("freerdp", ContainerType::Rdp),
    ("parallels desktop", ContainerType::Vm),
    ("vmware fusion", ContainerType::Vm),
    ("virtualbox", ContainerType::Vm),
    ("utm", ContainerType::Vm),
    ("vnc viewer", ContainerType::Vnc),
    ("realvnc", ContainerType::Vnc),
    ("citrix workspace", ContainerType::Citrix),
];

/// Parse window title bars to extract content labels and types.
///
/// Each app family has its own title format convention. The parser tries
/// app-specific patterns first, falling back to a generic splitter.
pub struct TitleBarParser;

impl TitleBarParser {
    pub fn new() -> Self {
        Self
    }

    /// Parse window title to extract content label and type.
    pub fn parse(&self, app_name: &str, window_title: &str) -> Option<ParsedContent> {
        if window_title.is_empty() {
            return None;
        }

        let lower_app = app_name.to_lowercase();

        // Try app-specific patterns first
        if let Some(result) = self.parse_ide(&lower_app, window_title) {
            return Some(result);
        }
        if let Some(result) = self.parse_browser(&lower_app, window_title) {
            return Some(result);
        }
        if let Some(result) = self.parse_communication(&lower_app, window_title) {
            return Some(result);
        }
        if let Some(result) = self.parse_terminal(&lower_app, window_title) {
            return Some(result);
        }
        if let Some(result) = self.parse_document(&lower_app, window_title) {
            return Some(result);
        }
        if let Some(result) = self.parse_design(&lower_app, window_title) {
            return Some(result);
        }

        // Generic: use first segment before " - " as content
        self.parse_generic(window_title)
    }

    /// Check if app is a known container (RDP/VM/VNC/Citrix).
    pub fn is_container_app(&self, app_name: &str) -> Option<ContainerType> {
        let lower = app_name.to_lowercase();
        for (pattern, container_type) in CONTAINER_APPS {
            if lower.contains(pattern) {
                return Some(*container_type);
            }
        }
        None
    }
}

impl Default for TitleBarParser {
    fn default() -> Self {
        Self::new()
    }
}
