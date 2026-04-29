use oneshim_core::models::tiered_memory::ContentType;

use super::helpers::{extract_domain_hint, extract_project_from_path};
use super::{ParsedContent, TitleBarParser};

impl TitleBarParser {
    /// VSCode/Cursor: `{file} - {project} - Visual Studio Code`
    /// IntelliJ/WebStorm: `{project} – {file}` (em-dash)
    /// Xcode: `{project} — {file}` (em-dash \u{2014})
    pub(super) fn parse_ide(&self, lower_app: &str, title: &str) -> Option<ParsedContent> {
        let is_xcode = lower_app.contains("xcode");
        // "xcode" contains "code", so exclude it from VSCode detection
        let is_vscode = !is_xcode && (lower_app.contains("code") || lower_app.contains("cursor"));
        let is_jetbrains = lower_app.contains("intellij")
            || lower_app.contains("webstorm")
            || lower_app.contains("pycharm")
            || lower_app.contains("android studio")
            || lower_app.contains("goland")
            || lower_app.contains("rider")
            || lower_app.contains("rustrover")
            || lower_app.contains("clion");

        if is_vscode {
            // Format: "{file} - {project} - Visual Studio Code"
            // or:     "{file} - {project} - Cursor"
            let parts: Vec<&str> = title.split(" - ").collect();
            if parts.len() >= 2 {
                let file = parts[0].trim();
                // Extract project from middle segment (index 1) when 3+ parts
                let project = if parts.len() >= 3 {
                    let proj = parts[1].trim();
                    if !proj.is_empty() {
                        Some(proj.to_string())
                    } else {
                        None
                    }
                } else {
                    None
                };
                if !file.is_empty() {
                    return Some(ParsedContent {
                        content_label: file.to_string(),
                        content_type: ContentType::File,
                        confidence: 0.95,
                        project,
                        domain_hint: None,
                    });
                }
            }
        }

        if is_jetbrains {
            // Format: "{project} – {file}" (em-dash \u{2013})
            if let Some(idx) = title.find('\u{2013}') {
                let project_part = title[..idx].trim();
                let file = title[idx + '\u{2013}'.len_utf8()..].trim();
                if !file.is_empty() {
                    let project = if !project_part.is_empty() {
                        Some(project_part.to_string())
                    } else {
                        None
                    };
                    return Some(ParsedContent {
                        content_label: file.to_string(),
                        content_type: ContentType::File,
                        confidence: 0.90,
                        project,
                        domain_hint: None,
                    });
                }
            }
            // Fallback: some JetBrains IDEs use " - " instead
            // Format: "{project} - {file} - {IDE}" or "{project} - {file}"
            let parts: Vec<&str> = title.split(" - ").collect();
            if parts.len() >= 3 {
                // {project} - {file} - {IDE}
                let project_part = parts[0].trim();
                let file = parts[1].trim();
                if !file.is_empty() {
                    let project = if !project_part.is_empty() {
                        Some(project_part.to_string())
                    } else {
                        None
                    };
                    return Some(ParsedContent {
                        content_label: file.to_string(),
                        content_type: ContentType::File,
                        confidence: 0.70,
                        project,
                        domain_hint: None,
                    });
                }
            } else if parts.len() == 2 {
                // {project} - {file} or {file} - {IDE}
                let file = parts[0].trim();
                if !file.is_empty() {
                    return Some(ParsedContent {
                        content_label: file.to_string(),
                        content_type: ContentType::File,
                        confidence: 0.60,
                        project: None,
                        domain_hint: None,
                    });
                }
            }
        }

        if is_xcode {
            // Format: "{project} — {file}" (em-dash \u{2014})
            if let Some(idx) = title.find('\u{2014}') {
                let project_part = title[..idx].trim();
                let file = title[idx + '\u{2014}'.len_utf8()..].trim();
                if !file.is_empty() {
                    let project = if !project_part.is_empty() {
                        Some(project_part.to_string())
                    } else {
                        None
                    };
                    return Some(ParsedContent {
                        content_label: file.to_string(),
                        content_type: ContentType::File,
                        confidence: 0.90,
                        project,
                        domain_hint: None,
                    });
                }
            }
            // Fallback: Xcode sometimes uses " \u{2013} " (en-dash)
            if let Some(idx) = title.find('\u{2013}') {
                let project_part = title[..idx].trim();
                let file = title[idx + '\u{2013}'.len_utf8()..].trim();
                if !file.is_empty() {
                    let project = if !project_part.is_empty() {
                        Some(project_part.to_string())
                    } else {
                        None
                    };
                    return Some(ParsedContent {
                        content_label: file.to_string(),
                        content_type: ContentType::File,
                        confidence: 0.85,
                        project,
                        domain_hint: None,
                    });
                }
            }
            // Fallback: " - " separator
            let parts: Vec<&str> = title.split(" - ").collect();
            if parts.len() >= 2 {
                let project_part = parts[0].trim();
                let file = parts[1].trim();
                if !file.is_empty() {
                    let project = if !project_part.is_empty() {
                        Some(project_part.to_string())
                    } else {
                        None
                    };
                    return Some(ParsedContent {
                        content_label: file.to_string(),
                        content_type: ContentType::File,
                        confidence: 0.75,
                        project,
                        domain_hint: None,
                    });
                }
            }
        }

        None
    }

