//! PolicyCode - Policy category error codes. `policy.*` prefix.

define_code_enum! {
    /// Policy category error codes.
    pub enum PolicyCode {
        /// Policy denied.
        Denied => "policy.denied",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn as_str_round_trip_unique() {
        let codes: Vec<&str> = PolicyCode::all().iter().map(|c| c.as_str()).collect();
        let unique: HashSet<_> = codes.iter().collect();
        assert_eq!(codes.len(), unique.len());
    }

    #[test]
    fn naming_convention() {
        for c in PolicyCode::all() {
            let s = c.as_str();
            assert!(s
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch == '.' || ch == '_'));
            assert!(s.contains('.'));
        }
    }

    #[test]
    fn display_matches_as_str() {
        for c in PolicyCode::all() {
            assert_eq!(format!("{c}"), c.as_str());
        }
    }
}
