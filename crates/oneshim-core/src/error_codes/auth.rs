//! AuthCode — Auth 카테고리 에러 코드. `auth.*` 접두사.

define_code_enum! {
    /// Auth 카테고리 에러 코드.
    pub enum AuthCode {
        /// 인증 실패 (잘못된 credential 등).
        Failed => "auth.failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn as_str_round_trip_unique() {
        let codes: Vec<&str> = AuthCode::all().iter().map(|c| c.as_str()).collect();
        let unique: HashSet<_> = codes.iter().collect();
        assert_eq!(codes.len(), unique.len());
    }

    #[test]
    fn naming_convention() {
        for c in AuthCode::all() {
            let s = c.as_str();
            assert!(s
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch == '.' || ch == '_'));
            assert!(s.contains('.'));
        }
    }

    #[test]
    fn display_matches_as_str() {
        for c in AuthCode::all() {
            assert_eq!(format!("{c}"), c.as_str());
        }
    }
}
