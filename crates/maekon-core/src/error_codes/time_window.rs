//! TimeWindowCode — TimeWindow 카테고리 에러 코드. `time_window.*` 접두사.

define_code_enum! {
    /// TimeWindow 카테고리 에러 코드.
    pub enum TimeWindowCode {
        /// start > end 검증 실패.
        InvertedBounds => "time_window.inverted_bounds",
        /// RFC3339 timestamp 파싱 실패.
        ParseFailed => "time_window.parse_failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn as_str_round_trip_unique() {
        let codes: Vec<&str> = TimeWindowCode::all().iter().map(|c| c.as_str()).collect();
        let unique: HashSet<_> = codes.iter().collect();
        assert_eq!(codes.len(), unique.len());
    }

    #[test]
    fn naming_convention() {
        for c in TimeWindowCode::all() {
            let s = c.as_str();
            assert!(s
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch == '.' || ch == '_'));
            assert!(s.starts_with("time_window."));
        }
    }

    #[test]
    fn display_matches_as_str() {
        for c in TimeWindowCode::all() {
            assert_eq!(format!("{c}"), c.as_str());
        }
    }
}
