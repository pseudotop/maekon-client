use oneshim_core::models::tiered_memory::WorkType;

/// Contextual information about the user's current activity, used to enrich
/// short search queries with relevant keywords before embedding.
#[derive(Debug, Clone, Default)]
pub struct ActivityContext {
    /// Name of the currently active application (e.g. "VSCode", "Slack").
    pub app_name: String,
    /// Recent content labels from the segment buffer (e.g. "main.rs", "#general").
    pub content_labels: Vec<String>,
    /// Classified work type, when available.
    pub work_type: Option<WorkType>,
}

/// Pure-logic query expander that prepends activity-context keywords to short
/// user queries before they are embedded.
///
/// Design: only expand when the query is short (<3 words) AND context is
/// available. Detailed queries (3+ words) pass through unchanged so the user's
/// intent is preserved.
pub struct QueryExpander;

impl QueryExpander {
    /// Expand a search query using the given activity context.
    ///
    /// - When `context` is `None` or the query already has 3+ words, return
    ///   the original query unchanged.
    /// - When the query is short (<3 words) and context is available, append
    ///   the app name, work-type keyword, and up to 3 content labels.
    pub fn expand(query: &str, context: Option<&ActivityContext>) -> String {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return trimmed.to_string();
        }

        let word_count = trimmed.split_whitespace().count();

        // Detailed queries pass through as-is
        if word_count >= 3 {
            return trimmed.to_string();
        }

        let ctx = match context {
            Some(c) => c,
            None => return trimmed.to_string(),
        };

        // Build expansion tokens from context
        let mut tokens: Vec<&str> = Vec::new();

        if !ctx.app_name.is_empty() {
            tokens.push(&ctx.app_name);
        }

        if let Some(ref wt) = ctx.work_type {
            if let Some(kw) = work_type_keyword(wt) {
                tokens.push(kw);
            }
        }

        // Append up to 3 content labels (deduplicating against query)
        let query_lower = trimmed.to_lowercase();
        let mut label_count = 0;
        for label in &ctx.content_labels {
            if label_count >= 3 {
                break;
            }
            let label_trimmed = label.trim();
            if label_trimmed.is_empty() {
                continue;
            }
            // Skip if the label is already part of the query
            if query_lower.contains(&label_trimmed.to_lowercase()) {
                continue;
            }
            tokens.push(label_trimmed);
            label_count += 1;
        }

        if tokens.is_empty() {
            return trimmed.to_string();
        }

        // Deduplicate tokens against the original query words
        let query_words: Vec<String> = trimmed
            .split_whitespace()
            .map(|w| w.to_lowercase())
            .collect();

        let unique_tokens: Vec<&str> = tokens
            .into_iter()
            .filter(|t| !query_words.contains(&t.to_lowercase()))
            .collect();

        if unique_tokens.is_empty() {
            return trimmed.to_string();
        }

        format!("{} {}", trimmed, unique_tokens.join(" "))
    }
}

