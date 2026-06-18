//! NotFoundCode — NotFound category error codes. `not_found.*` prefix.

define_code_enum! {
    /// NotFound category error codes.
    pub enum NotFoundCode {
        /// The specified resource could not be found.
        ResourceMissing => "not_found.resource_missing",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn as_str_round_trip_unique() {
        let codes: Vec<&str> = NotFoundCode::all().iter().map(|c| c.as_str()).collect();
        let unique: HashSet<_> = codes.iter().collect();
        assert_eq!(codes.len(), unique.len());
    }

    #[test]
    fn naming_convention() {
        for c in NotFoundCode::all() {
            let s = c.as_str();
            assert!(s
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch == '.' || ch == '_'));
            assert!(s.contains('.'));
        }
    }

    #[test]
    fn display_matches_as_str() {
        for c in NotFoundCode::all() {
            assert_eq!(format!("{c}"), c.as_str());
        }
    }
}
