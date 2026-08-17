//! ConfigCode — Config category error codes.
//!
//! Naming: the `config.*` prefix is the default. When adding new codes, follow
//! the ADR-019 §2 convention.
//!
//! Exception: `UnsupportedProviderBedrock` intentionally uses the
//! `provider.bedrock.unsupported` wire code so that the observability dashboard
//! can group provider-unsupported errors under a single `provider.*` namespace.
//! This exception is documented in ADR-019 §3.

// `define_code_enum!` is re-exported at crate root via `#[macro_export]` in
// `error_codes/macros.rs`; no explicit `use` needed within the same crate.

define_code_enum! {
    /// Config category error codes.
    pub enum ConfigCode {
        /// Config file parse failure or schema mismatch.
        Invalid => "config.invalid",
        /// Config file is well-formed but written by a NEWER client (#10985).
        ///
        /// Distinct from `Invalid` because the remedies are opposite: a corrupt
        /// file should be replaced so the next launch is clean, while a
        /// future-versioned one must be left alone — it is the user's settings,
        /// still valid, and overwriting it destroys them on any rollback.
        SchemaTooNew => "config.schema_too_new",
        /// Required config field missing.
        Missing => "config.missing",
        /// Config value outside the allowed range.
        OutOfRange => "config.out_of_range",
        /// AWS Bedrock intentionally unsupported (see ADR-019 §5 re-introduction checklist).
        UnsupportedProviderBedrock => "provider.bedrock.unsupported",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn as_str_round_trip_unique() {
        let codes: Vec<&str> = ConfigCode::all().iter().map(|c| c.as_str()).collect();
        let unique: HashSet<_> = codes.iter().collect();
        assert_eq!(codes.len(), unique.len(), "duplicate codes: {codes:?}");
    }

    #[test]
    fn naming_convention() {
        for c in ConfigCode::all() {
            let s = c.as_str();
            assert!(
                s.chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch == '.' || ch == '_'),
                "non-conforming character in {s:?}"
            );
            assert!(s.contains('.'), "missing dot in {s:?}");
        }
    }

    #[test]
    fn display_matches_as_str() {
        for c in ConfigCode::all() {
            assert_eq!(format!("{c}"), c.as_str());
        }
    }
}
