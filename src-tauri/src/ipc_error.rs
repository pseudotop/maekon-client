//! Tauri IPC error envelope for ADR-019 typed-code propagation.
//!
//! See [ADR-019](../../docs/architecture/ADR-019-error-code-infrastructure.md)
//! §Known Follow-up #1. Before this module existed, every Tauri command signature
//! was `Result<T, String>` and errors were collapsed via `.map_err(|e| e.to_string())`
//! — the typed `err.code()` established by ADR-019 never reached the frontend.
//!
//! With `IpcError` the frontend receives `{"code": "...", "message": "..."}`
//! and can branch programmatically (i18n keying, retry decisions, etc.) instead
//! of substring-matching the display string.
//!
//! ## Usage
//!
//! ```ignore
//! use crate::ipc_error::IpcError;
//! use oneshim_core::error::CoreError;
//!
//! #[tauri::command]
//! pub async fn some_command(...) -> Result<Response, IpcError> {
//!     service.call(...).await.map_err(IpcError::from)
//! }
//! ```
//!
//! For adapter-error-returning service calls, the `From` chain walks through
//! `CoreError`:
//!
//! ```ignore
//! // oneshim_network::NetworkError -> CoreError -> IpcError
//! network_client.post(...).await.map_err(IpcError::from)
//! ```
//!
//! ## Migration
//!
//! Ongoing — see `docs/reviews/2026-04-20-adr019-followup-ipc-error-dto-design.md`
//! for the phased migration plan. This module ships the foundation; individual
//! command files migrate in separate PRs (low-risk read-only commands first,
//! then state-mutating, then streaming/heavy-IO).

use oneshim_core::error::CoreError;
use oneshim_core::error::GuiInteractionError;
use serde::Serialize;

/// Tauri IPC error envelope. Serializes as `{"code": "...", "message": "..."}`.
///
/// The `code` field matches the ADR-019 wire contract (see
/// `crates/oneshim-core/tests/wire_contract_snapshot.expected.txt`). Frontend
/// consumers should branch on `code` — NOT substring-match `message`.
///
/// Construct via the `From<CoreError>` impl rather than building directly;
/// direct construction (`IpcError::new(...)`) is allowed for input-validation
/// errors where no upstream `CoreError` exists (e.g., argument parsing or
/// precondition violations at command entry).
#[derive(Debug, Clone, Serialize)]
pub struct IpcError {
    /// Wire-format error code (e.g., `"config.invalid"`, `"network.timeout"`).
    pub code: String,
    /// Human-readable message. Includes `[code]` prefix from `CoreError` Display.
    pub message: String,
}

