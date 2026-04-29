//! ServiceCode — Service availability 카테고리 에러 코드. `service.*` 접두사.

define_code_enum! {
    /// Service 카테고리 에러 코드.
    pub enum ServiceCode {
        /// 로컬 서킷 브레이커가 열려 fast-fail 상태 (서버 측 장애와 구별).
        CircuitOpen => "service.circuit_open",
        /// 서비스 일시 사용 불가.
        Unavailable => "service.unavailable",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn as_str_round_trip_unique() {
        let codes: Vec<&str> = ServiceCode::all().iter().map(|c| c.as_str()).collect();
        let unique: HashSet<_> = codes.iter().collect();
        assert_eq!(codes.len(), unique.len());
    }

    #[test]
    fn naming_convention() {
        for c in ServiceCode::all() {
            let s = c.as_str();
            assert!(s
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch == '.' || ch == '_'));
            assert!(s.contains('.'));
        }
    }

    #[test]
    fn display_matches_as_str() {
        for c in ServiceCode::all() {
            assert_eq!(format!("{c}"), c.as_str());
        }
    }
}