    /// Chrome/Arc/Edge/Firefox/Safari/Brave: `{page_title} - {browser}`
    pub(super) fn parse_browser(&self, lower_app: &str, title: &str) -> Option<ParsedContent> {
        let is_browser = lower_app.contains("chrome")
            || lower_app.contains("arc")
            || lower_app.contains("edge")
            || lower_app.contains("firefox")
            || lower_app.contains("safari")
            || lower_app.contains("brave")
            || lower_app.contains("opera");

        if !is_browser {
            return None;
        }

        // Format: "{page_title} - {browser_name}"
        // Take everything before the last " - " separator
        if let Some(idx) = title.rfind(" - ") {
            let page = title[..idx].trim();
            if !page.is_empty() {
                let domain_hint = extract_domain_hint(page);
                return Some(ParsedContent {
                    content_label: page.to_string(),
                    content_type: ContentType::WebPage,
                    confidence: 0.90,
                    project: None,
                    domain_hint,
                });
            }
        }

        // No separator — use entire title
        let domain_hint = extract_domain_hint(title);
        Some(ParsedContent {
            content_label: title.to_string(),
            content_type: ContentType::WebPage,
            confidence: 0.70,
            project: None,
            domain_hint,
        })
    }

    /// Slack: `{workspace} - {channel}` or `{channel} | {workspace}`
    pub(super) fn parse_communication(
        &self,
        lower_app: &str,
        title: &str,
    ) -> Option<ParsedContent> {
        let is_slack = lower_app.contains("slack");
        let is_teams = lower_app.contains("teams");
        let is_discord = lower_app.contains("discord");

        if is_slack {
            // Slack: "{channel} | {workspace}" or "{workspace} - {channel}"
            if let Some(idx) = title.find(" | ") {
                let channel = title[..idx].trim();
                if !channel.is_empty() {
                    return Some(ParsedContent {
                        content_label: channel.to_string(),
                        content_type: ContentType::Channel,
                        confidence: 0.95,
                        project: None,
                        domain_hint: None,
                    });
                }
            }
            if let Some(idx) = title.find(" - ") {
                let channel = title[idx + 3..].trim();
                if !channel.is_empty() {
                    return Some(ParsedContent {
                        content_label: channel.to_string(),
                        content_type: ContentType::Channel,
                        confidence: 0.90,
                        project: None,
                        domain_hint: None,
                    });
                }
            }
        }

        if is_teams || is_discord {
            // Teams/Discord: "{channel/chat} - {app}" or "{channel} | {server}"
            if let Some(idx) = title.find(" | ") {
                let channel = title[..idx].trim();
                if !channel.is_empty() {
                    return Some(ParsedContent {
                        content_label: channel.to_string(),
                        content_type: ContentType::Channel,
                        confidence: 0.85,
                        project: None,
                        domain_hint: None,
                    });
                }
            }
            if let Some(idx) = title.find(" - ") {
                let channel = title[..idx].trim();
                if !channel.is_empty() {
                    return Some(ParsedContent {
                        content_label: channel.to_string(),
                        content_type: ContentType::Channel,
                        confidence: 0.80,
                        project: None,
                        domain_hint: None,
                    });
                }
            }
        }

        None
    }

