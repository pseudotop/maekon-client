use oneshim_core::error::CoreError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum VisionError {
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error("OCR error: {0}")]
    Ocr(String),
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<VisionError> for CoreError {
    fn from(err: VisionError) -> Self {
        match err {
            VisionError::Core(e) => e,
            VisionError::Ocr(msg) => CoreError::OcrError {
                code: oneshim_core::error_codes::ProviderCode::OcrFailed,
                message: msg,
            },
            VisionError::Internal(msg) => CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: msg,
            },
        }
    }
}
