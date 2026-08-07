//! Built-in connector registry (ADR-034 P4).
//!
//! The product decision this module encodes: Maekon supports a small set of
//! **essential** tools, not a broad catalog (#9855 decision, 2026-08-05 —
//! essential tools only, calendar is essential). Each built-in connector is one entry here,
//! gated by its own Cargo feature, so "essential tools only" is a
//! **compile-time fact**: a build without a connector's feature contains
//! neither its registry row nor (via the composition root reading this
//! registry) its OAuth provider config.
//!
//! What this registry deliberately is NOT:
//! - It is not the extension install registry (`ExtensionRegistryPort`). That
//!   surface was retired in #9639 because nothing called `register_package`
//!   and the IPC advertised a feature that could never work. Reviving it has
//!   a documented order (src-tauri `lib.rs` — annotations, IPC lines, AND a
//!   real `register_package` call site) that this module does not attempt.
//! - It is not a runtime plugin mechanism. MK-EXT (#8582) Phase 1 allows
//!   `first_party_builtin` only; entries here are compiled in, never loaded.

/// One built-in connector's identity, as the composition root needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinConnector {
    /// Extension identity key (`com.maekon.…`).
    pub extension_id: &'static str,
    /// OAuth provider id — the SecretStore/keychain namespace the connector's
    /// tokens live under. Must match the id its `OAuthProviderConfig` carries,
    /// or token lookup fails silently; both sides source the same
    /// `maekon_core::ports::oauth` constant to make drift impossible.
    pub oauth_provider_id: &'static str,
    /// OAuth scopes the connector requests. Read-only by MK-EXT invariant
    /// (#8582): a write scope in this list is a review error.
    pub oauth_scopes: &'static [&'static str],
}

/// Every built-in connector compiled into this build.
///
/// Order is stable (registry order = presentation order). Adding an entry
/// means: a new feature flag, a row here under that flag, and the composition
/// root's OAuth wiring recognising the new provider id — the registry test
/// pins the first two, the src-tauri test pins the third.
pub fn builtin_connectors() -> &'static [BuiltinConnector] {
    &[
        #[cfg(feature = "connector-google-calendar")]
        BuiltinConnector {
            extension_id: crate::google_calendar::GOOGLE_CALENDAR_EXTENSION_ID,
            oauth_provider_id: maekon_core::ports::oauth::GOOGLE_CALENDAR_PROVIDER_ID,
            oauth_scopes: &[maekon_core::ports::oauth::GOOGLE_CALENDAR_READONLY_SCOPE],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(feature = "connector-google-calendar")]
    fn registry_lists_exactly_the_essential_set() {
        // The essential set today is calendar, alone. A second entry appearing
        // here without a product decision is what this assertion exists to
        // catch — growing the catalog is deliberate, not incidental (#9855).
        let all = builtin_connectors();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].extension_id, "com.maekon.google_calendar");
    }

    #[test]
    #[cfg(feature = "connector-google-calendar")]
    fn calendar_identity_matches_the_core_constants() {
        // Both the connector and maekon-network's provider registry source
        // these constants from maekon-core; the registry row must too, or the
        // keychain namespace drifts from the token writer's.
        let c = &builtin_connectors()[0];
        assert_eq!(
            c.oauth_provider_id,
            maekon_core::ports::oauth::GOOGLE_CALENDAR_PROVIDER_ID
        );
        assert_eq!(
            c.oauth_scopes,
            &[maekon_core::ports::oauth::GOOGLE_CALENDAR_READONLY_SCOPE]
        );
    }

    #[test]
    fn every_scope_is_read_only() {
        // MK-EXT invariant (#8582): connectors never request a write scope.
        // `readonly` in every Google scope URL is the enforceable spelling of
        // that rule for the current catalog; a connector whose provider spells
        // read-only differently must extend this test, not skip it.
        for connector in builtin_connectors() {
            for scope in connector.oauth_scopes {
                assert!(
                    scope.ends_with(".readonly"),
                    "{} requests non-read-only scope {scope}",
                    connector.extension_id
                );
            }
        }
    }
}
