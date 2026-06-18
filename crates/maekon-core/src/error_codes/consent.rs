//! ConsentCode — Consent category error codes. `consent.*` prefix.

define_code_enum! {
    /// Consent category error codes.
    pub enum ConsentCode {
        /// Consent required (not yet obtained).
        Required => "consent.required",
        /// Consent expired (re-consent required).
        Expired => "consent.expired",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn as_str_round_trip_unique() {
        let codes: Vec<&str> = ConsentCode::all().iter().map(|c| c.as_str()).collect();
        let unique: HashSet<_> = codes.iter().collect();
        assert_eq!(codes.len(), unique.len());
    }

    #[test]
    fn naming_convention() {
        for c in ConsentCode::all() {
            let s = c.as_str();
            assert!(s
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch == '.' || ch == '_'));
            assert!(s.contains('.'));
        }
    }

    #[test]
    fn display_matches_as_str() {
        for c in ConsentCode::all() {
            assert_eq!(format!("{c}"), c.as_str());
        }
    }
}
