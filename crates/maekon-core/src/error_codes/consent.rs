//! ConsentCode — Consent 카테고리 에러 코드. `consent.*` 접두사.

define_code_enum! {
    /// Consent 카테고리 에러 코드.
    pub enum ConsentCode {
        /// 동의 필요 (아직 받지 못함).
        Required => "consent.required",
        /// 동의 만료 (재-동의 필요).
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
