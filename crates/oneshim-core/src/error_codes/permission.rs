//! PermissionCode — Permission 카테고리 에러 코드. `permission.*` 접두사.

define_code_enum! {
    /// Permission 카테고리 에러 코드.
    pub enum PermissionCode {
        /// 권한 거부 (일반).
        PermissionDenied => "permission.permission_denied",
        /// 개인정보 권한 거부.
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
