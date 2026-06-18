use maekon_core::error::CoreError;
use thiserror::Error;

/// Error type specific to the maekon-analysis crate (ADR-001 §1)
#[derive(Debug, Error)]
pub enum AnalysisError {
    /// Transparently propagates a maekon-core error
    #[error(transparent)]
    Core(#[from] CoreError),

    /// Vector index (HNSW, etc.) related error
    #[error("vector index error: {0}")]
    VectorIndex(String),

    /// Clustering algorithm failure (GMM, HDBSCAN, etc.)
    #[error("clustering failed: {0}")]
    Clustering(String),
}

impl From<AnalysisError> for CoreError {
    fn from(err: AnalysisError) -> Self {
        match err {
            AnalysisError::Core(e) => e,
            AnalysisError::VectorIndex(msg) => CoreError::Analysis {
                code: maekon_core::error_codes::ProviderCode::AnalysisFailed,
                message: msg,
            },
            AnalysisError::Clustering(msg) => CoreError::Analysis {
                code: maekon_core::error_codes::ProviderCode::AnalysisFailed,
                message: msg,
            },
        }
    }
}
