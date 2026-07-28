//! Sync setup surface contracts (`/api/sync/setup`).
//!
//! Moved out of `maekon-web` handlers per the web contract boundary
//! (public DTOs live in `maekon-api-contracts`; #8685 R01 public CI gate).

use serde::{Deserialize, Serialize};

/// Current sync setup state.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SyncSetupStatus {
    /// The `config.sync.enabled` master switch.
    pub enabled: bool,
    /// The configured transport (`file` / `lan` / `remote`).
    pub transport: String,
    /// Whether a passphrase is present in the OS keychain. The passphrase value
    /// itself is NEVER returned — only its presence.
    pub passphrase_set: bool,
    /// True after a change that only takes effect on the next app start.
    pub restart_required: bool,
}

/// Request body for `POST /api/sync/setup`.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SyncSetupRequest {
    /// New passphrase to store in the keychain. When `None`/omitted the existing
    /// keychain entry is left untouched (e.g. a pure enable/disable toggle).
    #[serde(default)]
    pub passphrase: Option<String>,
    /// Desired `config.sync.enabled` state.
    pub enabled: bool,
    /// Optional transport override (`file` / `lan` / `remote`).
    #[serde(default)]
    pub transport: Option<String>,
}