impl IpcError {
    /// Construct a new `IpcError` directly. Prefer the `From<CoreError>` impl
    /// when an upstream error is available. This constructor is useful for
    /// argument-validation failures at the Tauri command boundary, where no
    /// `CoreError` has been produced yet.
    ///
    /// `code` SHOULD match one of the wire codes in the
    /// `wire_contract_snapshot.expected.txt` registry; callers are responsible
    /// for picking the semantically correct code.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for IpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Matches the CoreError Display convention: "[{code}] {message}"
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for IpcError {}

impl From<CoreError> for IpcError {
    fn from(err: CoreError) -> Self {
        Self {
            code: err.code().to_string(),
            message: err.to_string(),
        }
    }
}

impl From<GuiInteractionError> for IpcError {
    fn from(err: GuiInteractionError) -> Self {
        Self {
            code: err.code().to_string(),
            message: err.to_string(),
        }
    }
}

// --- Adapter-error From chain impls ---
//
// Each adapter error type in the workspace has `impl From<AdapterError> for CoreError`.
// We bridge those into `IpcError` via a two-hop conversion so command code can
// write `service_call().await.map_err(IpcError::from)` regardless of whether
// `service_call` returns `CoreError` or an adapter error.

// Unconditionally-available adapters (always in the default + build graph).

impl From<oneshim_storage::error::StorageError> for IpcError {
    fn from(err: oneshim_storage::error::StorageError) -> Self {
        IpcError::from(CoreError::from(err))
    }
}

impl From<oneshim_automation::error::AutomationError> for IpcError {
    fn from(err: oneshim_automation::error::AutomationError) -> Self {
        IpcError::from(CoreError::from(err))
    }
}

impl From<oneshim_vision::error::VisionError> for IpcError {
    fn from(err: oneshim_vision::error::VisionError) -> Self {
        IpcError::from(CoreError::from(err))
    }
}

impl From<oneshim_analysis::error::AnalysisError> for IpcError {
    fn from(err: oneshim_analysis::error::AnalysisError) -> Self {
        IpcError::from(CoreError::from(err))
    }
}

impl From<oneshim_suggestion::error::SuggestionError> for IpcError {
    fn from(err: oneshim_suggestion::error::SuggestionError) -> Self {
        IpcError::from(CoreError::from(err))
    }
}

impl From<oneshim_monitor::error::MonitorError> for IpcError {
    fn from(err: oneshim_monitor::error::MonitorError) -> Self {
        IpcError::from(CoreError::from(err))
    }
}

// Feature-gated adapters — the parent deps are optional in src-tauri/Cargo.toml.

#[cfg(feature = "analysis")]
impl From<oneshim_network::error::NetworkError> for IpcError {
    fn from(err: oneshim_network::error::NetworkError) -> Self {
        IpcError::from(CoreError::from(err))
    }
}

#[cfg(feature = "embedding")]
impl From<oneshim_embedding::error::EmbeddingError> for IpcError {
    fn from(err: oneshim_embedding::error::EmbeddingError) -> Self {
        IpcError::from(CoreError::from(err))
    }
}

// --- Convenience From impls for common stdlib types at validation boundaries ---

impl From<std::io::Error> for IpcError {
    fn from(err: std::io::Error) -> Self {
        IpcError::from(CoreError::from(err))
    }
}

impl From<serde_json::Error> for IpcError {
    fn from(err: serde_json::Error) -> Self {
        IpcError::from(CoreError::from(err))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oneshim_core::error_codes;

    #[test]
    fn ipc_error_from_core_error_preserves_wire_code() {
        let core = CoreError::Config {
            code: error_codes::ConfigCode::Invalid,
            message: "bad value".into(),
        };
        let ipc: IpcError = core.into();
        assert_eq!(ipc.code, "config.invalid");
        assert!(ipc.message.contains("bad value"));
        // The Display impl on CoreError already embeds the [code] marker.
        assert!(ipc.message.contains("[config.invalid]"));
    }

    #[test]
    fn ipc_error_from_core_error_timeout_variant() {
        let core = CoreError::RequestTimeout {
            code: error_codes::NetworkCode::Timeout,
            timeout_ms: 5000,
        };
        let ipc: IpcError = core.into();
        assert_eq!(ipc.code, "network.timeout");
        assert!(ipc.message.contains("5000"));
    }

    #[test]
    fn ipc_error_from_core_error_bedrock_unsupported() {
        let core = CoreError::Config {
            code: error_codes::ConfigCode::UnsupportedProviderBedrock,
            message: "AWS Bedrock is intentionally unsupported in this build".into(),
        };
        let ipc: IpcError = core.into();
        assert_eq!(ipc.code, "provider.bedrock.unsupported");
    }

    #[test]
    fn ipc_error_from_gui_interaction_error() {
        let err = GuiInteractionError::Unauthorized {
            code: error_codes::GuiCode::Unauthorized,
        };
        let ipc: IpcError = err.into();
        assert_eq!(ipc.code, "gui.unauthorized");
    }

    #[cfg(feature = "analysis")]
    #[test]
    fn ipc_error_from_adapter_error_chains_through_core() {
        // NetworkError::Timeout { .. } → CoreError::RequestTimeout { NetworkCode::Timeout, .. }
        //                             → IpcError { code: "network.timeout", .. }
        let net = oneshim_network::error::NetworkError::Timeout { timeout_ms: 3000 };
        let ipc: IpcError = net.into();
        assert_eq!(ipc.code, "network.timeout");
    }

    #[test]
    fn ipc_error_from_adapter_error_storage_variant() {
        let storage = oneshim_storage::error::StorageError::NotFound {
            resource_type: "session".into(),
            id: "abc".into(),
        };
        let ipc: IpcError = storage.into();
        assert_eq!(ipc.code, "not_found.resource_missing");
    }

    #[test]
    fn ipc_error_from_io_chains_to_internal_io() {
        let io = std::io::Error::other("disk full");
        let ipc: IpcError = io.into();
        // #[from] variants in CoreError carry a hardcoded wire code per ADR-019 §7.
        assert_eq!(ipc.code, "internal.io");
        assert!(ipc.message.contains("disk full"));
    }

    #[test]
    fn ipc_error_from_serde_json_chains_to_internal_serialization() {
        let json_err: serde_json::Error =
            serde_json::from_str::<i32>("not a number").expect_err("should fail");
        let ipc: IpcError = json_err.into();
        assert_eq!(ipc.code, "internal.serialization");
    }

    #[test]
    fn ipc_error_new_direct_construction() {
        // For argument-validation errors where no upstream CoreError exists.
        let ipc = IpcError::new("validation.invalid_arguments", "missing field");
        assert_eq!(ipc.code, "validation.invalid_arguments");
        assert_eq!(ipc.message, "missing field");
    }

    #[test]
    fn ipc_error_serialization_shape() {
        // Contract test: the wire format must be {"code": "...", "message": "..."}
        // — not a renamed field, not camelCased, not wrapped in an envelope.
        let ipc = IpcError::new("config.invalid", "test message");
        let json = serde_json::to_value(&ipc).expect("serialize");
        assert_eq!(json["code"], "config.invalid");
        assert_eq!(json["message"], "test message");
        assert_eq!(
            json.as_object().unwrap().len(),
            2,
            "IpcError should serialize to exactly 2 fields; any addition is a breaking change"
        );
    }
}
