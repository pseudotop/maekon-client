use maekon_core::models::suggestion::Suggestion;

/// Metadata-only result of one local analysis attempt.
///
/// Keeping skipped work distinct from a valid provider response prevents the
/// scheduler and product surface from presenting every empty vector as "the
/// model found no candidate" (#11737). The legacy `Vec<Suggestion>` methods
/// remain as compatibility wrappers for callers that only need candidates.
#[derive(Debug, Clone)]
pub enum AnalysisRunOutcome {
    Generated(Vec<Suggestion>),
    NoCandidate,
    Throttled,
    NoInput,
    Unchanged,
}

impl AnalysisRunOutcome {
    pub(crate) fn into_suggestions(self) -> Vec<Suggestion> {
        match self {
            Self::Generated(suggestions) => suggestions,
            Self::NoCandidate | Self::Throttled | Self::NoInput | Self::Unchanged => Vec::new(),
        }
    }
}
