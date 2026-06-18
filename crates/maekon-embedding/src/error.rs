use maekon_core::error::CoreError;
use maekon_core::error_codes::InternalCode;
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
            // Prefix with "[embedding] " so Loki/Grafana label selectors can
            // isolate embedding regressions from other internal.generic errors
            // without adding a new wire-contract code variant (ADR-019 §1).
            EmbeddingError::Internal(msg) => CoreError::Internal {
                code: InternalCode::Generic,
                message: format!("[embedding] {msg}"),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedding_internal_prefixed_in_core_error() {
        let err = EmbeddingError::Internal("fastembed init failed: oom".to_owned());
        let core: CoreError = err.into();
        let msg = format!("{core}");
        assert!(
            msg.contains("[embedding]"),
            "CoreError message must carry [embedding] prefix for log filterability: {msg}"
        );
        assert_eq!(core.code(), "internal.generic");
    }
}
