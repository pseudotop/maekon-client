use thiserror::Error;

use crate::error_codes::{
    AudioCode, AuthCode, ConfigCode, ConsentCode, GuiCode, InternalCode, NetworkCode, NotFoundCode,
    OAuthCode, PermissionCode, PolicyCode, ProviderCode, SandboxCode, SecretCode, ServiceCode,
    StorageCode, TimeWindowCode, UiCode, ValidationCode,
};
use crate::ports::oauth::OAuthErrorKind;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("Configuration error [{code}]: {message}")]
    Config { code: ConfigCode, message: String },

    #[error("Network error [{code}]: {message}")]
    Network { code: NetworkCode, message: String },

    #[error("Request timed out [{code}] after {timeout_ms}ms")]
    RequestTimeout { code: NetworkCode, timeout_ms: u64 },

    #[error("Request rate limit exceeded [{code}], retry after {retry_after_secs}s")]
    RateLimit {
        code: NetworkCode,
        retry_after_secs: u64,
    },

    #[error("Service temporarily unavailable [{code}]: {message}")]
    ServiceUnavailable { code: ServiceCode, message: String },

    #[error("Authentication error [{code}]: {message}")]
    Auth { code: AuthCode, message: String },

    #[error("OAuth error [{code}] for provider {provider}: {message}")]
    OAuthError {
        code: OAuthCode,
        provider: String,
        message: String,
    },

    #[error("OAuth refresh error [{code}] for provider {provider}: [{kind:?}] {message}")]
    OAuthRefreshError {
        code: OAuthCode,
        provider: String,
        kind: OAuthErrorKind,
        message: String,
    },

    #[error("Validation failed [{code}] - {field}: {message}")]
    Validation {
        code: ValidationCode,
        field: String,
        message: String,
    },

    #[error("Invalid arguments [{code}]: {message}")]
    InvalidArguments {
        code: ValidationCode,
        message: String,
    },

    #[error("{resource_type} not found [{code}]: {id}")]
    NotFound {
        code: NotFoundCode,
        resource_type: String,
        id: String,
    },

    #[error("UI element not found [{code}]: {name}")]
    ElementNotFound { code: UiCode, name: String },

    #[error("Internal error [{code}]: {message}")]
    Internal { code: InternalCode, message: String },

    #[error("Policy denied [{code}]: {message}")]
    PolicyDenied { code: PolicyCode, message: String },

    #[error("Consent required [{code}]: {message}")]
    ConsentRequired { code: ConsentCode, message: String },

    #[error("Consent expired [{code}]")]
    ConsentExpired { code: ConsentCode },

    #[error("Sandbox initialization failed [{code}]: {message}")]
    SandboxInit { code: SandboxCode, message: String },

    #[error("Sandbox execution failed [{code}]: {message}")]
    SandboxExecution { code: SandboxCode, message: String },

    #[error("Sandbox unsupported on platform [{code}]: {message}")]
    SandboxUnsupported { code: SandboxCode, message: String },

    #[error("Execution timeout [{code}] exceeded: {timeout_ms}ms")]
    ExecutionTimeout { code: SandboxCode, timeout_ms: u64 },

    #[error("Privacy denied [{code}]: {message}")]
    PrivacyDenied {
        code: PermissionCode,
        message: String,
    },

    #[error("Permission denied [{code}]: {message}")]
    PermissionDenied {
        code: PermissionCode,
        message: String,
    },

    #[error("OCR error [{code}]: {message}")]
    OcrError { code: ProviderCode, message: String },

    #[error("Analysis error [{code}]: {message}")]
    Analysis { code: ProviderCode, message: String },

    #[error("Audio capture error [{code}]: {message}")]
    AudioCapture { code: AudioCode, message: String },

    #[error("Speech-to-text error [{code}]: {message}")]
    SpeechToText { code: AudioCode, message: String },

    #[error("Storage error [{code}]: {message}")]
    Storage { code: StorageCode, message: String },

    #[error("Time window error [{code}]: {message}")]
    TimeWindow {
        code: TimeWindowCode,
        message: String,
    },

    #[error("Secret store error [{code}]: {message}")]
    SecretStoreError { code: SecretCode, message: String },

    // === `#[from]`-wrapped external error types ===
    // Wire codes are hardcoded in Display templates because these variants do
    // not carry a typed `code:` field (spec §4.6). Must stay in sync with the
    // `impl code()` arms in this file. Wire-format immutability (ADR-019 §1,
    // "Released code strings are immutable (wire contract)") means these
    // strings won't change.
    #[error("Serialization error [internal.serialization]: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("I/O error [internal.io]: {0}")]
    Io(#[from] std::io::Error),
}

