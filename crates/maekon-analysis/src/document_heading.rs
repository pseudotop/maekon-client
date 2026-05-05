//! Document title and heading extraction from accessibility text.
//!
//! When AppSubcategory is DocumentEditor and extracted_text is available,
//! attempts to extract a document title or current heading for richer
//! content labels in the analysis pipeline.

/// Result of document heading extraction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentHeadingInfo {
    /// Extracted heading text (trimmed, max 100 chars).
    pub heading: String,
    /// Heading level if detectable (1 = title, 2 = H2, etc.). 0 = unknown.
    pub level: u8,
}

/// Maximum heading length to capture.
const MAX_HEADING_LEN: usize = 100;

/// Extract a document heading from accessibility-extracted text.
///
/// Detection heuristics (ordered by priority):
/// 1. Markdown headings: lines starting with `#`, `##`, etc.
/// 2. First non-empty line if it is short enough to be a title (< 80 chars)
///    and the second line is empty or a separator
///
/// Returns `None` if no heading pattern is detected.
pub fn extract_document_heading(text: &str) -> Option<DocumentHeadingInfo> {
    let lines: Vec<&str> = text.lines().collect();

    // Strategy 1: Markdown headings
    for line in &lines {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            let level = trimmed.chars().take_while(|&c| c == '#').count() as u8;
            let heading = trimmed[level as usize..].trim().trim_matches('#').trim();
            if !heading.is_empty() {
                return Some(DocumentHeadingInfo {
                    heading: truncate(heading, MAX_HEADING_LEN),
                    level,
                });
            }
        }
    }

    // Strategy 2: First short line followed by empty line or separator
    if let Some(first) = lines.first().map(|l| l.trim()) {
        if !first.is_empty() && first.len() < 80 {
            let second = lines.get(1).map_or("", |l| l.trim());
            let is_title_like =
                second.is_empty() || second.chars().all(|c| c == '=' || c == '-' || c == '_');
            if is_title_like {
                return Some(DocumentHeadingInfo {
                    heading: truncate(first, MAX_HEADING_LEN),
                    level: 0,
                });
            }
        }
    }

    None
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() > max {
        // UTF-8 safe truncation: find the last char boundary before max
        let end = s
            .char_indices()
            .take_while(|(i, _)| *i < max)
            .last()
            .map_or(0, |(i, c)| i + c.len_utf8());
        format!("{}...", &s[..end])
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_h1() {
        let text = "# Project Overview\n\nSome content here.";
        let result = extract_document_heading(text).unwrap();
        assert_eq!(result.heading, "Project Overview");
        assert_eq!(result.level, 1);
    }

    #[test]
    fn markdown_h2() {
        let text = "## Architecture\nThe system uses...";
        let result = extract_document_heading(text).unwrap();
        assert_eq!(result.heading, "Architecture");
        assert_eq!(result.level, 2);
    }

    #[test]
    fn title_with_separator() {
        let text = "Meeting Notes\n=============\nAttendees: ...";
        let result = extract_document_heading(text).unwrap();
        assert_eq!(result.heading, "Meeting Notes");
        assert_eq!(result.level, 0);
    }

    #[test]
    fn title_with_empty_second_line() {
        let text = "Budget Report Q4\n\nTotal spending...";
        let result = extract_document_heading(text).unwrap();
        assert_eq!(result.heading, "Budget Report Q4");
        assert_eq!(result.level, 0);
    }

    #[test]
    fn no_heading_in_prose() {
        let text = "This is a long paragraph that continues for a while and does not look like a heading at all because it is too long to be one.";
        assert!(extract_document_heading(text).is_none());
    }

    #[test]
    fn empty_text() {
        assert!(extract_document_heading("").is_none());
    }

    #[test]
    fn heading_with_trailing_hashes() {
        let text = "## Design Spec ##\n\nContent...";
        let result = extract_document_heading(text).unwrap();
        assert_eq!(result.heading, "Design Spec");
        assert_eq!(result.level, 2);
    }
}
