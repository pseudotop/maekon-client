use oneshim_core::error::CoreError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SuggestionError {
    #[error(transparent)]
    Core(#[from] CoreError),
}

impl From<SuggestionError> for CoreError {
    fn from(err: SuggestionError) -> Self {
        match err {
            SuggestionError::Core(e) => e,
        }
    }
}