    /// Terminal/iTerm/Warp/Alacritty: `{user}@{host}: {path}`
    pub(super) fn parse_terminal(&self, lower_app: &str, title: &str) -> Option<ParsedContent> {
        let is_terminal = lower_app.contains("terminal")
            || lower_app.contains("iterm")
            || lower_app.contains("warp")
            || lower_app.contains("alacritty")
            || lower_app.contains("kitty")
            || lower_app.contains("hyper")
            || lower_app.contains("konsole");

        if !is_terminal {
            return None;
        }

        // Format: "{user}@{host}: {path}" or just "{path}"
        if let Some(colon_idx) = title.find(": ") {
            let path = title[colon_idx + 2..].trim();
            if !path.is_empty() {
                let project = extract_project_from_path(path);
                return Some(ParsedContent {
                    content_label: path.to_string(),
                    content_type: ContentType::File,
                    confidence: 0.85,
                    project,
                    domain_hint: None,
                });
            }
        }

        // Fallback: use entire title
        if !title.is_empty() {
            let project = extract_project_from_path(title);
            return Some(ParsedContent {
                content_label: title.to_string(),
                content_type: ContentType::File,
                confidence: 0.60,
                project,
                domain_hint: None,
            });
        }

        None
    }

    /// Excel/Sheets/Word/Notion: various document formats
    pub(super) fn parse_document(&self, lower_app: &str, title: &str) -> Option<ParsedContent> {
        let is_office = lower_app.contains("excel")
            || lower_app.contains("word")
            || lower_app.contains("powerpoint")
            || lower_app.contains("numbers")
            || lower_app.contains("pages")
            || lower_app.contains("keynote")
            || lower_app.contains("sheets")
            || lower_app.contains("docs");

        let is_notion = lower_app.contains("notion");

        if is_office {
            // Format: "{filename} - {app_name}"
            if let Some(idx) = title.find(" - ") {
                let file = title[..idx].trim();
                if !file.is_empty() {
                    return Some(ParsedContent {
                        content_label: file.to_string(),
                        content_type: ContentType::File,
                        confidence: 0.90,
                        project: None,
                        domain_hint: None,
                    });
                }
            }
        }

        if is_notion {
            // Notion: "{page} / {workspace}" or "{page} - Notion"
            if let Some(idx) = title.find(" / ") {
                let page = title[..idx].trim();
                if !page.is_empty() {
                    return Some(ParsedContent {
                        content_label: page.to_string(),
                        content_type: ContentType::WebPage,
                        confidence: 0.90,
                        project: None,
                        domain_hint: None,
                    });
                }
            }
            if let Some(idx) = title.find(" - ") {
                let page = title[..idx].trim();
                if !page.is_empty() {
                    return Some(ParsedContent {
                        content_label: page.to_string(),
                        content_type: ContentType::WebPage,
                        confidence: 0.85,
                        project: None,
                        domain_hint: None,
                    });
                }
            }
        }

        None
    }

    /// Figma: `{file} \u{2013} Figma`
    pub(super) fn parse_design(&self, lower_app: &str, title: &str) -> Option<ParsedContent> {
        let is_figma = lower_app.contains("figma");
        let is_sketch = lower_app.contains("sketch");

        if is_figma {
            // Figma: "{file} \u{2013} Figma" (em-dash) or "{file} - Figma"
            if let Some(idx) = title.find('\u{2013}') {
                let file = title[..idx].trim();
                if !file.is_empty() {
                    return Some(ParsedContent {
                        content_label: file.to_string(),
                        content_type: ContentType::File,
                        confidence: 0.90,
                        project: None,
                        domain_hint: None,
                    });
                }
            }
            if let Some(idx) = title.find(" - ") {
                let file = title[..idx].trim();
                if !file.is_empty() {
                    return Some(ParsedContent {
                        content_label: file.to_string(),
                        content_type: ContentType::File,
                        confidence: 0.85,
                        project: None,
                        domain_hint: None,
                    });
                }
            }
        }

        if is_sketch {
            if let Some(idx) = title.find(" - ") {
                let file = title[..idx].trim();
                if !file.is_empty() {
                    return Some(ParsedContent {
                        content_label: file.to_string(),
                        content_type: ContentType::File,
                        confidence: 0.85,
                        project: None,
                        domain_hint: None,
                    });
                }
            }
        }

        None
    }

    /// Generic fallback: use first segment before " - " as content.
    pub(super) fn parse_generic(&self, title: &str) -> Option<ParsedContent> {
        if title.is_empty() {
            return None;
        }

        let label = if let Some(idx) = title.find(" - ") {
            title[..idx].trim()
        } else {
            title.trim()
        };

        if label.is_empty() {
            return None;
        }

        Some(ParsedContent {
            content_label: label.to_string(),
            content_type: ContentType::Unknown,
            confidence: 0.50,
            project: None,
            domain_hint: None,
        })
    }
}