impl CoreError {
    /// Wire-format error code. UI, log, telemetry entry point.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Config { code, .. } => code.as_str(),
            Self::Network { code, .. } => code.as_str(),
            Self::RequestTimeout { code, .. } => code.as_str(),
            Self::RateLimit { code, .. } => code.as_str(),
            Self::ServiceUnavailable { code, .. } => code.as_str(),
            Self::Auth { code, .. } => code.as_str(),
            Self::OAuthError { code, .. } => code.as_str(),
            Self::OAuthRefreshError { code, .. } => code.as_str(),
            Self::Validation { code, .. } => code.as_str(),
            Self::InvalidArguments { code, .. } => code.as_str(),
            Self::NotFound { code, .. } => code.as_str(),
            Self::ElementNotFound { code, .. } => code.as_str(),
            Self::Internal { code, .. } => code.as_str(),
            Self::PolicyDenied { code, .. } => code.as_str(),
            Self::ConsentRequired { code, .. } => code.as_str(),
            Self::ConsentExpired { code } => code.as_str(),
            Self::SandboxInit { code, .. } => code.as_str(),
            Self::SandboxExecution { code, .. } => code.as_str(),
            Self::SandboxUnsupported { code, .. } => code.as_str(),
            Self::ExecutionTimeout { code, .. } => code.as_str(),
            Self::PrivacyDenied { code, .. } => code.as_str(),
            Self::PermissionDenied { code, .. } => code.as_str(),
            Self::OcrError { code, .. } => code.as_str(),
            Self::Analysis { code, .. } => code.as_str(),
            Self::AudioCapture { code, .. } => code.as_str(),
            Self::SpeechToText { code, .. } => code.as_str(),
            Self::Storage { code, .. } => code.as_str(),
            Self::TimeWindow { code, .. } => code.as_str(),
            Self::SecretStoreError { code, .. } => code.as_str(),

            // #[from]-wrapped external variants (derived)
            Self::Serialization(_) => InternalCode::Serialization.as_str(),
            Self::Io(_) => InternalCode::Io.as_str(),
        }
    }
}

// Manual From impl (NOT `#[from]`) per Phase 2 iter-1 C1: each TimeWindowError
// variant must map to its corresponding TimeWindowCode, which thiserror's
// derive cannot express for struct-variants.
impl From<crate::types::TimeWindowError> for CoreError {
    fn from(err: crate::types::TimeWindowError) -> Self {
        Self::TimeWindow {
            code: err.code(),
            message: err.to_string(),
        }
    }
}

/// GUI 상호작용 전용 에러 (AutomationPort GUI 메서드에서 사용).
#[derive(Debug, Error)]
pub enum GuiInteractionError {
    #[error("GUI session token is invalid [{code}]")]
    Unauthorized { code: GuiCode },

    #[error("GUI session '{name}' not found [{code}]")]
    NotFound { code: GuiCode, name: String },

    #[error("Invalid GUI request [{code}]: {message}")]
    BadRequest { code: GuiCode, message: String },

    #[error("GUI request forbidden [{code}]: {message}")]
    Forbidden { code: GuiCode, message: String },

    #[error("GUI focus drift detected [{code}]: {message}")]
    FocusDrift { code: GuiCode, message: String },

    #[error("GUI ticket is no longer valid [{code}]: {message}")]
    TicketInvalid { code: GuiCode, message: String },

    #[error("GUI runtime unavailable [{code}]: {message}")]
    Unavailable { code: GuiCode, message: String },

    #[error("GUI runtime failed [{code}]: {message}")]
    Internal { code: GuiCode, message: String },
}

impl GuiInteractionError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unauthorized { code } => code.as_str(),
            Self::NotFound { code, .. } => code.as_str(),
            Self::BadRequest { code, .. } => code.as_str(),
            Self::Forbidden { code, .. } => code.as_str(),
            Self::FocusDrift { code, .. } => code.as_str(),
            Self::TicketInvalid { code, .. } => code.as_str(),
            Self::Unavailable { code, .. } => code.as_str(),
            Self::Internal { code, .. } => code.as_str(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_denied_display() {
        let err = CoreError::PermissionDenied {
            code: PermissionCode::PermissionDenied,
            message: "macOS Accessibility".to_string(),
        };
        let msg = format!("{err}");
        assert!(msg.contains("Permission denied"));
        assert!(msg.contains("macOS Accessibility"));
    }

    #[test]
    fn config_code_returns_config_invalid() {
        let err = CoreError::Config {
            code: ConfigCode::Invalid,
            message: "bad".into(),
        };
        assert_eq!(err.code(), "config.invalid");
    }

    #[test]
    fn serialization_code_returns_internal_serialization() {
        let err: CoreError = serde_json::from_str::<i32>("not a number")
            .expect_err("should be error")
            .into();
        assert_eq!(err.code(), "internal.serialization");
    }

    #[test]
    fn io_code_returns_internal_io() {
        let err: CoreError = std::io::Error::other("x").into();
        assert_eq!(err.code(), "internal.io");
    }

    #[test]
    fn bedrock_unsupported_code() {
        let err = CoreError::Config {
            code: ConfigCode::UnsupportedProviderBedrock,
            message: "AWS Bedrock is intentionally unsupported in this build".into(),
        };
        assert_eq!(err.code(), "provider.bedrock.unsupported");
    }

    #[test]
    fn gui_code_returns_gui_unauthorized() {
        let err = GuiInteractionError::Unauthorized {
            code: GuiCode::Unauthorized,
        };
        assert_eq!(err.code(), "gui.unauthorized");
    }

    /// ADR-019 §1 regression guard: #[from] variants must surface their wire
    /// code in Display output so users logging `{err}` see the code without
    /// needing to call err.code() separately. The code is hardcoded in the
    /// template (these variants don't carry a typed code: field per spec §4.6).
    #[test]
    fn from_variants_display_includes_wire_code() {
        let ser_err: CoreError = serde_json::from_str::<i32>("nope")
            .expect_err("should be serde error")
            .into();
        assert!(
            format!("{ser_err}").contains("[internal.serialization]"),
            "Serialization Display missing wire code: {ser_err}"
        );

        let io_err: CoreError = std::io::Error::other("fail").into();
        assert!(
            format!("{io_err}").contains("[internal.io]"),
            "Io Display missing wire code: {io_err}"
        );
    }
}
