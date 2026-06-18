//! ValidationCode — Validation category error codes. `validation.*` prefix.

define_code_enum! {
    /// Validation category error codes.
    pub enum ValidationCode {
        /// Specific field validation failure.
        InvalidField => "validation.invalid_field",
        /// Function/method argument validation failure.
        InvalidArguments => "validation.invalid_arguments",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn as_str_round_trip_unique() {
        let codes: Vec<&str> = ValidationCode::all().iter().map(|c| c.as_str()).collect();
        let unique: HashSet<_> = codes.iter().collect();
        assert_eq!(codes.len(), unique.len());
    }

    #[test]
    fn naming_convention() {
        for c in ValidationCode::all() {
            let s = c.as_str();
            assert!(s
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch == '.' || ch == '_'));
            assert!(s.contains('.'));
        }
    }

    #[test]
    fn display_matches_as_str() {
        for c in ValidationCode::all() {
            assert_eq!(format!("{c}"), c.as_str());
        }
    }
}
