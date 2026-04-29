//! ValidationCode — Validation 카테고리 에러 코드. `validation.*` 접두사.

define_code_enum! {
    /// Validation 카테고리 에러 코드.
    pub enum ValidationCode {
        /// 특정 필드 검증 실패.
        InvalidField => "validation.invalid_field",
        /// 함수/메서드 인자 검증 실패.
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