/// Map a WorkType variant to a descriptive keyword for query expansion.
fn work_type_keyword(wt: &WorkType) -> Option<&'static str> {
    match wt {
        WorkType::ActiveCoding => Some("coding"),
        WorkType::CodeReview => Some("review"),
        WorkType::Writing => Some("writing"),
        WorkType::Reading => Some("reading"),
        WorkType::Designing => Some("design"),
        WorkType::FormFilling => Some("form"),
        WorkType::Browsing => Some("browsing"),
        WorkType::PassiveMeeting => Some("meeting"),
        WorkType::ActiveMeeting => Some("meeting"),
        WorkType::Navigation => Some("navigation"),
        WorkType::TerminalCommands => Some("terminal"),
        WorkType::LogReading => Some("log"),
        WorkType::DocumentWriting => Some("writing"),
        WorkType::DocumentReading => Some("reading"),
        WorkType::ChatComposing => Some("chat"),
        WorkType::Unknown => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_query_expanded_with_context() {
        let ctx = ActivityContext {
            app_name: "VSCode".to_string(),
            content_labels: vec!["auth.rs".to_string(), "login module".to_string()],
            work_type: Some(WorkType::ActiveCoding),
        };

        let result = QueryExpander::expand("auth", Some(&ctx));
        assert!(result.starts_with("auth"));
        assert!(result.contains("VSCode"));
        assert!(result.contains("coding"));
        assert!(result.contains("auth.rs"));
    }

    #[test]
    fn long_query_returned_as_is() {
        let ctx = ActivityContext {
            app_name: "VSCode".to_string(),
            content_labels: vec!["main.rs".to_string()],
            work_type: Some(WorkType::ActiveCoding),
        };

        let result = QueryExpander::expand("what did I work on yesterday", Some(&ctx));
        assert_eq!(result, "what did I work on yesterday");
    }

    #[test]
    fn three_word_query_not_expanded() {
        let ctx = ActivityContext {
            app_name: "Chrome".to_string(),
            content_labels: vec!["docs".to_string()],
            work_type: Some(WorkType::Reading),
        };

        let result = QueryExpander::expand("meeting notes today", Some(&ctx));
        assert_eq!(result, "meeting notes today");
    }

    #[test]
    fn no_context_returns_original() {
        let result = QueryExpander::expand("auth", None);
        assert_eq!(result, "auth");
    }

    #[test]
    fn empty_query_returns_empty() {
        let ctx = ActivityContext {
            app_name: "VSCode".to_string(),
            content_labels: vec!["test".to_string()],
            work_type: None,
        };
        let result = QueryExpander::expand("", Some(&ctx));
        assert_eq!(result, "");
    }

    #[test]
    fn whitespace_only_query_returns_empty() {
        let result = QueryExpander::expand("   ", None);
        assert_eq!(result, "");
    }

    #[test]
    fn two_word_query_expanded() {
        let ctx = ActivityContext {
            app_name: "Slack".to_string(),
            content_labels: vec!["#general".to_string()],
            work_type: Some(WorkType::PassiveMeeting),
        };

        let result = QueryExpander::expand("meeting notes", Some(&ctx));
        assert!(result.starts_with("meeting notes"));
        assert!(result.contains("Slack"));
        assert!(result.contains("#general"));
    }

    #[test]
    fn duplicate_tokens_removed() {
        let ctx = ActivityContext {
            app_name: "VSCode".to_string(),
            content_labels: vec!["VSCode".to_string(), "test".to_string()],
            work_type: None,
        };

        let result = QueryExpander::expand("VSCode", Some(&ctx));
        // "VSCode" should not be duplicated; "test" should be appended
        let vscode_count = result.matches("VSCode").count();
        assert_eq!(vscode_count, 1);
        assert!(result.contains("test"));
    }

    #[test]
    fn content_labels_capped_at_three() {
        let ctx = ActivityContext {
            app_name: "".to_string(),
            content_labels: vec![
                "a.rs".to_string(),
                "b.rs".to_string(),
                "c.rs".to_string(),
                "d.rs".to_string(),
                "e.rs".to_string(),
            ],
            work_type: None,
        };

        let result = QueryExpander::expand("code", Some(&ctx));
        // At most 3 labels appended
        assert!(result.contains("a.rs"));
        assert!(result.contains("b.rs"));
        assert!(result.contains("c.rs"));
        assert!(!result.contains("d.rs"));
    }

    #[test]
    fn unknown_work_type_no_keyword() {
        let ctx = ActivityContext {
            app_name: "Terminal".to_string(),
            content_labels: vec![],
            work_type: Some(WorkType::Unknown),
        };

        let result = QueryExpander::expand("build", Some(&ctx));
        assert_eq!(result, "build Terminal");
    }

    #[test]
    fn empty_context_fields_returns_original() {
        let ctx = ActivityContext {
            app_name: "".to_string(),
            content_labels: vec![],
            work_type: None,
        };

        let result = QueryExpander::expand("test", Some(&ctx));
        assert_eq!(result, "test");
    }

    #[test]
    fn all_work_types_have_keyword_except_unknown() {
        let types_with_keywords = [
            WorkType::ActiveCoding,
            WorkType::CodeReview,
            WorkType::Writing,
            WorkType::Reading,
            WorkType::Designing,
            WorkType::FormFilling,
            WorkType::Browsing,
            WorkType::PassiveMeeting,
            WorkType::ActiveMeeting,
            WorkType::Navigation,
        ];
        for wt in &types_with_keywords {
            assert!(
                work_type_keyword(wt).is_some(),
                "{wt:?} should have a keyword"
            );
        }
        assert!(work_type_keyword(&WorkType::Unknown).is_none());
    }
}
