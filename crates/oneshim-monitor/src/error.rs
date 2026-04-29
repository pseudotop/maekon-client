use oneshim_core::error::CoreError;
use oneshim_core::error_codes::InternalCode;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MonitorError {
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<MonitorError> for CoreError {
    fn from(err: MonitorError) -> Self {
        match err {
            MonitorError::Core(e) => e,
            MonitorError::Internal(msg) => CoreError::Internal {
                code: InternalCode::Generic,
                message: msg,
            },
        }
    }
}
