// crates/oneshim-storage/src/error.rs
use oneshim_core::error::CoreError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error(transparent)]
    Core(#[from] CoreError),

    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("secret store error: {0}")]
    SecretStore(String),

    #[error("{resource_type} not found: {id}")]
    NotFound { resource_type: String, id: String },

    #[error("encryption error: {0}")]
    Encryption(String),

    #[error("validation failed — {field}: {message}")]
    Validation { field: String, message: String },

    #[error("configuration error: {0}")]
    Config(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl From<StorageError> for CoreError {
    fn from(err: StorageError) -> Self {
        match err {
            StorageError::Core(e) => e,
            StorageError::Sqlite(e) => CoreError::Storage {
                code: oneshim_core::error_codes::StorageCode::Failed,
                message: e.to_string(),
            },
            StorageError::Io(e) => CoreError::Io(e),
            StorageError::SecretStore(msg) => CoreError::SecretStoreError {
                code: oneshim_core::error_codes::SecretCode::Failed,
                message: msg,
            },
            StorageError::NotFound { resource_type, id } => CoreError::NotFound {
                code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
                resource_type,
                id,
            },
            StorageError::Encryption(msg) => CoreError::Storage {
                code: oneshim_core::error_codes::StorageCode::Failed,
                message: format!("encryption: {msg}"),
            },
            StorageError::Validation { field, message } => CoreError::Validation {
                code: oneshim_core::error_codes::ValidationCode::InvalidField,
                field,
                message,
            },
            StorageError::Config(msg) => CoreError::Config {
                code: oneshim_core::error_codes::ConfigCode::Invalid,
                message: msg,
            },
            // StorageError::Internal is constructed almost exclusively at SQLite
            // operation boundaries (400+ sites: query/execute/commit/prepare + KDF,
            // AES, disk, lock poison). Wire code is therefore storage.failed —
            // the observed failure IS a storage operation, even when the root
            // cause is a panic (mutex poison) or a crypto library. Callers that
            // need a different code should construct CoreError directly.
            StorageError::Internal(msg) => CoreError::Storage {
                code: oneshim_core::error_codes::StorageCode::Failed,
                message: msg,
            },
        }
    }
}
