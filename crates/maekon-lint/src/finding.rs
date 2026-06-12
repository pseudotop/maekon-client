use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    NonEnglish,
    I18n,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone)]
pub(crate) struct Finding {
    pub(crate) severity: Severity,
    pub(crate) category: &'static str,
    pub(crate) path: PathBuf,
    pub(crate) line: usize,
    pub(crate) column: usize,
    pub(crate) message: String,
    pub(crate) snippet: String,
}

impl Finding {
    pub(crate) fn new(
        severity: Severity,
        category: &'static str,
        path: PathBuf,
        line: usize,
        column: usize,
        message: String,
        snippet: String,
    ) -> Self {
        Self {
            severity,
            category,
            path,
            line,
            column,
            message,
            snippet,
        }
    }
}
