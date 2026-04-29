//! InternalCode — 내부 에러 및 `#[from]`-wrapped 외부 에러용 코드. `internal.*` 접두사.
//!
//! `Io` / `Serialization`은 `impl CoreError::code()`에서 파생 반환만 하고
//! variant 필드로 저장되지는 않음 (spec §4.6).

define_code_enum! {
    /// Internal 카테고리 에러 코드.
    pub enum InternalCode {
        /// 일반 내부 에러.
        Generic => "internal.generic",
        /// `std::io::Error` `#[from]` 래핑 에러.
        Io => "internal.io",
        /// `serde_json::Error` `#[from]` 래핑 에러.
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
