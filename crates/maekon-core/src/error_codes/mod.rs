//! Error code registry — central management of wire-format error identifiers.
//!
//! Per ADR-019 §2, every code follows the naming convention
//! `{domain}.{category}[.{qualifier}]` and is immutable after first release.
//!
//! Each code enum is defined via the `define_code_enum!` macro for single-source-
//! of-truth between variant list, `as_str` match, `Display` impl, and `all()`
//! enumerator. See `macros.rs` for the macro definition.

#[macro_use]
mod macros;

// 20 code enum submodules grouped by domain.
pub mod audio;
pub mod auth;
pub mod autostart;
pub mod config;
pub mod consent;
pub mod gui;
pub mod internal;
pub mod network;
pub mod not_found;
pub mod oauth;
pub mod permission;
pub mod policy;
pub mod provider;
pub mod sandbox;
pub mod secret;
pub mod service;
pub mod storage;
pub mod time_window;
pub mod ui;
pub mod validation;

pub use audio::AudioCode;
pub use auth::AuthCode;
pub use autostart::AutostartCode;
pub use config::ConfigCode;
pub use consent::ConsentCode;
pub use gui::GuiCode;
pub use internal::InternalCode;
pub use network::NetworkCode;
pub use not_found::NotFoundCode;
pub use oauth::OAuthCode;
pub use permission::PermissionCode;
pub use policy::PolicyCode;
pub use provider::ProviderCode;
pub use sandbox::SandboxCode;
pub use secret::SecretCode;
pub use service::ServiceCode;
pub use storage::StorageCode;
pub use time_window::TimeWindowCode;
pub use ui::UiCode;
pub use validation::ValidationCode;

/// Collects every wire-format code string across every code enum, sorted.
///
/// Internal test helper for wire-contract snapshot test at
/// `tests/wire_contract_snapshot.rs`. Marked `#[doc(hidden)]` to signal it is
/// not an external API.
#[doc(hidden)]
pub fn all_codes() -> Vec<&'static str> {
    let mut codes: Vec<&'static str> = Vec::new();
    for c in AudioCode::all() {
        codes.push(c.as_str());
    }
    for c in AutostartCode::all() {
        codes.push(c.as_str());
    }
    for c in AuthCode::all() {
        codes.push(c.as_str());
    }
    for c in ConfigCode::all() {
        codes.push(c.as_str());
    }
    for c in ConsentCode::all() {
        codes.push(c.as_str());
    }
    for c in GuiCode::all() {
        codes.push(c.as_str());
    }
    for c in InternalCode::all() {
        codes.push(c.as_str());
    }
    for c in NetworkCode::all() {
        codes.push(c.as_str());
    }
    for c in NotFoundCode::all() {
        codes.push(c.as_str());
    }
    for c in OAuthCode::all() {
        codes.push(c.as_str());
    }
    for c in PermissionCode::all() {
        codes.push(c.as_str());
    }
    for c in PolicyCode::all() {
        codes.push(c.as_str());
    }
    for c in ProviderCode::all() {
        codes.push(c.as_str());
    }
    for c in SandboxCode::all() {
        codes.push(c.as_str());
    }
    for c in SecretCode::all() {
        codes.push(c.as_str());
    }
    for c in ServiceCode::all() {
        codes.push(c.as_str());
    }
    for c in StorageCode::all() {
        codes.push(c.as_str());
    }
    for c in TimeWindowCode::all() {
        codes.push(c.as_str());
    }
    for c in UiCode::all() {
        codes.push(c.as_str());
    }
    for c in ValidationCode::all() {
        codes.push(c.as_str());
    }
    codes.sort();
    codes
}

#[cfg(test)]
mod aggregator_tests {
    use super::all_codes;

    #[test]
    fn all_codes_returns_sorted_unique() {
        let codes = all_codes();
        let mut sorted = codes.clone();
        sorted.sort();
        assert_eq!(codes, sorted, "all_codes() must return sorted output");

        let mut deduped = codes.clone();
        deduped.dedup();
        assert_eq!(codes.len(), deduped.len(), "all_codes() must be unique");
    }

    #[test]
    fn all_codes_non_empty() {
        assert!(!all_codes().is_empty());
    }
}
