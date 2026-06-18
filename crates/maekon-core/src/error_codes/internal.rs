//! InternalCode — codes for internal errors and `#[from]`-wrapped external
//! errors. `internal.*` prefix.
//!
//! `Io` / `Serialization` are only returned derivedly from
//! `impl CoreError::code()` and are not stored as variant fields (spec §4.6).

define_code_enum! {
    /// Internal category error codes.
    pub enum InternalCode {
        /// Generic internal error.
        Generic => "internal.generic",
        /// `std::io::Error` `#[from]`-wrapped error.
        Io => "internal.io",
        /// `serde_json::Error` `#[from]`-wrapped error.
        Serialization => "internal.serialization",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn as_str_round_trip_unique() {
        let codes: Vec<&str> = InternalCode::all().iter().map(|c| c.as_str()).collect();
        let unique: HashSet<_> = codes.iter().collect();
        assert_eq!(codes.len(), unique.len());
    }

    #[test]
    fn naming_convention() {
        for c in InternalCode::all() {
            let s = c.as_str();
            assert!(s
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch == '.' || ch == '_'));
            assert!(s.contains('.'));
        }
    }

    #[test]
    fn display_matches_as_str() {
        for c in InternalCode::all() {
            assert_eq!(format!("{c}"), c.as_str());
        }
    }
}
