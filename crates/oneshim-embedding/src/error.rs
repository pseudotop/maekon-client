use oneshim_core::error::CoreError;
use oneshim_core::error_codes::InternalCode;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EmbeddingError {
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<EmbeddingError> for CoreError {
    fn from(err: EmbeddingError) -> Self {
        match err {
            EmbeddingError::Core(e) => e,
            EmbeddingError::Internal(msg) => CoreError::Internal {
                code: InternalCode::Generic,
                message: msg,
            },
        }
    }
}
