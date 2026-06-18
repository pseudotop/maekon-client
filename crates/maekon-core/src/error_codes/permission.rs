//! PermissionCode — Permission category error codes. `permission.*` prefix.

define_code_enum! {
    /// Permission category error codes.
    pub enum PermissionCode {
        /// Permission denied (general).
        PermissionDenied => "permission.permission_denied",
        /// Privacy permission denied.
        PrivacyDenied => "permission.privacy_denied",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn as_str_round_trip_unique() {
        let codes: Vec<&str> = PermissionCode::all().iter().map(|c| c.as_str()).collect();
        let unique: HashSet<_> = codes.iter().collect();
        assert_eq!(codes.len(), unique.len());
    }

    #[test]
    fn naming_convention() {
        for c in PermissionCode::all() {
            let s = c.as_str();
            assert!(s
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch == '.' || ch == '_'));
            assert!(s.contains('.'));
        }
    }

    #[test]
    fn display_matches_as_str() {
        for c in PermissionCode::all() {
            assert_eq!(format!("{c}"), c.as_str());
        }
    }
}
