//! NetworkCode — Network 카테고리 에러 코드. `network.*` 접두사.

define_code_enum! {
    /// Network 카테고리 에러 코드.
    pub enum NetworkCode {
        /// 요청 타임아웃 초과.
        Timeout => "network.timeout",
        /// 서버 레이트 리밋 도달 (429).
        RateLimit => "network.rate_limit",
        /// 세분화 미완료.
        Generic => "network.generic",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn as_str_round_trip_unique() {
        let codes: Vec<&str> = NetworkCode::all().iter().map(|c| c.as_str()).collect();
        let unique: HashSet<_> = codes.iter().collect();
        assert_eq!(codes.len(), unique.len());
    }

    #[test]
    fn naming_convention() {
        for c in NetworkCode::all() {
            let s = c.as_str();
            assert!(s
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch == '.' || ch == '_'));
            assert!(s.contains('.'));
        }
    }

    #[test]
    fn display_matches_as_str() {
        for c in NetworkCode::all() {
            assert_eq!(format!("{c}"), c.as_str());
        }
    }
}
